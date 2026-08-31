<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 05 — Carve the config-transaction module out of `dispatch.rs` into `edit`

**What to build:** The config-mutation half of the dispatch task moves behind one
small interface in a new `daemon/src/edit.rs`. A pure function decides what a
requested mutation does to the stored `Config` — apply it in memory, validate it,
and describe the post-commit effects to run — and answers that question with no
I/O, no async, and no channels. `dispatch.rs` keeps only the event loop, the
`Command` → `Edit` translation, and the execution of those effects against the
runtime state it owns. The input engine (`handle_event`, the Chord handlers,
`fire`) stops taking `&mut Config` and a `config_path`.

Today `dispatch.rs` (8102 lines) holds two unrelated deep modules behind no
interface: the input engine, and a ~950-line config-transaction cluster
(`handle_command`'s 27-arm match plus `switch_profile`,
`cascade_rename_profile_switch_targets`, `profile_switch_references`,
`macro_references`, `stepper_references`, `take_stepper_direction_elsewhere{,_from_chords}`,
`publish_actuation_snapshot`). Symptoms: `handle_command` takes 15 parameters and
`run` threads 13, each with an `#[allow(clippy::too_many_arguments)]` and an
apologetic comment, and each recent feature (`rx_depth`, `rx_device_info`,
`toggle_lap_target`) added a parameter to all of them. The `CommandHarness` spawns
the entire `run` task — injector, 7 channels, a tempfile, two `RecordingSink`s —
just to ask "does `CreateProfile` reject a duplicate name"; ~110 command tests all
pay that cost, and the pure question (*given this `Config` and this request, what
is the resulting `Config` and error?*) can't be asked directly.

## The module shape

The interface is one pure function and its async wrapper. These type shapes
encode decisions from the grilling and are load-bearing:

```rust
// daemon/src/edit.rs

/// A single requested mutation to the stored Config. One data-only variant
/// per mutating Command (24), same fields minus the `reply` sender.
/// GetConfig / GetState / StopAllToggles have no Edit — they never touch Config.
enum Edit {
    SetBinding { input, layer, binding },
    ClearBinding { input, layer },
    SetModeKeyRole { role },
    CreateProfile { name },
    DeleteProfile { name },
    RenameProfile { old_name, new_name },
    SwitchProfile { name },
    SetActuationPoint { input, actuation, release },
    ClearActuationPoint { input },
    SetDefaultActuation { actuation, release },
    ResetActuationPoints,
    SetForceDigital { force },
    CreateMacro { name, steps },
    RenameMacro { macro_id, new_name },
    DeleteMacro { macro_id },
    SetMacroSteps { macro_id, steps },
    CreateStepper { name, items },
    RenameStepper { stepper_id, new_name },
    DeleteStepper { stepper_id },
    SetStepperItems { stepper_id, items },
    SetChordBinding { inputs, layer, binding },
    ClearChordBinding { inputs, layer },
    SetAxisAssignment { input, layer, target },
    ClearAxisAssignment { input, layer },
}

/// A post-commit effect the caller must run — described here, performed by
/// the dispatch task against the runtime state it owns.
enum Effect {
    RepublishActuation,
    RecomputeAxes { layer: Layer },
    ForgetAxisContribution(Input),
    SignalCaptureMode(bool),
    StopToggle(Input),
    StopAllToggles,
    StopAllAnalogRepeats,
    ResetAxisOutputs,
    DropStepperCursor(StepperId),
    ClampStepperCursor { stepper: StepperId, len: usize },
    AnnounceProfileChange(String),
}

enum CreatedId { Macro(MacroId), Stepper(StepperId) }

struct Outcome {
    effects: Vec<Effect>,
    /// Set only by CreateMacro / CreateStepper — the freshly minted id the
    /// D-Bus reply carries back.
    created: Option<CreatedId>,
}

/// The deep module. Clones `config`, applies `edit` in memory, runs
/// `config::validate` against the result, returns the new Config by value
/// plus the effects to run. No I/O, no async. Rollback is dropping the clone.
/// Operation preconditions (NotFound, AlreadyExists, "can't delete the active
/// Profile", "still-referenced Macro/Stepper", blank create/rename name) are
/// explicit early-return `Err` in each arm; structural invariants stay in
/// `config::validate`.
fn plan(config: &Config, edit: Edit) -> Result<(Config, Outcome), CommandError>;

/// The thin async wrapper: `plan`, then `config::persist`, then assign on
/// success. Supersedes `config::persist_edit`.
async fn apply(config: &mut Config, path: &Path, edit: Edit) -> Result<Outcome, CommandError>;
```

## What moves, what stays

- **Into `edit.rs`:** `plan`, `apply`, the types above, and the helper cluster —
  `cascade_rename_profile_switch_targets`, `profile_switch_references`,
  `macro_references`, `stepper_references`,
  `take_stepper_direction_elsewhere{,_from_chords}`, the `switch_profile` edit
  logic, and `publish_actuation_snapshot`'s resolution logic (its `watch::Sender`
  send becomes `Effect::RepublishActuation`).
- **Retired:** `config::persist_edit`. `plan` absorbs its `validate` step;
  `config::persist` (bumped to `pub(crate)`) does the write; `apply` does the
  snapshot-free assign-on-success. `RenameProfile`'s `old_name == new_name`
  short-circuit stays a guard in the translation arm, ahead of `apply`, exactly
  as ticket 03 kept it ahead of `persist_edit`.
- **Stays in `dispatch.rs`:** the `run` event loop; a slimmed `handle_command`
  (still that name) — the 3 non-edit arms inline, the 24 edit arms each a
  mechanical `edit::apply` → `reply.send` → `run_effects`; a private
  `run_effects` + `EffectCtx` (a struct borrowing `injector`, `toggles`,
  `stepper_cursors`, `axis_state`, `analog_repeats`, `actuation_tx`,
  `capture_control_tx`, `signal_emitter`, `active_layer`), built fresh per call
  site. `EffectCtx` and `run_effects` are dispatch-internal, never part of
  `edit`'s interface. `run_effects` no-ops `RecomputeAxes { layer }` when
  `layer != active_layer` (the check `plan` can't make).

## The `Command` → `Edit` → effect flow

- `handle_command` translates each mutating `Command` into its `Edit` (field for
  field, drop `reply`), calls `edit::apply`, **sends `reply` before running
  effects** — uniformly, for every arm. This deletes `SwitchProfile`'s bespoke
  reply-before-signal reasoning: the hazard it dodged is now the default shape.
  `CreateMacro` / `CreateStepper` map `Outcome.created` into their reply.
  Precondition error *messages* are preserved verbatim in the `plan` arms.
- `switch_profile` folds into `plan` as `Edit::SwitchProfile`, emitting
  `[StopAllToggles, RepublishActuation, ResetAxisOutputs, StopAllAnalogRepeats,
  AnnounceProfileChange]`.
- Firing an `Action::ProfileSwitch` binding from the input path: `handle_event`
  and the Chord handlers return `io::Result<Vec<Edit>>` (empty or one in
  practice — only Fire-once on `Down`). The `run` loop is the sole commit point:
  for each returned `Edit` it calls `edit::apply` + `run_effects` in order,
  preserving today's loop-order semantics for a retroactive multi-switch, and
  log-and-ignores an `Err` (a dangling `ProfileSwitch` target is already
  impossible post-`validate`). This is what lets `handle_event` /
  `handle_chord_event` / `handle_chord_timeout` / `fire` drop `&mut Config` and
  `config_path` to `&Config`.

## Relationship to tickets 03 and 04

This is the third step on the same path. Ticket 03 made *edit + persist* atomic
(`persist_edit`); ticket 04 single-sourced *validation* on that path
(`config::validate`); this ticket lifts the whole transaction — edit, validate,
persist, and the post-commit effect derivation — into one deep, pure, testable
module and shrinks `dispatch` to the event loop plus effect execution.
`config::validate` is unchanged and stays the single invariant point. Append a
Comment to ticket 03's file noting `persist_edit` was superseded by `edit::apply`
here.

## Landing in one pass

Convert all 24 arms and rewire the input path in one pass, not incrementally — a
half-migrated `handle_command` carrying both `persist_edit` and `edit::apply` is
harder to read than either alone (ticket 03 / 04 precedent). The **test-deletion
sweep** below may be staged behind the implementation within the same PR, since
converting ~110 harness call sites is mechanical and independently verifiable.

## Tests: replace, don't layer

- New synchronous, table-driven `edit` test module — one-plus row per `Edit`
  variant asserting the resulting `Config` and the `CommandError` for each
  precondition and invariant path. No tokio, no tempfile. This is the new test
  surface.
- `edit::apply`: two async tests — a persist failure rolls back (`Config`
  untouched, error is `IoError`); success persists and returns the `Outcome`.
  These absorb `persist_edit`'s three existing tests.
- Dispatch harness — **keep** (~15–25 of today's ~110): one proving execution of
  each `Effect` variant (actuation republish, axis recompute +
  `ForgetAxisContribution`, `SignalCaptureMode`, `StopToggle` / `StopAllToggles`,
  stepper-cursor drop / clamp, `AnnounceProfileChange`), the input-path
  `ProfileSwitch`-firing path, and one full-harness persist-failure rollback.
- Dispatch harness — **delete**: every test that only asserts "`Command` +
  `Config` → resulting `Config` / `CommandError`" — that coverage moves to
  `edit`.

## Out of scope

Carving `parse` / `serialize` / `load_or_seed` / `persist` / the legacy scan into
a `config::toml` submodule — a separate architecture-review candidate, already
deferred by ticket 04.

**Blocked by:** None — ticket 04 (`config::validate`) is resolved (`6e1d531`).

**Status:** resolved

- [x] `daemon/src/edit.rs` exists and exposes `Edit`, `Effect`, `CreatedId`,
      `Outcome`, `plan`, and `apply`; nothing else from the module is `pub(crate)`.
- [x] `plan` is synchronous, takes `&Config`, does no I/O, returns the resulting
      `Config` by value plus an `Outcome`; a rejected edit returns `Err` and the
      caller's `Config` is never touched.
- [x] `plan` runs `config::validate` on the edited `Config`; operation
      preconditions (`NotFound`, `AlreadyExists`, active-Profile delete,
      referenced-entry delete, blank create/rename name) are explicit `Err`
      returns in the `plan` arms, with their existing messages preserved.
- [x] `config::persist_edit` no longer exists; `config::persist` is `pub(crate)`;
      `edit::apply` is the only thing that persists a live edit.
- [x] `handle_command` is reduced to translation: 3 non-edit arms inline, 24 edit
      arms each `edit::apply` → `reply.send` → `run_effects`, with `reply` sent
      before effects run for every arm.
- [x] `switch_profile` no longer exists as a standalone function; `SwitchProfile`
      is an `Edit` variant handled by `plan`, and `ActiveProfileChanged` still
      fires after the reply on the `Command` path.
- [x] `handle_event`, `handle_chord_event`, `handle_chord_timeout`, and `fire` no
      longer take `&mut Config` or a `config_path`; they take `&Config` and
      return the `Edit`s (if any) for the `run` loop to commit. (`fire` never
      took either — it takes `&config.macros`/`&config.steppers` and is
      unchanged; the other three switched to `&Config` and return `Vec<Edit>`.)
- [x] Firing an `Action::ProfileSwitch` binding from the input path still
      switches the Profile, persists, force-stops Toggles, republishes actuation,
      resets axes, and stops Analog-repeats, and still emits
      `ActiveProfileChanged`.
- [x] `run_effects` + `EffectCtx` are private to `dispatch.rs`; `RecomputeAxes`
      is a no-op when its layer is not the active Layer.
- [x] `edit` has a per-`Edit`-variant synchronous test module; `edit::apply` has
      its two async tests; the dispatch harness keeps only the
      effect-execution / input-path-`ProfileSwitch` / rollback tests and the
      `Command`-plus-`Config` outcome tests are gone.
- [x] `#[allow(clippy::too_many_arguments)]` is removed from `handle_command`
      (now 7 args). The input-engine functions that still exceed the threshold
      keep it (`handle_event` 12, `handle_chord_event` 9,
      `fire_individual_retroactively` / `handle_chord_timeout` 8 each).
- [x] `CONTRIBUTING.md`'s "structural invariant" bullet is reworded into the full
      recipe for a new mutating `Command` (wire variant → `Edit` variant →
      `plan` arm → mechanical translation line → `Effect` variant only if there
      is a post-commit side effect).
- [x] Ticket 03's file has a Comment noting `persist_edit` was superseded by
      `edit::apply`.
- [x] Full Daemon suite green (339); `cargo fmt --check` clean; `cargo clippy
      --all-targets -- -D warnings` clean; GUI (356) and packaging suites pass.

## Comments

**2026-08-31** — Filed from an architecture-review grilling session (candidate 1
of the review that produced tickets 03 and 04; candidate 3, the GUI domain
mirror, is a separate handoff). Design tree settled over four rounds:

- **Core seam** is a pure `plan(&Config, Edit) -> Result<(Config, Outcome),
  CommandError>`, not the async `apply(&mut Config, Edit) -> Committed` the
  review first floated — the pure core is what collapses ~110 tokio harness
  tests into synchronous table tests. `apply` is a thin async wrapper.
- **Module** is a new `daemon/src/edit.rs`, not a `config/` directory split, not
  growth of `command.rs`. `config.rs` stays the storage/validate/persist module.
- **`Edit`** is a dedicated data-only enum (no `oneshot` senders, not a closure,
  not reused `Command`). Read-only and Config-free arms (`GetConfig`,
  `GetState`, `StopAllToggles`) stay wholly in `dispatch`.
- **Effects** are a `Vec<Effect>` (~11 variants), not a struct of `Option`s —
  ordering matters for `SwitchProfile`. Execution stays in `dispatch`
  (`run_effects` + a private `EffectCtx`), not in `edit`.
- **Preconditions move into `plan`** (it returns `CommandError`, which already
  has `NotFound` / `AlreadyExists`); the translation layer becomes purely
  mechanical. The invariant-vs-precondition *distinction* is unchanged — both
  now live behind the one testable interface.
- **Reply ordering** becomes uniform: reply before effects, for every arm. This
  deletes `SwitchProfile`'s special-case reentrancy-hazard reasoning.
- **`switch_profile` folds into `plan`**; the input path returns `Vec<Edit>` up
  to the `run` loop, which is the sole commit point — letting the input-engine
  functions drop `&mut Config` / `config_path`.
- **`persist_edit` is retired** two tickets after it landed — its job is now
  split across `plan` (validate) and `apply` (persist + assign).
- **One pass** for the implementation (ticket 03 / 04 precedent); the
  test-deletion sweep is stageable within the same PR.
- No ADR (the candidate was accepted, not rejected for a load-bearing reason);
  no `CONTEXT.md` term — config-machinery names weren't treated as domain
  vocabulary for `persist_edit` or `validate` either.
- The `config::toml` submodule split stays a separate candidate.

## Answer

Implemented in one pass. `daemon/src/edit.rs` is the new deep module:
`Edit` (24 data-only variants), `Effect` (11 variants), `CreatedId`, `Outcome`,
the pure `plan(&Config, Edit) -> Result<(Config, Outcome), CommandError>`, and
the async `apply` (`plan` → `config::persist` → assign-on-success). The helper
cluster (`cascade_rename_profile_switch_targets`, `*_references`,
`take_stepper_direction_elsewhere{,_from_chords}`, `active_profile_mut`,
`switch_profile`'s edit logic) moved in as private fns.

`config::persist_edit` is gone; `config::persist` is `pub(crate)`, called only
by `edit::apply`. `dispatch::handle_command` (7 params, no `#[allow]`) is now
translation only: 3 non-edit arms inline, 24 arms each `edit::apply` →
`reply.send` → `run_effects`, uniformly reply-before-effects — which deleted
`SwitchProfile`'s bespoke reentrancy reasoning. `dispatch` gained a private
`EffectCtx` + `run_effects` (`RecomputeAxes` no-ops off the active Layer) and a
`commit_input_edits` helper. `handle_event` / `handle_chord_event` /
`handle_chord_timeout` / `fire_individual_retroactively` dropped `&mut Config` +
`config_path`, take `&Config`, and return `io::Result<Vec<edit::Edit>>`; the
`run` loop is the sole commit point and log-and-ignores a failed input-path
`apply`.

Tests: new synchronous `edit::tests` module — ~25 cases, one-plus per variant
(resulting `Config`, effect vec, `created` id) plus a table of every
precondition / invariant rejection asserting the caller's `Config` is
untouched — and `edit::apply`'s two async tests (persist success / rollback).
55 `Command`-plus-`Config` outcome tests deleted from the dispatch harness
along with 9 now-unused harness methods; the effect-execution / input-path
`ProfileSwitch` / rollback tests stayed. Daemon 339 green, clippy
`--all-targets -D warnings` clean, fmt clean, GUI 356 + packaging green.

`/code-review high` (fork) raised three points, all addressed:

1. `RenameProfile { old_name == new_name }` initially short-circuited to
   `Ok(())` ahead of the existence check, changing the missing-Profile case
   from `NotFound` to `Ok`. Fixed: the translation arm's same-name guard now
   does the `NotFound` check itself (`Ok` iff the Profile exists), matching
   ticket 03's arm exactly, still with no `config.toml` write.
2. A single retroactive chord miss that fires a `ProfileSwitch` member
   *alongside* non-switch members: every member's binding now resolves against
   the pre-switch Profile and the switch's effects run afterward, rather than
   interleaved as the old inline `switch_profile` call did. This is an
   inherent consequence of the input path no longer holding `&mut Config`
   (an acceptance criterion) — an extreme edge (mixed-Action members of one
   chord all timing out at once) — documented in `commit_input_edits`.
   Multiple `SwitchProfile` edits still commit last-write-wins in order.
3. Coverage of a few `config::validate` *accept* paths that the deleted
   dispatch tests used to touch (SetBinding + AnalogRepeat on a grid Input,
   SetBinding + an allowlisted ControllerButton, Held bindings surviving a
   role flip) added back as `edit::tests` assertions.

Final: Daemon 340 green.
