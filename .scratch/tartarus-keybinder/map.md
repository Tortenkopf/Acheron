Label: wayfinder:map

## Destination

A working, personally-used MVP Linux application, named **Acheron**, providing offline keybinding and macro control for the Razer Tartarus Pro. A Rust Daemon captures evdev and injects via uinput, implementing Profiles, Layers (Mode-key), and per-Binding Trigger modes (Fire-once, Hold-to-repeat, Toggle) for both Keypress and Macro Actions. A Python+GTK4 GUI edits configuration and shows a tray icon, talking to the Daemon over D-Bus. See CONTEXT.md for vocabulary and docs/adr/ for the architecture rationale (0001-0004).

## Notes

**This map carries execution** — resolving a ticket means actually building and testing against the real, connected Tartarus Pro, not only deciding. Task-type tickets are real implementation/investigation work, not just discussion.

**Grounding facts** (see CONTEXT.md for vocabulary, docs/adr/ for architecture rationale):
- OpenRazer's driver for this device exposes lighting only — no macro/remap DBus surface (ADR-0002).
- The device enumerates as three evdev nodes: `main` emits the Mode key (`KEY_LEFTALT`) *and* the thumbstick (`KEY_UP`/`DOWN`/`LEFT`/`RIGHT`, 4-way, no center click); `if01` emits the 20-key grid (4×5) as standard keycodes; `if02` emits the scroll wheel (`REL_WHEEL`/`REL_WHEEL_HI_RES`) and its middle click (`BTN_MIDDLE`). Full Input → (node, code) table: [Enumerate physical inputs](./issues/01-enumerate-physical-inputs.md).
- System: Ubuntu, GNOME Shell 50.1, Wayland only (other DEs/display servers are out of scope). `ubuntu-appindicators` extension is active, so tray icons render without extra setup. User is in the `plugdev` group; reading the three device nodes worked without extra permission setup during investigation. `uinput` write access also works without extra setup — `/dev/uinput` carries an explicit ACL (`user:justin:rw-`) — confirmed end-to-end (evdev grab -> uinput inject -> real text field) in [Prove evdev/uinput pipeline](./issues/02-prove-evdev-uinput-pipeline.md).
- No Rust toolchain currently installed; Python 3.14 with GTK3/4 (PyGObject) is available. No PyQt/PySide, no Rust/Go/Node currently installed.
- Tartarus Pro USB ID: `1532:0244`.

**Skills to consult**: default to `/grilling` and `/domain-modeling` for decision tickets, per Wayfinder's ticket types. Use `/prototype` for "how should it look/behave" questions (GUI layout, tray menu design). Keep CONTEXT.md and docs/adr/ updated as the model sharpens further.

**Standing architectural discipline — keep the Daemon's capture layer swappable**: analog per-key depth sensing is real hardware on this device but is deliberately out of scope for the MVP (see Out of scope — `open-tartarus-driver` research). It rides a raw-HID stream evdev can't see, requiring a second capture path and a continuous-input concept the current `Binding`/`Trigger mode` model doesn't have room for (see [research/analog-pressure-sensitivity.md](./research/analog-pressure-sensitivity.md)). Nothing needs building for this now, but when designing the Daemon's internal input-capture layer, don't hard-wire config schema, the D-Bus surface, or Binding logic to assume "capture == evdev" — keep capture behind an internal abstraction so a second source (`hidraw` analog) could be added later without a rewrite. A light discipline to hold while building the MVP, not a reason to build anything analog-related now.

## Decisions so far

- [Analog/pressure-sensitivity research](./issues/05-research-analog-pressure-sensitivity.md) — Tartarus Pro grid keys have real analog-optical depth sensing, but it rides an undocumented raw-HID report outside the evdev nodes the daemon uses, so it's out of scope rather than a cheap extension.
- [Enumerate physical inputs](./issues/01-enumerate-physical-inputs.md) — full Input → (node, evdev code) table for all 20 grid keys, Mode key, thumbstick, and wheel captured live; corrects the earlier assumption that the thumbstick is on `if02` — it's actually on `main`, alongside the Mode key.
- [Prove evdev/uinput pipeline](./issues/02-prove-evdev-uinput-pipeline.md) — grab-and-inject mechanism confirmed end-to-end live; `uinput` write access works for this user with no extra setup (pre-existing ACL on `/dev/uinput`); no change to the Rust Daemon choice in docs/adr/0003.
- [Decide config file format](./issues/03-decide-config-file-format.md) — Daemon owns `~/.config/acheron/config.toml` exclusively (single TOML file, `schema_version = 1`); GUI edits only via D-Bus, never touches the file. Also settled the tool's name: **Acheron**.
- [Decide toggle behavior across switches](./issues/04-decide-toggle-behavior-across-switches.md) — active toggles survive Layer changes but are killed on Profile switch; the physical key that started a toggle always stops it first, regardless of what its current Layer binds it to.
- [Decide Daemon data model](./issues/06-decide-daemon-data-model.md) — composite `Input` enum, sparse `HashMap<Input, Binding>` per Layer (passthrough when absent), Mode key is independently bindable per-Profile (`LayerSwitch` vs `Bound`) through the same lookup, Keypress and Macro both compile to one `Vec<MacroStep>` executor, and active Toggles track live held keys for a clean force-release on stop.
- [Design Daemon capture/injection event loop](./issues/07-design-daemon-capture-event-loop.md) — single `tokio` runtime, actor-style dispatch task owns all state via one shared channel (evdev, D-Bus, and firing tasks all feed it — no locks); firing spawns a task per Action with `uinput` writes serialized through one injector task; capture lives behind a `CaptureSource` seam; capture failure is fatal, deferring recovery to systemd.
- [Decide D-Bus interface surface](./issues/08-decide-dbus-interface-surface.md) — one flat object (`/com/acheron/Daemon`, one combined interface), atomic per-entity mutating methods that apply immediately (no draft/save step), bulk `GetConfig()` + separate `GetState()`, three live-push signals (`ActiveProfileChanged`/`ActiveLayerChanged`/`ActiveTogglesChanged`, full-snapshot toggles), `Input` reuses its existing TOML string form on the wire, `Action`/`MacroStep` marshal as `a{sv}` dicts with a type tag (not JSON), and a small set of named `com.acheron.Daemon.Error.*` errors.
- [Design GUI information architecture](./issues/09-design-gui-information-architecture.md) — one main view, **Device Overview**, that visually mirrors the physical Tartarus Pro (grid + wheel-as-5th-column + rotated thumbstick diamond + circular Mode key + separate key-20 paddle, per Device Picture.jpg/layout.md); **Action Table** demoted from a standalone page to a closed-by-default collapsible sidebar off it, reusing the same Profile/Layer selection rather than duplicating pickers; a third "focused wizard" variant was built, compared, and dropped. Live prototype: [prototype/09-gui-information-architecture/prototype.py](../../prototype/09-gui-information-architecture/prototype.py).

## Not yet specified

(empty — every item graduated into a ticket below; see the frontier for what's next)

## Out of scope

- Lighting/RGB and any OpenRazer integration for it — other tools (RazerGenie, Polychromatic) already cover this, and even Synapse itself delegates lighting to a companion app.
- Automatic, focused-application-based Profile switching.
- Support for desktop environments or display servers other than this system's GNOME/Wayland setup.
- Live macro recording (capturing real keypresses/timing) — MVP Macros are hand-specified only; recording could become a separate future effort after the MVP ships.
- Analog/pressure-sensitive Bindings for the grid keys — the Tartarus Pro's keys are genuine Razer Analog Optical switches with a real per-key depth signal, protocol reverse-engineered by a community driver (`open-tartarus-driver`, Windows-only — Linux `hidraw` compatibility is a reasonable inference, not a verified fact), but it requires an undocumented vendor mode-unlock command and a separate `hidraw` capture path outside the evdev+uinput design (ADR-0002), plus a new continuous-input Binding concept beyond discrete Trigger modes. See [research/analog-pressure-sensitivity.md](./research/analog-pressure-sensitivity.md).
