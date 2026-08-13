Type: grilling
Blocked by: 06
Status: resolved

## Question

Decide the D-Bus interface(s) the Daemon exposes to the GUI (per [ADR-0004](../../../docs/adr/0004-dbus-for-daemon-gui-ipc.md)): object paths, method names/signatures for reading and mutating Profiles/Layers/Bindings/Actions (built on the data model from [Decide Daemon data model](./06-decide-daemon-data-model.md)), and signals for runtime state the GUI needs to reflect (active Profile, active Layer while Mode key is held, active Toggles). Confirm whether the GUI needs live push (signals) vs poll-on-open for each piece of state, and whether config mutations from the GUI apply immediately or require an explicit save/apply step given the Daemon exclusively owns `config.toml`.

## Answer

Grilling session, 2026-08-13. Supported by a background research check into `zvariant`/Python D-Bus binding capabilities for marshalling heterogeneous Rust enums.

**API shape** — one flat D-Bus object at `/com/acheron/Daemon` on bus name `com.acheron.Daemon`, exposing everything through methods keyed by entity name/path rather than an ObjectManager hierarchy of per-Profile/Binding objects. Matches the sparse `HashMap`-keyed-by-entity shape the data model ([Decide Daemon data model](./06-decide-daemon-data-model.md)) already settled on; ObjectManager's per-entity object paths and `InterfacesAdded`/`Removed` signals are real complexity with no payoff for a personally-used MVP. Single combined interface (also `com.acheron.Daemon`) — at this surface's size (~9 methods, 3 signals) a second interface namespace buys nothing.

**Mutation granularity and apply semantics** — atomic per-entity methods (`CreateProfile`, `DeleteProfile`, `RenameProfile`, `SetModeKeyRole`, `SwitchProfile`, `SetBinding`, `ClearBinding`), each a complete, self-contained, immediately-validated-and-applied edit: in-memory state updates and `config.toml` (exclusively Daemon-owned, per [Decide config file format](./03-decide-config-file-format.md)) is rewritten right away. No buffered draft, no explicit save/apply step, no "discard changes" flow — every call either fully succeeds or returns one of a small set of named errors (below). Rejected alternative: whole-config replace (`GetConfig`/`SetConfig` round-trip) — loses per-field validation errors and opens a stale-read-then-clobber race that's free to just not design in, even single-user.

**Reads** — asymmetric from writes on purpose: `GetConfig() -> a{sv}` returns the *entire* document (all Profiles/Layers/Bindings) in one call to hydrate the GUI's editor, since the whole file is small and the GUI needs the full picture regardless of write granularity. `GetState() -> (profile: s, layer: s, active_toggles: as)` is separate — the live runtime triple (active Profile, active Layer, active Toggles), for the GUI to sync on connect/reopen (the Daemon runs independent of the GUI, per CONTEXT.md).

**Signals** — live push for all three runtime-state pieces, not poll, since Layer changes on every Mode-key press/release and Toggles change the instant a toggle key fires — both too latency-sensitive/frequent for polling to make sense for a tray icon meant to reflect state live. Three separate signals (not one bundled `StateChanged`), since the three change at very different rates and bundling would re-send the toggle list on every Layer flip for nothing:
- `ActiveProfileChanged(name: s)`
- `ActiveLayerChanged(layer: s)` — `"base"` / `"held"`
- `ActiveTogglesChanged(active_inputs: as)` — full current snapshot every time, not a delta; D-Bus signals aren't guaranteed-delivery, so a snapshot self-corrects if one is ever missed, and the payload (at most 20 `Input`s) is tiny regardless.

**Wire-encoding conventions**, informed by a research check confirming `zvariant`'s derive macros only auto-marshal enums-with-data when every variant shares the same field shape — which `Input`, `Action`, and `MacroStep` do not:
- `Input` → plain string (`"s"`), reusing the exact `Display`/`FromStr` form already built in [Decide Daemon data model](./06-decide-daemon-data-model.md) for the TOML file (`grid_r1c1`, `mode_key`, `thumbstick_up`, `wheel_scroll_up`, …). Sidesteps the heterogeneous-enum marshalling problem entirely for this type — no custom `zvariant::Type` impl needed.
- `Action` / `MacroStep` → `a{sv}` dict with a `"type"` tag key (e.g. `{"type": "keypress", "key": "KEY_F1", "modifiers": [...]}`, `{"type": "macro", "steps": [...]}}`), hand-written `Serialize`/`Deserialize` on the Rust side. Chosen over a JSON-string-in-`"s"` fallback: the research found `a{sv}`-with-tag is genuinely idiomatic/common D-Bus practice, keeps the wire format introspectable via `dbus-send`/`d-feet`, and doesn't undercut [ADR-0004](../../../docs/adr/0004-dbus-for-daemon-gui-ipc.md)'s own stated reason for choosing D-Bus (introspection tooling) the way an opaque JSON blob would.
- `TriggerMode`, `Layer`, mode-key role → plain strings (`"fire_once"`/`"hold_to_repeat"`/`"toggle"`, `"base"`/`"held"`, `"layer_switch"`/`"bound"`) — unit-only enums, same style, no ambiguity.
- `Binding` (Trigger mode + Action) → one `a{sv}` bundling both, so a single `SetBinding` call carries one self-contained blob rather than parallel trigger/action arguments.
- `GetConfig()`'s return recursively reuses these same conventions (profiles → layers → bindings → `Input` string keys → `Binding` `a{sv}`) rather than a second encoding scheme for the same types.
- Python side: `dbus-python` is effectively dead; implementation should use `dbus-fast`/`dbus-next` or PyGObject's `Gio.DBusProxy` (already in the GTK4 GUI's dependency tree) — `a{sv}` arrives as a plain Python `dict`.

**Errors** — a small set of named D-Bus errors under `com.acheron.Daemon.Error.*`, not one generic error or one per validation rule, so the GUI can respond differently without string-matching a message body:
- `com.acheron.Daemon.Error.NotFound`
- `com.acheron.Daemon.Error.AlreadyExists`
- `com.acheron.Daemon.Error.InvalidBinding`
- `com.acheron.Daemon.Error.IoError`

No new tickets surfaced. This closes out the D-Bus surface branch of the map.

## Correction (from [Decide systemd service packaging](./10-decide-systemd-service-packaging.md))

`GetState()` gains a fourth field, `device_connected: b`, alongside `profile`/`layer`/`active_toggles` — whether the Daemon's `CaptureSource` currently sees the Tartarus Pro's device nodes (per ticket 10's device-absent-is-non-fatal poll loop, correcting [ticket 07](./07-design-daemon-capture-event-loop.md)). A new signal, `DeviceConnectionChanged(connected: b)`, is added alongside the other three for the same live-push reason: the GUI needs to reflect this changing while its window is open, not just on connect.
