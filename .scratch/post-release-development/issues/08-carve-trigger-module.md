<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 08 — Unify `fire` and `execute_chord_fire` into a pure `trigger` module

**What to build:** The Trigger-mode dispatch matrix — the
`match (binding.trigger, event.state)` that decides whether a press spawns a
Fire-once firing, holds a bare `KeyDown`, starts a Toggle loop, starts a held
Toggle, or force-releases a stuck firing — moves behind one small interface in a
new `daemon/src/trigger.rs`. A pure, synchronous function decides what a
`(Binding, EventState, slot-liveness)` triple does, with no I/O, no async, no
`Injector`, and no channels. `dispatch.rs` keeps a single generic executor that
performs the decision against the runtime state it owns, keyed by `Input` for
the individual path and by `ChordKey` for the Chord path. `fire` and
`execute_chord_fire` both cease to exist as standalone functions.

Today `dispatch.rs` (5762 lines) holds this one algorithm as **two near-verbatim
implementations**: `fire` (lines 1467–1626, ~159 lines, 10 params,
`#[allow(clippy::too_many_arguments)]`) keyed by `Input` into `toggles` /
`in_flight`, and `execute_chord_fire` (lines 1174–1296, ~122 lines, 9 params,
`#[allow]`) keyed by `ChordKey` into `chord_runtime.firings` /
`chord_runtime.toggles`. Their `match` bodies are arm-for-arm identical:

- `(HoldToRepeat, Repeat)` + `Action::ControllerButton` → ignored (ticket 75/76)
- `(HoldToRepeat, Down)` + `Action::ControllerButton` → bare `KeyDown` hold
- `(HoldToRepeat, Repeat)` + mouse-button `Keypress` → ignored (ticket 79/80)
- `(HoldToRepeat, Down)` + mouse-button `Keypress` → bare `KeyDown` hold
- `(FireOnce, Down) | (HoldToRepeat, Down|Repeat)` → overlap guard, `compile_action`, `spawn_fire_once`
- `(Toggle, Down)` + mouse-button `Keypress` → `spawn_held` (ticket 82/83)
- `(Toggle, Down)` + `Action::ControllerButton` → `spawn_held` (ticket 78)
- `(Toggle, Down)` → `compile_action`, `ActiveToggle::spawn`

`execute_chord_fire`'s own doc comment (dispatch.rs:1162) says it plainly:
"`fire`'s exact mirror for a Chord's own Trigger-mode dispatch … Formerly
`fire_chord`." The only real differences are the slot key type and that `fire`
additionally carries `(FireOnce | HoldToRepeat | AnalogRepeat, Up) →
force_release_stuck` (the Chord path force-releases through the separate
`ChordEffect::ReleaseChordFiring` / `StopChordToggle` effects instead), and that
`fire` threads `AnalogRepeat` alongside `HoldToRepeat` (a Chord can never be
`AnalogRepeat` — `config::validate` rejects it). Every one of tickets 75, 76,
78, 79, 80, and 82 had to tune an arm **in both places**.

This is the fifth step on the path tickets 03–07 walked, and the one ticket 07
explicitly deferred: its "Out of scope" section reads _"A real unification —
one `trigger` module both paths call — is its own candidate with its own blast
radius. File it once this ticket lands and the residual duplication is
visible."_ Ticket 07 has landed (`9b88763`); the duplication is visible.

## The module shape

The interface is one pure function, one decision enum, and one liveness enum.
These type shapes encode decisions from the grilling and are load-bearing:

```rust
// daemon/src/trigger.rs
//
// Pure and synchronous. Imports Binding / Action / TriggerMode / KeyCode /
// EventState and NOTHING from executor, injector, or edit. No &Injector,
// no async, no channels, no task spawning.

/// Liveness of one firing/toggle slot, passed IN to `decide` rather than
/// held — the function stays pure, tests construct it directly. Absent
/// (`None`) means no live firing or toggle for that key. Replaces
/// `chord::ChordSlot` verbatim (same three states); `chord::ChordSlot` is
/// deleted and `chord::feed` / `dispatch::chord_slots` take `trigger::Slot`.
pub(crate) enum Slot {
    /// An active Toggle-mode firing.
    Toggle,
    /// A Fire-once / Hold-to-repeat firing still in flight.
    FiringUnfinished,
    /// A Fire-once firing that has already completed on its own — the map
    /// entry lingers (never cleaned, mirroring `fire`'s own `in_flight`),
    /// so this must be distinct from `None` or the slot could never fire
    /// again.
    FiringFinished,
}

/// What a `(Binding, EventState, Slot)` triple resolves to. Data-only; the
/// dispatch-side executor performs it. `SpawnFireOnce` / `StartToggleLoop`
/// stay abstract (no compiled steps) so `compile_action` runs in the
/// executor AFTER `decide` has cleared the overlap guard — a dropped firing
/// must never advance a Stepper cursor (dispatch.rs:1557–1567).
pub(crate) enum TriggerDecision {
    /// Overlap guard hit (`Slot::FiringUnfinished`), or an inert
    /// (state, mode) pair (Fire-once + Repeat/Up, Toggle + Repeat/Up, a
    /// ControllerButton/mouse-button Repeat).
    Nothing,
    /// Compile `binding.action` and spawn a one-shot firing.
    SpawnFireOnce,
    /// Hold a bare, unbalanced `KeyDown` — mouse-button / ControllerButton
    /// Hold-to-repeat `Down` (tickets 75/76, 79/80). Released later by
    /// `ForceReleaseStuck` on the individual path, or
    /// `ChordEffect::ReleaseChordFiring` on the Chord path.
    HoldKeyDown(KeyCode),
    /// Compile `binding.action` and start a looping Toggle.
    StartToggleLoop,
    /// Start a single-held Toggle — mouse-button / ControllerButton Toggle
    /// (tickets 78, 82/83).
    StartToggleHeld(KeyCode),
    /// Force-release whatever this key's firing left stuck — Fire-once /
    /// Hold-to-repeat / Analog-repeat `Up` on the individual path.
    ForceReleaseStuck,
}

/// The deep module. `binding` carries `.trigger` and `.action`; `slot` is the
/// liveness of this key's existing firing/toggle (`None` == absent). No I/O,
/// no async. `ProfileSwitch` never reaches here (handled upstream in
/// `dispatch_individual_down`; a Chord's Action can't be `ProfileSwitch`).
pub(crate) fn decide(
    binding: &Binding,
    state: EventState,
    slot: Option<Slot>,
) -> TriggerDecision;

/// Shared executor helpers, so the Chord release effects and the individual
/// toggle-stop stop being three hand-rolled copies of two lines each.
pub(crate) async fn force_release_stuck<K: Eq + Hash>(
    firings: &HashMap<K, FiringHandle>, key: &K, injector: &Injector,
);
pub(crate) async fn stop_toggle<K: Eq + Hash>(
    toggles: &mut HashMap<K, ActiveToggle>, key: &K,
);
```

`force_release_stuck` / `stop_toggle` are the one place `trigger` touches
`executor` / `injector` types — they are executor helpers, deliberately not part
of the pure core, kept in `trigger.rs` only because both the `Input` path and
the `ChordKey` path call them and a shared home beats a third copy. If that
feels wrong at implementation time, put them in `dispatch.rs` next to
`perform_trigger` instead — the pure `decide` + `TriggerDecision` + `Slot` is
the load-bearing part.

## The dispatch-side executor

```rust
// dispatch.rs — internal, NOT part of trigger's interface, mirrors EffectCtx.

/// The runtime state `perform_trigger` performs a `TriggerDecision` against,
/// generic over the slot key (`Input` for the individual path, `ChordKey`
/// for the Chord path). Built fresh per call site from the `run` task's
/// locals, same discipline as `EffectCtx` (ticket 05).
struct TriggerCtx<'a, K> {
    injector: &'a Injector,
    firings: &'a mut HashMap<K, FiringHandle>,
    toggles: &'a mut HashMap<K, ActiveToggle>,
    macros: &'a HashMap<MacroId, MacroDef>,
    steppers: &'a HashMap<StepperId, StepperDef>,
    stepper_cursors: &'a mut HashMap<StepperId, usize>,
    toggle_lap_target: Duration,
}

/// Performs `decision` — `compile_action` (behind the guard `decide` already
/// cleared) + `executor::spawn_fire_once` / `ActiveToggle::spawn{,_held}` +
/// map insert, or `force_release_stuck`. Never produces an `edit::Edit`
/// (`ProfileSwitch` is handled before this is ever reached).
async fn perform_trigger<K: Eq + Hash + Clone>(
    decision: trigger::TriggerDecision,
    key: K,
    binding: &Binding,
    ctx: &mut TriggerCtx<'_, K>,
) -> io::Result<()>;
```

## What moves, what stays

- **Into `trigger.rs`:** the `(TriggerMode, EventState, Action-shape)` match
  logic from both `fire` and `execute_chord_fire`, rewritten to return a
  `TriggerDecision` instead of performing it; `Slot` (ex-`chord::ChordSlot`);
  the `force_release_stuck` / `stop_toggle` generic helpers.
- **Deleted:** `fire` and `execute_chord_fire` as standalone functions.
  `chord::ChordSlot` (folded into `trigger::Slot`).
- **Stays in `dispatch.rs`:**
  - `TriggerCtx` + `perform_trigger` (dispatch-internal, per the `EffectCtx`
    precedent — never part of `trigger`'s interface).
  - `compile_action` / `resolve_step` unchanged (already shared; still called
    from inside `perform_trigger`).
  - `dispatch_individual_down` — unchanged except its inner `fire(...)` call
    becomes `trigger::decide` + `perform_trigger`. It keeps its
    `ProfileSwitch → Edit` and unbound-passthrough branches.
  - `run_chord_effects` — its `FireChord` arm becomes `trigger::decide` +
    `perform_trigger` (keyed by `ChordKey`); its `ReleaseChordFiring` /
    `StopChordToggle` arms call `trigger::force_release_stuck` /
    `trigger::stop_toggle`.
  - `handle_event` — its inline toggle-stop-on-`Down` (dispatch.rs:787–792)
    calls `trigger::stop_toggle(&mut toggles, &event.input)`; its
    `Repeat | Up` arm's `fire(...)` call becomes `trigger::decide` +
    `perform_trigger`.
- **`chord.rs` decision logic is frozen.** `chord::feed` / `chord::tick` still
  emit `FireChord { state: Down | Repeat }` and the separate
  `ReleaseChordFiring` / `StopChordToggle` release effects — no
  `FireChord { state: Up }`, no routing of Chord release through `decide`. Only
  the *executor* side unifies. `chord::feed`'s signature changes only in that
  its `live: &HashMap<ChordKey, ChordSlot>` parameter becomes
  `&HashMap<ChordKey, trigger::Slot>`.

## The `handle_event` / `run_chord_effects` → `trigger` → executor flow

- **Individual `Down`** (`dispatch_individual_down`): after the
  `ProfileSwitch → Edit` short-circuit, `let d = trigger::decide(&binding,
  EventState::Down, slot_for(&in_flight, &toggles, input)); perform_trigger(d,
  input, &binding, &mut ctx).await?`.
- **Individual `Repeat | Up`** (`handle_event` tail): same two lines with
  `event.state`.
- **Chord `FireChord { key, binding, state }`** (`run_chord_effects`): `let d =
  trigger::decide(&binding, state, slot_for(&chord_runtime, &key));
  perform_trigger(d, key, &binding, &mut chord_ctx).await?`.
- The `slot_for` helper reads the existing three-way liveness from a
  `(firings, toggles)` pair — the individual path's version replaces `fire`'s
  inline `in_flight.get(&input).is_finished()` checks; the Chord path's version
  is today's `dispatch::chord_slots` logic for a single key.

## Relationship to tickets 03–07

Ticket 03 made *edit + persist* atomic; 04 single-sourced *validation*; 05
lifted the whole config transaction into the pure `edit` module; 06 built the
contract-tested `rules` mirror in the GUI; 07 carved the Chord state machine
into the pure `chord` module. This ticket does the same for the last deep
algorithm still duplicated inside `dispatch.rs` — the Trigger-mode firing
matrix — and is the follow-up ticket 07's "Out of scope" section named. Append
a Comment to ticket 07's file noting the deferred `trigger` unification landed
here.

## Landing in one pass

Convert both functions, fold `chord::ChordSlot` into `trigger::Slot`, and rewire
all three call sites in one pass — a half-migrated state with `fire` and
`trigger::decide` both live is harder to read than either alone (ticket
03/04/05/06/07 precedent). The **test-deletion sweep** below may stage behind
the implementation within the same PR.

## Behaviour-preservation protocol

This is a mechanical carve on the latency-critical input path — same risk
profile as ticket 07, same protocol:

- Diff each ported `match` arm line-by-line against `HEAD`. The load-bearing
  invariants: the `in_flight` / `chord_runtime.firings` overlap guard
  (`!handle.is_finished()` → drop this firing); `compile_action` runs only
  *after* the guard passes, so a dropped Fire-once/Hold-to-repeat Step firing
  never advances the cursor; `force_release_stuck` vs `toggle.stop()` semantics
  and call ordering; the `matches!(action, Action::ControllerButton { .. })` /
  `Action::Keypress { key, .. } if is_mouse_button(key)` guards select the same
  arms.
- The existing `run_scripted` end-to-end tests (individual + Chord uinput byte
  sequences) must pass against the new code **before** any old test is deleted.
- `/code-review` on both the Standards and Spec axes, as tickets 05 and 07 did.

## Tests: replace, don't layer

- **New synchronous `trigger::tests` module** — an exhaustive table over
  `(TriggerMode × EventState × Action-shape × Option<Slot>) → TriggerDecision`.
  No tokio, no injector, no tempfile. This is the new test surface. It covers:
  Fire-once fires only on `Down`; Hold-to-repeat on `Down` + every `Repeat`,
  not `Up`; the `ControllerButton` Hold-to-repeat carve (`Down` →
  `HoldKeyDown`, `Repeat` → `Nothing`); the mouse-button Hold-to-repeat carve;
  `Toggle` → `StartToggleLoop`; `Toggle` + mouse-button / `ControllerButton` →
  `StartToggleHeld`; `AnalogRepeat` rides the Hold-to-repeat arms; `Up` +
  Fire-once / Hold-to-repeat / Analog-repeat → `ForceReleaseStuck`; the overlap
  guard (`Slot::FiringUnfinished` → `Nothing`, `FiringFinished` / `None` →
  proceed).
- **Delete from the dispatch harness** — the four Chord-path executor tests
  whose decision content now lives in the table and whose executor branch is no
  longer distinct from the individual one:
  - `hold_to_repeat_chord_controller_button_ignores_repeat_and_releases_on_member_up`
  - `hold_to_repeat_chord_mouse_button_ignores_repeat_and_releases_on_member_up`
    (ticket 07's Standards review restored this one _"because
    `execute_chord_fire`'s mouse-button arm is a distinct executor branch"_ —
    exactly the branch this ticket removes)
  - `toggle_chord_mouse_button_holds_a_single_keydown_and_full_completion_stops_it`
  - `toggle_chord_controller_button_holds_a_single_keydown_and_full_completion_stops_it`
  - plus any purely-decision individual test (`fire_once_binding_ignores_repeat_
    and_up_fires_only_on_down`, `hold_to_repeat_fires_on_down_and_every_repeat_
    but_not_up`, `analog_repeat_digital_sourced_behaves_like_hold_to_repeat`)
    whose byte-level assertion is redundant with a kept test.
- **Keep in the dispatch harness** — one byte-level pass *per spawn kind, not
  per slot-key type*, through `perform_trigger`: the unbalanced-macro
  force-release-on-`Up`, one mouse-button hold, one `ControllerButton` hold,
  `toggle_keyboard_key_still_loops_at_dispatch_level`,
  `fire_once_macro_action_runs_its_delayed_steps_in_order`,
  `toggle_starts_on_down_and_the_same_key_stops_it_on_the_next_down`. Keep every
  `run_scripted` end-to-end test, the Step-binding cursor tests (they exercise
  compile-after-guard), and
  `a_chord_member_whose_individual_binding_is_a_profile_switch_switches_on_early_release`.

## Out of scope

- **The Analog-repeat engine** (`dispatch.rs:223–494` — `run_analog_repeat_loop`,
  `update_analog_repeats`, `fire_analog_repeat_pulse`). Its own
  architecture-review candidate (carve the pure rate-curve decision core, the
  `chord.rs` treatment for the other hardware-tuned timing engine). `trigger`
  only touches the `(TriggerMode, EventState)` match, and treats `AnalogRepeat`
  as an alias of `HoldToRepeat` exactly as `fire` does today.
- **A `DispatchState` struct** for the `run` task's ~18 locals (the candidate
  that would delete `TriggerCtx`, `EffectCtx`, and every
  `#[allow(clippy::too_many_arguments)]` at once). Separate candidate;
  `TriggerCtx` follows the `EffectCtx` precedent in the meantime.
- Any change to whether a Chord Toggle survives a Layer or Profile switch
  (still flagged for the domain owner, unchanged since ticket 07).
- The `config::store` submodule split (separate candidate, already declined by
  this review).

**Blocked by:** None — ticket 07 (`chord.rs`, the `feed` / `ChordEffect` /
`ChordSlot` precedent this builds on) is resolved (`9b88763`).

**Status:** resolved

- [x] `daemon/src/trigger.rs` exists and exposes `Slot`, `TriggerDecision`,
      `decide`, `force_release_stuck`, and `stop_toggle`; nothing else is
      `pub(crate)`. `decide` is synchronous, takes `&Binding` + `EventState` +
      `Option<Slot>`, does no I/O, spawns no task, takes no `&Injector`.
- [x] `fire` and `execute_chord_fire` no longer exist as functions in
      `dispatch.rs`.
- [x] `chord::ChordSlot` is deleted; `chord::feed` and `dispatch::chord_slots`
      use `trigger::Slot`. `chord.rs` emits the same `ChordEffect`s as before
      (no `FireChord { state: Up }`).
- [x] `dispatch::perform_trigger` + `TriggerCtx<'_, K>` are private to
      `dispatch.rs`, generic over the slot key, and return `io::Result<()>`
      (never an `edit::Edit`). `compile_action` runs inside `perform_trigger`,
      after `decide` has returned a non-`Nothing` decision.
- [x] All three call sites route through `trigger::decide` + `perform_trigger`:
      `dispatch_individual_down`'s bound-`Down` path, `handle_event`'s
      `Repeat | Up` arm, and `run_chord_effects`'s `FireChord` arm.
- [x] `run_chord_effects`'s `ReleaseChordFiring` / `StopChordToggle` /
      `ForceReleaseIndividual` arms and `handle_event`'s inline
      toggle-stop-on-`Down` all call `trigger::force_release_stuck` /
      `trigger::stop_toggle` — no hand-rolled
      `firings.get(&key).force_release_stuck` / `toggles.remove(&key); .stop()`
      copies remain outside those helpers. **Deviation:** `stop_toggle`
      returns `bool` (was `-> ()` in the sketch) so `handle_event`'s
      "press consumed by the stop" early-return has its signal; the
      `StopChordToggle` caller ignores it.
- [x] A dropped Fire-once / Hold-to-repeat `Step` firing (overlap guard hit)
      still does not advance the Stepper cursor — `perform_trigger` calls
      `compile_action` only for a non-`Nothing` decision, and `decide` returns
      `Nothing` for `Some(Slot::FiringUnfinished)`. Covered by the kept
      `fire_once_step_binding_*` / `hold_to_repeat_step_binding_*` cursor tests.
- [x] Firing a Fire-once / Hold-to-repeat / Toggle Chord, and the
      mouse-button / `ControllerButton` sustained-hold carve-outs (tickets 75,
      76, 78, 79, 80, 82), produce byte-for-byte the same uinput sequences as
      `HEAD` — verified by the existing `run_scripted` + dispatch-harness
      byte tests (including the four Chord-path executor tests) passing against
      the new code before any deletion (353 green pre-deletion).
- [x] New synchronous `trigger::tests` table module (2 tests: the exhaustive
      `decision_table` and `overlap_guard_only_blocks_on_an_unfinished_firing`);
      the four Chord-path executor tests are deleted, plus three purely-decision
      individual tests now redundant with the table
      (`fire_once_binding_ignores_repeat_and_up_fires_only_on_down`,
      `hold_to_repeat_fires_on_down_and_every_repeat_but_not_up` — the latter
      still byte-covered by `hold_to_repeat_keyboard_key_still_refires_on_every_repeat`,
      `analog_repeat_digital_sourced_behaves_like_hold_to_repeat`). The dispatch
      harness keeps one byte-level test per spawn kind plus every end-to-end /
      cursor / ProfileSwitch-member test.
- [x] `#[allow(clippy::too_many_arguments)]` is gone with `fire` /
      `execute_chord_fire` themselves (`perform_trigger` takes 4 args + a
      `TriggerCtx`). `handle_event` / `run_chord_effects` / `dispatch_individual_down`
      keep their pre-existing (ticket 07) `#[allow]`s unchanged.
- [x] `CONTRIBUTING.md` gains a "Changing how a Trigger mode fires" bullet,
      in the house style of the "new mutating `Command`" and "Changing
      Chord-detection behaviour" bullets.
- [x] Ticket 07's file has a Comment noting the deferred `trigger` unification
      landed here.
- [x] Full Daemon suite green (345 tests); `cargo fmt --check` clean; `cargo
      clippy --all-targets -- -D warnings` clean. GUI and packaging suites
      untouched (no wire / D-Bus / catalog change).

## Comments

**2026-09-01** — Filed from an architecture-review grilling session (candidate 1
of the review whose other candidates were the `wire.py` ↔ `dbus/wire.rs` codec
contract test, the `daemon_stub.py` operation-logic contract, a `DispatchState`
struct, the Analog-repeat pure core, GUI library-CRUD unification, a
`binding_editor.py` split, and the `config::store` split — the last declined as
"just moves it"). Design tree settled over three rounds:

- **Purity model:** a pure synchronous `decide(&Binding, EventState,
  Option<Slot>) -> TriggerDecision`, not a generic in-place `fire<K>` and not an
  encapsulated executor module — the whole payoff is collapsing the
  injector-and-task tests to a synchronous decision table, and the
  `(TriggerMode, EventState, Action)` matrix is exactly what tickets 75/76/78/
  79/80/82 keep re-tuning in two places.
- **Module:** new top-level `daemon/src/trigger.rs`, peer to `edit.rs` /
  `chord.rs`. Name matches CONTEXT.md's existing "Trigger mode" term.
- **`TriggerDecision`:** resolved `KeyCode` payloads for the two bare-hold
  variants; `SpawnFireOnce` / `StartToggleLoop` stay abstract so
  `compile_action` runs in the executor behind the overlap guard (cursor
  non-advance on a dropped firing).
- **Release ownership:** `chord.rs`'s decision logic is frozen — it keeps
  emitting `ReleaseChordFiring` / `StopChordToggle`. Only the executor helpers
  unify: those two Chord effects plus `handle_event`'s inline toggle-stop all
  route through shared `trigger::force_release_stuck` / `trigger::stop_toggle`.
- **`Slot`:** `chord::ChordSlot` (identical three states) is folded into
  `trigger::Slot` rather than kept as a second enum.
- **Executor:** dispatch-internal `perform_trigger<K>` + a `TriggerCtx<'_, K>`
  borrow-struct, per the ticket-05 `EffectCtx` precedent, rather than nine loose
  params and a seventh `#[allow]`.
- **One pass** (ticket 03–07 precedent); test-deletion sweep stageable within
  the PR. Behaviour-preservation protocol copied from ticket 07 (line-by-line
  arm diff, `run_scripted` green before deletions, `/code-review` Standards +
  Spec).
- **No CONTEXT.md term** ("Trigger mode" already defined, not sharpened here),
  **no ADR** (candidate accepted, not rejected). One `CONTRIBUTING.md` bullet.

Facts dug from the code during the grilling (not asked of the user): `chord.rs`
emits `FireChord` only with `state` `Down` / `Repeat`, never `Up` — the Chord
release path is entirely the separate `ReleaseChordFiring` / `StopChordToggle`
effects; `fire`'s sole extra arm over `execute_chord_fire` is `(FireOnce |
HoldToRepeat | AnalogRepeat, Up) → force_release_stuck`; `compile_action` (and
its `stepper_cursors` mutation) already runs only after the overlap guard in
both functions; `handle_event`'s toggle-stop-on-`Down` (dispatch.rs:787) is the
same two lines as `run_chord_effects`'s `StopChordToggle` arm.

**2026-09-01** — Resolved (`dev`). `daemon/src/trigger.rs` is the pure core:
`decide(&Binding, EventState, Option<Slot>) -> TriggerDecision` is `fire`'s
exact matrix, arm-for-arm, with the overlap guard reduced to
`!matches!(slot, Some(Slot::FiringUnfinished))`. `dispatch` keeps the
dispatch-internal `TriggerCtx<'_, K>` + generic `perform_trigger<K>` executor
(`compile_action` + `spawn_fire_once` / `ActiveToggle::spawn{,_held}` + map
insert, or `trigger::force_release_stuck`), plus `slot_for` (the single-key
form of `chord_slots`' three-way liveness read). All three call sites —
`dispatch_individual_down`'s bound-`Down`, `handle_event`'s `Repeat | Up` arm,
`run_chord_effects`'s `FireChord` arm — route through `trigger::decide` +
`perform_trigger`. `fire` and `execute_chord_fire` are gone; `chord::ChordSlot`
is folded into `trigger::Slot` (`chord::feed` / `dispatch::chord_slots` take
it). `chord.rs`'s decision logic is untouched — still emits `FireChord {
Down | Repeat }` + the separate `ReleaseChordFiring` / `StopChordToggle`
effects, which now call `trigger::force_release_stuck` / `trigger::stop_toggle`
alongside `handle_event`'s inline toggle-stop.

Deviations from the sketch: (1) `trigger::stop_toggle` returns `bool` (the
sketch showed `-> ()`) so `handle_event`'s "this press is consumed by the
stop" early-return keeps its signal; the `StopChordToggle` caller ignores it.
`chord`'s public items (`ChordMachine` / `ChordEffect` / `ChordOutcome` /
`feed` / `tick` / `next_deadline`) become `pub(crate)` to match `edit.rs`'s
ticket-05 convention — needed so `chord::feed` can take the `pub(crate)`
`trigger::Slot` without a `private_interfaces` warning; the `lib.rs` manifest
is untouched.

End-to-end behaviour unchanged: the full dispatch harness (`run_scripted` +
every byte-level test, including the four Chord-path executor tests) was green
against the new code *before* any deletion (353 tests). Then the four Chord
executor tests plus three purely-decision individual tests were removed and the
synchronous `trigger::tests` table added: 345 green, `cargo fmt --check` clean,
`cargo clippy --all-targets -- -D warnings` clean.

**2026-09-01** (post-review) — `/code-review` (Standards + Spec axes), both
run as parallel sub-agents. Spec: faithful arm-for-arm carve, all boxes hold,
the disclosed deviations judged sound, `decide` verified against the deleted
`fire` / `execute_chord_fire` bodies. Standards: no hard violations. Applied
the findings — (a) a `trigger_ctx!` macro so the three `perform_trigger` call
sites stop spelling the `TriggerCtx` literal (mirrors this file's own
`effect_ctx!` idiom); (b) rewrote `decide`'s bare-hold carve-outs around one
`sustained_hold_key(&Action) -> Option<KeyCode>` helper, collapsing the four
near-identical `matches!(… if is_mouse_button …)` guard arms into two;
(c) `Slot::is_firing` moved off the type into a private `chord::slot_is_firing`
free fn, so `trigger`'s `pub(crate)` surface is exactly the five named items;
(d) swept the last stale `dispatch::fire` / `execute_chord_fire` doc references
in `input.rs` / `config.rs`; (e) rewrapped two mangled comment lines in
`chord.rs`. Also self-caught before review: `slot_for` now consults the
firings map first (the old `fire` overlap guard only ever inspected firings —
a `Toggle`-priority read could have let a fresh `SpawnFireOnce` through where
`fire` dropped it, in a pathological both-maps-populated state). 345 green,
fmt + clippy clean after the fixes.
