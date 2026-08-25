Type: task
Blocked by: 20
Status: resolved

## Question

Build Analog-repeat for real: land `TriggerMode::AnalogRepeat` per
[ticket 20](./20-decide-analog-repeat-trigger-mode.md)'s settled design, and tune its numeric
constants live against the real Tartarus Pro. Non-blocking for v1.0.

Scope, per ticket 20's decisions:

- New `TriggerMode::AnalogRepeat` variant: config schema, D-Bus wire encoding
  (`dbus/wire.rs`'s `trigger_mode_str`/`trigger_mode_from_str`), GUI `TRIGGER_OPTIONS`.
- `dispatch.rs` gets its own receiver on the existing `depth_tx` watch channel (mirrors how it
  already holds `actuation_tx`), rather than relying on `PhysicalEvent.depth`'s coarser
  Repeat-cadence delivery.
- A new per-Input background-task type (its own `HashMap<Input, _>` tracking, alongside the
  existing `toggles`/`in_flight`), spawned when a grid key's Depth crosses a small **fixed**
  deadzone threshold going up (deliberately not the key's tunable Actuation point — the rate
  curve wants the key's full 0–255 travel range), cancelled when Depth crosses back down through
  that same fixed threshold. Each tick recomputes its own next sleep duration from the latest
  Depth, linearly mapped across the full 0–255 range to a min/max Hz.
- Each fire is a Down+Up pulse of a fixed short duration (not Depth-scaled).
- Above a fixed near-full-travel threshold, the key holds down solid instead of continuing to
  tap.
- Digital Capture mode (no Depth): falls back to plain Hold-to-repeat, kernel-autorepeat cadence
  — matches how every other grid key already behaves there.
- GUI: "Analog-repeat" greyed out as a Trigger-mode option for non-Grid Inputs (mirrors the
  existing `is_grid_input()` gate on the Actuation & release section).
- **Tune live against the real device**: the deadzone threshold, the min/max Hz bounds, the
  fixed fire-duration, and the hold-solid threshold are all TBD — pick values that feel right
  against actual key travel, not blind numbers carried over from the design ticket.

## Answer

Landed end to end against ticket 20's settled design, AFK — **no physical Tartarus Pro was
available in this session**, so the four numeric constants (deadzone, min/max Hz, pulse-hold
duration, hold-solid threshold) are documented placeholders, not live-tuned values; each is a
single named `dispatch.rs` constant with a doc comment flagging it as such, so tuning later is a
one-line change per constant, no architecture change needed.

**`TriggerMode::AnalogRepeat`**: a fourth `config.rs` enum variant. Wire encoding
(`dbus/wire.rs`) as `"analog_repeat"`. GUI `TRIGGER_OPTIONS` gained a fourth entry, offered only
for a Grid Input (mirroring `ACTION_TYPES`'s own `is_grid_input`-gated "Axis" exclusion, ticket
60's Answer) rather than literally "greyed out" as the ticket's own text asked — `Gtk.DropDown`
has no per-item sensitivity (ticket 55's precedent for the identical limitation), so exclusion is
the only structural-prevention mechanism actually available. Rejected outright (Daemon `SetBinding`
+ `config::parse`, mirroring `InvalidStepTrigger`'s exact precedent) on a non-Grid Input, and on a
Chord's own Binding (mirroring `InvalidChordProfileSwitch` — a Chord fires on a discrete
member-set completion, not one grid key's continuous Depth); `daemon_stub.py` mirrors both checks.

**Architecture**: `dispatch::run` already held its own `rx_depth: watch::Receiver<HashMap<Input,
u8>>` clone from ticket 71's axis-assignment work (reused here rather than growing the function's
already-large parameter list with a second depth channel, an intentional deviation from the
ticket's literal "gets its own receiver" — the existing one already serves the purpose). A new
per-Input `HashMap<Input, ActiveAnalogRepeat>` (`analog_repeats`), spawned/cancelled by a new
`update_analog_repeats`, called from the same `rx_depth.changed()` arm that already drives
`handle_depth_update`'s Axis resolution — a rising edge through the fixed deadzone on a grid Input
whose active-Layer Binding is `AnalogRepeat` spawns `ActiveAnalogRepeat` (steps compiled once via
the existing `compile_action`, the same "once per press" precedent `fire()` already sets — a
Stepper Action's cursor advances once per "press session," not auto-cycling at the tick rate); a
falling edge stops it. `ActiveAnalogRepeat` itself (`cancel`/`handle`, `spawn`/`stop`) is
structurally identical to `ActiveToggle`, just depth-driven instead of a fixed lap. Its own task
body (`run_analog_repeat_loop`) reads live Depth off its own clone of the watch channel on every
tick — below the hold-solid threshold, computes a linear-interpolated period from
`ANALOG_REPEAT_MIN_HZ`/`_MAX_HZ` across the full 0-255 range and fires a pulse (`fire_analog_repeat_pulse`:
every `KeyDown` step, sleep `ANALOG_REPEAT_PULSE_HOLD`, every `KeyUp` step in reverse — reuses
`executor::execute_step`/`force_release`, promoted `pub(crate)` alongside `keypress_steps`'s
existing precedent for the same reason); at or above it, holds every `KeyDown` step solid with no
further tapping until Depth drops back below. Deliberately ignores any `MacroStep::Delay` a Macro
Action might embed — Analog-repeat's whole idea is one fixed-duration pulse, not a timed sequence.

**The Analog/Digital split**: `handle_event` already carries `event.depth: Option<u8>` per Input
(ticket 17). For an `AnalogRepeat` Binding, an Analog-sourced Down/Repeat/Up (`event.depth: Some`,
synthesized from the key's ordinary *tunable* Actuation/Release points — a different threshold
pair than Analog-repeat's own fixed deadzone) is swallowed outright, mirroring the Axis-assignment
swallow that already sits right above it — real firing is `update_analog_repeats`'s background
task, never `fire()`. A Digital-sourced event (`event.depth: None`) falls through to `fire()`,
whose match arms now treat `AnalogRepeat` exactly like `HoldToRepeat` (ticket 20's Digital Capture
mode fallback) — one shared arm-set addition, not a parallel branch.

**Reset points**: every `analog_repeats` task is force-stopped (force-releasing whatever it's
mid-pulse holding) on Layer switch and Profile switch — mirroring `reset_axis_outputs`'s exact
"an Analog-repeat task is tied to one specific Layer/Profile's Binding" reasoning, not `ActiveToggle`'s
deliberately-persisted-across-switches precedent — and on an Analog-to-Digital capture-mode
transition, since the live-Depth stream every task reads goes stale the moment that happens and
Digital-sourced events for the same Binding are about to start reaching `fire()`'s own fallback
instead (a still-running task would otherwise double-fire alongside it).

**Two accepted residual gaps**, both documented in code rather than engineered around, in the
same spirit as ticket 71's own opposite-signed-halves tie-break: (1) a Binding changed away from
`AnalogRepeat` via a live `SetBinding` while Depth stays continuously above the deadzone (no
intervening crossing) leaves the stale task running with its spawn-time-compiled steps until Depth
next crosses the deadzone; (2) a grid key that is *both* a Chord member and individually
`AnalogRepeat`-triggered fires once via the ordinary one-shot path when it resolves retroactively
(`fire_individual_retroactively`), rather than starting the depth-driven task — a narrow
combination outside this fast-follow's own driving-sim use case.

**A real bug caught by this ticket's own new tests, intermittently**: `run_analog_repeat_loop`'s
hold-solid branch waits on `tokio::select! { cancel.cancelled(), depth_rx.changed() }` — when
Depth crosses back below the deadzone, the external `update_analog_repeats` both stops publishing
into `depth_rx` *and* calls `task.stop()` (cancellation) off the same event, so both branches can
become ready together and `select!` doesn't always pick cancellation first. Picking `depth_rx.
changed()` instead let the loop fall through to the ordinary tapping branch with a stale
below-deadzone Depth, firing one spurious extra pulse at the curve's own minimum rate before the
real cancellation ever landed — `analog_repeat_holds_solid_above_the_hold_threshold` caught it as
an intermittent `batches.len()` of 3 instead of 2, reproducing on roughly 2 in 5 runs. Fixed by
having the loop check the deadzone itself on every iteration (not just hold-solid vs. tapping) —
below it, wait for cancellation instead of computing a rate and firing, regardless of why the
wakeup happened. Verified with 30 back-to-back runs of the regression test, all clean, plus 15
full-suite runs.

**Tests** (no hardware, so all against the real dispatch pipeline with a real `Injector`/
`RecordingSink`, `#[tokio::test(start_paused = true)]` + `tokio::time::advance` for the two
timing-dependent ones — the same proven pattern `overlapping_same_input_firings_are_dropped_not_queued`
already uses in this file): Digital-sourced Down/Repeat/Up behaves exactly like Hold-to-repeat;
Analog-sourced Down/Repeat/Up is fully swallowed (zero output) with no depth crossing published;
the background task fires periodically at the rate its own `ANALOG_REPEAT_MIN_HZ`/`_MAX_HZ`
constants predict and produces nothing further once Depth drops back below the deadzone; holding
above the hold-solid threshold produces exactly one `KeyDown` (no tapping) until force-released on
stop; `SetBinding`/`SetChordBinding` reject/accept exactly per the rules above, live and
persisted. 330→339 Rust tests, 284→290 Python tests, `cargo clippy`/`cargo fmt --check` and
`ruff`-equivalent (none configured beyond pytest) all clean.

**Not done this session, both explicitly scoped to hardware access**: live tuning of the five
placeholder constants, and live-hardware verification of the whole strand (feel of the rate curve,
the hold-solid threshold, and the Digital-mode fallback against a real driving-sim-style use).
Spawned [Verify and tune Analog-repeat on hardware](./73-task-verify-and-tune-analog-repeat-on-hardware.md),
matching every other build ticket's own decide/prototype/build → verify precedent on this map.
