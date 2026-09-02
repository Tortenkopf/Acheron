Status: ready-for-agent

# Acheron — Profile Status-LED indicator

## Problem Statement

The Razer Tartarus Pro has three fixed-colour indicator LEDs on its left side — orange,
green, blue — that Synapse uses on Windows to show which onboard keymap is active. On Linux
nothing drives them: no shipping OpenRazer release implements the `0x0B` LED path for this
device (the only real implementation, OpenRazer PR #2336, is unmerged), so the firmware's
power-on default (orange only) is all a user ever sees. Acheron already switches the user's
whole Binding set by **Profile**, but that switch has no physical indication — with the GUI
and tray closed, there is no at-a-glance way to tell which Profile is active.

This effort was **gated** on a hardware feasibility test ([ticket 01](./issues/01-prototype-status-led-controllability.md)):
if the LEDs turned out not to be host-controllable on the real device, or driving them did
something adverse to the unit, the effort would have been archived with the negative result
recorded — the same discipline the analog-capture strand was held to. **The gate passed**
(fw v1.2, 2026-09-02): all three LEDs are independently host-controllable, all-off is
reachable, and ~25 writes plus a driver-mode enter/relock caused no reset or re-enumeration.

## Solution

Every **Profile** carries a **Status LED assignment** — a triple of independent on/off states
for the orange, green, and blue LEDs. The Daemon asserts the active Profile's assignment
whenever that Profile becomes active, on Daemon startup, and **on every device (re)connect**
(the firmware reclaims the LEDs to its orange-only default on every USB enumeration, so this
is a hard requirement, not an optimisation). On a clean Daemon exit all three LEDs are
cleared.

The assignment is edited from the GUI — a "Status LEDs" group of three colour lozenges in the
Device Overview, editing the active Profile like every other per-Profile control — and stored
per Profile in `config.toml`. Layers never touch the Status LEDs; they track the Profile only.

There is **no global opt-out setting** and **no brightness / colour / effect control** — the
hardware is on/off only and the three colours are fixed. This matches how the analog strand
was scoped: if it is safe we do it plainly, we do not add a knob to turn it off.

This spec is the hand-off to a **separate implementation effort**. Every decision below is
settled by the map (`.scratch/tartarus-status-leds/map.md`) and its resolved tickets; nothing
here is left open.

## User Stories

1. As a Tartarus Pro owner on Linux, I want each Profile to define an on/off state for the
   three side LEDs, so that the physical device shows which Profile is active without opening
   the GUI or tray.
2. As a user, I want the LED state to follow the active Profile deterministically — switching
   to a Profile always shows exactly that Profile's assignment — so that the indicator never
   drifts from reality.
3. As a user, I want every Profile to have a defined Status-LED state (defaulting to all-off),
   so that switching Profiles is fully predictable and a Profile I never configured simply
   shows all LEDs dark rather than leaving the previous Profile's LEDs lit.
4. As a user, I want the Daemon to re-assert the active Profile's LED state whenever I plug
   the device back in or restart the Daemon, so that a routine unplug/replug doesn't leave
   the LEDs showing the firmware's default instead of my Profile.
5. As a user, I want all three LEDs cleared when the Daemon exits cleanly, so that a stopped
   Acheron doesn't leave a stale indicator lit.
6. As a user, I want to set a Profile's LED assignment from the GUI by clicking three colour
   lozenges near the device picture, so that configuring the indicator is as direct as
   editing a Binding.
7. As a user, I want the LED lozenges to show the active Profile's stored state even while
   the device is disconnected, so that I can see and edit the assignment regardless of
   whether the pad is plugged in.
8. As a user, I want the Mode key / Layer to never change the LEDs, so that the indicator
   always means "which Profile" and not "am I holding Mode right now" (I already know that).
9. As a user editing `config.toml` by hand, I want the Status-LED assignment stored as a
   plain named table per Profile, so that I can read and change it outside the GUI.
10. As a user upgrading from a build without this feature, I want my existing `config.toml`
    to keep working unchanged, with every Profile defaulting to all-off.

## Implementation Decisions

### The wire frame

Derived from source by [ticket 02](./issues/02-research-status-led-wire-protocol.md)
([`research/status-led-wire-protocol.md`](./research/status-led-wire-protocol.md), all
primary-source cited) and **verified byte-for-byte on hardware** by
[ticket 01](./issues/01-prototype-status-led-controllability.md) (fw v1.2). It is a standard
Razer extended-matrix static-effect command aimed at a dedicated LED id — expressible through
Acheron's existing `daemon/src/capture/analog.rs::build_razer_cmd` with **no helper changes**:

```
build_razer_cmd(0x1F, 0x0F, 0x02, &[0x00, 0x0B, 0x01, 0x00, 0x00, 0x01, r, g, b])
```

- `txn = 0x1F`, `command_class = 0x0F`, `command_id = 0x02` (write), `data_size = 0x09`.
- `arg0 = 0x00` — storage byte. Send `0x00` for intent-clarity. **The byte is inert on this
  firmware** (ticket 01: neither NOSTORE `0x00` nor VARSTORE `0x01` persists across a USB
  re-enumeration; a settled read-back always echoes `0x01`). **No storage-mode config knob** —
  there is no observable behaviour to expose.
- `arg1 = 0x0B` — LED id (`SIDE_STRIPE_LED`).
- `arg2 = 0x01` — effect id: static. Never `effect_none` (id `0x00`): ticket 01 confirmed it
  ACKs but does nothing to these fixed-colour LEDs.
- `arg3 = 0x00`, `arg4 = 0x00` — unused for the static effect. (CommandPost sends `arg4 = 0x01`;
  ticket 01 verified both values work and the device echoes `0x00` back regardless — keep
  `0x01` only as a fallback if a future unit doesn't react.)
- `arg5 = 0x01` — colour count.
- `arg6 / arg7 / arg8 = r / g / b` — one fixed-colour LED each (`0xFF` = on, `0x00` = off),
  independently addressable. Orange ← `r`, green ← `g`, blue ← `b`.
- CRC is computed by `build_razer_cmd`.

**"Off" is the same frame with the relevant channel byte(s) `0x00`** — all-off is
`&[0x00, 0x0B, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00]`. All-off `(0,0,0)` is
hardware-reachable (ticket 01, criterion 5).

**No driver-mode call.** The `0x0F/0x02` frame works regardless of Capture mode — OpenRazer
drives this device's lighting with driver mode permanently disabled, CommandPost sends no
mode command, and ticket 01 lit the LEDs with `device_mode = 00 00`. The LED write is
therefore **independent of Analog vs Digital Capture mode** and never sends `0x00/0x04`. The
normal→driver-mode transition is exactly the reset risk this effort was cautious about, so
avoiding it is a deliberate property, not an accident.

**Read-back (`command_id 0x82`).** Reliable on fw v1.2 including cold reads after a replug
(ticket 01, criterion 2), but the OpenRazer maintainer found it unreliable *across devices*
— the specific thing that kept PR #2336 unmerged. **The design keeps the Daemon's `Config`
as the single authoritative triple and re-sends the whole frame on every change** regardless;
a startup `0x82` seed is trustworthy on this unit but is not required by the architecture and
is not specified as a dependency.

### Daemon architecture

Settled by [ticket 03](./issues/03-daemon-architecture-for-status-leds.md). The
hidraw-ownership and no-driver-mode choices are recorded in
**[ADR-0006](../../docs/adr/0006-status-leds-driven-from-dispatch-over-short-lived-hidraw-fds.md)**
(refines ADR-0002); this section states the shape, not the rationale.

**Write primitive — `analog.rs`, modelled on `relock()`.** Two new standalone functions in
`daemon/src/capture/analog.rs`, siblings of `relock()` and `read_device_info()`:

- `pub fn assert_status_leds(leds: StatusLeds) -> io::Result<()>` — `discover_hidraw()` → open
  the Interface-2 control node (`CONTROL_INTERFACE`) read+write → one `build_razer_cmd(...)`
  frame via `HIDIOCSFEATURE` → drop the fd. No read-back, no retry loop, no driver-mode call,
  no unlock. One SET ioctl on a freshly-opened, immediately-closed node.
- `pub fn clear_status_leds() -> io::Result<()>` — the same frame with `r = g = b = 0x00`.

A dedicated `capture/led.rs` module was rejected — it would force `discover_hidraw` /
`build_razer_cmd` / the ioctl wrapper to `pub(crate)` for no real separation gain, and
`relock()` already establishes "a standalone control-channel write that isn't really capture"
living in `analog.rs`.

The fd is **short-lived** (open-send-close per call) because the write is an occasional
one-shot — Profile switch, connect — exactly like `relock()` / `read_device_info()`, unlike
the analog grid task which polls Depth many times a second and holds its Interface-2 handle
continuously. Short-lived ownership also makes the LED write **totally independent of the
capture layer**: in Digital Capture mode nothing holds an Interface-2 handle at all, so a
"share the capture handle" design would have nothing to share half the time and would have to
thread an `Arc<Mutex<File>>` across the supervisor↔dispatch boundary and handle mode-swap
races.

**Writer — a dedicated non-fatal `led` task.** A new task, sibling of `injector`, owning the
actual device writes:

- `main.rs` creates `tokio::sync::watch::channel::<Option<StatusLeds>>(None)` and
  `tokio::spawn`s the `led` task with the receiver. It is **not** a branch in `main.rs`'s
  top-level `tokio::select!` — an LED write failure must never exit the process. Failures are
  logged once; the task keeps running.
- Loop: `rx.changed().await` → `borrow_and_update()` the latest `Option<StatusLeds>` → if
  `Some`, `tokio::task::spawn_blocking(move || analog::assert_status_leds(leds))` and await
  that (serialising writes within the task) → on `Err`, log once (device absent = `NotFound`,
  harmless). `watch` semantics coalesce a burst of Profile switches to the final triple — no
  queue of stale writes, no out-of-order A→B→A landing.
- The task also performs the shutdown all-off write (see "Startup / shutdown" below).

Chosen over a detached `spawn(spawn_blocking)` per assert straight from dispatch (a fast
switch burst could land writes out of order) and over awaiting `spawn_blocking` inline on the
dispatch loop (~1–2 ms stall per switch/connect, and `discover_hidraw` walks sysfs). The
dedicated task mirrors `injector` (a task dispatch pushes to) and `actuation_tx` / `depth_tx`
(a `watch` of latest state).

**Decider — dispatch, sole owner.** Dispatch owns `Config` and is the only component that
decides what the LEDs should show; the supervisor stays entirely LED-agnostic.

- `Config` is the authoritative triple — **no cached `led_state` in `DispatchState`**.
  `config.active_profile().status_leds` *is* the source; every assert sends the full frame
  read from it. There are no partial writes and no non-active-Profile write path (below), so
  the "keep an authoritative triple for partial updates" concern does not arise.
- **`Effect::AssertStatusLeds`** — a new **unit** variant in `daemon/src/edit.rs`'s `Effect`
  enum, alongside `ReconcileStepperCursor` / `AnnounceProfileChange`. `edit::plan` appends it
  from two arms:
  - `Edit::SwitchProfile` — added to its existing effect list, last (order is irrelevant —
    LEDs are independent of Toggles / axes / Analog-repeat).
  - `Edit::SetStatusLeds` (below) — **unconditionally**.
- **`run_effects`** handles `AssertStatusLeds` by calling a private `DispatchState` helper
  `push_status_leds(&self, config: &Config)`, which reads `config.active_profile().status_leds`
  and does `self.led_tx.send(Some(leds))` (`led_tx` is the `watch::Sender` handed in from
  `main.rs`, held on `DispatchState`).

**Startup + (re)connect assertion.** The firmware reclaims the LEDs to orange-only on every
USB enumeration (ticket 01), so re-asserting on connect is a hard requirement and startup is
just "the first connect":

- In `dispatch::run`'s `rx_connection` select arm, **after** `handle_connection_change(...)`,
  call `state.push_status_leds(&config)` on **every message where `connected == true`** — no
  flag, no dependence on the connection *transition*. (`DispatchState.device_connected` starts
  optimistically `true`, and `handle_connection_change` early-returns when the bool is
  unchanged, so hanging the assert off transition detection would miss the startup assert.)
  Asserting on every `true` is safe — the write is idempotent on the hardware (ticket 01: 25+
  writes, no adverse behaviour), the `CaptureSource` contract already guarantees dispatch only
  observes genuine value changes, and a stray redundant `true` costs one ~1 ms ioctl.
- This path does **not** go through `Effect` — it reads `Config` and calls `push_status_leds`
  directly. `run_effects` (Profile switch) and this arm (connect) are the two call sites of
  the one helper.
- **No pre-loop assertion** in `dispatch::run`'s init block — the device may not be present
  yet, and the connect edge covers the present-at-startup case.
- Works identically in Analog and Digital Capture mode — `assert_status_leds` opens its own
  Interface-2 fd regardless of what capture is doing.

**No on-device-keymap re-assert hook.** [Ticket 01](./issues/01-prototype-status-led-controllability.md)
confirmed the Tartarus Pro has no host-independent on-device keymap switch (the LED↔keymap
link is Synapse-side only), so there is nothing to re-assert after.

**Device absent / disconnected.** `assert_status_leds` returns `Err`
(`discover_hidraw` finds no Interface 2 → `io::ErrorKind::NotFound`), exactly like `relock()`.
The `led` task logs it once and carries on. A Profile switch with no device connected persists
`config.toml` normally, the LED write no-ops, and the next connect edge asserts the (now
current) active Profile's triple.

**No non-active-Profile write path.** Every mutating D-Bus method is Profile-unscoped and
`edit::plan` applies it to `active_profile`; the GUI's Device Overview renders only the active
Profile and the sidebar calls `switch_profile` **first**. So a Status-LEDs edit is
*structurally* always an edit to the active Profile — no `target == active` gate is needed,
and the "active vs selected-for-editing Profile" framing from the ticket text does not apply
(there is no such distinction anywhere in Acheron).

### Config schema

Settled by [ticket 04](./issues/04-gui-daemon-surface-for-status-leds.md) §1. **Additive
`#[serde(default)]` field — no `schema_version` bump.** `config::SCHEMA_VERSION` stays `1`;
`config::parse` hard-refuses any other value and there is no migration machinery. Every prior
field addition (tickets 17 / 18 / 51 / 54) landed this way; a bump to `2` would brick every
existing `config.toml` on upgrade.

- **New type** in `daemon/src/config.rs`, beside `ActuationPoint`:

  ```rust
  #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
  pub struct StatusLeds {
      #[serde(default)]
      pub orange: bool,
      #[serde(default)]
      pub green: bool,
      #[serde(default)]
      pub blue: bool,
  }
  ```

  Field names `orange` / `green` / `blue` (the fixed hardware colours). A **named struct**,
  never `[bool; 3]` or a tuple (ticket 03 §7) — so a later brightness byte, effect/speed
  params, or a sibling `backlight` sub-struct is an *additive* change and the
  `watch<Option<StatusLeds>>` channel type and `Effect::AssertStatusLeds` variant do not
  churn. `Default` derives to all-`false`. Per-field `#[serde(default)]` so a hand-edited
  partial inline table (`status_leds = { orange = true }`) still parses.

- **New field** on `Profile` (`config.rs:130`), after `default_actuation` /
  `actuation_overrides`:

  ```rust
  #[serde(default)]
  pub status_leds: StatusLeds,
  ```

  `#[serde(default)]` **without** `skip_serializing_if` — always written back in full, like
  `default_actuation` and `mode_key_role`. `toml::to_string_pretty` renders it as a
  `[profiles.<name>.status_leds]` sub-table with all three keys spelled out:

  ```toml
  [profiles.gaming.status_leds]
  orange = true
  green = false
  blue = false
  ```

- **Migration = the serde default.** A pre-feature `config.toml` has no `status_leds` key →
  each Profile parses to `StatusLeds::default()` = `(off, off, off)`, the charting-settled
  all-off default, reached the same way every other added field's default is.
- **No `config::validate` rule** — every `(bool, bool, bool)` is structurally valid.
- **New parse test** `a_pre_status_led_config_defaults_status_leds`, mirroring
  `a_pre_ticket_17_config_defaults_actuation_fields_and_force_digital`: a minimal
  `schema_version = 1` file with a Profile that has only `base` set must parse with
  `profile.status_leds == StatusLeds::default()`.
- **`dbus/wire.rs::profile_to_dict`** (`wire.rs:379`) gains a `"status_leds"` entry — a nested
  dict `{ "orange": bool, "green": bool, "blue": bool }`, following `actuation_point_to_dict`.
  A `config_to_dict` test asserts the three keys, mirroring
  `config_to_dict_serializes_default_actuation_and_actuation_overrides`.

### D-Bus surface

Settled by [ticket 04](./issues/04-gui-daemon-surface-for-status-leds.md) §2–§3.

- **`Edit::SetStatusLeds { orange: bool, green: bool, blue: bool }`** — a new data-only
  variant in `daemon/src/edit.rs`'s `Edit` enum. `plan` arm, modelled on `SetActuationPoint`:

  ```rust
  Edit::SetStatusLeds { orange, green, blue } => {
      active_profile_mut(&mut next).status_leds = StatusLeds { orange, green, blue };
      effects.push(Effect::AssertStatusLeds);
  }
  ```

  **Whole triple in one call** — one frame drives all three channels, so a per-channel
  `SetStatusLed { channel, on }` would force a daemon-side read-modify-write and a
  channel-name enum on the wire for no gain. `Effect::AssertStatusLeds` is **unconditional**
  (same as `SetActuationPoint` → `RepublishActuation`). `plan` still returns `Result` for
  symmetry with every persisting `Edit` (the `config.toml`-write-failure case); it never
  fails on its own account.

- **D-Bus method** `SetStatusLeds(bbb) -> ()` on `com.acheron.Daemon` (`dbus/mod.rs`), shaped
  like `set_default_actuation` (`mod.rs:731`): build the `Edit` directly, `self.apply(...)`.

- **No `GetState()` addition.** The active Profile's stored triple is fully visible via
  `GetConfig`. The only thing config cannot show is the sub-second "orange-only" transient on
  every device (re)connect before dispatch asserts — it is self-healing, invisible to a user
  not watching the physical device during a replug, and (unlike `capture_mode`, which was
  added to `GetState` in `tartarus-input-expansion` ticket 21) there is **no
  hardware-divergence path**: ticket 01 found no on-device keymap switch, so the hardware
  cannot diverge from config behind the Daemon's back. No `command::State` field, no
  `state_to_dict` key.

- **No new signal.** There is no generic config-changed signal in the Daemon —
  `SetBinding` / `SetActuationPoint` / `SetDefaultActuation` emit nothing; the GUI rebuilds
  from `GetConfig` after its own calls and on `ActiveProfileChanged` (which also covers a
  hardware `Action::ProfileSwitch`). `SetStatusLeds` follows suit.

A live "what is physically lit right now" indicator, if ever wanted, is a separate ticket
bundled with the host-streamed-animation work (see "Further notes").

### GUI

Settled by [ticket 04](./issues/04-gui-daemon-surface-for-status-leds.md) §5, per the user's
mockup [`screenshots/Status LED location Mockup.png`](./screenshots/Status%20LED%20location%20Mockup.png).

- **Placement:** a new column in `build_main_view`'s `device_row`
  (`gui/acheron_gui/device_overview.py:824`), **between the thumbstick `stick_col` and
  `build_chords_section`** — a `Status LEDs` heading over **three vertically-stacked colour
  lozenges**: orange (top), green (middle), blue (bottom), roughly mirroring the physical
  LEDs' left-side placement. The mockup is a rough sketch; the real widgets are uniform and
  aligned.
- **Widget:** each lozenge is a click-to-toggle button (`Gtk.ToggleButton`, or a `Gtk.Button`
  carrying its own `.status-led` + per-colour CSS class). **Lit** = full-saturation colour
  fill + a visible border/glow; **unlit** = heavily desaturated + flat. The lit/unlit
  contrast must be *strong* — it is the primary state signal, not a faint brightness shift.
- **No visible per-lozenge text**, but each gets a **tooltip** (`"Orange status LED — on"` /
  `"… — off"`) and an accessible name/description (`set_tooltip_text` + `Gtk.Accessible`
  properties) so a colour-blind user is not relying on hue + brightness alone. Group heading
  stays `Status LEDs`.
- **Which Profile:** the **active** Profile, full stop — there is no
  "selected-for-editing vs active" distinction in Acheron.
- **State source:** always `config["profiles"][profile]["status_leds"]` — never a live
  hardware read. On a **newly created Profile** the three lozenges show dark, because
  `status_leds` defaults all-`false` and "never set" is byte-identical to "explicitly
  all-off" — no special-casing.
- **When the device is disconnected:** the lozenges still show the stored config state (orange
  lit if the active Profile says so), matching every other Device Overview control; the Daemon
  re-asserts the shown state on the next connect.
- **Layer / destination visibility:** Grid destination **only** (alongside Chords, also
  Grid-only); shown **identically on both Base and Held** — the group is Profile-scoped, not
  Layer-scoped, and does not change or disappear when the layer bar flips. Not rendered in the
  Library destination. Renders from `status_leds` regardless of `selected_layer`.
- **Edit flow:** each lozenge's toggle handler reads **all three** current lozenge states and
  calls `client.set_status_leds(orange, green, blue)`; the group then rebuilds from config on
  the shared `on_change`, like everything else on the panel. The `Effect::AssertStatusLeds`
  that `SetStatusLeds` emits drives the device immediately.

**GUI mirror obligations (ADR-0005) — mechanical, no logic:**

- **`daemon_client.py`:** `set_status_leds(self, orange: bool, green: bool, blue: bool)` →
  `self._call("SetStatusLeds", GLib.Variant("(bbb)", (orange, green, blue)))`, plus the
  abstract-method stub in the `Protocol`.
- **`daemon_stub.py`:** same signature — mutates
  `self._profiles[self._active_profile]["status_leds"]`, appends
  `("set_status_leds", orange, green, blue)` to `self.calls`; the stub's seed Profile dict
  gains `"status_leds": {"orange": False, "green": False, "blue": False}`.
- **`wire.py` / `read_model.py`:** surface `status_leds` in the config dict the GUI reads —
  the mirror of `profile_to_dict`'s new entry.
- **`rules.py`: nothing.** It mirrors only the Daemon's pure validation core; "three bools,
  always valid" has no invariant to mirror.

### Startup / shutdown behaviour

- **Startup:** the active Profile's Status-LED state is asserted on the first `connected == true`
  from `rx_connection` (there is no separate pre-loop assert — see "Daemon architecture").
- **Every reconnect:** re-asserted on every subsequent `connected == true`. Hard requirement —
  the firmware reclaims the LEDs on every enumeration.
- **Clean exit:** `main.rs::relock_and_exit` (the SIGTERM/SIGINT path) sends the all-off frame
  via `analog::clear_status_leds()` **before** `analog::relock()`, best-effort, log-and-continue.
  **Not** on the supervisor's swap-away-from-analog `relock()` — there the Daemon is still
  running and the device still present, so the LEDs must keep showing the active Profile. Only
  a clean Daemon exit clears. All-off `(0,0,0)` is hardware-reachable (ticket 01, criterion 5),
  so the contingency where the shutdown clear would have been dropped does not trigger.
- **Brief transient:** there is an unavoidable sub-second "orange-only" window on every
  connect, between enumeration and the Daemon's assert. Not reported or signalled anywhere.

### Domain vocabulary

Two terms, added to `CONTEXT.md` as part of this ticket (the map reserved them during
charting under the same lazy discipline the `tartarus-input-expansion` effort held Chord /
Stepper to):

- **Status LED** — one of the three fixed-colour (orange, green, blue) on/off indicator LEDs
  on the device's left side. *Avoid:* "profile LED", Razer's "keymap indicator", "Chroma".
- **Status LED assignment** — the per-Profile triple of on/off states, asserted on Profile
  switch, on Daemon startup, and on every device (re)connect.

Implementation work on this spec should use these names and avoid the ruled-out synonyms.

## Testing Decisions

- **Daemon — the `led` task seam.** The `led` task consumes a `watch<Option<StatusLeds>>` and
  calls `analog::assert_status_leds` / `clear_status_leds`. Tests exercise dispatch's decider
  logic through that channel: assert that `Edit::SwitchProfile` and `Edit::SetStatusLeds` each
  push the expected triple, that a burst of switches coalesces to the final triple, and that
  every `connected == true` from the connection channel re-pushes the active Profile's triple.
  The actual `HIDIOCSFEATURE` write is not unit-tested — it is one `build_razer_cmd` frame
  whose every byte is already verified on hardware (ticket 01) and cross-checked by
  `prototype/01-status-leds/prototype.py`'s `selftest`.
- **Daemon — config.** `a_pre_status_led_config_defaults_status_leds` (parse test, above) plus
  the `config_to_dict` assertion that `profile_to_dict` emits the nested `status_leds` dict.
- **Daemon — `edit::plan`.** A unit test that `Edit::SetStatusLeds { .. }` sets
  `active_profile().status_leds` and returns `[Effect::AssertStatusLeds]`, and that
  `Edit::SwitchProfile`'s effect list now contains `AssertStatusLeds`.
- **GUI — the D-Bus client seam (`DaemonStub`).** The three lozenges render lit/unlit from the
  stub's active-Profile `status_leds`; clicking one calls `set_status_leds` with the full
  triple; the group rebuilds from config on `on_change`; a newly created Profile shows all
  three dark; the group renders identically on Base and Held and only on the Grid destination;
  it still renders the stored state when the stub reports the device disconnected.
- **GUI — rules mirror.** Nothing — `rules.py` gets no addition, so no contract test changes.
- No end-to-end test against real hardware is specified — ticket 01's prototype already
  validated the frame live, and this spec's testing scope is the two seams above.

## Out of Scope

Carried from the map so the implementation effort inherits the boundary:

- **The per-key Chroma backlight** (grid keys + mouse wheel) — Acheron already ignores it.
  Primarily aesthetic; a different mechanism (`command_id 0x03` linear matrix vs `0x02` LED
  effect) and a different concern from the Status LEDs.
- **LED brightness control** — the hardware is on/off only (user-confirmed).
- **Non-static LED effects** (breathing / pulse) — the effect byte stays `0x01` (static).
- **Custom / arbitrary LED colours** — the three colours are fixed in hardware.
- **Automatic or per-application Profile/LED switching** — excluded by the definition of
  Profile (never switched automatically).
- **A bindable "set Status LED" Action** — the LEDs are Profile-driven only; a Binding cannot
  fire an LED change directly.
- **A storage-mode (NOSTORE/VARSTORE) config knob** — the `arg0` byte is inert on this
  firmware; nothing to expose.
- **A live "what is physically lit" indicator in `GetState()`** — no hardware-divergence path
  exists; if ever wanted, it rides with the host-streamed-animation work below.

## Further Notes

- **ADR.** [ADR-0006](../../docs/adr/0006-status-leds-driven-from-dispatch-over-short-lived-hidraw-fds.md)
  ("Status LEDs are driven from the dispatch task over short-lived Interface-2 hidraw fds")
  is filed alongside this spec. It refines ADR-0002 and answers the question a future reader
  of the capture layer will ask — "why doesn't the LED write share the capture handle?".
- **Retroactive corrections already folded in.** [Ticket 04](./issues/04-gui-daemon-surface-for-status-leds.md)
  corrected the charting-Q15 / earlier-ticket-text "`schema_version` bump + migration" to
  "additive `#[serde(default)]` field, no bump", and resolved "`GetState()` additions /
  signal (if any)" to **none**. [Ticket 01](./issues/01-prototype-status-led-controllability.md)
  corrected ticket 02's storage-persistence predictions (nothing persists) and removed the
  on-device-keymap re-assert hook. This spec reflects the corrected final state.
- **Future-proofing (from [ticket 03](./issues/03-daemon-architecture-for-status-leds.md) §8).**
  A richer lighting feature stays additive against this architecture: more on/off channels →
  a wider `StatusLeds` struct, nothing else moves; brightness instead of on/off → the frame
  already carries a full `0x00`–`0xFF` byte per channel, the fields become `u8`-shaped;
  firmware-driven effects → still one short-lived write, the effect-id byte changes and the
  payload widens to `{effect, speed, colour}`. Only **host-streamed animation** (the Daemon
  pushing an RGB frame every ~33 ms) would strain the shape — the `led` task would grow a
  persistent fd + render loop — but the seams survive: `dispatch (decides) → watch channel →
  led task → Interface-2 hidraw` is unchanged, and a persistent fd is a *refinement* of
  "Interface-2 hidraw, no driver mode", not a reversal. **Binding constraint on any future
  lighting work:** the device has a single control channel and lighting frames must not
  interleave — any future lighting surface routes through the **same `led` task**, never a
  second parallel writer opening Interface 2 independently.
- **Prior art in the tree.** `prototype/01-status-leds/prototype.py` (standalone, stdlib-only,
  `selftest` cross-checks every protocol constant) is the verified reference for the frame;
  `research/status-led-wire-protocol.md` is the primary-source wire spec; the raw hardware
  evidence is `assets/01-RESULTS.md` + `assets/01-*.jsonl`. All on `dev` — the release rebuild
  keeps `prototype/` and `.scratch/` out of `main`.
- Every ticket on the map (`.scratch/tartarus-status-leds/map.md`) is resolved as of this
  spec. Implementation is a **fresh effort** — this spec is the hand-off.
