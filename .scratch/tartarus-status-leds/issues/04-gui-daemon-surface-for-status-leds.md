Type: grilling
Blocked by: 01, 03
Status: resolved (Charon, 2026-09-02)

## Question

Decide the **GUI ↔ daemon surface** and the **config schema** for Status-LED assignments.
Grilling + domain-modeling against the real code. Decisions only — no build. Builds on the
architecture settled in [ticket 03](./03-daemon-architecture-for-status-leds.md).

Settle at least:

- **`config.toml` schema.** Confirmed shape (charting Q15): a named table per Profile,
  `status_leds = { orange = true, green = false, blue = false }` under `[profiles.<name>]`.
  Pin the exact key names, the Rust type on `Profile` (`daemon/src/config.rs:130` — e.g. a
  small `StatusLeds` struct vs `[bool; 3]`), the `#[serde]` attributes, and the
  `schema_version` bump + migration (all-off default for Profiles without the key). Check
  against the existing migration tests in `config.rs`.
- **D-Bus command.** New `Command` variant to set a Profile's Status-LED assignment (shape it
  like `SetBinding` / `SetActuationPoint` — Profile-scoped, atomically persisted). Name it;
  decide whether it sets the whole triple or one channel at a time.
- **`GetState()` exposure.** Does the GUI need the active Profile's current Status-LED state
  reported back (like `capture_mode` was added in `tartarus-input-expansion` ticket 17), and
  a change signal? Or is the config the single source of truth the GUI already has?
  ([Ticket 01](./01-prototype-status-led-controllability.md) found no on-device keymap switch,
  so the hardware can't diverge from config *behind the daemon's back* — but the brief
  orange-only window on every connect, before the daemon asserts, is a real transient state.)
- **Storage-mode config knob — likely drop.** Ticket 02's write-up floated a NOSTORE/VARSTORE
  config option; [ticket 01](./01-prototype-status-led-controllability.md) found the byte
  inert on our unit (nothing persists either way). Confirm there is nothing user-visible to
  expose and this knob is cut.
- **The GUI control.** Confirmed (charting Q4/Q14): three labelled colour toggles in a
  "Status LEDs" group on the Device Overview panel, near the Profile selector. Pin down which
  file (`gui/acheron_gui/...`), whether it edits the *active* Profile or the
  *currently-selected-for-editing* Profile, and how it reflects state (the toggles show the
  selected Profile's stored triple).
- **GUI rules mirror.** Does any of this need to land in the GUI's `rules` mirror module
  (`post-release-development` ticket 06)? Likely not — there's no validation beyond "three
  bools" — but confirm.

Output: `## Answer` with the settled surface + schema; append the gist to the map's Decisions
so far.

## Comments

**2026-09-02 (Charon) — [ticket 03](./03-daemon-architecture-for-status-leds.md) resolved;
this ticket is now the frontier.** Inputs it fixes for the questions above:

- **`Effect` wiring is already decided.** `edit::plan`'s new set-Status-LEDs arm emits the
  unit `Effect::AssertStatusLeds` **unconditionally** — same as `SetActuationPoint` →
  `RepublishActuation`. This ticket only picks the `Command`/`Edit` name and payload shape;
  the effect variant and its `run_effects` handler belong to ticket 03's architecture.
- **Drop the "active vs currently-selected-for-editing Profile" question.** There is no such
  distinction in Acheron — every mutating D-Bus method is Profile-unscoped and applies to
  `active_profile`; the GUI's Device Overview renders only the active Profile and the
  sidebar calls `switch_profile` first (`device_overview.py:187–219`). The control edits the
  active Profile, full stop. So "does the write gate on *is this the active Profile*" is
  moot — it structurally always is.
- **`StatusLeds` is a named struct** (`{ orange, green, blue }`), not `[bool; 3]` / a tuple
  (ticket 03 §7) — so a later brightness byte or effect params don't churn the type. Pin
  the `#[serde]` attrs, `config.toml` key names, and `schema_version` bump + all-off
  migration here.
- **Storage-mode knob: cut, confirmed.** Ticket 03 §3 keeps no cached `led_state` at all
  (`Config` is the authoritative triple); combined with ticket 01's "the byte is inert",
  there is nothing user-visible to expose.
- **`GetState()` exposure** is still genuinely open for this ticket — the brief orange-only
  window on every connect (before dispatch asserts) is a real transient the GUI can't see
  from config alone; decide whether that needs reporting/signalling or not.

## Answer

**The settled GUI ↔ daemon surface + config schema.** Grilled + code-checked against
`daemon/src/` and `gui/acheron_gui/` on `dev` (HITL, Charon, 2026-09-02). Decisions only —
no build. Feeds [ticket 05](./05-write-status-led-spec.md) (the spec). Builds on
[ticket 03](./03-daemon-architecture-for-status-leds.md)'s architecture; nothing here
re-opens it.

### 1. `config.toml` schema — additive, no `schema_version` bump

**The charting Q15 / ticket-text "`schema_version` bump + migration" is wrong for this
codebase and is dropped.** `config::SCHEMA_VERSION` is still `1`; `config::parse` hard-refuses
any other value (`ConfigError::UnsupportedSchemaVersion` → the daemon won't start), and there
is no migration machinery. Every prior field addition — ticket 17 `default_actuation` /
`actuation_overrides` / `force_digital`, ticket 18 `held` / `mode_key_role`, ticket 51
`macros`, ticket 54 `steppers` — landed as a `#[serde(default)]` field with **no bump**,
guarded by a dedicated parse test (`a_pre_ticket_17_config_defaults_actuation_fields_and_force_digital`).
A bump to `2` would brick every existing `config.toml` on upgrade.

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

  Field names `orange` / `green` / `blue` (the fixed hardware colours, user-confirmed). Named
  struct, not `[bool; 3]` / tuple (ticket 03 §7) — a later brightness byte or effect params
  are then additive and the `watch<Option<StatusLeds>>` channel type + `Effect::AssertStatusLeds`
  variant don't churn. `Default` derives to all-`false`. Per-field `#[serde(default)]` so a
  hand-edited partial inline table (`status_leds = { orange = true }`) still parses.

- **New field** on `Profile` (`config.rs:130`), placed after `default_actuation` /
  `actuation_overrides`:

  ```rust
  #[serde(default)]
  pub status_leds: StatusLeds,
  ```

  `#[serde(default)]` **without** `skip_serializing_if` — always written back in full, exactly
  like `default_actuation` and `mode_key_role` (only the sparse *maps* carry
  `skip_serializing_if`). `toml::to_string_pretty` renders it as a
  `[profiles.<name>.status_leds]` sub-table with all three keys spelled out
  (`orange = false` …), matching how `default_actuation` serializes today.

- **Migration = the serde default.** A `config.toml` written before this feature has no
  `status_leds` key on any Profile → each parses to `StatusLeds::default()` = `(off, off,
  off)`, which *is* the charting-settled all-off migration default, reached the same way every
  other added field's default is. **No `config::validate` rule** — every `(bool, bool, bool)`
  is structurally valid; there is nothing to check. New parse test
  `a_pre_status_led_config_defaults_status_leds`, mirroring the ticket-17 one:
  a minimal `schema_version = 1` file with a Profile that has only `base` set must parse with
  `profile.status_leds == StatusLeds::default()`.

- **`dbus/wire.rs::profile_to_dict`** (`wire.rs:379`) gains a `"status_leds"` entry — a nested
  dict `{ "orange": bool, "green": bool, "blue": bool }`, following `actuation_point_to_dict`'s
  shape. A `config_to_dict` test asserts the three keys, mirroring
  `config_to_dict_serializes_default_actuation_and_actuation_overrides`.

### 2. D-Bus mutation — one whole-triple `Edit` / method, no gate, no signal

- **`Edit::SetStatusLeds { orange: bool, green: bool, blue: bool }`** — a new data-only
  variant in `daemon/src/edit.rs`'s `Edit` enum. `plan` arm (modelled on `SetActuationPoint`,
  `edit.rs:401`):

  ```rust
  Edit::SetStatusLeds { orange, green, blue } => {
      active_profile_mut(&mut next).status_leds = StatusLeds { orange, green, blue };
      effects.push(Effect::AssertStatusLeds);
  }
  ```

  Whole triple in one call — one frame drives all three channels (ticket 02), so a
  per-channel `SetStatusLed { channel, on }` would force a daemon-side read-modify-write and a
  channel-name enum to validate on the wire, for no gain. `Effect::AssertStatusLeds` is
  **unconditional**, exactly like `SetActuationPoint` → `RepublishActuation`: every mutating
  D-Bus method is Profile-unscoped and applies to `active_profile`, and the GUI switches
  Profile before editing (ticket 03 §5) — a Status-LEDs edit is *structurally* always an edit
  to the active Profile, so no `target == active` check. `plan` still returns `Result` for
  symmetry (the `config.toml`-write-failure case every persisting `Edit` carries); it never
  fails on its own account. The `Effect::AssertStatusLeds` variant + its `run_effects` handler
  belong to [ticket 03](./03-daemon-architecture-for-status-leds.md)'s architecture (§3), not
  this ticket.

- **D-Bus method** `SetStatusLeds(bbb) -> ()` on `com.acheron.Daemon` (`dbus/mod.rs`),
  shaped like `set_default_actuation` (`mod.rs:731`): build the `Edit` directly, `self.apply(...)`.

- **No new signal.** There is no generic config-changed signal in the daemon —
  `SetBinding` / `SetActuationPoint` / `SetDefaultActuation` emit nothing; the GUI rebuilds
  from `GetConfig` after its own calls, and on `ActiveProfileChanged` (which also covers a
  hardware `Action::ProfileSwitch`). `SetStatusLeds` follows suit.

### 3. `GetState()` — no addition

The active Profile's stored triple is fully visible via `GetConfig`. The *only* thing config
cannot show is the sub-second "orange-only" transient on every device (re)connect before
dispatch asserts (ticket 03 §4). [Ticket 01](./01-prototype-status-led-controllability.md)
found no on-device keymap switch, so the hardware cannot diverge from config behind the
daemon's back — the transient is the whole case, it is self-healing, and it is invisible to a
user not watching the physical device during a replug.

→ **No `command::State` field, no `state_to_dict` key, no `StatusLedsChanged` signal, no
dispatch `SignalEmitter` plumbing.** The GUI renders the stored triple, same as bindings and
actuation points. (`capture_mode` was added to `GetState` in `tartarus-input-expansion`
ticket 21 precisely because the *actual* capture mode can change under a running daemon
independently of config — Status LEDs have no such divergence path, so the parallel does not
apply.) If a live "what is physically lit right now" indicator is ever wanted, it is a
separate ticket bundled with the host-streamed-animation work (ticket 03 §8).

### 4. Storage-mode config knob — cut, confirmed

Ticket 02's write-up floated a NOSTORE/VARSTORE option;
[ticket 01](./01-prototype-status-led-controllability.md) found the `arg0` byte inert on our
unit (nothing persists either way across re-enumeration); ticket 03 §3 keeps no cached device
state at all. **Nothing user-visible to expose.** The daemon sends `arg0 = 0x00` as a fixed
intent-marker (ticket 02) — not a config field.

### 5. The GUI control — a "Status LEDs" group in the device area

**Placement** (per the user's mockup, `screenshots/Status LED location Mockup.png`): a new
column in `build_main_view`'s `device_row` (`gui/acheron_gui/device_overview.py:824`),
**between the thumbstick `stick_col` and `build_chords_section`** — a heading `Status LEDs`
over **three vertically-stacked colour lozenges**: orange (top), green (middle), blue
(bottom), roughly mirroring the physical LEDs' left-side placement on the real device. The
mockup is a rough sketch; the real widgets are uniform and aligned.

- **Widget:** each lozenge is a click-to-toggle button (`Gtk.ToggleButton` or a `Gtk.Button`
  carrying its own `.status-led` + per-colour CSS class). **Lit** = full-saturation colour
  fill + a visible border/glow; **unlit** = heavily desaturated + flat. The lit/unlit
  contrast must be *strong*, not a faint brightness shift — it is the primary state signal.
- **No visible per-lozenge text** (keeps the mockup's clean look), but each gets a
  **tooltip** (`"Orange status LED — on"` / `"… — off"`) and an accessible name/description
  (`set_tooltip_text` + `Gtk.Accessible` properties) so a colour-blind user is not relying on
  hue + brightness alone. Group heading stays `Status LEDs`.
- **Which Profile:** the **active** Profile, full stop — there is no
  "selected-for-editing vs active" distinction anywhere in Acheron (ticket 03 §5). The
  ticket-text framing of that question is dropped.
- **State source:** always `config["profiles"][profile]["status_leds"]` — never a live
  hardware read (no `GetState`). On a **newly created Profile** the three lozenges show dark,
  because `status_leds` defaults all-`false` and "never set" is byte-identical to "explicitly
  all-off" (§1) — no special-casing. The first toggle the user flips calls
  `set_status_leds(...)`, which persists that Profile's triple and (via
  `Effect::AssertStatusLeds`) immediately drives the device.
- **When the device is disconnected:** the lozenges still show the stored config state (orange
  lit if the active Profile says so). This matches every other Device Overview control
  (bindings/actuation/chords all render the Profile's config while disconnected), and the
  daemon re-asserts the shown state on the next connect (ticket 03 §4). The lozenge means
  "this Profile's Status-LED assignment," which is what the daemon drives the hardware to.
- **Layer / destination visibility:** Grid destination **only** (alongside Chords, also
  Grid-only); shown **identically on both Base and Held** — the group is Profile-scoped, not
  Layer-scoped, so it does not change or disappear when the layer bar flips. Not rendered in
  the Library destination. Renders from `status_leds` regardless of `selected_layer`.
- Each lozenge's toggle handler reads **all three** current lozenge states and calls
  `client.set_status_leds(orange, green, blue)`; the group then rebuilds from config on the
  shared `on_change`, like everything else on the panel.

### 6. GUI mirror obligations (ADR-0005) — mechanical, no logic

- **`daemon_client.py`:** `set_status_leds(self, orange: bool, green: bool, blue: bool)` →
  `self._call("SetStatusLeds", GLib.Variant("(bbb)", (orange, green, blue)))`, plus the
  abstract-method stub in the `Protocol`.
- **`daemon_stub.py`:** same signature — mutates
  `self._profiles[self._active_profile]["status_leds"]`, appends
  `("set_status_leds", orange, green, blue)` to `self.calls`; the stub's seed Profile dict
  gains `"status_leds": {"orange": False, "green": False, "blue": False}`.
- **`wire.py` / `read_model.py`:** surface `status_leds` in the config dict the GUI reads —
  the mirror of `profile_to_dict`'s new entry.
- **`rules.py`: nothing.** It mirrors only the daemon's *pure validation core*
  (`valid_action_kinds`, `valid_triggers`, slug, chord conflicts). "Three bools, always valid"
  has no invariant to mirror. Confirmed against `post-release-development` ticket 06's rules
  mirror.

### 7. Corrections this ticket makes to the map / downstream

- The map's charting-Q15 line and [ticket 05](./05-write-status-led-spec.md)'s "Config
  schema" bullet both say "`schema_version` bump + migration" — **corrected to: additive
  `#[serde(default)]` field, no bump** (§1). Ticket 05's comment is updated accordingly.
- [Ticket 05](./05-write-status-led-spec.md)'s "D-Bus surface — `GetState()` additions /
  signal (if any)" resolves to **none** (§3).
- No new `CONTEXT.md` term and no ADR from this ticket — the `Status LED` /
  `Status LED assignment` glossary entries and ADR-0006 land with
  [ticket 05](./05-write-status-led-spec.md), as already planned.
- No fog graduates (the map's **Not yet specified** is already empty) and nothing new is
  ruled out of scope. No new tickets — [ticket 05](./05-write-status-led-spec.md) is the last
  one and is now unblocked.
