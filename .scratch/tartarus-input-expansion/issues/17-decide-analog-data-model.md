Type: grilling
Blocked by: 16
Status: resolved

## Question

Decide **how analog depth is represented** across the Daemon's event stream, the `Binding`
model, the config schema, and the D-Bus wire. This is the one analog ticket promoted into
the v1.0 required floor (see the map's Destination): the capture rework and every feature
above it stay non-blocking, but the *model* must be settled before the remaining
Binding-editor tickets (01, 02, 03, 05, 15) write their half of `binding_editor.py` and the
config schema against a shape analog will force us to break.

Deliberately **not** wired as a blocker on those five — see the map's standing discipline.
It is simply the ticket to take next.

Settle at least:

- **Does `PhysicalEvent` widen to carry depth?** `daemon/src/capture/mod.rs` currently
  defines `PhysicalEvent { input: Input, state: EventState }` with `EventState` as
  `Down`/`Repeat`/`Up`, and its doc comment states the stream is the *only* contract
  anything downstream relies on. The cheap option is for an analog `CaptureSource` to
  threshold depth internally and emit exactly today's `PhysicalEvent`, changing nothing
  downstream. That cannot support the Analog-repeat Trigger mode ([ticket
  20](./20-decide-analog-repeat-trigger-mode.md)), live depth in the GUI ([ticket
  19](./19-prototype-trigger-point-ux-and-live-depth.md)), or real analog axes (ticket 14),
  all of which need depth to reach `dispatch.rs`. Decide the widened shape — an optional
  depth field, a separate variant, a parallel channel — and what it means for `fake.rs` and
  the 72 existing Daemon tests.
- **Where does the actuation point live?** Per-`Binding`, per-`Input` per-Profile, a global
  default with per-Binding override, or per-Profile only. Note the asymmetry: an actuation
  point is a property of a *physical grid key*, while a `Binding` is scoped to a
  Profile/Layer pair — so a naive per-Binding field means the same key can have two
  different actuation points in Base and Held, which may or may not be wanted.
- **One threshold or two?** A single threshold chatters at the boundary. Decide whether the
  model carries an actuation point plus a separate (lower) release point — hysteresis — and
  whether the user sees both or the release point is derived.
- **What happens to the 14 non-grid Inputs?** Mode key, thumbstick ×4, wheel ×3 have no
  depth and never will. Decide whether the model makes depth structurally optional on
  `Input` (so a thumbstick Binding simply has no actuation point), or whether grid keys
  become a distinct type. Depends on [ticket 16](./16-task-analog-mode-hardware-facts.md)'s
  finding on whether those Inputs even survive driver mode.
- **How is device mode represented, and what does the config/wire say about it?** Per the
  map's Notes the digital path survives as an automatic degradation path plus an explicit
  user-facing force-digital override. Decide where that override lives (config file, D-Bus
  call, both), how the Daemon reports which mode it actually landed in, and what the GUI
  shows when the user asked for analog and got digital.
- **Config migration**: whether an existing `config.toml` written by the shipped MVP still
  loads unchanged, and what an actuation point defaults to when absent.

## Answer

Settled across seven decisions, grilled 2026-08-17. All five pieces the Destination names —
`PhysicalEvent`, `Binding`, the config schema, the D-Bus wire, and device-mode representation
— are covered.

### 1. `PhysicalEvent` widens with an optional field

```rust
pub struct PhysicalEvent {
    pub input: Input,
    pub state: EventState,
    pub depth: Option<u8>,
}
```

`None` for every evdev-sourced event (including the 8 non-grid Inputs even while analog is
active) and for any grid key while degraded to digital. `Some(value)` for analog-sourced grid
events. Keeps `PhysicalEvent` the single contract the module doc comment already promises,
at the mechanical (not architectural) cost of adding `depth: None` to existing construction
sites across `fake.rs` and the 72 Daemon tests. Rejected a separate `EventState` variant (touches
every exhaustive match on trigger dispatch) and a parallel depth channel (reintroduces the
"no ordering guarantee between channels" hazard `dispatch.rs`'s own tests already flag for
`rx_connection`/`rx_commands`). `EventState::Down/Repeat/Up` keep their existing meaning
unchanged; how the analog source *synthesizes* Down/Repeat/Up from depth thresholds (including
matching kernel autorepeat timing) is ticket 18's job, not this ticket's.

### 2. Hysteresis: the model carries both an actuation point and a release point

Chatter risk is real (256 distinct depth values, ~1ms-granularity reports while moving, per
ticket 13/16). Whether the user *sees* both markers or one derived value is ticket 19's UX call;
the model carries both regardless, so ticket 19 never has to widen it.

### 3. `Input` stays unified; depth is validated, not typed

No `GridKey` split. Anything that sets an actuation point on a non-`Grid` `Input` is rejected
with a `DaemonError::InvalidBinding` at the D-Bus boundary (`Daemon::parse_input` already has
the precedent for this kind of validation). `Input` remains the one key used for Binding
lookup, the `config.toml` schema, and the D-Bus wire — a parallel type would only exist to
carry one field.

### 4. Device mode: a `Config`-level preference plus a live-reported actual state

- `Config.force_digital: bool` — top-level, not per-Profile (a hardware/troubleshooting
  setting, not a gameplay one), `#[serde(default)]`. Persisted like any other `Command`, set
  live via a new `SetForceDigital(bool)` D-Bus method, mirroring `SetOutputSuppressed`'s
  pattern exactly. Returned as part of `GetConfig()`'s existing full-document dict — no new
  method needed to read it back.
- `GetState()` gains a `capture_mode: String` (`"analog"` / `"digital"`) field — the mode the
  Daemon actually landed in right now, distinct from the `force_digital` preference. Needs
  live push same as `device_connected`: ticket 16 proved the actual mode can change under a
  *running* Daemon (survives suspend, not a power cycle), so a new `CaptureModeChanged(mode:
  String)` signal mirrors `DeviceConnectionChanged` exactly.

### 5. Actuation-point scoping: per-Input, per-Profile, with a Profile-level default

Not per-Binding (would let the same physical key have two actuation points across Base/Held —
the exact asymmetry the ticket's question flagged, and physically wrong: the key doesn't move
differently because Mode is held). Not global (can't express a "racing" Profile feeling
different from an "FPS" Profile, which fits Profile's existing manually-switched-per-game
role). Not per-Profile-only (throws away the per-key resolution ticket 13 found real).

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActuationPoint {
    pub actuation: u8,
    pub release: u8,
}

impl Default for ActuationPoint {
    fn default() -> Self {
        ActuationPoint { actuation: 128, release: 112 }
    }
}
```

On `Profile`:

```rust
#[serde(default)]
pub default_actuation: ActuationPoint,
#[serde(default, skip_serializing_if = "HashMap::is_empty")]
pub actuation_overrides: HashMap<Input, ActuationPoint>,
```

A grid key with no entry in `actuation_overrides` uses `default_actuation`. 128/112
(half-travel, 16-point margin) is an explicit placeholder, not a tuned value — ticket 19 is
where it gets felt through a real UI and adjusted. The GUI is expected to offer both a
per-key override control and a "reset all keys to Profile default" affordance (user
requirement surfaced during grilling) — the latter is `ResetActuationPoints()` below, not 20
individual clears.

### 6. Config migration: additive, no `schema_version` bump

Both new `Profile` fields are `#[serde(default)]`, following the exact precedent ticket 18 set
for `held`/`mode_key_role` — a pre-ticket-17 `config.toml` still loads unchanged, silently
gaining `default_actuation: 128/112` and no overrides. `Config.force_digital` is likewise
`#[serde(default)]` (`false`). `SCHEMA_VERSION` stays `1`.

### 7. D-Bus wire: five new methods, active-Profile-scoped, one persist each

Mirrors `SetBinding`/`ClearBinding`/`StopAllToggles`'s existing atomic-and-persisted
conventions:

- `SetActuationPoint(input: String, actuation: u8, release: u8) -> Result<(), DaemonError>` —
  sets a per-key override on the active Profile. Rejects a non-`Grid` `input` (§3) and,
  expected as an implementation-level invariant, `release > actuation`.
- `ClearActuationPoint(input: String) -> Result<(), DaemonError>` — removes the override,
  reverting to `default_actuation`.
- `SetDefaultActuation(actuation: u8, release: u8) -> Result<(), DaemonError>` — sets the
  active Profile's `default_actuation`.
- `ResetActuationPoints() -> Result<(), DaemonError>` — clears every override on the active
  Profile in one call and one `config.toml` rewrite (the GUI's "reset all keys" button).
- `SetForceDigital(force: bool) -> Result<(), DaemonError>` — the live setter for §4's
  preference.

All five need matching `Command` variants in `daemon/src/command.rs`, following the existing
`reply: oneshot::Sender<...>` shape every other mutating Command already uses.

### What this unblocks

[Ticket 18](./18-rework-capture-path-for-analog.md) (capture-path rework),
[ticket 19](./19-prototype-trigger-point-ux-and-live-depth.md) (trigger-point UX/live depth),
and [ticket 20](./20-decide-analog-repeat-trigger-mode.md) (Analog-repeat Trigger mode) all
build against this model now. None of their own ticket bodies needed correction — each already
deferred its analog-data-model specifics to this ticket rather than assuming a shape.

## Comments

**[Ticket 16](./16-task-analog-mode-hardware-facts.md) has resolved; this ticket is
unblocked.** Two corrections to the question above:

- **"What happens to the 14 non-grid Inputs?" — there are 8, not 14**, and they all survive
  driver mode. `Input` has 28 variants: 20 `Grid`, plus `ModeKey`, `Thumbstick` ×4 and
  `Wheel` ×3. So the question is live exactly as posed (does depth become structurally
  optional on `Input`, or do grid keys become a distinct type?) — but it covers 8 Inputs,
  and none of them is at risk of vanishing.
- **Device-mode representation has a new asymmetry to model**: the mode survives
  suspend/resume but not a power cycle, and an unclean Daemon death leaves it on. Whatever
  the model says about "which mode the Daemon landed in" therefore cannot be write-once at
  startup — the device can change mode underneath a running Daemon, and only a USB
  re-enumeration signals it.
