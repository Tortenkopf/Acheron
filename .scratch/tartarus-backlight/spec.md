Status: ready-for-agent

# Acheron — Profile Lighting assignment

## Problem Statement

The Razer Tartarus Pro has an RGB backlight matrix across its 20 grid keys and scroll wheel
(the Mode key and thumbstick are solid black plastic — not RGB-capable at all). Acheron already
drives the device's three fixed-colour Status LEDs per Profile ([`tartarus-status-leds`](../tartarus-status-leds/spec.md)),
but nothing on Linux drives this larger backlight: OpenRazer's own daemon supports it generically
(it is Acheron's primary source for the wire protocol, see below), but Acheron does not integrate
OpenRazer for remapping or lighting (ADR-0002), and there is no other Linux path to it. With the
GUI and tray closed, or even open, a user has no way to give each Profile its own look the way
Synapse does on Windows.

This effort is **ungated** — unlike `tartarus-status-leds`, there is no kill-gate prototype.
ADR-0006 already proved the extended-matrix hidraw transport is host-controllable on this exact
device (Status LEDs ride the same command family, a different LED id); this effort only needed
the backlight-specific byte layout ([ticket 01](./issues/01-research-backlight-wire-protocol.md))
and one hardware-verification pass ([ticket 02](./issues/02-hardware-verification.md)), not a
feasibility test that could kill the effort outright. Both are done: ticket 01 produced an
implementation-ready wire spec for 11 of 13 effects, both custom-layout commands, and brightness,
all cited to the vendored OpenRazer 3.12.4 driver/daemon source; ticket 02 verified the
source-derived frames live on the real device (fw v1.2), settled the one genuine "needs
hardware" gap source couldn't answer (the scroll wheel's column index), and found no adverse
device behaviour across ~15 writes plus a relock.

## Solution

Every **Profile** carries a **Lighting assignment** — either a **Fixed effect** (one of the
firmware's built-in, device-autonomous effects: Static, Spectrum, Reactive, Wave, Breath, or
Starlight) or a **Custom layout** (a fixed colour per matrix cell across the 20 grid keys + the
scroll wheel), plus a **brightness** level (`0x00`–`0xFF`, raw wire value) that applies
regardless of which is active. Fixed effect and Custom layout are mutually exclusive — a
Profile is in exactly one Lighting state at a time, matching firmware reality (only one
matrix-effect command is "active" at once) and the same one-state-per-Profile shape Status LEDs
already use. A new Profile defaults to **Off** (matrix dark), mirroring Status LEDs'
migration-safe all-off default.

The Daemon asserts the active Profile's Lighting assignment whenever that Profile becomes
active, on Daemon startup, and on every device (re)connect — the same assertion discipline as
Status LEDs, for consistency and to guarantee the device matches config after any possible
desync, even though (unlike Status LEDs) the firmware does not necessarily reset Lighting on
every enumeration (see "Startup / shutdown behaviour"). Lighting shares the existing `led` task
with Status LEDs rather than a new task, since the device has one control channel and lighting
frames — Status-LED or backlight — must never interleave (ADR-0006's own forward note).

The assignment is edited from a new **Lighting** tab in the GUI (a third arm of the Device
Overview destination switch, alongside Grid and Library) — a flat mode selector plus
per-effect parameter controls, a paintable grid for Custom layout, and an always-visible
brightness slider — and stored per Profile in `config.toml`.

This spec is the hand-off to a **separate implementation effort**. Every decision below is
settled by the map (`.scratch/tartarus-backlight/map.md`) and its resolved tickets; nothing here
is left open.

## User Stories

1. As a Tartarus Pro owner on Linux, I want each Profile to define its own backlight look — a
   built-in firmware effect or a custom per-key colour layout — so that switching Profiles
   changes the device's appearance along with its Bindings.
2. As a user, I want the backlight to follow the active Profile deterministically — switching to
   a Profile always shows exactly that Profile's Lighting assignment — so that the backlight
   never drifts from reality.
3. As a user, I want every Profile to have a defined Lighting assignment (defaulting to Off), so
   that a Profile I never configured simply leaves the matrix dark rather than showing a stale
   effect.
4. As a user, I want the Daemon to re-assert the active Profile's Lighting whenever I plug the
   device back in or restart the Daemon, so that the backlight always matches my current
   Profile.
5. As a user, I want to pick one of the firmware's built-in effects (Static, Spectrum, Reactive,
   Wave, Breath, Starlight) and set its colour(s)/speed/direction where applicable, so that I can
   use a device-autonomous look that costs the Daemon nothing once asserted.
6. As a user, I want to paint an arbitrary colour onto each of the 20 grid keys and the scroll
   wheel individually, so that I can build a fully custom static layout.
7. As a user, I want a "fill all keys" shortcut when painting a Custom layout, so that I can set
   a base colour quickly before touching up individual keys.
8. As a user, I want one brightness control that applies regardless of which effect or layout is
   active, so that I can dim the backlight without changing its colour or pattern.
9. As a user, I want to copy another Profile's whole Lighting assignment onto the one I'm
   editing, so that I don't have to rebuild a look from scratch across similar Profiles.
10. As a user, I want the Mode key and thumbstick shown in the Lighting tab for physical-layout
    fidelity but never paintable or lit, so that the tab matches the real device.
11. As a user editing `config.toml` by hand, I want the Lighting assignment stored as plain
    fields per Profile, so that I can read and change it outside the GUI.
12. As a user upgrading from a build without this feature, I want my existing `config.toml` to
    keep working unchanged, with every Profile defaulting to Off at zero brightness.

## Implementation Decisions

### The wire frames

Derived from source by [ticket 01](./issues/01-research-backlight-wire-protocol.md)
([`research/backlight-wire-protocol.md`](./research/backlight-wire-protocol.md), every claim
cited to file + line in the vendored OpenRazer 3.12.4 tree) and verified on hardware by
[ticket 02](./issues/02-hardware-verification.md) (fw v1.2,
[`assets/02-RESULTS.md`](./assets/02-RESULTS.md)). Every frame rides the same
`command_class 0x0F` / `struct razer_report` / CRC machinery as the Status LEDs
([`tartarus-status-leds/research/status-led-wire-protocol.md` §4](../tartarus-status-leds/research/status-led-wire-protocol.md#4-data_size--argument-count),
not re-derived here), on the same Interface-2 control node, `report_index = response_index =
0x02`.

**LED ids** — `BACKLIGHT_LED = 0x05` for every effect-select frame, confirmed distinct from the
Status LEDs' `SIDE_STRIPE_LED = 0x0B`. Brightness is the one exception: it targets `ZERO_LED =
0x00` (a Tartarus Pro/V2-specific quirk — every other Razer keyboard uses `BACKLIGHT_LED` for
brightness). All effect-select frames use `VARSTORE` (`arg0 = 0x01`) and `transaction_id
0x1F` — **including Breath**, whose driver source actually transmits `transaction_id 0x3F` (a
copy/paste bug: the frame is built with `0x3F`, sent, then overwritten to `0x1F` in a dead
store that's never resent). Ticket 02 confirmed both values work on hardware, so the spec uses
`0x1F` uniformly, for consistency with every other effect.

**Effect-select frames** (`command_id 0x02`), one `HIDIOCSFEATURE` write each, firmware then
runs the effect autonomously:

| effect | effect byte | `data_size` | extra args |
|---|---|---|---|
| Off (firmware's own "none") | `0x00` | `0x06` | — |
| Static | `0x01` | `0x09` | 1 colour |
| Spectrum | `0x03` | `0x06` | none — cycles autonomously |
| Wave | `0x04` | `0x06` | direction (`0x01` right / `0x02` left), fixed speed byte `0x28` |
| Reactive | `0x05` | `0x09` | speed `0x01–0x04`, 1 colour |
| Breath (random/single/dual) | `0x02` | `0x06`/`0x09`/`0x0C` | 0/1/2 colours per style |
| Starlight (random/single/dual) | `0x07` | `0x06`/`0x09`/`0x0C` | speed `0x01–0x03`, 0/1/2 colours per style |

**Custom layout — two-step write-then-arm**, confirmed from source (both the OpenRazer dbus
surface's `setKeyRow`→`setCustom` call order and `RippleManager`'s per-tick sequencing
demonstrate it) and exercised on hardware by ticket 02:

1. **`matrix_custom_frame`** (`command_id 0x03`, `data_size 0x47` fixed) — one write carrying
   `[row_index=0x00, start_col, stop_col, RGB×N]`. A full-row write uses `start_col=0x00`,
   `stop_col=0x14` (20), 21 RGB triples (63 bytes of real payload, tail zero-padded to the fixed
   71-byte frame). `arg0`/`arg1` are unused (no VARSTORE/LED-id concept on this command).
2. **`matrix_effect_custom`** (`command_id 0x02`, effect `0x08`, `data_size 0x0C`) — the
   "arm"/display trigger for whatever the last `matrix_custom_frame` write staged.
   `variable_storage`/`led_id` are hardcoded `0x00` for this one frame (not VARSTORE /
   `BACKLIGHT_LED`) — a generic trigger, not an LED-addressed command.

**Column addressing — settled on hardware, corrects the map's original charting note.** The
1×21 matrix's column order is: columns `0..18` = grid keys `1..19` in order, column `19` = the
**scroll wheel**, column `20` = grid key `20`. The wheel is second-to-last in wire order, not
the 21st/last cell as originally charted from the user's hardware memory — the kernel driver
carries no Tartarus-Pro-specific remap table, so this was genuinely undeterminable from source
and required ticket 02's single-column hardware sweep. The Mode key and thumbstick occupy no
column at all — they are solid black plastic, confirmed not RGB-capable on the real unit, not
merely unlit.

**Brightness** — `command_id 0x04` set / `0x84` get, `data_size 0x03`, `arg0 = VARSTORE`,
`arg1 = ZERO_LED (0x00)`, `arg2 = brightness`. Range is `0x00`–`0xFF` (ticket 02 confirmed
against source's prediction — **not** the 0–100 OpenRazer's own daemon maps to internally;
Acheron's `brightness: u8` field is the raw wire byte, no percentage remap). `0x00` alone blacks
out the matrix independent of whatever effect/layout is otherwise asserted (a simpler "off"
primitive than a zero-colour static frame) — confirmed on hardware, no adverse behaviour.

**Ripple / ripple-random-colour are out of scope — not a device-side command.** Ticket 01 found
OpenRazer's daemon fakes ripple with a software `RippleEffectThread` streaming ~25 Hz
`matrix_custom_frame`/`matrix_effect_custom` writes — there is no `matrix_effect_ripple` sysfs
file or driver builder anywhere in the vendored tree. That mechanism is exactly the
"host-streamed per-frame Custom-layout animation" the map's charting already excluded (see "Out
of Scope"). `FixedEffect` carries no ripple variant.

**No driver-mode call, ever.** Every frame above works regardless of Capture mode — ticket 02
ran wave, static, and all three breathing variants from `device_mode 00 00` with no issues,
mirroring the Status LEDs' own "no driver mode needed" finding. The normal→driver-mode
transition is the reset risk ADR-0006 already flagged; Lighting writes never trigger it.

Full per-effect byte tables, every claim cited to a specific driver function and line, are in
[`research/backlight-wire-protocol.md`](./research/backlight-wire-protocol.md); the live-device
run log is in [`assets/02-RESULTS.md`](./assets/02-RESULTS.md).

### Daemon architecture

Settled by [ticket 03](./issues/03-daemon-architecture-for-lighting.md). Recorded in
**[ADR-0012](../../docs/adr/0012-lighting-shares-the-led-task-no-varstore-shutdown-clear.md)**
(refines ADR-0006, which itself refines ADR-0002); this section states the shape, not the
rationale.

**Writer — the existing `led` task, extended, not a sibling.** Per ADR-0006's own forward note
("all lighting frames — now and future — route through the one `led` task"):

- A second, independent channel — `lighting_tx: watch::Sender<Option<LightingState>>` /
  `lighting_rx` — is created in `main.rs` alongside the existing `led_tx`/`led_rx`
  (`StatusLeds`), and handed to both `led::spawn` and `DispatchState`, mirroring `led_tx`'s
  wiring exactly. Rejected: one combined channel bundling `StatusLeds` and `LightingState` — it
  would force every Status-LED-only push to also resupply the current Lighting value (or vice
  versa), for no serialization benefit the two-channel/one-task shape doesn't already give.
- `led::run`'s loop becomes `loop { tokio::select! { ... } }` over both receivers. Each arm
  still `borrow_and_update()`s → `spawn_blocking`s the actual write → **awaits** it before the
  loop repeats — the serialization primitive: two receivers, one task, one write in flight at a
  time, regardless of which channel woke it.
- `capture/analog.rs` gains `assert_lighting(state: LightingState) -> io::Result<()>`, a
  standalone function modelled on `assert_status_leds` — `discover_hidraw()` → open Interface 2
  → one or two sequential `HIDIOCSFEATURE` calls (a `CustomLayout` assert is the write-then-arm
  two-step, still one short-lived fd, not two separate task iterations) → drop the fd.

**Config type — a sum type, typed fully now against ticket 01's wire facts:**

```rust
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LightingAssignment {
    #[default]
    Off,
    FixedEffect { effect: FixedEffect },
    CustomLayout { colours: [Colour; 21] },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FixedEffect {
    Static { colour: Colour },
    Spectrum,
    Reactive { colour: Colour, speed: u8 },   // 1-4
    Wave { direction: WaveDirection },
    Breath { style: BreathStyle },
    Starlight { style: BreathStyle, speed: u8 },   // 1-3
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "style", rename_all = "snake_case")]
pub enum BreathStyle {
    Random,
    Single { colour: Colour },
    Dual { first: Colour, second: Colour },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WaveDirection { Left, Right }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Colour { pub r: u8, pub g: u8, pub b: u8 }
```

- `Off` is its own variant, not a bool — asserting it means sending the firmware's own "none"
  effect frame (§ wire frames above), not a config-level no-op distinct from a device-level one.
- `Reactive`/`Starlight` carry `speed` (a gap in ticket 03's first pass, corrected by ticket 04
  once the wire tables confirmed both effects take a real speed byte). `Wave` uses a fresh
  2-variant `WaveDirection`, not `daemon/src/input.rs`'s existing 4-variant `Direction` — Wave
  only ever sends one of two wire values, and reusing the axis-input type would make
  `Up`/`Down` representable-but-invalid.
- `Colour` and `BreathStyle` are **named structs/enums**, never a tuple or hex string — the same
  reasoning `StatusLeds` already documents: keeps the type additive-friendly if a future field
  (e.g. a new effect parameter) needs to land without touching every call site.
- `brightness: u8` lands as a **sibling field on `Profile`**, alongside `lighting:
  LightingAssignment` — not nested in the enum, since it applies uniformly regardless of which
  Lighting state is active (one more byte on the same one-shot frame).
- Tagging style (`#[serde(tag = "...", rename_all = "snake_case")]`) matches this file's
  existing precedent for data-carrying enums (`Action`).

**Dispatch wiring — `Effect::AssertLighting`, mirrors `AssertStatusLeds` exactly:**

- A new unit variant `Effect::AssertLighting` in `edit.rs`'s `Effect` enum, alongside
  `AssertStatusLeds`.
- `edit::plan`'s `SwitchProfile` arm appends it unconditionally alongside `AssertStatusLeds`
  (order between the two is irrelevant — independent devices/writes serialised by the shared
  `led` task, not by ordering here).
- `Edit::SetLighting` (below) appends it unconditionally too, mirroring `SetStatusLeds` — no
  `target == active` gate needed, by the same structural argument Status LEDs already
  established: every mutating D-Bus method is Profile-unscoped, and the GUI always edits the
  active Profile.
- `run_effects` gets a sibling arm calling a new `DispatchState::push_lighting(&self, config:
  &Config)` helper (mirroring `push_status_leds`) — reads `config.active_profile().lighting`
  and `.brightness`, sends `Some(LightingState { assignment, brightness })` on `self.
  lighting_tx`.
- **No cached `lighting_state` in `DispatchState`** — `Config` stays the sole authoritative
  source, same as Status LEDs: every assert sends the full assignment + brightness, so there is
  no partial-update problem.

### Config schema

Settled by [ticket 03](./issues/03-daemon-architecture-for-lighting.md) §2–§3. **Additive
`#[serde(default)]` fields — no `schema_version` bump**, following every prior additive
`Profile` field (`status_leds`, `chords_base`, `axis_base`, `deep_stages`) exactly:

```rust
#[serde(default)]
pub lighting: LightingAssignment,
#[serde(default)]
pub brightness: u8,
```

- **Migration = the serde default.** A pre-feature `config.toml` has no `lighting`/`brightness`
  keys → every Profile parses to `LightingAssignment::Off` / `brightness: 0`, the charting-
  settled all-off default, reached the same way every other added field's default is.
- **No `config::validate` rule beyond type bounds** — every `LightingAssignment` variant and
  every `u8` brightness value is structurally valid; there is no cross-field invariant to check
  (speed/colour ranges are enforced at the type level — `u8` already bounds them).
- `dbus/wire.rs::profile_to_dict` gains `"lighting"` (a nested tagged dict, `Action`'s existing
  convention extended to `LightingAssignment`/`FixedEffect`) and `"brightness"` (a plain byte)
  entries, following `status_leds`'s precedent.
- New parse test mirroring `a_pre_status_led_config_defaults_status_leds`: a minimal
  `schema_version = 1` file with a Profile that has only `base` set must parse with
  `profile.lighting == LightingAssignment::Off` and `profile.brightness == 0`.

### D-Bus surface

Settled by [ticket 04](./issues/04-gui-daemon-surface-for-lighting.md) §6–§7.

- **`SetLighting(assignment: a{sv}, brightness: y) -> ()`** — one whole-assignment call,
  mirroring `SetStatusLeds`'s whole-triple shape (no partial-update bookkeeping; `Config` stays
  authoritative) and `action_to_dict`'s existing tagged-dict convention for sum types (a `"type"`
  key plus per-variant fields), now extended to `LightingAssignment`/`FixedEffect`. `Colour`
  rides as a nested `(yyy)` byte-triple — the first RGB value ever encoded on this project's
  D-Bus surface (Status LEDs are fixed hues; no colour value existed before this effort).
- `Edit::SetLighting { assignment: LightingAssignment, brightness: u8 }` — a new data-only
  variant in `edit.rs`, `plan` arm modelled on `SetStatusLeds`: writes both fields onto
  `active_profile_mut`, pushes `Effect::AssertLighting` unconditionally.
- **No new signal.** `GetConfig` already carries everything; there is no live device-state
  divergence to report — VARSTORE persists the effect selection device-side (see "Startup /
  shutdown behaviour"), and (as with Status LEDs) there is no on-device control that could
  change Lighting behind the Daemon's back.
- **`rules.py`: nothing to mirror.** No cross-field validation beyond what each GUI widget's own
  range already enforces (speed/brightness bounds, RGB from the colour dialog) — same conclusion
  Status LEDs' own ticket 04 reached.
- **Copy-from-Profile is client-side only, no new D-Bus method.** The GUI already holds every
  Profile's `lighting`/`brightness` via `GetConfig`; "copy from Profile X" reads X's stored
  values out of the already-loaded config dict and calls `set_lighting(...)` with them against
  the active Profile — exactly the write `SetLighting` already does. A dedicated
  `CopyLighting(source_profile)` method would just be `SetLighting` wrapped in a lookup,
  protecting no invariant a client-side copy doesn't.

**GUI mirror obligations (ADR-0005) — mechanical, no logic:**

- **`daemon_client.py`:** `set_lighting(self, assignment: dict, brightness: int)` →
  `self._call("SetLighting", GLib.Variant("(a{sv}y)", (assignment, brightness)))`, plus the
  abstract-method stub in the `Protocol`.
- **`daemon_stub.py`:** same signature — mutates `self._profiles[self._active_profile]`'s
  `lighting`/`brightness`, appends `("set_lighting", assignment, brightness)` to `self.calls`;
  the stub's seed Profile dict gains `"lighting": {"type": "off"}, "brightness": 0`.
- **`wire.py` / `read_model.py`:** surface `lighting`/`brightness` in the config dict the GUI
  reads — the mirror of `profile_to_dict`'s new entries.

### GUI

Settled by [ticket 04](./issues/04-gui-daemon-surface-for-lighting.md), via text grilling for
§1–2 and 6–8, and [a 3-variant prototype](../../prototype/04-lighting-tab-layout/prototype.py)
(branch `prototype/tartarus-backlight-04-lighting-tab-layout`, not on `dev`) for the tab layout
(§5) once text alone proved insufficient.

- **Placement:** a third arm of `device_overview.py`'s `build_destination_switch` — "Lighting",
  alongside Grid and Library — keeping the Profile sidebar exactly as Grid does (not Library's
  Steppers/Macros sidebar swap).
- **Mode selector — one flat 8-entry list, not nested to match the config type's `Off |
  FixedEffect{effect} | CustomLayout` shape:** Off, Static, Spectrum, Reactive, Wave, Breath,
  Starlight, Custom layout. Selecting any entry commits immediately via `set_lighting(...)` —
  same as every other immediate-write control on this panel, no Save/Apply button on the tab.
- **Per-effect parameter controls**, all `Gtk.ColorDialogButton` (GTK4's native colour picker)
  for colour fields — genuinely arbitrary RGB, unlike Status LEDs' 3 fixed hues, so no bespoke
  swatch widget:
  - Static: colour.
  - Spectrum: no controls — cycles autonomously.
  - Reactive: colour + speed 1–4 (`Gtk.SpinButton`).
  - Wave: Left/Right toggle pair.
  - Breath: style Random/Single/Dual (`Gtk.ToggleButton` group) + 0/1/2 colour pickers as the
    style requires.
  - Starlight: same style group + speed 1–3 (`Gtk.SpinButton`) + 0/1/2 colour pickers.
- **Custom-layout painter:** the Grid destination's key **geometry** (the `Gtk.Grid` row/col
  loop + wheel/Mode-key/thumbstick placement) is factored into a shared helper both the real
  Grid button grid and a new paint-button grid call — button **behaviour** stays separate
  (`make_input_button` is tightly coupled to Binding/Chord editing; a paint button just applies
  the current colour on click). A persistent **current-colour** `Gtk.ColorDialogButton` plus a
  **"Fill all keys"** bulk-fill button live in the same horizontal control strip as every other
  mode's params. Clicking a key paints it with the current colour **immediately**
  (`set_lighting(...)` fires per click, the full 21-colour array each time — no working-
  copy/Apply step, matching Status LEDs' click→persist→drive pattern; `Config` stays the sole
  source of truth). The grid **only renders when Custom layout is selected** — not shown, not
  even dimmed, for any other mode. **Scope: bulk-fill only.** Eyedropper (loading a painted
  key's colour as the new current-colour) and named palette/swatch reuse are explicitly deferred
  — see "Out of Scope."
- **Brightness:** one `Gtk.Scale`, **0–255 raw byte** — no percentage mapping (Acheron's
  `brightness: u8` is already the raw wire value; no other Acheron field does a cosmetic 0–100
  remap). **Always visible** regardless of the selected mode (a harmless no-op while Off — the
  matrix stays dark regardless), avoiding mode-dependent show/hide logic for a field that
  structurally always exists on `Profile`. **Commits on drag-end only**, matching the
  actuation-point depth marker's own `on_drag_end` commit pattern — firing a full
  `HIDIOCSFEATURE` write (a 21-colour payload in Custom-layout mode) on every pixel of slider
  movement would be wasteful for no benefit.
- **Tab layout — settled via prototype, not text alone:** one horizontal control strip (mode
  selector + per-effect params + brightness, including the Custom-layout current-colour picker
  and Fill-all-keys button) sits **above** the device area; the device area shows the paint grid
  only for Custom layout, a placeholder otherwise. This beat a persistent 220px sidebar and a
  two-pane canvas-toolbar layout in the prototype round. The per-effect params panel is
  **horizontal, not vertical** — a vertical stack made the whole strip change height per mode
  (visually unstable); the window gets a **1100px minimum width** (not auto-fit) so it doesn't
  grow when switching to the busiest mode (Starlight: style toggle + speed + 2 colour pickers).
- **Copy-from-Profile:** an affordance to copy another Profile's whole Lighting assignment onto
  the one being edited — one-shot copy, not a live link (see "D-Bus surface").
- **Which Profile:** the active Profile, full stop — there is no "selected-for-editing vs.
  active" distinction anywhere in Acheron.
- **When the device is disconnected:** the tab still shows the stored config state, matching
  every other Device Overview control; the Daemon re-asserts the shown state on the next
  connect.

### Startup / shutdown behaviour

- **Startup + every reconnect:** the active Profile's Lighting assignment is asserted on every
  `connected == true` from `rx_connection` — the same trigger and reasoning as Status LEDs
  (idempotent, cheap relative to the ~1–2ms `discover_hidraw` cost either way, `watch`
  coalescing already bounds a same-tick burst to one write). A `CustomLayout` assert being a
  full 21-colour payload instead of 3 bits doesn't change this — still one or two sequential
  `HIDIOCSFEATURE` ioctls per connect event, not a hot path.
- **No shutdown clear — a deliberate asymmetry with Status LEDs.** Every backlight effect-select
  frame carries `VARSTORE`, so the firmware persists the asserted effect in its own storage,
  unlike Status LEDs' `NOSTORE` frame (which the firmware always reclaims to orange-only on
  reconnect — the reason Status LEDs' exit-time clear exists at all). `relock_and_exit` calls
  `analog::clear_status_leds()` but has **no Lighting equivalent** — a clear-on-exit write would
  only actively erase the user's chosen look from the device's own storage for no benefit, since
  the next connect or Daemon startup re-asserts the active Profile's Lighting regardless. See
  ADR-0012.
- **No on-device-keymap re-assert hook** — same finding as Status LEDs (the Tartarus Pro has no
  host-independent on-device keymap switch), so there is nothing else to re-assert after.

### Domain vocabulary

Four terms, added to `CONTEXT.md`'s `### Configuration` section as part of this ticket (the map
reserved them during charting under the same lazy discipline Status LEDs held):

- **Lighting** — the Tartarus Pro's RGB backlight across its 20 grid keys and scroll wheel,
  distinct from the three fixed-colour Status LEDs. Driven only by the active Profile's Lighting
  assignment, never by a Binding or a Layer.
- **Lighting assignment** — the per-Profile choice of a Fixed effect or a Custom layout, plus a
  brightness level, asserted on Profile switch, on Daemon startup, and on every device
  (re)connect. Defaults to Off for a new Profile.
- **Fixed effect** — one of the firmware's built-in, device-autonomous Lighting effects (Static,
  Spectrum, Reactive, Wave, Breath, Starlight), asserted with a single one-shot command the
  firmware then runs on its own. Mutually exclusive with Custom layout.
- **Custom layout** — the other Lighting-assignment state: a fixed, non-animated colour per
  matrix cell (the 20 grid keys + the scroll wheel), painted once and asserted as a static
  frame.

Full definitions and avoid-lists are in `CONTEXT.md`. Implementation work on this spec should
use these names and avoid the ruled-out synonyms recorded there.

## Testing Decisions

- **Daemon — the `led` task seam.** The `led` task's second `watch<Option<LightingState>>` arm
  is tested the same way the existing `StatusLeds` arm is: assert that `Edit::SwitchProfile` and
  `Edit::SetLighting` each push the expected `LightingState`, that a burst of edits coalesces to
  the final value, and that every `connected == true` re-pushes the active Profile's Lighting.
  A dedicated test confirms both channels' writes are serialised (no interleaved partial frames)
  when both fire in the same tick. The actual `HIDIOCSFEATURE` writes are not unit-tested — the
  frames are already verified on hardware (ticket 02).
- **Daemon — config.** The new parse-default test (above) plus a `config_to_dict` assertion that
  `profile_to_dict` emits the `lighting`/`brightness` entries correctly for each
  `LightingAssignment` variant.
- **Daemon — `edit::plan`.** A unit test that `Edit::SetLighting { .. }` sets
  `active_profile().lighting`/`.brightness` and returns `[Effect::AssertLighting]`, and that
  `Edit::SwitchProfile`'s effect list now contains both `AssertStatusLeds` and
  `AssertLighting`.
- **GUI — the D-Bus client seam (`DaemonStub`).** The mode selector and per-effect params render
  from the stub's active-Profile `lighting`; selecting a mode or changing a param calls
  `set_lighting` with the full assignment; the Custom-layout grid only renders in Custom-layout
  mode and paints per click; brightness commits only on drag-end; a newly created Profile shows
  Off at brightness 0; Copy-from-Profile reads another Profile's stored values and calls
  `set_lighting` with them; the tab still renders the stored state when the stub reports the
  device disconnected.
- **GUI — rules mirror.** Nothing — `rules.py` gets no addition, so no contract test changes.
- No end-to-end test against real hardware is specified — ticket 02's verification pass already
  validated the frames live, and this spec's testing scope is the two seams above.

## Out of Scope

Carried from the map so the implementation effort inherits the boundary:

- **Acheron-driven, remap-aware reactive/per-keypress lighting** ("tier 3") — the dispatch task
  writing a lighting frame per keypress, e.g. reacting to the *remapped* key rather than the
  physical one. A fresh effort later if wanted, not a resumption of this map. (The firmware's
  own `reactive` effect, which is genuinely device-autonomous, is in scope as a Fixed effect —
  a different thing entirely; keep the two "reactive"s terminologically distinct.)
- **Ripple / ripple-random-colour** — not a device-side command (see "The wire frames"); a
  host-streamed implementation would cost a steady ~25 Hz write stream, which is exactly the
  "host-streamed per-frame Custom-layout animation" excluded below.
- **Layer-scoped lighting overrides** — Lighting is Profile-only, mirroring Status LEDs.
- **A bindable "set Lighting" Action** — Lighting is Profile-driven only; a Binding cannot fire
  a lighting change directly.
- **Automatic or per-application Profile/Lighting switching** — excluded by the definition of
  Profile (never switched automatically).
- **Host-streamed per-frame Custom-layout animation** — this effort covers one static per-key
  colour layout only. ADR-0006 already flagged streamed animation as a distinct future revisit
  of the `led` task's fd lifetime.
- **Custom-layout painter eyedropper and named palette/swatch reuse** — a bulk-fill button
  ships, but loading a painted key's colour as the new current-colour, or saving/reusing named
  colours across keys or Profiles, are real scope growth (a second interaction mode; a new
  persisted concept with no config-model home) better judged after the plain painter has seen
  use — a fresh ticket later if wanted, not a resumption of this map.

## Further Notes

- **ADR.** [ADR-0012](../../docs/adr/0012-lighting-shares-the-led-task-no-varstore-shutdown-clear.md)
  ("Lighting shares the `led` task's second channel; VARSTORE means no shutdown clear") is filed
  alongside this spec. It refines ADR-0006 (itself a refinement of ADR-0002) and answers the two
  questions a future reader would most need "why" for: why Lighting doesn't get its own task,
  and why it doesn't get a shutdown clear the way Status LEDs do.
- **Prior art in the tree.** [`research/backlight-wire-protocol.md`](./research/backlight-wire-protocol.md)
  is the primary-source wire spec (every claim cited to a driver/daemon file + line);
  [`assets/02-RESULTS.md`](./assets/02-RESULTS.md) plus the `assets/02-*.jsonl` files are the
  raw hardware evidence; [`prototype/04-lighting-tab-layout/prototype.py` on branch
  `prototype/tartarus-backlight-04-lighting-tab-layout`](../../prototype/04-lighting-tab-layout/prototype.py)
  is the (not-on-`dev`) reference for the tab layout decision. All on `dev` except the
  prototype branch — the release rebuild keeps `prototype/` and `.scratch/` out of `main`.
- Every ticket on the map (`.scratch/tartarus-backlight/map.md`) is resolved as of this spec.
  Implementation is a **fresh effort** (a `tartarus-backlight-impl` directory) — this spec is
  the hand-off, not a resumption of this map.
