<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 07 — Carve the Chord-detection state machine out of `dispatch.rs` into `chord`

**What to build:** The ~50 ms Chord simultaneity window and its event routing
move behind one small interface in a new `daemon/src/chord.rs`. A pure,
synchronous state machine decides what an incoming `PhysicalEvent` (or a window
timeout) does — which Chord completes, which member is suppressed, which member
fires its individual Binding retroactively — and answers that question with no
I/O, no async, no `Injector`, and no channels. `dispatch.rs` keeps the event
loop and gains an executor that performs the machine's described effects against
the runtime state it owns. `handle_event` stops reaching into `ChordState`.

Today `dispatch.rs` (6139 lines) holds the Chord machine as ~470 lines of free
functions — `fire_chord`, `release_chord_firing`, `fire_individual_retroactively`,
`handle_chord_event`, `handle_chord_timeout`, `chord_keys_containing`,
`chord_window_deadline` — that mutate a private `ChordState` in place and drive
`&Injector` directly. `handle_event` threads `chord_state` beside 11 other
params under `#[allow(clippy::too_many_arguments)]` and makes the chord-routing
decision itself (it reads `chord_state.claimed` at line 837 and calls
`chord_keys_containing` at line 840). `fire_individual_retroactively` reaches
across into the *individual*-Input firing path (`toggles`, `in_flight`, `fire`)
and can return `edit::Edit::SwitchProfile`. The Chord timing rules that keep
getting re-tuned against real hardware (tickets 40, 67, the thumbstick-diagonal
worked example) live entirely in this pure decision logic, but the only way to
test them today is the full `run_scripted` harness — spawn `run`, an injector,
seven channels, a tempfile.

This is the fourth step on the path tickets 03–06 walked. Ticket 03 made
*edit + persist* atomic; ticket 04 single-sourced *validation*; ticket 05 lifted
the whole config transaction into a pure, testable `edit` module and shrank
`dispatch` to the event loop plus effect execution; this ticket does the same
for the Chord machine — the other deep module hiding in `dispatch.rs` behind no
interface.

## The module shape

The interface is two decision functions plus a deadline accessor. These type
shapes encode decisions from the grilling and are load-bearing:

```rust
// daemon/src/chord.rs
//
// Pure and synchronous. Imports Config / ChordKey / Binding / Input /
// PhysicalEvent / EventState / Instant and NOTHING from executor, injector,
// or edit. No &Injector, no async, no channels.

/// The Chord-detection state machine's own state — pure bookkeeping only.
/// Reset fresh on every dispatch task start, same as today's `ChordState`.
/// The ChordKey-keyed firing/toggle *handles* are NOT here (see ChordRuntime).
pub struct ChordMachine {
    window: Option<ChordWindow>,
    claimed: HashSet<Input>,
}

/// Unchanged from today's private struct — moves in verbatim.
struct ChordWindow {
    down: BTreeSet<Input>,
    deadline: Instant,
}

/// Liveness of each Chord's dispatch-side firing/toggle, passed IN each call
/// rather than held by the machine — the machine stays pure, tests construct
/// this map directly. Absent key == no live firing or toggle for that Chord.
pub enum ChordSlot {
    /// An active Toggle-mode Chord.
    Toggle,
    /// A Fire-once / Hold-to-repeat Chord firing still in flight.
    FiringUnfinished,
    /// A Fire-once Chord whose firing has already completed on its own — the
    /// map entry lingers (never cleaned, mirroring `fire`'s own `in_flight`),
    /// so this must be a distinct state from "absent" or the Chord could
    /// never complete again.
    FiringFinished,
}

/// A post-decision effect the dispatch executor must perform.
pub enum ChordEffect {
    FireChord { key: ChordKey, binding: Binding, state: EventState },
    ReleaseChordFiring { key: ChordKey },
    StopChordToggle { key: ChordKey },
    /// Fire `input`'s own individual Binding as a synthetic fresh Down
    /// (window elapsed, or member released before completing).
    FireIndividual { input: Input },
    /// Immediately force-release what the preceding `FireIndividual` started —
    /// emitted only on the early-release path, never on a timeout. The
    /// machine knows which case it is, so it states it rather than making the
    /// executor re-derive it.
    ForceReleaseIndividual { input: Input },
    /// A Chord member's own individual Binding is `Action::ProfileSwitch` —
    /// the executor turns this into the `edit::Edit::SwitchProfile` it
    /// already commits via `commit_input_edits`.
    SwitchProfile { name: String },
}

pub enum ChordOutcome {
    /// Not an event the Chord machine owns — `handle_event` falls through to
    /// ordinary Binding lookup. Replaces `handle_event`'s inline
    /// `chord_state.claimed` / `chord_keys_containing` check.
    NotMine,
    Handled(Vec<ChordEffect>),
}

/// `event` routing + window bookkeeping. `chords` is
/// `profile.chords(active_layer)` — the machine needs no other Config view
/// (macros/steppers/individual-binding lookup are the executor's job).
/// Ordering within the returned Vec is significant and preserves today's
/// behaviour: completions (`FireChord`) before re-completion stops
/// (`StopChordToggle`); `FireIndividual` immediately followed by its
/// `ForceReleaseIndividual` on the early-release path.
pub fn feed(
    m: &mut ChordMachine,
    chords: &HashMap<ChordKey, Binding>,
    live: &HashMap<ChordKey, ChordSlot>,
    event: PhysicalEvent,
) -> ChordOutcome;

/// The window deadline elapsed — every still-pending member fires its
/// individual Binding retroactively. Needs no `chords` map: each pending
/// member becomes `FireIndividual { input }` and the executor resolves the
/// Binding against the current Layer.
pub fn tick(m: &mut ChordMachine, now: Instant) -> ChordOutcome;

/// The active window's deadline, or `None`. The `run` loop's `select!`
/// timeout branch arms on this (replacing `chord_window_deadline`).
pub fn next_deadline(m: &ChordMachine) -> Option<Instant>;
```

## What moves, what stays

- **Into `chord.rs`:** `ChordWindow`, the `ChordState` bookkeeping fields
  (`window`, `claimed`) as `ChordMachine`, the routing/completion/suppression/
  retroactive logic from `handle_chord_event` and `handle_chord_timeout`, the
  `chord_keys_containing` helper, and the `CHORD_WINDOW` const. All of it
  rewritten to return `ChordEffect`s instead of performing them.
- **Stays in `dispatch.rs`:**
  - `ChordRuntime { firings: HashMap<ChordKey, FiringHandle>, toggles: HashMap<ChordKey, ActiveToggle> }`
    — one `run`-local, mirroring how `AxisState` bundles its two maps. Built
    fresh per dispatch task start. The executor builds the `live:
    HashMap<ChordKey, ChordSlot>` snapshot from it per `feed` call
    (`toggles` → `Toggle`; `firings` → `FiringUnfinished` / `FiringFinished`
    by `handle.is_finished()`).
  - The effect executor for `FireChord` / `ReleaseChordFiring` /
    `StopChordToggle` — today's `fire_chord` and `release_chord_firing`
    bodies moved essentially verbatim, still keyed by `ChordKey`, operating
    on `ChordRuntime`, sitting next to `fire`. This is where `compile_action`
    (and its `stepper_cursors` mutation), `executor::spawn_fire_once`,
    `ActiveToggle::spawn*`, and every `&Injector` write stay.
  - `dispatch_individual_down(&Config, Layer, Input, &mut toggles, &mut in_flight,
    &mut stepper_cursors, toggle_lap_target) -> io::Result<Vec<edit::Edit>>`
    — carved out of `handle_event`'s tail (the `ProfileSwitch → Edit` /
    bound → `fire` / unbound → passthrough branches). Called by **both** the
    ordinary `handle_event` path and the `FireIndividual` executor, so the
    retroactive fire is not a second copy of that logic. Not a re-entry into
    `handle_event` — that would re-run the layer-switch / toggle-stop / axis /
    chord guards against a synthetic Down, which is wrong.
  - `next_deadline` wiring: the `select!` branch arms on `chord::next_deadline(&machine)`
    and calls `chord::tick(&mut machine, Instant::now())` when it fires.
- **No behaviour change on switch cleanup:** chord-scoped firings/toggles are
  not cleaned on a Layer or Profile switch today (only individual `toggles`
  are). Preserve that. Whether a Chord Toggle *should* survive a Profile
  switch is a real open question — flag it for the domain owner, do not change
  it in this mechanical carve.

## The `handle_event` → `chord` → effect flow

- `handle_event` calls `chord::feed` **unconditionally**, after the
  layer-switch / toggle-stop-on-Down / axis-assigned-swallow guards and before
  ordinary Binding lookup. `ChordOutcome::NotMine` → fall through to the
  existing `bindings.get(&event.input)` path. `ChordOutcome::Handled(effects)`
  → run the executor, collect any `Vec<edit::Edit>` it produces (from
  `SwitchProfile` / a `FireIndividual` that resolves to a ProfileSwitch), and
  hand them to `commit_input_edits` exactly as today.
- The `select!` timeout branch calls `chord::tick`, runs the executor the same
  way, and commits any resulting `Edit`s.
- `handle_event` loses its `chord_state` parameter and its inline
  `chord_state.claimed` / `chord_keys_containing` check — the machine owns that
  predicate now. Params drop from 12 to (at most) 10; keep the `#[allow]` if it
  still trips the threshold, drop it if it doesn't.

## Landing in one pass

Convert the cluster, carve `dispatch_individual_down`, rewire `handle_event` and
the `select!` branch, in one pass — a half-migrated `handle_event` carrying both
the inline chord routing and a `chord::feed` call is harder to read than either
alone (ticket 03 / 04 / 05 precedent). The test-deletion sweep below may stage
behind the implementation within the same PR.

## Tests: replace, don't layer

- New synchronous `chord::tests` module — drives `feed` / `tick`, asserts
  `ChordOutcome`. No tokio, no injector, no tempfile. This is the new test
  surface. It gets:
  - The decision logic moved out of the dispatch harness:
    `thumbstick_diagonals_fire_independently_and_share_a_member`,
    `hold_to_repeat_chord_refires_only_on_the_leader_members_repeat`,
    `toggle_chord_survives_releasing_one_member_and_stops_on_a_fresh_completion`,
    `a_fire_once_chord_fires_again_after_being_fully_released_and_re_pressed`,
    subset/superset completion in one Down pass, and the window-timeout
    retroactive path.
  - A table over `(TriggerMode, EventState, ChordSlot)` → expected
    `Vec<ChordEffect>`, exercised directly rather than only transitively.
- Dispatch harness — **keep** (~3–4): one per effect variant that actually
  reaches uinput — `toggle_chord_mouse_button_holds_a_single_keydown_and_full_completion_stops_it`,
  `toggle_chord_controller_button_holds_a_single_keydown_and_full_completion_stops_it`,
  `hold_to_repeat_chord_controller_button_ignores_repeat_and_releases_on_member_up`
  — plus one full `feed → FireIndividual → ProfileSwitch` input-path commit.
- Dispatch harness — **delete**: every remaining chord test that only asserts a
  decision (which Chord completed, which member was suppressed) rather than a
  uinput byte sequence.

## Out of scope

- **Unifying `fire` and `fire_chord`.** `fire_chord` is a near-verbatim copy of
  `fire` — the same `(TriggerMode, EventState)` dispatch over `ChordKey`
  instead of `Input`. Q1(a)'s split already removes most of the duplication for
  free (both become the dispatch executor performing a "fire this Binding"
  effect). A real unification — one `trigger` module both paths call — is its
  own candidate with its own blast radius (the input-path `fire` is not under
  review here). File it once this ticket lands and the residual duplication is
  visible.
- The daemon `config::store` submodule split (separate architecture-review
  candidate).
- Any change to whether a Chord Toggle survives a Profile switch.

**Blocked by:** None — ticket 05 (`edit.rs`, the `plan`/`Effect` precedent this
follows) is resolved (`61c10c1`).

**Status:** resolved

- [x] `daemon/src/chord.rs` exists and exposes `ChordMachine`, `ChordSlot`,
      `ChordEffect`, `ChordOutcome`, `feed`, `tick`, and `next_deadline`;
      nothing else is `pub(crate)`. It imports nothing from `executor`,
      `injector`, or `edit`, contains no `async fn`, and takes no `&Injector`.
- [x] `feed` takes `&HashMap<ChordKey, Binding>` (not `&Config` / `&Profile`),
      a `&HashMap<ChordKey, ChordSlot>` liveness snapshot, and a
      `PhysicalEvent`; `tick` takes only `&mut ChordMachine` and `Instant`.
      (`feed` calls `Instant::now()` internally when it opens a fresh window,
      exactly as the old `handle_chord_event` Down arm did.)
- [x] `ChordOutcome::NotMine` is returned for any event the machine does not
      own; `handle_event` no longer reads `chord_state.claimed` or calls
      `chord_keys_containing`, and calls `chord::feed` unconditionally after
      the layer-switch / toggle-stop / axis guards.
- [x] `dispatch::ChordRuntime` holds the `ChordKey`-keyed `FiringHandle` /
      `ActiveToggle` maps as one `run`-local; `dispatch::chord_slots` derives
      the `ChordSlot` snapshot from it (including `FiringFinished` via
      `is_finished()`).
- [x] The `FireChord` / `ReleaseChordFiring` / `StopChordToggle` executor
      (`run_chord_effects` + `execute_chord_fire`) is the old `fire_chord` /
      `release_chord_firing` logic, keyed by `ChordKey`, next to `fire`; all
      `compile_action` / `stepper_cursors` mutation / spawning / uinput writes
      are dispatch-side.
- [x] `dispatch_individual_down` is carved from `handle_event`'s tail and
      called by both the ordinary Down path and the `FireIndividual` executor
      — the retroactive-fire logic exists once. `ForceReleaseIndividual` is a
      distinct effect emitted only on the early-release path.
- [x] Firing an `Action::ProfileSwitch` as a Chord member's individual
      Binding (retroactively, or on timeout) still switches the Profile and
      runs its post-commit effects. **Deviation:** there is no
      `ChordEffect::SwitchProfile` variant — the pure machine has no view of a
      member's *individual* Binding, so it can only ever emit
      `FireIndividual { input }`; the executor resolves that through
      `dispatch_individual_down`, which returns the `edit::Edit::SwitchProfile`
      for `commit_input_edits` (same runtime behaviour, one fewer hop). See
      the new `chord.rs` `FireIndividual` doc comment and the
      `dispatch::tests` commit test.
- [x] The `select!` timeout branch arms on `chord::next_deadline` and calls
      `chord::tick`; `chord_window_deadline` is gone (replaced by
      `wait_for_chord_deadline(Option<Instant>)`).
- [x] Chord-scoped firings/toggles are still not cleaned on a Layer/Profile
      switch (unchanged); the open question about Chord-Toggle survival across
      a Profile switch is flagged for the domain owner in a comment on
      `edit::plan`'s `SwitchProfile` → `Effect::StopAllToggles` push.
- [x] New synchronous `chord::tests` module (11 tests) with the moved decision
      tests plus the `(TriggerMode, EventState, ChordSlot)` table. The dispatch
      harness keeps the byte-level chord tests (`toggle_chord_mouse_button…`,
      `toggle_chord_controller_button…`, `hold_to_repeat_chord_controller_button…`,
      `hold_to_repeat_chord_mouse_button…` — the last kept on a Standards-review
      note: `execute_chord_fire`'s mouse-button arm is a distinct executor
      branch) plus a new
      `a_chord_member_whose_individual_binding_is_a_profile_switch_switches_on_early_release`;
      the four decision-only chord tests (`thumbstick_diagonals…`,
      `hold_to_repeat_chord_refires_only_on_the_leader…`,
      `toggle_chord_survives_releasing_one_member…`,
      `a_fire_once_chord_fires_again…`) are gone from the harness.
- [x] `handle_chord_event` / `handle_chord_timeout` / `fire_chord` no longer
      exist as dispatch free functions under those names (logic split between
      `chord.rs` and the executor). `#[allow(clippy::too_many_arguments)]`
      stays on `handle_event` — it still trips the threshold (13 args: the old
      `chord_state` became `chord_machine` + `chord_runtime`; the prose's
      "12 → ≤10" estimate did not land, no other arg could leave without
      unrelated bundling).
- [x] `CONTRIBUTING.md` gains a "Changing Chord-detection behaviour" bullet
      alongside the "new mutating `Command`" recipe.
- [x] Full Daemon suite green (350 tests); `cargo fmt --check` clean; `cargo
      clippy --all-targets -- -D warnings` clean. GUI and packaging suites
      untouched (no wire / D-Bus / catalog change).

## Comments

**2026-09-01** — Filed from an architecture-review grilling session (candidate 1
of the review that also re-surfaced the `config::store` split and the
`wire.py` ↔ `dbus/wire.rs` codec mirror as candidates 2–3). Design tree settled
over three rounds:

- **Purity:** a pure synchronous core (`feed` / `tick` returning
  `Vec<ChordEffect>`), not an async encapsulated module — the whole
  justification is collapsing the ~8 `run_scripted` chord tests to synchronous
  ones, and the recurring hardware-tuned timing bugs (tickets 40, 67,
  diagonals) all live in the pure part. Chord-firing liveness is passed in as a
  `HashMap<ChordKey, ChordSlot>` snapshot so the machine never holds a
  `FiringHandle` / `ActiveToggle`.
- **Module:** new top-level `daemon/src/chord.rs`, matching the `edit.rs`
  precedent — a peer to the input engine, not a submodule of it.
- **Routing predicate moves into the module** (`ChordOutcome::NotMine`) —
  `handle_event` keeping its fingers in `chord_state.claimed` is the leak that
  keeps the module shallow.
- **`fire` / `fire_chord` unification deferred** to a follow-up candidate — real
  deep-module opportunity (a shared `trigger` module) but its own blast radius,
  and Q1(a) removes most of the duplication for free anyway.
- **`FireIndividual` dispatch:** a `dispatch_individual_down` helper carved from
  `handle_event`'s tail, shared by both paths — not recursion through
  `handle_event`. Early-release force-release is a distinct
  `ForceReleaseIndividual` effect, not implicit executor behaviour.
- **Liveness snapshot:** one `HashMap<ChordKey, ChordSlot>` with a three-way
  enum, not three parallel `HashSet`s — makes `FiringFinished` an explicit
  state and the map directly constructible in a table test.
- **`ChordRuntime`** bundles the two `ChordKey`-keyed handle maps as one
  `run`-local, mirroring `AxisState`.
- **One pass** (ticket 03 / 04 / 05 precedent); test-deletion sweep stageable
  within the same PR.
- No ADR (candidate accepted, not rejected); no `CONTEXT.md` change ("Chord" is
  already defined and this doesn't sharpen it).

Facts dug from the code during the grilling (not asked of the user):
`ChordState` / `ChordWindow` are private to `dispatch.rs` with no external
readers; `fire_chord` is a near-verbatim copy of `fire`; chord-scoped
firings/toggles are never cleaned on a Layer or Profile switch
(`stop_all_toggles` and `switch_profile` touch only the individual `toggles`
map); the window `select!` branch re-creates its `sleep_until` future every loop
iteration so a window opened/extended/cleared by `handle_event` in between is
always picked up next iteration.

**2026-09-01** — Resolved (`dev`). `daemon/src/chord.rs` is the pure core:
`feed` / `tick` return `Vec<ChordEffect>`; `next_deadline` arms the `select!`
branch. `dispatch` keeps `ChordRuntime` (the two `ChordKey`-keyed handle maps),
`chord_slots` (the `ChordSlot` snapshot), `run_chord_effects` (the executor),
`execute_chord_fire` (ex-`fire_chord`), `dispatch_individual_down` (the
`handle_event` Down tail, now shared with the retroactive path), and
`wait_for_chord_deadline`. `handle_event` calls `chord::feed` unconditionally
and owns no chord predicate. End-to-end behaviour is unchanged — the four
pre-existing dispatch chord tests that were deleted all still passed against the
new code before removal, and the `dbus` integration chord tests are untouched
and green.

One design deviation from the ticket sketch, detailed in the checklist: no
`ChordEffect::SwitchProfile` variant (nothing pure can emit it — the machine
never sees a member's individual Binding). `FireIndividual` + the shared
`dispatch_individual_down` cover the ProfileSwitch-member case with identical
runtime behaviour.

**2026-09-01** (post-review) — `/code-review` (Standards + Spec axes). Spec:
faithful mechanical carve, all boxes hold, the `ChordEffect::SwitchProfile`
omission judged sound, no behavioural drift (risky ports checked line-by-line
against `HEAD`). Standards: no hard violations; applied the judgement-call
cleanups — `m` → `machine` param naming, `chords_with_member` helper (restores
the old `chord_keys_containing` predicate, was open-coded 3×),
`close_window_if_drained` helper (was duplicated in `feed_down`/`feed_up`),
hoisted the AnalogRepeat swallow guard above `handle_event`'s `event.state`
match (was duplicated across the Down and Repeat|Up arms), and restored
`hold_to_repeat_chord_mouse_button_ignores_repeat_and_releases_on_member_up`
(distinct `execute_chord_fire` branch, no other coverage). 350 tests green.
