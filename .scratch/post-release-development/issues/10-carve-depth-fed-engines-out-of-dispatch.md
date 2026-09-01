<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 10 — Carve the two Depth-fed engines out of `dispatch.rs` into `axis` and `analog_repeat`

**What to build:** The two engines fed from `run`'s single `rx_depth`
`select!` arm — Axis-assignment output resolution (ticket 59/71) and the
Analog-repeat rate curve (ticket 20/39) — move behind one small interface
each, in a new `daemon/src/axis.rs` and `daemon/src/analog_repeat.rs`.

`axis::Engine` is pure and synchronous: it owns the per-Input contribution /
axis-owner state and answers "given these resolved contributions and this
Layer's assignment map, what `ABS_*` writes should happen?" with a
`Vec<AxisWrite>` — no `&Injector`, no `async`, no channels. `dispatch` keeps
the `depth → value` ramp (`config::resolve_axis_value`, which needs the
per-Input Actuation point) and performs the writes.

`analog_repeat` splits in two: a pure decision core (`tick_plan`,
`reconcile`, `pulse_hold_for` — the hardware-tuned numbers, finally
table-testable) and `analog_repeat::Engine`, a spawned-task supervisor
shaped like `executor::ActiveToggle` that owns the
`HashMap<Input, ActiveAnalogRepeat>` and its cancel tokens. `dispatch` keeps
`compile_action` and hands the engine pre-compiled `Vec<MacroStep>`.

Neither engine ever sees `Config`, `CaptureMode`, or `Layer`.

## The friction

`dispatch.rs` (5033 lines) still carries these two engines inline, ~400
lines of module-level code plus two `DispatchState` methods and a
free-function edge handler, interleaved between `run`'s `select!` loop and
`handle_command` / `run_effects`:

- **Axis** (lines 83, 111–247, 1624–1638 + `handle_depth_update` at 731):
  `const AXIS_DIGITAL_STEP`, `struct AxisState`, `resolve_axis_contribution`
  (the §5 conflict rule — **pure, and with zero direct unit tests**),
  `recompute_and_emit_axes`, `reset_axis_outputs`, `handle_axis_edge_event`.
- **Analog-repeat** (lines 254–462 + `update_analog_repeats` at 840): six
  `ANALOG_REPEAT_*` constants, `struct ActiveAnalogRepeat` (a `JoinHandle` +
  `CancellationToken`), `fire_analog_repeat_pulse`, `run_analog_repeat_loop`
  (the rate curve, the deadzone / hold-solid bands, the spurious-pulse
  guard), `stop_all_analog_repeats`.

Two independent concepts — CONTEXT.md defines "Axis assignment" and
"Analog-repeat" as separate terms — sharing nothing but the `rx_depth`
snapshot value. Every test of either runs through the full `CommandHarness` /
`Seam` rig: spawn `run`, an injector, seven channels, a tempfile. The §5
conflict rule and the rate curve are exactly the logic the archived
`tartarus-input-expansion` map still carries fog for (rate-curve
refinement, Sticky/latching mode, and — raised in this grilling — plausibly
user-editable actuation curves).

## Relationship to tickets 03–09

Ticket 03 made *edit + persist* atomic; 04 single-sourced *validation*; 05
lifted the config transaction into the pure `edit` module; 06 built the
contract-tested GUI `rules` mirror; 07 carved the Chord state machine into
the pure `chord` module; 08 unified the Trigger-mode matrix into the pure
`trigger` module; 09 concentrated the dispatch task's runtime state into one
`DispatchState`. This ticket carves the last two deep engines still living
inline in `dispatch.rs` — the ones ticket 08's "Out of scope" section named
(_"The Analog-repeat engine … Its own architecture-review candidate (carve
the pure rate-curve decision core, the `chord.rs` treatment for the other
hardware-tuned timing engine)"_) and ticket 59 banked a seam forward for
(_"axis resolution should be its own `(Depth, edge_event) → axis_value` seam
so a future Sticky/latching mode is a small addition, not a rewrite"_).

## The `axis` module

```rust
// daemon/src/axis.rs
//
// Pure and synchronous. Imports Input / AxisTarget / AxisPolarity /
// AbsoluteAxisCode / EventState. NOTHING from executor, injector, edit, or
// dispatch. No &Injector, no async, no channels. The depth→value ramp
// (config::resolve_axis_value — it needs the per-Input Actuation point)
// stays in dispatch; the engine's inputs are already-resolved 0-255
// contributions.

/// The per-Input axis output state (ticket 59/71). `contributions` is the
/// live 0-255 value each Axis-assigned Input wants to drive its target
/// with; `owners` is which single Input currently wins each signed axis's
/// opposite-half suppression (§5). Both reset fresh per dispatch task start
/// (ex-`dispatch::AxisState`, verbatim fields).
pub(crate) struct Engine {
    contributions: HashMap<Input, u8>,
    owners: HashMap<AbsoluteAxisCode, Input>,
}

/// One `ABS_*` write the dispatch executor must emit via
/// `injector.set_axis_value(code, value)`. `value` is already signed
/// (negative for a driven negative half).
pub(crate) struct AxisWrite {
    pub code: AbsoluteAxisCode,
    pub value: i32,
}

impl Engine {
    /// The continuous Analog path (ticket 59 §7). `resolved` carries the
    /// depth→value output for the Inputs dispatch just recomputed off a
    /// fresh `rx_depth` snapshot (`config::resolve_axis_value` per Input).
    /// Merges them into `contributions`, then re-runs §5 resolution for
    /// every `ABS_*` code `axis_map` touches — plus any stale code an owner
    /// still lingers on — and returns the writes. Replaces
    /// `handle_depth_update`'s tail + `recompute_and_emit_axes`.
    pub(crate) fn resolve(
        &mut self,
        axis_map: &HashMap<Input, AxisTarget>,
        resolved: &HashMap<Input, u8>,
    ) -> Vec<AxisWrite>;

    /// Re-run §5 resolution off the stored `contributions` with no new
    /// delta — `run_effects`' `RecomputeAxes` handler, after a live
    /// `SetAxisAssignment` / `ClearAxisAssignment` changed `axis_map`.
    /// Equivalent to `resolve(axis_map, &HashMap::new())`; named for the
    /// call site's intent.
    pub(crate) fn recompute(
        &mut self,
        axis_map: &HashMap<Input, AxisTarget>,
    ) -> Vec<AxisWrite>;

    /// The Digital Capture-mode fallback (ticket 59 §6): step this Input's
    /// contribution up by `AXIS_DIGITAL_STEP` on Down/Repeat (saturating),
    /// reset to 0 on Up, then re-resolve `axis_map`. Replaces
    /// `handle_axis_edge_event`. Only ever reached for a genuinely
    /// Digital-sourced event — dispatch gates on `event.depth.is_none()`.
    pub(crate) fn step_digital(
        &mut self,
        axis_map: &HashMap<Input, AxisTarget>,
        input: Input,
        state: EventState,
    ) -> Vec<AxisWrite>;

    /// Drop this Input's contribution outright — `run_effects`'
    /// `ForgetAxisContribution`, emitted by `ClearAxisAssignment`. The
    /// caller follows with `recompute`.
    pub(crate) fn forget(&mut self, input: Input);

    /// Center every owned `ABS_*` code and clear all state — a Layer/Profile
    /// switch (`handle_layer_switch`, `run_effects`' `ResetAxisOutputs`).
    /// Returns `vec![]` with no state touched when nothing has ever driven
    /// output (the overwhelmingly common Layer/Profile — preserves
    /// `reset_axis_outputs`'s write-free fast path).
    pub(crate) fn reset(&mut self) -> Vec<AxisWrite>;
}

/// The §5 runtime-conflict rule (ticket 59 §5) — moves in verbatim from
/// `dispatch::resolve_axis_contribution`, stays a private fn, and finally
/// gets direct unit tests (it has none today; every current axis-conflict
/// test runs through `CommandHarness`).
fn resolve_axis_contribution(
    positive: &[(Input, u8)],
    negative: &[(Input, u8)],
    current_owner: Option<Input>,
) -> (i32, Option<Input>);
```

`AXIS_DIGITAL_STEP` moves into `axis.rs`.

## The `analog_repeat` module

```rust
// daemon/src/analog_repeat.rs
//
// The decision core (`tick_plan`, `reconcile`, `pulse_hold_for`) is pure and
// synchronous. `Engine` owns tokio tasks and is NOT pure — the same shape as
// `executor::ActiveToggle`. Imports Input / Action / KeyCode / MacroStep /
// executor / injector / watch. NOTHING from dispatch, edit, config::Config,
// chord, or trigger.

// The six build-time-tuned constants move in verbatim (tickets 20/39;
// hardware-confirmed as-shipped by ticket 73):
//   ANALOG_REPEAT_DEADZONE, _MIN_HZ, _MAX_HZ, _PULSE_HOLD,
//   _CONTROLLER_PULSE_HOLD, _HOLD_SOLID

/// What one iteration of the repeat loop should do, given the current Depth
/// and whether the loop is already holding the key solid. Pure — captures
/// every band decision in today's `run_analog_repeat_loop` body in one
/// call, without forcing the loop's `select!` structure to change.
/// `release_solid_first` mirrors today's "if holding_solid { release up
/// steps }" that runs before the deadzone / tapping branches.
pub(crate) enum TickPlan {
    /// Depth ≥ HOLD_SOLID: press every Down step solid if not already
    /// holding, then wait on `depth_rx.changed()` / cancel.
    HoldSolid,
    /// Depth < DEADZONE: `update` is about to stop this task (or a stale
    /// wakeup is racing it) — wait, don't fire a spurious minimum-rate
    /// pulse (preserves the dispatch.rs:410–421 guard).
    Idle { release_solid_first: bool },
    /// In the tapping band: fire one Down/hold/Up pulse, then sleep so the
    /// tick-to-tick spacing is `period` measured from the tick start.
    Tap { period: Duration, release_solid_first: bool },
}

pub(crate) fn tick_plan(depth: u8, holding_solid: bool) -> TickPlan;

/// Spawn/stop policy off one `rx_depth` snapshot — today's
/// `update_analog_repeats` head. `repeat_inputs` is the set of Inputs whose
/// active-Layer Binding is `TriggerMode::AnalogRepeat` (computed by
/// dispatch from `Config`). `active` is the Inputs with a live task.
/// Iterates `snapshot` exactly as today (every grid key is present on every
/// Analog report). Pure, table-tested.
pub(crate) fn reconcile(
    active: &HashSet<Input>,
    repeat_inputs: &HashSet<Input>,
    snapshot: &HashMap<Input, u8>,
) -> Vec<Reconcile>;
pub(crate) enum Reconcile { Spawn(Input), Stop(Input) }

/// Analog-repeat's per-fire hold: the 35 ms frame-safe floor for a
/// `ControllerButton` output (ticket 78), the 15 ms dwell for every other
/// Action. Pure. Dispatch passes `&binding.action`.
pub(crate) fn pulse_hold_for(action: &Action) -> Duration;

/// The task supervisor. Owns `HashMap<Input, ActiveAnalogRepeat>` (each a
/// `JoinHandle` + `CancellationToken`), reset fresh per dispatch task start
/// (ex-`DispatchState::analog_repeats`). `ActiveAnalogRepeat`,
/// `fire_analog_repeat_pulse`, `run_analog_repeat_loop` move in as private
/// items; `run_analog_repeat_loop` shrinks to a shell driving `tick_plan`.
pub(crate) struct Engine {
    tasks: HashMap<Input, ActiveAnalogRepeat>,
}

impl Engine {
    /// Run `reconcile` against the live task set, perform every `Stop`
    /// (cancel + await — the engine owns the map), and return the Inputs
    /// that need a fresh task. Dispatch compiles each one's steps
    /// (`compile_action`, staying dispatch-side) and calls `spawn`.
    /// Replaces `update_analog_repeats`'s body.
    pub(crate) async fn update(
        &mut self,
        repeat_inputs: &HashSet<Input>,
        snapshot: &HashMap<Input, u8>,
    ) -> Vec<Input>;

    /// Compile-once-at-spawn (a Stepper cursor advances per press-session,
    /// not per tick — mirrors `perform_trigger`). `steps` and `pulse_hold`
    /// arrive pre-resolved from dispatch. Sync, like today's
    /// `ActiveAnalogRepeat::spawn`.
    pub(crate) fn spawn(
        &mut self,
        injector: Injector,
        input: Input,
        steps: Vec<MacroStep>,
        pulse_hold: Duration,
        depth_rx: watch::Receiver<HashMap<Input, u8>>,
    );

    /// Force-stop every task — a Layer/Profile switch, an Analog→Digital
    /// transition (`handle_layer_switch`, `handle_capture_mode_change`,
    /// `run_effects`' `StopAllAnalogRepeats`). Ex-`stop_all_analog_repeats`.
    pub(crate) async fn stop_all(&mut self);
}
```

`rate_period(depth) -> Duration` (the `1 / lerp(MIN_HZ, MAX_HZ, depth/255)`
math) stays a private helper feeding `tick_plan`'s `Tap.period`; the table
asserts it at ticket 73's verified sample points (depth 12 ≈ 2.85 Hz,
100 ≈ 9 Hz, 235 ≈ 18.6 Hz).

## What moves, what stays

- **Into `axis.rs`:** `AXIS_DIGITAL_STEP`, `AxisState` (as `Engine`'s
  fields), `resolve_axis_contribution` (private), `recompute_and_emit_axes`
  and `reset_axis_outputs` (rewritten to return `Vec<AxisWrite>` instead of
  awaiting the injector), `handle_axis_edge_event` (as `step_digital`).
- **Into `analog_repeat.rs`:** the six `ANALOG_REPEAT_*` constants,
  `ActiveAnalogRepeat` (+ `spawn` / `stop`), `fire_analog_repeat_pulse`,
  `run_analog_repeat_loop` (shrunk to a `tick_plan` shell),
  `stop_all_analog_repeats` (as `Engine::stop_all`).
- **Stays in `dispatch.rs`:**
  - `handle_depth_update` and `update_analog_repeats` — still `&mut self`
    methods on `DispatchState` (they need `&Config` for the axis map /
    Actuation points / `repeat_inputs`, `&mut stepper_cursors` for
    `compile_action`, and both engines), but thin: resolve the ramp /
    compute `repeat_inputs`, delegate, emit.
  - `compile_action` / `resolve_step` — unchanged, dispatch-internal, still
    called from `perform_trigger` and now from the `update` spawn loop. They
    do **not** move (a move would force `analog_repeat` to depend on
    `dispatch`, or drag `Config` + `stepper_cursors` into the engine).
  - `config::resolve_axis_value(depth, point)` call — the `depth → value`
    ramp, dispatch-side, per Input, before `axis::Engine::resolve`.
  - Emission: `for w in writes { injector.set_axis_value(w.code, w.value)
    .await }` — errors **swallowed** at every call site (see the flow
    section; this is a deliberate unification, not a port of today's
    inconsistent `?` / `let _ =`).
  - `handle_layer_switch` / `handle_capture_mode_change` — free functions,
    now taking a narrow `&mut axis::Engine` / `&mut analog_repeat::Engine`
    borrow instead of `&mut AxisState` / `&mut HashMap<…>` (ticket 09's
    "narrow borrow is fine" rule).
- **`DispatchState` fields:** `axis_state: AxisState` → `axis: axis::Engine`;
  `analog_repeats: HashMap<Input, ActiveAnalogRepeat>` →
  `analog_repeat: analog_repeat::Engine`.
- **`edit::Effect` surface — unchanged.** `RecomputeAxes { layer }`,
  `ForgetAxisContribution(input)`, `ResetAxisOutputs`, `StopAllAnalogRepeats`
  are still emitted by the pure `edit::plan`; `edit.rs` imports nothing from
  the new modules. Only their `run_effects` handlers rewire (below).

## The `rx_depth` arm / `run_effects` / switch-handler flow

- **`rx_depth` `select!` arm** (unchanged shape — two independent calls
  sharing only the snapshot value):

  ```rust
  let snapshot = rx_depth.borrow_and_update().clone();
  state.handle_depth_update(&config, &snapshot).await;      // axis
  state.update_analog_repeats(&config, &rx_depth, &snapshot).await;
  ```

- **`handle_depth_update`:** for each Axis-assigned Input in the active
  Layer, `resolved.insert(input, config::resolve_axis_value(depth,
  profile.resolved_actuation_point(input)))`; then
  `for w in self.axis.resolve(axis_map, &resolved) { emit(w) }`. Empty axis
  map still short-circuits before any work.

- **`update_analog_repeats`:** compute `repeat_inputs: HashSet<Input>` from
  the active Layer's bindings; `for input in
  self.analog_repeat.update(&repeat_inputs, &snapshot).await { let b =
  bindings[&input]; let steps = compile_action(&b.action, &config.macros,
  &config.steppers, &mut self.stepper_cursors); self.analog_repeat.spawn(
  self.injector.clone(), input, steps,
  analog_repeat::pulse_hold_for(&b.action), rx_depth.clone()); }`.

- **`run_effects`:** `RecomputeAxes { layer }` → `if layer ==
  self.active_layer { for w in self.axis.recompute(axis_map) { emit(w) } }`;
  `ForgetAxisContribution(i)` → `self.axis.forget(i)`; `ResetAxisOutputs` →
  `for w in self.axis.reset() { emit(w) }`; `StopAllAnalogRepeats` →
  `self.analog_repeat.stop_all().await`.

- **`handle_layer_switch`:** `for w in axis.reset() { emit(w) }`;
  `analog_repeat.stop_all().await`. **`handle_capture_mode_change`:** on the
  transition to Digital, `analog_repeat.stop_all().await`. Neither engine is
  told *which* mode or Layer — the caller decides when to call.

- **`handle_event`:** the `AnalogRepeat && event.depth.is_some()` swallow
  guard and the `axis_map.contains_key(&event.input)` swallow / route stay
  exactly where they are — event routing, not engine state.

## Landing in one pass

Convert both clusters, move the constants / types, rewire the `rx_depth`
arm, `run_effects`, `handle_layer_switch`, `handle_capture_mode_change`, and
the two `DispatchState` methods — one PR (ticket 03–09 precedent). A
half-migrated `handle_depth_update` carrying both the inline
`recompute_and_emit_axes` and an `axis::Engine` call is harder to read than
either end state. The test-deletion sweep may stage behind the
implementation within the same PR.

## Behaviour-preservation protocol

Same risk profile as tickets 07/08 — a mechanical carve on the
latency-critical input path — so the same protocol:

- **Diff each ported body line-by-line against `HEAD`.** Load-bearing
  invariants: `resolve_axis_contribution`'s owner tie-break (and the
  "positive half wins a genuinely simultaneous first activation" default);
  `recompute_and_emit_axes`'s stale-code zeroing loop (the code-review
  finding at dispatch.rs:184–201); `reset_axis_outputs`'s write-free
  fast path when `owners` is empty; `run_analog_repeat_loop`'s
  below-deadzone spurious-pulse guard (the intermittent-test fix, now
  `TickPlan::Idle`); `compile`-once-at-spawn (a Stepper cursor advances per
  press-session, not per tick); `reconcile` iterating `snapshot` (not the
  full Input set) exactly as the old loop did; the
  `entry().or_insert_with` "spawn only if absent" semantics.
- **The kept dispatch-harness integration tests must pass against the new
  code before any old test is deleted.**
- **`/code-review` on both the Standards and Spec axes**, as tickets 05, 07,
  and 08 did.

## Tests: replace, don't layer

- **New synchronous `axis::tests`** — no injector, tokio, or tempfile; the
  new primary surface. Gets:
  - A `resolve_axis_contribution` table: unsigned lone contributor; two
    same-half → greater wins; opposite halves + owner → owner holds;
    opposite halves + no owner → positive tie-break; both-zero →
    `(0, None)`.
  - `Engine` behaviours asserting `Vec<AxisWrite>`: stale-code zeroing;
    `step_digital` ramp / saturate / `Up`-reset; `reset()` centers owned
    codes only (and is a no-op otherwise); `forget()` + `recompute()`.
  - **Moved down from the harness:**
    `two_keys_sharing_one_same_signed_target_take_the_greater_depth`,
    `opposite_signed_halves_let_the_already_active_key_keep_driving`, and
    the decision content of `digital_mode_step_fallback_ramps_up_on_repeat_
    and_resets_on_release` and
    `retargeting_an_axis_assignment_zeroes_the_old_abs_code`.
- **New synchronous `analog_repeat::tests`:**
  - `tick_plan` table over `(depth-band × holding_solid)` → `TickPlan`,
    plus `Tap.period` asserted at ticket 73's three verified sample depths.
  - `reconcile` table over `(in repeat_inputs? × depth vs DEADZONE ×
    already active?)` → `Vec<Reconcile>`.
  - `pulse_hold_for`: `ControllerButton` → 35 ms, else 15 ms.
- **Kept in the dispatch harness** — one thin integration per seam that
  reaches uinput or crosses the module boundary:
  `analog_repeat_analog_sourced_events_are_swallowed`,
  `an_analog_sourced_event_on_an_axis_assigned_key_never_passes_through`
  (both `handle_event` routing);
  `analog_repeat_task_fires_periodically_above_the_deadzone_and_stops_below_it`;
  `analog_repeat_controller_button_uses_the_controller_pulse_hold_floor`;
  `analog_repeat_holds_solid_above_the_hold_threshold`;
  `continuous_depth_updates_drive_the_assigned_axis_live`;
  `clear_axis_assignment_zeroes_a_still_live_output_that_dropped_out_of_the_map`
  (the `run_effects` → engine → uinput stuck-output invariant);
  `a_layer_switch_centers_any_live_axis_output` (thin — guards the
  `handle_layer_switch` → `axis.reset()` + `analog_repeat.stop_all()`
  wiring).
- **Deleted from the dispatch harness** — decision-only tests now redundant
  with the tables: `two_keys_sharing_one_same_signed_target_take_the_
  greater_depth`, `opposite_signed_halves_let_the_already_active_key_keep_
  driving`, `retargeting_an_axis_assignment_zeroes_the_old_abs_code`,
  `digital_mode_step_fallback_ramps_up_on_repeat_and_resets_on_release`.
- `edit.rs`'s `Effect::RecomputeAxes` / `ForgetAxisContribution`
  plan-emission tests and `config.rs`'s `resolve_axis_value` ramp tests are
  **untouched** (no `edit` / `config` surface changes).

## Out of scope

- **The rate-curve / conflict-rule refinements themselves** — a curved
  (more-resolution-near-the-top) Analog-repeat mapping, per-Binding
  configurable bounds, axis Sticky/latching mode, user-editable actuation
  curves. This ticket makes each of those a small change in one testable
  module; it does not make any of them.
- **A `depth::` umbrella module** over the two engines. They share only the
  `rx_depth` snapshot value (a plain function argument) — no shared logic,
  and CONTEXT.md has no such concept.
- **Folding `analog_repeat` into `executor`.** Considered (it is a
  firing-task supervisor like `ActiveToggle`); rejected — the rate curve /
  deadzone / hold-solid thresholds are Analog-repeat *Trigger-mode* domain,
  and folding in drags `depth_rx` and `Input`-keying into `executor`.
- Any change to `compile_action` / `resolve_step` or where they live.
- Any change to whether a Chord Toggle survives a Layer or Profile switch
  (still flagged for the domain owner, unchanged since ticket 07).

**Blocked by:** None — tickets 05 (`edit`), 07 (`chord`), 08 (`trigger`),
and 09 (`DispatchState`) are all resolved (`0301693`).

**Status:** resolved

- [x] `daemon/src/axis.rs` exists and exposes `Engine`, `AxisWrite`,
      `resolve` / `recompute` / `step_digital` / `forget` / `reset`; nothing
      else is `pub(crate)`. It imports nothing from `executor`, `injector`,
      `edit`, or `dispatch`, contains no `async fn`, and takes no
      `&Injector`. `resolve_axis_contribution` is a private fn with its own
      unit-test table. `AXIS_DIGITAL_STEP` lives here.
- [x] `daemon/src/analog_repeat.rs` exists and exposes `Engine`, `TickPlan`,
      `Reconcile`, `tick_plan`, `reconcile`, `pulse_hold_for`, and
      `Engine::{update, spawn, stop_all}`; nothing else is `pub(crate)`.
      `tick_plan` / `reconcile` / `pulse_hold_for` are synchronous and do no
      I/O. The six `ANALOG_REPEAT_*` constants, `ActiveAnalogRepeat`,
      `fire_analog_repeat_pulse`, and `run_analog_repeat_loop` are private
      to it (plus private helpers `rate_period` / `release_solid`). It
      imports nothing from `dispatch`, `edit`, `chord`, `trigger`, or
      `config::Config`.
- [x] `dispatch::AxisState`, `resolve_axis_contribution`,
      `recompute_and_emit_axes`, `reset_axis_outputs`,
      `handle_axis_edge_event`, `ActiveAnalogRepeat`,
      `fire_analog_repeat_pulse`, `run_analog_repeat_loop`, and
      `stop_all_analog_repeats` no longer exist in `dispatch.rs`.
- [x] `DispatchState` carries `axis: axis::Engine` and
      `analog_repeat: analog_repeat::Engine` in place of `axis_state` /
      `analog_repeats`.
- [x] The `rx_depth` `select!` arm is two independent delegating calls;
      `handle_depth_update` keeps the `config::resolve_axis_value` ramp and
      the empty-axis-map short-circuit; `update_analog_repeats` keeps
      `compile_action` and computes `repeat_inputs` dispatch-side.
- [x] Every axis `ABS_*` write goes through one dispatch-side emit loop over
      `Vec<AxisWrite>` with errors swallowed (`let _ =`) at every call site
      — `handle_depth_update` no longer `?`-propagates an injector write
      failure (it now returns `()`).
- [x] `run_effects`' four axis / analog-repeat effect arms call engine
      methods; `edit::Effect` and `edit::plan` are untouched;
      `handle_layer_switch` / `handle_capture_mode_change` take a narrow
      `&mut axis::Engine` / `&mut analog_repeat::Engine` borrow and stay
      free functions; the `handle_event` swallow guards are unmoved.
- [x] New synchronous `axis::tests` (15) and `analog_repeat::tests` (11) per
      the "replace, don't layer" split; the four decision-only harness tests
      are deleted; the kept harness integration tests pass against the new
      code before any deletion.
- [x] `CONTRIBUTING.md` gains a "Changing axis conflict resolution" bullet
      and a "Changing the Analog-repeat rate curve" bullet, in the house
      style of the "Changing Chord-detection behaviour" / "Changing how a
      Trigger mode fires" recipes.
- [x] Full Daemon suite green (367); `cargo fmt --check` clean; `cargo clippy
      --all-targets -- -D warnings` clean. GUI and packaging suites
      untouched (no wire / D-Bus / catalog / `config::validate` change).
- [x] `/code-review` (Standards + Spec) run and its findings dispositioned,
      as tickets 05 / 07 / 08 did.

## Comments

**2026-09-01** — Filed from an architecture-review grilling session
(candidate 2 of the review that also produced tickets 07 and 08; the review's
remaining candidate is the `wire.py` ↔ `daemon_stub.py` split-language
contract). Design tree settled over four rounds:

- **Worth doing:** yes — not on a duplication case (unlike 07/08; these
  clusters are single-copy) but on locality + testability + the 05/07/08
  arc. `dispatch.rs` is still 5033 lines and this is the largest remaining
  inline concept-cluster; the §5 conflict rule is pure but has zero direct
  tests; ticket 59 banked the axis seam forward explicitly; ticket 08 named
  the analog-repeat carve as a candidate. The user's framing: not a feature
  freeze — key features are in place but new ones are expected, and the
  archived map's fog (curved rate mapping, Sticky/latching, plausibly
  user-editable actuation curves) is live enough that the seam pays off.
- **Two modules**, matching CONTEXT.md's separate "Axis assignment" /
  "Analog-repeat" terms; one ticket, to amortize the `rx_depth`-arm rework.
- **`axis::Engine` pure, returns `Vec<AxisWrite>`**, dispatch emits — the
  chord/trigger "core decides, dispatch performs" split. The `depth → value`
  ramp (`config::resolve_axis_value`) stays dispatch-side because it needs
  the per-Input Actuation point and is already `config`'s function. Injector
  write errors swallowed uniformly (today's `?` in `handle_depth_update` vs
  `let _ =` in `run_effects` is accidental divergence — user confirmed
  swallow-everywhere).
- **`analog_repeat` splits:** pure `tick_plan` + `reconcile` +
  `pulse_hold_for` (the hardware-tuned numbers, table-tested) and an impure
  `Engine` task supervisor owning the `HashMap<Input, ActiveAnalogRepeat>`.
  Own module, not folded into `executor`. `tick_plan` is shaped to capture
  every band decision in one call *without* forcing the `run_analog_repeat_
  loop` `select!` structure to change (`release_solid_first` on `Idle` /
  `Tap` mirrors today's pre-branch release) — a lighter carve than a full
  loop rewrite, consistent with the behaviour-preservation protocol.
- **`compile_action` / `resolve_step` do not move.** Dispatch compiles and
  hands the engine `Vec<MacroStep>` + `Duration` (`Engine::update` returns
  the Inputs needing a task; dispatch loops, compiles, calls
  `Engine::spawn`). This resolves the handoff's stated worry — a move would
  invert the dependency direction or drag `Config` + `stepper_cursors` into
  the engine.
- **Neither engine sees `CaptureMode` or `Layer`.** Dispatch calls
  `stop_all` / `reset` from the switch handlers; the `handle_event` swallow
  guards stay dispatch-side; no `depth::` glue.
- **`edit::Effect` surface frozen** — only `run_effects` handlers rewire.
- **No CONTEXT.md change, no ADR, no `domain-modeling` run** (07/08
  precedent; the new type names are `ChordEffect`/`TriggerDecision`-style
  implementation vocab). Two `CONTRIBUTING.md` bullets.
- **One pass**, behaviour-preservation protocol copied from ticket 07.

Facts dug from the code during the grilling (not asked of the user):

- `resolve_axis_contribution` is pure but has **zero direct unit tests** —
  every axis-conflict test (`two_keys_sharing…`, `opposite_signed_halves…`,
  `a_layer_switch_centers…`) runs through the full `CommandHarness` +
  `push_depth` rig. (The originating architecture-review card's claim that
  it is "unit-tested in isolation" is wrong.)
- `recompute_and_emit_axes` returns `io::Result`; `handle_depth_update`
  `?`-propagates a failed injector write (tearing down the dispatch task),
  `run_effects` does `let _ =`. Accidental divergence.
- The originating handoff cited tickets 72 and 73 as "known future edits to
  exactly this code" — both are `Status: resolved`, and both concluded
  "keep all constants as shipped, no code change." The live justification is
  the archived `tartarus-input-expansion/map.md` fog notes (lines
  1011–1012) plus the user's user-editable-actuation-curves point, not
  scheduled work.
- `update_analog_repeats` iterates the depth `snapshot`, not the full Input
  set, so an active task for an Input absent from a later snapshot is not
  stopped by the reconcile pass — harmless because `capture::analog`
  publishes every grid key on every report, and the explicit `stop_all` on
  Layer / capture-mode transitions covers the Digital case. `reconcile`
  preserves this by iterating `snapshot`.
- `run_analog_repeat_loop`'s below-deadzone branch has an explicit
  spurious-pulse guard (dispatch.rs:410–421) added for an intermittent
  failure of `analog_repeat_holds_solid_above_the_hold_threshold` — becomes
  `TickPlan::Idle`, must survive the carve.
- `compile_action` has three call sites: two in `perform_trigger`, one in
  `update_analog_repeats`. Keeping it dispatch-side keeps all three callers
  in one place.

**2026-09-01** (resolved) — Landed in one pass. `dispatch.rs` 5033 → 4541
lines; the two engines are `daemon/src/axis.rs` (444 lines) and
`daemon/src/analog_repeat.rs` (501 lines).

- **`axis::Engine`** is pure/synchronous and returns `Vec<AxisWrite>`;
  dispatch keeps the `config::resolve_axis_value` ramp (builds a small
  `resolved: HashMap<Input, u8>` per depth tick, behind the unchanged
  empty-axis-map short-circuit) and performs every write. `resolve` =
  merge-then-`recompute`; `recompute` carries the stale-code zeroing sweep
  verbatim; `reset` keeps the write-free fast path when `owners` is empty;
  `step_digital` is the ex-`handle_axis_edge_event` body.
- **`analog_repeat`** splits into the pure `tick_plan` / `reconcile` /
  `pulse_hold_for` core (with a private `rate_period` feeding `Tap.period`)
  and the impure `Engine` task supervisor. `run_analog_repeat_loop` shrank
  to a shell that `match`es `tick_plan(depth, holding_solid)` — the
  `select!` structure and the below-deadzone `TickPlan::Idle`
  spurious-pulse guard are unchanged. `Engine::update` runs `reconcile`
  against its own live task set, performs every `Stop` inline, and returns
  the Inputs dispatch must `compile_action` + `spawn` (compile stays
  dispatch-side, so the engine never sees `Config` / `stepper_cursors`).
- **Injector write errors on the axis path are now uniformly swallowed**
  (`let _ =`) at all four call sites — `handle_depth_update` /
  `handle_axis_edge_event` used to `?`-propagate and tear the dispatch task
  down. This was the ticket's stated intent (user-confirmed
  swallow-everywhere); `/code-review` flagged that keypress injection on
  the same `handle_event` still propagates, so the two output paths diverge
  in failure behaviour — accepted as designed, noted here for the future
  reader.
- **Tests:** 15 `axis::tests` + 11 `analog_repeat::tests`, all synchronous
  (no injector / tokio / tempfile). Deleted the four decision-only harness
  tests (`two_keys_sharing…`, `opposite_signed_halves…`,
  `retargeting_an_axis_assignment…`, `digital_mode_step_fallback…`) and the
  now-unused `CommandHarness::set_axis_assignment` helper. Kept harness
  integration tests adjusted only where they referenced the now-private
  `ANALOG_REPEAT_*` constants: `analog_repeat_task_fires_periodically…`
  advances ~1s of paused time and asserts a bounded pulse count
  (≈ 18 batches, range 12–28 — wide enough for jitter, tight enough to
  catch a doubled rate / halved dwell / extra pulse-per-tick); the
  ControllerButton-floor test uses two local mirror consts
  (`AR_PULSE_HOLD` / `AR_CONTROLLER_PULSE_HOLD`, pinned by
  `analog_repeat::tests::pulse_hold_for_*`).
- **`/code-review` (Standards + Spec)** — no correctness regression in the
  carve (both modules verified line-for-line against the deleted
  `dispatch.rs` bodies). Three test-coverage / consistency observations;
  dispositioned: (a) added a thin
  `a_digital_sourced_axis_event_routes_through_the_engine_to_uinput`
  harness test to re-cover the `handle_event` → `step_digital` → uinput
  routing seam the deleted test was the sole guard for; (b) tightened the
  loosened `fires_periodically` count assertion to a bounded range;
  (c) the swallow-everywhere failure-behaviour divergence — accepted as the
  ticket's design (noted above).
