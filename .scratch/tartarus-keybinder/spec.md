Status: ready-for-agent

# Acheron — Tartarus Pro keybinding MVP

## Problem Statement

The Razer Tartarus Pro's own remapping/macro software, Synapse, is Windows-only and cloud-backed. Its Linux driver (OpenRazer) only wires up lighting for this device — no macro or remap surface exists for it (ADR-0002). A user of this device on Linux (Ubuntu, GNOME Shell, Wayland) has no way to remap its 20 grid keys, Mode key, thumbstick, or scroll wheel, define multi-key Macros, or use the device's Mode key as a Layer-shift the way Synapse's "Hypershift" does on Windows — short of adopting a generic remapper that has no natural model for this device's Layer/multi-node-input shape (ADR-0001).

## Solution

A personally-used MVP Linux application, **Acheron**, with two cooperating processes:

- A **Daemon** (Rust) that captures the device's evdev events directly, resolves each physical press against the active **Profile** and **Layer** to a **Binding**, and injects the resulting **Action** (Keypress or Macro) via a `uinput` virtual device — independent of whether the GUI is running.
- A **GUI** (Python + GTK4) that edits Profiles/Layers/Bindings/Macros and shows live daemon/device state, talking to the Daemon exclusively over D-Bus (ADR-0004). The GUI never touches config storage directly.

The Daemon owns `~/.config/acheron/config.toml` exclusively; the GUI mutates it only through D-Bus calls that apply immediately, with no draft/save step. The Daemon runs continuously as a `systemd --user` service, started at login and safety-net-started by the GUI on its own launch.

## User Stories

1. As a Tartarus Pro owner on Linux, I want any of the device's 20 grid keys remapped to a different key or shortcut, so that I can use the pad for arbitrary keyboard shortcuts instead of its default QWERTY-shaped output.
2. As a user, I want to bind the Mode key itself to a Keypress/Macro instead of using it as a Layer-shift, so that I can repurpose it per-Profile if I don't need Hypershift-style layering in that Profile.
3. As a user, I want the Mode key to default to acting as a momentary Layer-shift, so that holding it gives me a second full set of Bindings without extra configuration.
4. As a user, I want the thumbstick's four directions individually bindable, so that I can use it for arrow-key or WASD-style navigation remaps.
5. As a user, I want the scroll wheel's up/down scroll and middle click individually bindable, so that I can repurpose it beyond generic mouse-wheel scrolling.
6. As a user, I want to define multiple named Profiles (e.g. "gaming", "editing"), so that I can switch my whole Binding set to match what I'm doing.
7. As a user, I want Profile switching to be entirely manual, so that my Bindings never change underneath me based on which window happens to have focus.
8. As a user, I want each Profile to have its own Base and Held (Mode-key-held) Layer, so that every Profile can offer a full second set of Bindings via Hypershift-style layering.
9. As a user, I want a Binding's Action to be a single Keypress (including modifier chords like Ctrl+Shift+T), so that simple remaps don't require the Macro machinery.
10. As a user, I want a Binding's Action to instead be a hand-specified Macro — an ordered sequence of Keypresses, each with its own delay — so that one physical press can produce a scripted multi-key sequence.
11. As a user, I want each Binding to have a Trigger mode of Fire-once, Hold-to-repeat, or Toggle, independent of whether its Action is a Keypress or a Macro, so that I control how repeated/continuous input works per-Binding.
12. As a user, I want Fire-once Bindings to fire exactly once per physical press, so that a single tap never produces duplicate output.
13. As a user, I want Hold-to-repeat Bindings to keep re-firing for as long as I hold the physical Input, using the device's native key-repeat cadence, so that I don't need a separate repeat-rate setting.
14. As a user, I want Toggle Bindings to start running continuously (a looping Macro, or a held-down Keypress) on one press and keep running until I press the same Input again, so that I can free my hand rather than holding a key down.
15. As a user, I want a running Toggle to survive incidental Mode-key/Layer changes (e.g. grazing the Mode key while reaching for another control), so that a Layer flicker never kills an in-progress Macro loop or held key.
16. As a user, I want every active Toggle force-stopped the moment I switch Profile, so that nothing keeps running unbounded into a Profile it no longer makes sense in.
17. As a user, I want pressing the same physical key that started a Toggle to always stop that Toggle first — even if the current Layer has since rebound that key to something else — so that I always have one discoverable, reliable "off switch."
18. As a user, I want a Binding I haven't configured to passthrough its Input's normal keycode unchanged, so that I don't have to explicitly re-map all 28 Inputs just to keep default behavior on the ones I don't care about.
19. As a user, I want my configuration stored in one plain, human-readable TOML file, so that I can hand-edit or back it up outside the GUI if I want to.
20. As a user, I want the GUI to be the only supported way to edit config while the Daemon is running, so that there's never a conflicting concurrent writer to the file.
21. As a user, I want every edit I make in the GUI (rename a Profile, change a Binding, add a Macro step) to apply immediately, so that I never have to remember to hit an explicit "save" or "apply."
22. As a user, I want the GUI's main view to visually mirror the physical layout of my Tartarus Pro (grid, wheel, thumbstick, Mode key, key 20 paddle), so that I can click the control I'm looking at on the device rather than hunting through an abstract list.
23. As a user, I want a secondary, denser list view of all my current Bindings, so that I can audit or bulk-edit what's currently mapped without clicking through the device picture one control at a time.
24. As a user, I want that denser list view to default to showing only Bindings I've actually configured (not all 28 passthrough Inputs), so that it stays uncluttered, with an option to reveal the rest.
25. As a user, I want the same Binding-editing UI (Trigger mode, Keypress vs. Macro, key/modifier fields, Macro step list) available from both the device-picture view and the list view, so that editing behaves identically regardless of how I got there.
26. As a user, I want a tray icon showing my currently active Profile and Layer, so that I have at-a-glance status without opening the main window.
27. As a user, I want a quick-switch menu on the tray icon to change my active Profile, so that I don't need to open the main window just to switch Profiles.
28. As a user, I want the GUI to show me, without opening a terminal, whether the Daemon is currently running and whether the Tartarus Pro is currently connected, so that I understand why my Bindings aren't taking effect if something's wrong.
29. As a user, I want that Daemon-running/device-connected status visible both in the main window and in the tray, so that I get the same answer regardless of which I glance at.
30. As a user, I want Binding-editing blocked (with a clear on-screen reason) whenever the Daemon isn't running or the device isn't connected, so that I don't create the false impression that edits are taking live effect when they can't be.
31. As a user, I want the Daemon to keep running and quietly recover once I plug the Tartarus Pro back in — after booting before plugging it in, or unplugging it mid-session — rather than crashing or requiring a manual restart, so that a routine unplug/replug doesn't need me to intervene.
32. As a user, I want the Daemon to start automatically when I log in, so that remapping is active without me having to launch anything by hand.
33. As a user, I want the GUI, on its own launch, to make sure the Daemon is running (starting it if it isn't, clearing any stuck failure state), so that opening the GUI is a reliable way to get everything working again.
34. As a user, the very first time I run Acheron with no existing config file, I want to land straight in a working GUI against a sensible empty default (one Profile, both Layers present, everything passthrough) with no separate onboarding wizard, so that I can start binding controls immediately.
35. As a user, if my config file is ever corrupt or unparseable, I want the Daemon to refuse to start rather than silently discarding or rewriting my file, so that a hand-edit mistake never causes silent data loss.
36. As a developer maintaining the Daemon, I want its input-capture step hidden behind an internal abstraction (not hard-wired to "capture is evdev" throughout the config schema, D-Bus surface, or Binding logic), so that a second capture source could be added later without a rewrite, even though nothing analog is being built now.

## Implementation Decisions

### Scope and hardware

- Target device: Razer Tartarus Pro, USB ID `1532:0244`. Enumerates as three evdev nodes — `main` (Mode key `KEY_LEFTALT` + all four thumbstick directions as `KEY_UP/DOWN/LEFT/RIGHT`, no center click), `if01` (the 20-key grid as standard keycodes, confirmed 1:1 table in the "Enumerate physical inputs" ticket), `if02` (scroll wheel `REL_WHEEL`/`REL_WHEEL_HI_RES` + middle click `BTN_MIDDLE`).
- Target system: Ubuntu, GNOME Shell, Wayland only. Other desktop environments/display servers are out of scope.
- No distro packaging; a local `install.sh` is the only install path.

### Domain model (Daemon, Rust)

- `Input`: composite enum — `ModeKey`, `Grid(row, col)`, `Thumbstick(Direction)`, `Wheel(WheelEvent)` — not a flat enum, preserving the grid's 2D shape. Custom `Display`/`FromStr` serializes to flat snake_case strings (`mode_key`, `grid_r1c1`, `thumbstick_up`, `wheel_scroll_up`, `wheel_middle`) used identically in TOML and on the D-Bus wire.
- `Layer`: closed 2-variant enum, `Base` / `Held`. Every Profile always has both present at the type level (fixed hardware fact — one Mode key).
- Mode key is independently bindable: a per-Profile `mode_key_role: { LayerSwitch, Bound }` field routes its events either into today's Layer-activation behavior or through the identical Binding lookup any other Input uses, full Trigger-mode support included. Held-layer bindings are retained (not deleted) when unreachable under `Bound`, to avoid data loss if the role flips back.
- Lookup: per Layer, a sparse `HashMap<Input, Binding>`; an absent entry means passthrough (re-emit the Input's original keycode unchanged). Profiles are `HashMap<String, Profile>` keyed by name.
- `Action` (config-facing): `Keypress { modifiers: Modifiers, key: evdev::Key }` or `Macro { steps: Vec<MacroStepDTO> }`, where `MacroStepDTO` is `{ KeyDown(Key), KeyUp(Key), Delay(ms) }` — keyboard-only for MVP, matching CONTEXT.md's Macro definition; non-keyboard macro steps (wheel/thumbstick injection) are a deferred, additive enum-variant extension, not built now.
- Runtime `Binding`: both Action kinds compile at config-load time into one `steps: Vec<MacroStep>`, run by one shared executor — a Keypress becomes a canned modifier-down/key-down/key-up/modifier-up sequence. This is the only place Trigger-mode firing/stopping logic lives.
- `TriggerMode`: `FireOnce`, `HoldToRepeat` (bare unit variant, driven by the device's native evdev autorepeat — no separate repeat-interval config), `Toggle` (loops the step sequence until stopped).
- Active toggles: `HashMap<Input, ActiveToggle>`, each tracking the live `HashSet<Key>` of currently-down keys (updated as the executor processes `KeyDown`/`KeyUp` steps) plus a `CancellationToken`. The stop-key mechanism force-releases exactly the tracked keys rather than trusting the Macro to be well-formed. Profile switch clears the whole map at once.
- Config file: single TOML file at `~/.config/acheron/config.toml`, top-level `schema_version = 1` from the start. Sample shape (from the data-model ticket's prototype-grade snippet, illustrative not exhaustive):

```toml
schema_version = 1

[profiles.gaming]
mode_key_role = "layer_switch"

[profiles.gaming.layers.base.bindings.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }

[profiles.gaming.layers.held.bindings.grid_r2c1]
trigger = "toggle"
action = { type = "macro", steps = [
  { key_down = "KEY_A" }, { delay_ms = 50 }, { key_up = "KEY_A" }, { delay_ms = 100 },
] }
```

### Toggle behavior across Layer/Profile switches

- Layer change (Mode key pressed/released) never touches an active Toggle — it keeps running through any number of Layer transitions.
- Profile switch releases every active Toggle immediately, as part of the switch.
- Pressing the physical key that has an active Toggle always stops that Toggle first, regardless of what Binding the current Layer nominally assigns to that key now; only once stopped does the key resume evaluating the current Layer's own Binding.
- A Toggle's identity is pinned to the physical key that started it, independent of the live Binding lookup.

### Daemon event loop and concurrency

- One `tokio` runtime for the whole Daemon.
- `CaptureSource` abstraction: the only external contract is "produces a stream of normalized `PhysicalEvent { input, state: Down | Repeat | Up }` into the shared channel." The evdev implementation — three `spawn_blocking` background tasks (one per node: `main`/`if01`/`if02`), normalizing evdev's raw `EV_KEY` value onto Down/Repeat/Up — lives entirely behind this seam. This is the swappable capture layer the map's standing discipline calls for (a future `hidraw` analog source is a second implementation of the same trait, out of scope to build now).
- Dispatch task: single consumer of the shared `mpsc` channel, sole owner of all mutable Daemon state (active Profile/Layer, the `ActiveToggle` map) — no `Mutex` anywhere, mutation is serialized by construction. `FireOnce` fires only on `Down`. `HoldToRepeat` fires on `Down` and every subsequent `Repeat`. `Toggle` starts/stops only on `Down`.
- Firing execution: each Action firing spawns its own `tokio` task walking the compiled `Vec<MacroStep>` (`tokio::time::sleep` between `Delay` steps; loops indefinitely for `Toggle`), so Macro delays never block the dispatch task. All `uinput` writes are serialized through one dedicated injector task owning a single virtual device created once at startup and held for the process lifetime — firing tasks send write-commands over a channel rather than writing the fd directly, preventing interleaved writes from concurrently-running Toggles.
- D-Bus interleaving: `zbus`'s async server runs on the same `tokio` runtime; GUI-originated calls push a `Command` variant into the same `mpsc` channel the dispatch task already consumes alongside `PhysicalEvent`s — one state-owning consumer, no second lock or state copy.
- Failure handling splits by cause:
  - Device absent (nodes don't exist — boot-before-plugin, or a mid-run unplug) is non-fatal. `CaptureSource` polls the known `/dev/input/by-id/...` paths every ~2s until they open cleanly, then resumes. One poll loop covers both cases.
  - Genuine capture errors (e.g. a `uinput` write failure, an unexpected fd error unrelated to unplugging) are fatal — the Daemon exits and relies on systemd's `Restart=on-failure`.

### D-Bus surface (ADR-0004)

- One flat object, `/com/acheron/Daemon`, bus name `com.acheron.Daemon`, one combined interface (also `com.acheron.Daemon`) — no ObjectManager hierarchy.
- Mutating methods are atomic, per-entity, immediately validated and applied (in-memory + `config.toml` rewritten right away, no draft/save step): `CreateProfile`, `DeleteProfile`, `RenameProfile`, `SetModeKeyRole`, `SwitchProfile`, `SetBinding`, `ClearBinding`.
- Reads: `GetConfig() -> a{sv}` returns the entire document in one call (hydrates the GUI's editor). `GetState() -> (profile: s, layer: s, active_toggles: as, device_connected: b)` returns the live runtime snapshot, for the GUI to sync on connect/reopen.
- Signals (live push, not poll, since Layer/Toggle/connection state can change too fast/frequently for polling to track from a tray icon):
  - `ActiveProfileChanged(name: s)`
  - `ActiveLayerChanged(layer: s)` — `"base"` / `"held"`
  - `ActiveTogglesChanged(active_inputs: as)` — full snapshot every time, not a delta (D-Bus signals aren't guaranteed-delivery)
  - `DeviceConnectionChanged(connected: b)`
- Wire encoding: `Input` marshals as a plain string reusing its TOML `Display`/`FromStr` form. `Action`/`MacroStep`/`Binding` marshal as `a{sv}` dicts with a `"type"` tag key, hand-written `Serialize`/`Deserialize` (chosen over a JSON-string fallback to stay introspectable via `dbus-send`/`d-feet`, matching ADR-0004's own rationale for choosing D-Bus). `TriggerMode`/`Layer`/mode-key role marshal as plain tagged strings. `GetConfig()`'s return recursively reuses these same conventions.
- Errors: a small named set under `com.acheron.Daemon.Error.*` — `NotFound`, `AlreadyExists`, `InvalidBinding`, `IoError` — not one generic error or one per validation rule.
- GUI-side D-Bus client: PyGObject's `Gio.DBusProxy` (already in the GUI's dependency tree) or `dbus-fast`/`dbus-next` — not `dbus-python` (effectively dead). `a{sv}` arrives as a plain Python `dict`.

### GUI information architecture (GTK4)

**Build from the prototype directly, not from the prose below.** [`prototype/09-gui-information-architecture/prototype.py`](../../prototype/09-gui-information-architecture/prototype.py) is a live, running GTK4 implementation of Device Overview, the Action Table sidebar, the shared Binding editor, and the tray mock — it is the primary source for exact widget structure, layout math (grid spans, rotation, popover placement), and the `DaemonStub` shape, kept in place on `main` for this reason rather than left in a throwaway branch. [`prototype/12-daemon-device-status-indicators/prototype.py`](../../prototype/12-daemon-device-status-indicators/prototype.py) is the equivalent live source for the status chip, tray status line, and disabled-grid overlay, and reuses (rather than duplicates) ticket 09's `DaemonStub`/Device Overview/tray mock. The bullets below summarize the decisions those prototypes settled, for scanning and cross-referencing against tickets — treat the prototype code, not this summary, as authoritative on layout/structure details when the two would ever disagree.

- One main top-level view, **Device Overview**, mirroring the physical device: a `Gtk.Grid` for the 20-key pad (rows 1–3 five-wide, row 4 four-wide for keys 16–19), the wheel (scroll up / middle click / scroll down) occupying a continuation of column 5 starting at row 4, the thumbstick as a diamond of four buttons rotated 90° clockwise from a naive N/S/E/W layout (top lobe = Left, left lobe = Down, right lobe = Up, bottom lobe = Right — each showing the arrow glyph matching its own passthrough evdev output, not its screen position) with the Mode key as a circular button above the diamond's top lobe, and key 20 as a separate paddle-shaped button below the diamond (not inline with the grid). Profiles are a left-hand sidebar of buttons; Base/Held is a two-button tab row. Clicking any device control opens its Binding editor in a popover.
- **Action Table**: a collapsible sidebar off Device Overview, closed by default, with no Profile/Layer pickers of its own (reflects whatever Device Overview has selected). One expandable row per Input; only bound Inputs shown by default, with a "Show all inputs" checkbox to reveal passthrough rows. Sidebar-open state is GUI-only view state, kept separate from Daemon-mirroring state so it survives re-renders across edits/Profile switches.
- A third "focused wizard" navigation variant (dot-overview strip + one-binding-at-a-time editor) was prototyped and explicitly rejected — Device Overview plus the Action Table sidebar covers both the spatial and dense-list use cases without a third mode.
- The Binding editor (Trigger-mode dropdown, Keypress/Macro toggle, key+modifier fields, Macro step add/remove list) is one shared component used identically from both Device Overview's popovers and the Action Table's expandable rows.
- Tray icon (real implementation uses `AppIndicator3`/`AyatanaAppIndicator3`): shows active Profile/Layer as status text, exposes a "Quick switch" popover listing Profiles, and carries the same status line described below.
- Status indicators: a status chip (colour dot + label) above Device Overview, and a matching line in the tray, both reflecting all three reachable states (running+connected, running+disconnected, not running) from the same `GetState()`/signal data. Whenever status isn't running+connected, the entire Device Overview grid is disabled (`set_sensitive(False)`) under a dimmed `Gtk.Overlay` with a centered message naming which condition is unmet ("Daemon not running — start it to edit Bindings" / "Device disconnected — plug in the Tartarus Pro to edit Bindings"). Implementation note carried from the prototype: centering that overlay message needs `hexpand=True`/`vexpand=True` on the label in addition to `halign`/`valign = CENTER`, or it left-aligns.
- Daemon-presence detection: live `NameOwnerChanged` watch on `com.acheron.Daemon` on the session bus, not a one-shot check on window open (the Daemon can crash while the window is open).
- No separate onboarding wizard for first run — the GUI opens straight to Device Overview against the seed `Default` Profile (all-passthrough grid); that view is the first-run experience.

### Config lifecycle

- Missing file: the Daemon creates `~/.config/acheron/` and `config.toml` itself on startup and writes it immediately (not lazily on first mutation) — the file on disk always matches in-memory state. Seed content: one Profile named `Default`, `schema_version = 1`, both Layers present with empty Binding maps (all passthrough), `mode_key_role = "layer_switch"`, set active. No "echo" Bindings reproducing the onboard mapping.
- Corrupt/unparseable file (parse failure or unsupported `schema_version`): the Daemon refuses to start, exits non-zero with a clear parse error to the journal — no silent backup-and-reseed.

### Packaging and lifecycle

- `systemd --user` unit at `~/.config/systemd/user/acheron-daemon.service` — not a system unit (the permission story — `plugdev` group, existing `/dev/uinput` ACL — is already solved at the user level for this device/user).
- Unit: `Type=simple`, `ExecStart=%h/.local/bin/acheron-daemon`, `After=graphical-session.target`, `WantedBy=default.target`, `Restart=on-failure`, `RestartSec=1`, `StartLimitIntervalSec=60`, `StartLimitBurst=5`. Logs go to the default journal.
- Install: a small idempotent `install.sh` — build release binary, copy to `~/.local/bin/acheron-daemon`, copy unit file, `systemctl --user daemon-reload`, `systemctl --user enable --now acheron-daemon`. No distro packaging.
- Autostart is two-layered: login-enabled via `WantedBy=default.target` as the primary trigger, plus the GUI calling `org.freedesktop.systemd1.Manager.ResetFailed` then `StartUnit` over the session D-Bus connection on its own launch, as a safety net (no `systemctl` shell-out, no subprocess).

### Permissions confirmed on the development machine (re-verify on fresh install)

- User is in the `plugdev` group; reading the three device nodes works without extra setup.
- `/dev/uinput` write access works without extra setup for this user via a pre-existing ACL (`user:justin:rw-`, mode 660) — origin of that ACL wasn't investigated, so re-check on a fresh install; the app installer may need to set up a udev rule itself on a machine that lacks it.

### Explicitly out of scope for this spec

See "Out of scope" below — carried through unchanged from the map.

## Testing Decisions

- Good tests here exercise observable behavior through the two seams below, not internal state — assert on injected `uinput` writes / emitted D-Bus signals for the Daemon, and on rendered widget state / D-Bus calls made for the GUI, never on private struct fields.
- **Daemon seam — `CaptureSource`**: a fake implementation feeds synthetic `PhysicalEvent { input, state }` sequences into the dispatch task's channel, in place of the real evdev-reading background tasks. This exercises the real dispatch task, Trigger-mode firing (`FireOnce`/`HoldToRepeat`/`Toggle`), the Toggle-survives-Layer/dies-on-Profile-switch rules, the stop-key-always-wins mechanism, and D-Bus method/signal behavior — all without real hardware. Primary coverage target: everything under "Daemon event loop and concurrency" and "Toggle behavior across Layer/Profile switches" above.
- **GUI seam — the D-Bus client boundary**: a fake Daemon object implementing `com.acheron.Daemon`'s interface (the `DaemonStub` pattern already exercised live in the ticket 09 and ticket 12 prototypes) stands in for the real Daemon process. This exercises Device Overview rendering and click-to-edit, the Action Table sidebar's filter/show-all and open-state persistence across re-renders, the shared Binding editor, tray status text/quick-switch, and the status-chip/disabled-grid-overlay behavior across all three connection states — without a real Daemon or device.
- No cross-process/full end-to-end automated test against real hardware is specified here — the two prototypes already validated their respective UI/behavior live against a `DaemonStub` and against real evdev/uinput respectively (tickets 02, 09, 12); this spec's testing scope is the two seams above for whoever implements the real Daemon and GUI.
- Prior art: [prototype/09-gui-information-architecture/prototype.py](../../prototype/09-gui-information-architecture/prototype.py) and [prototype/12-daemon-device-status-indicators/prototype.py](../../prototype/12-daemon-device-status-indicators/prototype.py) already demonstrate the `DaemonStub` pattern live against real GTK4 widgets — the GUI seam's tests should reuse that stub shape rather than inventing a new one.

## Out of Scope

- Lighting/RGB and any OpenRazer integration for it (other tools already cover this).
- Automatic, focused-application-based Profile switching — Profile switching is always manual.
- Support for desktop environments or display servers other than this system's GNOME/Wayland setup.
- Live macro recording (capturing real keypresses/timing) — MVP Macros are hand-specified only.
- Analog/pressure-sensitive Bindings for the grid keys. Real hardware signal confirmed to exist (genuine Razer Analog Optical switches, 0–255 depth per key), but it rides an undocumented vendor-mode-unlocked `hidraw` report outside the evdev+uinput design, and Linux `hidraw` compatibility with the community protocol writeup is an inference, not a verified fact (the reference project, `open-tartarus-driver`, is Windows-only). Would also need a new continuous-input Binding concept beyond the discrete Trigger modes. See `research/analog-pressure-sensitivity.md`.
- Non-keyboard Macro steps (wheel/thumbstick injection as part of a Macro) — the `MacroStep` enum is keyboard-only for MVP; noted as a deferred, additive extension.
- A second `CaptureSource` implementation (e.g. `hidraw` for analog) — only the internal seam for one is being kept open, per the standing architectural discipline; nothing analog is being built.
- distro packaging (`.deb`/AUR) — `install.sh` is the only install path for this personally-used MVP.
- udev/netlink hotplug monitoring for device reconnect — a ~2s poll loop is the chosen mechanism instead.
- A GUI onboarding wizard/welcome dialog — Device Overview against the seed Profile is the first-run experience.

## Further Notes

- Tool name is **Acheron** (settled mid-map, during the config-file-format ticket) — config lives under `~/.config/acheron/`, D-Bus bus name is `com.acheron.Daemon`.
- Every ticket on the map (`.scratch/tartarus-keybinder/map.md`) is resolved as of this spec; two corrections were made retroactively to earlier tickets during later ones (ticket 10 corrected ticket 07's "any capture failure is fatal" to split device-absent from genuine errors, and correspondingly added a field/signal to ticket 08's D-Bus surface) — this spec already reflects the corrected, final state, not the original tickets in isolation.
- CONTEXT.md's glossary (Profile, Layer, Mode key, Input, Binding, Action, Keypress, Macro, Trigger mode, Fire-once, Hold-to-repeat, Toggle, Daemon, GUI) is the authoritative vocabulary for implementation work on this spec; avoid the synonyms it explicitly rules out (e.g. "driver"/"service"/"agent" for Daemon, "mapping"/"keybind" for Binding, "mode"/"shift state" for Layer).
