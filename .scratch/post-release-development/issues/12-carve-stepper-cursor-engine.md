<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 12 — Carve the Stepper cursor engine out of `dispatch.rs` into `stepper`

**What to build:** The Daemon-side runtime cursor for every Stepper list —
its wrap-around, its clamp-on-shrink, its drop-on-delete, its
default-to-first — moves behind one small interface in a new
`daemon/src/stepper.rs`, type `stepper::Cursors`. The
`StepperItem → Vec<MacroStep>` compilation that `dispatch::resolve_step`
carries today moves to `executor::compile_stepper_item`, beside the
`keypress_steps` / `controller_button_steps` helpers it already calls.
`dispatch::resolve_step` is deleted.

`stepper::Cursors` is pure and synchronous. It owns
`HashMap<StepperId, usize>`, imports `StepperId` / `StepperDef` /
`StepperItem` / `StepDirection` from `config`, and nothing from `executor`,
`edit`, `injector`, or `dispatch`. It never sees a `Config`, a `Layer`, or a
`CaptureMode`.

**No behaviour change.** Wrap-around, clamp, default-to-first,
drop-on-delete, and "silent to the GUI until the next `GetState`" are all
preserved exactly.

## The friction

Ticket 07 carved the Chord state machine into a pure `chord` module; 08 the
Trigger-mode matrix into `trigger`; 10 the axis §5 conflict rule and the
Analog-repeat rate curve into `axis` / `analog_repeat`. Each got a pure,
table-tested core and a one-line `CONTRIBUTING.md` recipe. **The Stepper
cursor never did** — it is the one member of the same family
("hardware/runtime rule fed by `Config` but not part of it") still smeared
across `dispatch.rs`:

- **`dispatch::resolve_step`** (`dispatch.rs:1073`) — the wrap arithmetic
  (`(cur + 1) % len` / `(cur + len - 1) % len`), the default-to-0, the
  zero-items short-circuit, the `stepper_cursors.insert(next)`, **and** the
  two-arm `StepperItem` → steps match.
- **`dispatch::run_effects`** (`dispatch.rs:553–558`) — the clamp
  (`*cursor = (*cursor).min(len - 1)`) and the drop.
- **`dispatch::handle_command`'s `GetState` arm** (`dispatch.rs:617`) — the
  default-to-0 rule again, rebuilding the snapshot inline.
- **`edit::plan`** (`edit.rs:505, 518, 523`) — makes the drop-vs-clamp
  decision itself and emits `Effect::DropStepperCursor` /
  `Effect::ClampStepperCursor` (2 of `Effect`'s 11 variants).
- **`&mut HashMap<StepperId, usize>`** threaded through `TriggerCtx`,
  `trigger_ctx!`, `compile_action`, `perform_trigger`, and
  `update_analog_repeats`.

Every direct test of the cursor rules runs through the dispatch harness
(`CommandHarness` / `Seam`) — `resolve_step_with_modifiers_compiles_…`
(`dispatch.rs:3307`), `resolve_step_compiles_a_controller_button_item_…`
(`3345`), `delete_stepper_command_clears_its_runtime_cursor` (`3496`),
`set_stepper_items_clamps_a_cursor_left_stranded_by_a_shrink` (`3558`),
`set_stepper_items_to_empty_resets_the_cursor_to_the_default` (`3627`).
There is no place to table-test "forward from item 2 of 4 lands on 3" the
way there is for the axis §5 rule.

CONTEXT.md's Stepper entry already anticipates the seam
("The current position is per-list runtime state only, independent of
Profile and Layer, and always resets to the list's first item on a Daemon
restart — never written to `config.toml`").

## The `stepper` module

```rust
// daemon/src/stepper.rs
//
// Pure and synchronous. Imports StepperId / StepperDef / StepperItem /
// StepDirection from config. NOTHING from executor, edit, injector, chord,
// trigger, or dispatch. No Config, no Layer, no CaptureMode, no async.

/// Every Stepper list's Daemon-side runtime cursor (ticket 03/54 —
/// CONTEXT.md: Stepper cursor). Reset fresh per dispatch task start
/// (ex-`DispatchState::stepper_cursors`); a Daemon restart is always
/// "every list at its first item". Dispatch-internal — never part of any
/// module's interface.
#[derive(Default)]
pub(crate) struct Cursors {
    positions: HashMap<StepperId, usize>,
}

impl Cursors {
    /// Advance/retreat `id`'s cursor by one and return the newly-selected
    /// item — "one motion moves the cursor and fires" (ticket 03's Answer).
    /// Wraps at either end. `None` for a zero-item list: nothing to select,
    /// cursor left untouched. Carries today's
    /// `.expect("SetBinding/config::parse validate every Action::Step
    /// references an existing StepperDef")`.
    pub(crate) fn step(
        &mut self,
        steppers: &HashMap<StepperId, StepperDef>,
        id: &StepperId,
        direction: StepDirection,
    ) -> Option<StepperItem>;

    /// Reconcile `id`'s cursor after its list definition changed (ticket
    /// 03/54). `id` absent from `steppers` (a `DeleteStepper` landed) → drop
    /// the entry; list present but empty → drop the entry; list present and
    /// non-empty → clamp an existing cursor to `items.len() - 1`, no-op if
    /// there is no entry or it is already in range. Replaces
    /// `Effect::DropStepperCursor` + `Effect::ClampStepperCursor`.
    pub(crate) fn reconcile(
        &mut self,
        steppers: &HashMap<StepperId, StepperDef>,
        id: &StepperId,
    );

    /// Every library entry's reported cursor, defaulting to `0` ("the
    /// list's first item") for one never yet stepped — richer for the GUI
    /// than only reporting touched entries (ticket 03/54). Replaces the
    /// inline build in `handle_command`'s `GetState` arm.
    pub(crate) fn snapshot(
        &self,
        steppers: &HashMap<StepperId, StepperDef>,
    ) -> HashMap<StepperId, usize>;

    /// `id`'s current position (0 if never stepped) — for test assertions
    /// and nothing else.
    pub(crate) fn position(&self, id: &StepperId) -> usize;
}
```

`executor::compile_stepper_item(item: StepperItem) -> Vec<MacroStep>` — the
two-arm match lifted verbatim from `resolve_step`'s tail:
`StepperItem::Key { key, modifiers } => keypress_steps(modifiers, key)`,
`StepperItem::ControllerButton { button } => controller_button_steps(button)`.
`pub(crate)`, beside its two callees.

## What moves, what stays

- **Into `stepper.rs`:** the cursor `HashMap` (as `Cursors::positions`), the
  wrap arithmetic and default-to-0 (`resolve_step`'s head → `step`), the
  zero-items short-circuit, the drop/clamp rules (`run_effects`'
  `DropStepperCursor` / `ClampStepperCursor` handlers → `reconcile`), the
  `GetState` default-to-0 (→ `snapshot`).
- **Into `executor.rs`:** the `StepperItem` → steps match
  (`resolve_step`'s tail → `compile_stepper_item`).
- **Deleted:** `dispatch::resolve_step`.
- **Stays in `dispatch.rs`:**
  - `compile_action` — now `Action::Step { stepper, direction } =>
    self.stepper.step(steppers, stepper, direction)
    .map(executor::compile_stepper_item).unwrap_or_default()`, `other =>
    executor::compile(other, macros)`. Still dispatch-internal, still called
    from `perform_trigger` and `update_analog_repeats`. Takes
    `&mut stepper::Cursors` where it took `&mut HashMap<StepperId, usize>`.
  - `update_analog_repeats` — compile-once-at-spawn unchanged; the
    `compile_action` call now threads `&mut self.stepper`.
- **`DispatchState` field:** `stepper_cursors: HashMap<StepperId, usize>` →
  `stepper: stepper::Cursors`. `Default`, reset per task start
  (CONTRIBUTING.md's "new piece of dispatch runtime state" rule).
- **`TriggerCtx` field:** `stepper_cursors: &'a mut HashMap<StepperId,
  usize>` → `stepper: &'a mut stepper::Cursors`. `trigger_ctx!` and the
  per-call-site threading **stay** — collapsing them is ticket 13's job
  (the trigger-executor home), banked forward here.
- **`command.rs`:** `State.stepper_cursors: HashMap<StepperId, usize>`
  unchanged — it is the D-Bus wire snapshot, and `snapshot()` produces
  exactly that shape.
- **`edit::Effect`:** `DropStepperCursor(StepperId)` +
  `ClampStepperCursor { stepper, len }` → one
  `ReconcileStepperCursor(StepperId)`. `edit::plan` emits it from
  `DeleteStepper` and both `SetStepperItems` branches; `edit.rs` imports
  nothing from `stepper`. `run_effects` handler:
  `self.stepper.reconcile(&config.steppers, &id)` against the just-committed
  `Config`. Effect enum: 11 → 10 variants.

## Landing in one pass

Create `stepper.rs`, add `executor::compile_stepper_item`, delete
`resolve_step`, rewire `compile_action` / `run_effects` / the `GetState`
arm / the `DispatchState` + `TriggerCtx` fields, collapse the two `Effect`
variants, then the test sweep — one PR (ticket 03–11 precedent). A
half-migrated `compile_action` carrying both `resolve_step` and a
`stepper::Cursors` call is harder to read than either end state.

## Behaviour-preservation protocol

Same risk profile as tickets 07/08/10 — a mechanical carve touching the
latency-critical input path — so the same protocol:

- **Diff each ported body line-by-line against `HEAD`.** Load-bearing
  invariants: `resolve_step`'s `.unwrap_or(0).min(len - 1)` current-position
  read (a stored cursor already past a shrunk list must still step
  correctly, not panic); the `len == 0` → `Vec::new()` short-circuit with
  the cursor left untouched; wrap direction (`Forward` = `+1`, `Backward` =
  `+ len - 1`, both `% len`); `compile`-once semantics on the
  `update_analog_repeats` spawn path (a Stepper cursor advances per
  press-session, not per tick); the `DeleteStepper`-then-`CreateStepper`-on-
  the-same-freed-slug case (`edit.rs:502–505` — the drop is what stops a
  stale position being inherited).
- **The kept dispatch-harness integration tests must pass against the new
  code before any old test is deleted.**
- **`/code-review` on both the Standards and Spec axes**, as tickets 05, 07,
  08 did.

## Tests: replace, don't layer

- **New synchronous `stepper::tests`** — no injector, tokio, or tempfile;
  the new primary surface:
  - `step`: forward advances; backward retreats; wrap at the top
    (last → first); wrap at the bottom (first → last); a missing cursor
    entry reads as 0; a zero-item list returns `None` and creates no entry;
    multi-step position tracking across several `step` calls.
  - `reconcile`: `id` gone → entry dropped; list emptied → entry dropped;
    list shrunk below a stored cursor → clamped to `len - 1`; list grown or
    unchanged → no-op; no cursor entry → no-op.
  - `snapshot`: one entry per `steppers` key; 0 for an untouched list; a
    stepped list reports its real position.
- **Moved to `executor::tests`** as `compile_stepper_item_*` — the two
  `resolve_step_*` tests from `dispatch.rs` (`:3307`, `:3345`); they are
  compile-output assertions now, beside
  `controller_button_steps_helper_matches_the_compile_arm`.
- **Updated in place** — `edit.rs`'s effect assertions (`:1280`, `:1300`,
  `:1313`): `Effect::DropStepperCursor` / `ClampStepperCursor` →
  `Effect::ReconcileStepperCursor(sid)`.
- **Kept in the dispatch harness** — the edit→effect→engine wiring, one
  integration each, unchanged behaviour:
  `delete_stepper_command_clears_its_runtime_cursor` (`:3496`),
  `set_stepper_items_clamps_a_cursor_left_stranded_by_a_shrink` (`:3558`),
  `set_stepper_items_to_empty_resets_the_cursor_to_the_default` (`:3627`).
- **Kept unchanged** — `step_binding_over_real_dbus_advances_the_cursor_and_
  injects_the_new_item` (`dbus/mod.rs:2099`) and the `GetState` cursor-
  reporting coverage.

## Decisions from the grilling

- **New `stepper` module, type `stepper::Cursors`** (not `Engine` — no
  resolution/timing loop runs here; `chord::ChordMachine` already shows the
  sibling naming isn't rigid). Not folded into `executor` (stateless,
  lower-level) or `config` (not serialized).
- **`step` returns `Option<StepperItem>`**, not `Vec<MacroStep>` — the
  item→steps match is pure compilation, which is `executor`'s job; the
  module stays executor-free and the cursor test surface stays direct
  ("landed on item 3", not "emitted these steps").
- **Every method takes `&config.steppers` + specifics** — uniform with
  `axis::Engine`'s `axis_map` parameter; `step` is a verbatim relocation of
  the `resolve_step` signature.
- **One `Effect::ReconcileStepperCursor(StepperId)`**, not two effects and
  not zero — `edit::plan` keeps the locality (it names the list that
  changed) but stops making the drop-vs-clamp decision; that rule moves into
  the module. Imperative name, matching `RecomputeAxes` /
  `ForgetAxisContribution`.
- **Scoped to the carve.** The `trigger_ctx!` macro / `TriggerCtx`
  threading survive with a retyped field; ticket 13 (trigger-executor home)
  removes them.
- **CONTEXT.md gains a "Stepper cursor" entry** after "Stepper" in the
  Configuration section; **CONTRIBUTING.md gains a "Changing Stepper cursor
  behaviour" recipe** after the Analog-repeat one, plus the step-4 Effect
  example changes from "dropping a stepper cursor" to "reconciling a stepper
  cursor after its list changed". `domain-modeling` run for the CONTEXT.md
  edit (07/10 precedent — a carved runtime concept that CONTEXT.md already
  half-names).
- **Drive-by stale-ref fixes** (called out as such in the commit):
  `config.rs:711` (`dispatch::resolve_step` → `executor::compile_stepper_item`
  — caused by this change), `config.rs:265` (`dispatch::handle_command`'s
  `SetChordBinding` handler → `edit::plan`'s `SetChordBinding` arm, ticket 11
  drift), `CONTRIBUTING.md:220` (`handle_axis_edge_event` →
  `axis::Engine::step_digital`, ticket 10 drift).

## Facts dug from the code during the grilling (not asked of the user)

- No consumer of the cursor map outside `dispatch` / `edit` / `command`
  (`executor` / `config` only mention it in comments). No `main.rs` call
  site.
- `StepperItem`, `StepDirection`, `Modifiers` all derive `Copy` — `step`
  returning an owned `StepperItem` needs no lifetime.
- `StepperDef { name, items: Vec<StepperItem> }` carries no id; `step` /
  `reconcile` / `snapshot` need both the `&StepperId` (map key) and the def.
- `analog_repeat.rs` already imports `crate::executor` — a carved module
  depending on `executor` has precedent; `stepper` deliberately does not
  need to (Q4 decision).
- `edit::plan`'s `SetStepperItems` already branches on `new_len == 0` to
  choose `DropStepperCursor` over `ClampStepperCursor` — the drop-vs-clamp
  decision it will stop making.
- `executor::keypress_steps` / `controller_button_steps` are already
  `pub(crate)` "so `dispatch::resolve_step` can reuse" them — the comment
  updates to name `compile_stepper_item`.

**Blocked by:** None — tickets 05 (`edit`), 09 (`DispatchState`) and 10
(`axis` / `analog_repeat`) are all resolved (`9a59e37`).

**Status:** resolved

- [x] `daemon/src/stepper.rs` exists and exposes `Cursors` with `step` /
      `reconcile` / `snapshot` (+ `#[cfg(test)] position`); nothing else is
      `pub(crate)`. It imports only `config` types — nothing from `executor`,
      `edit`, `injector`, `chord`, `trigger`, or `dispatch` — contains no
      `async fn`, and never sees a `Config` / `Layer` / `CaptureMode`.
- [x] `dispatch::resolve_step` is deleted; `compile_action` calls
      `cursors.step(…).map(executor::compile_stepper_item).unwrap_or_default()`.
- [x] `executor::compile_stepper_item(StepperItem) -> Vec<MacroStep>` holds
      the two-arm match, beside `keypress_steps` / `controller_button_steps`.
- [x] `DispatchState.stepper_cursors: HashMap<…>` → `stepper: stepper::Cursors`;
      `TriggerCtx` / `trigger_ctx!` / `compile_action` / `update_analog_repeats`
      thread `&mut stepper::Cursors`. The macro and the threading survive
      (candidate B / a later ticket removes them).
- [x] `edit::Effect::{DropStepperCursor, ClampStepperCursor}` → one
      `ReconcileStepperCursor(StepperId)` (enum 11 → 10). `edit::plan` emits
      it from `DeleteStepper` and `SetStepperItems`; `run_effects` calls
      `self.stepper.reconcile(&config.steppers, &id)` against the committed
      `Config`.
- [x] `command::State.stepper_cursors` (the D-Bus wire snapshot) is
      unchanged; `GetState` builds it via `self.stepper.snapshot(…)`.
- [x] New synchronous `stepper::tests` (12); the two `resolve_step_*` tests
      moved to `executor::tests` as `compile_stepper_item_*`; the `edit.rs`
      effect assertions updated; the dispatch-harness edit→effect→engine
      integration tests kept and green.
- [x] CONTEXT.md "Stepper cursor" entry; CONTRIBUTING.md "Changing Stepper
      cursor behaviour" recipe + step-4 Effect wording.
- [x] Drive-by stale-ref fixes: `config.rs` `StepperItem::ControllerButton`
      doc + `ChordKey` doc, `CONTRIBUTING.md` `handle_axis_edge_event`.
- [x] `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, and the
      full daemon suite (379) all green; GUI suite (397) unaffected.

## Comments

**2026-09-01** — Filed from the `/improve-codebase-architecture` grilling
(candidate A of the review saved at
`.scratch/post-release-development/research/architecture-review-2026-09-01.html`;
the other candidates — the trigger-executor home (B), a single `Edit` commit
path (C), a GUI `read_model` module (D), and `PerLayer<T>` for the Profile
(E) — are unfiled). Design tree settled over three rounds; see the
"Decisions from the grilling" section.

Landed in one PR. `/code-review` (Standards + Spec) ran clean — no
correctness findings; the line-by-line behaviour-preservation check against
the deleted `resolve_step` / effect handlers confirmed wrap arithmetic,
`.unwrap_or(0).min(len-1)`, the `len == 0` short-circuit, drop-vs-clamp,
`GetState` default-to-0, and compile-once-at-spawn are all preserved. Two
stale-doc findings from the review (`config.rs` `StepperItem::ControllerButton`
still naming `dispatch::resolve_step`, and `stepper.rs::step` citing
`resolve_step`'s `.expect`) fixed before commit.

No hardware verification: this is a behaviour-preserving carve of the input
path with the dispatch-harness integration tests as the gate, same as
tickets 07 / 08 / 10.
