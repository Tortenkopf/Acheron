# 03 — `stage::Engine` wired into dispatch for Handoff / No-Return / Additive

**What to build:** A grid key configured with a deep stage in Handoff, No-Return, or
Additive mode now actually fires both stages through the running daemon, off live (or
test-harness) Depth — the feature's first end-to-end-observable behavior. A
hand-edited `config.toml` with a Handoff deep stage on `grid_r1c1` produces exactly
the op sequence spec.md's tables describe as Depth ramps through both bands.
Quick-Skip-mode keys are accepted by config but not yet specially handled — they
behave as an unaffected primary-only key for now (ticket 04 completes them); this is
forward progress, not a regression.

Source of truth: [`spec.md`](../../tartarus-dual-stage-keys/spec.md) §"Pipeline
architecture" and §"Staging-mode state machine", and
[ADR-0007](../../../docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md).

**Blocked by:** 02

**Status:** resolved

- [x] `stage::Engine`, non-pure, the third depth engine alongside `axis::Engine`/
      `analog_repeat::Engine`. Owns: per-key deep-band `KeyState` fed to the same
      `observe()` capture uses for the primary band; a shadow primary-band
      `KeyState` so the engine tracks the primary band itself off the depth stream
      (never needing the primary `PhysicalEvent`); and the deep stage's own
      `trigger::Slots<StageKey>` (Additive can hold both stages live at once, each a
      fully independent Binding).
- [x] `DispatchState` gains a `stage: stage::Engine` field, constructed at
      `DispatchState::new` alongside `axis`/`analog_repeat`.
- [x] `stage::Engine::update(&mut self, config: &Config, active_layer: Layer,
      snapshot: &HashMap<Input, u8>) -> Vec<(target, StageOp)>` runs from the
      existing `rx_depth.changed()` `select!` arm in `run`, third after
      `state.handle_depth_update(...)` and `state.update_analog_repeats(...)`. Deep
      `ActuationPoint` for an Input comes from
      `profile.deep_stages.get(&input).map(|c| c.actuation)`.
- [x] Deep-stage firing: ops performed directly against `state.stage`'s
      `Slots<StageKey>` via `trigger::decide(&deep_binding, ..)` + `Slots::
      perform(..)` — no synthetic `PhysicalEvent` round trip. `ReleasePrimary`/
      `RepressPrimary` ops reach into the **primary** `Input` keyspace instead
      (`state.individual`'s `Slots::force_release`/`stop_toggle` to release,
      `dispatch_individual_down` to re-press) — precedented by the Chord executor's
      own `FireIndividual`/`ForceReleaseIndividual` effects touching two keyspaces
      from one shell.
- [x] `stage::Engine::stop_all()` (force-releases every live deep slot + resets
      per-key `KeyState`) is wired into the two pre-existing `analog_repeat.
      stop_all()` call sites: `handle_layer_switch` and `handle_capture_mode_change`'s
      Digital-transition branch. (The two *new* teardown mechanisms — `SwitchProfile`'s
      effect list, cascade-delete, the disconnect hook — are ticket 06's job,
      deliberately deferred.)
- [x] Ordering is resolved synchronously per-event from the arriving depth report —
      a same-report double-crossing (a fast ramp jumping both bands in one hidraw
      report) walks the full logical op sequence via mechanical replay, exactly as
      if the two crossings had arrived in separate reports, never depending on
      `rx_events`/`rx_depth.changed()` arrival order under `tokio::select!`'s
      unordered tie-break (closing the ordering race spec.md flags ticket 01's
      original design hadn't fully covered).
- [x] Trigger-mode composition needs no per-combination logic beyond `decide`/
      `perform`'s existing Fire/Release rule: verify with an integration test that
      Handoff primary=Toggle/deep=Fire-once produces a **fresh** Toggle loop on
      `RepressPrimary` (not a resume), and that Additive with both stages
      Hold-to-repeat holds/repeats each stage on its own untouched cadence.
- [x] `stage::Engine` integration tests exercised through the `rx_depth`/`rx_events`
      channels the way dispatch's other engines are (the harness precedent
      `analog_repeat`/`axis` integration tests already use): a same-report
      double-crossing resolves synchronously and produces the mechanically-replayed
      sequence, not a short-circuited one; each of Handoff/No-Return/Additive's full
      transition tables reproduced end-to-end against real `Slots<StageKey>`/
      `Slots<Input>` state (not just the pure `stage.rs` unit tests from ticket 02).
- [x] Digital Capture mode: verify (or add, if not already structurally guaranteed)
      that a dual-stage key in Digital mode fires only its primary — the deep stage
      never engages because `stage::Engine::update` only ever runs off `rx_depth`,
      which the Digital-mode capture source never populates.

## Comments

**2026-09-05** — Implemented as specified, with a few signature/shell divergences from
the checklist's literal prose, each recorded on `stage::Engine::update`'s own doc
comment:

- `Engine::update` takes one bundled `EngineDeps<'_>` (config/active_layer/`individual`/
  injector/cursors/toggle_lap_target) rather than the checklist's flatter parameter
  list — clippy's `too_many_arguments` gate, the same reason `trigger::PerformDeps`
  bundles `Slots::perform`'s own scattered deps.
- `Engine::update` performs **every** op itself — including `FirePrimary`/
  `RepressPrimary` against `individual` — and returns `Vec<edit::Edit>`, not the
  checklist's `Vec<(target, StageOp)>` for a caller to walk. An earlier draft deferred
  `FirePrimary`/`RepressPrimary` to `DispatchState`, since only it has
  `dispatch_individual_down`'s `Action::ProfileSwitch` short-circuit; that briefly
  looked plausible until the 1-report-skip release row
  (`[ReleaseDeep, RepressPrimary, ReleasePrimary]`) exposed the flaw — deferring
  `RepressPrimary` while performing the trailing `ReleasePrimary` inline reordered them
  (the release ran as a same-tick no-op *before* the deferred repress ever fired,
  leaving a Toggle repress permanently live instead of briefly flashing and releasing).
  Fixed by inlining a small `fire()` helper (generalized over `K = Input` / `StageKey`)
  that replicates `dispatch_individual_down`'s Down-side logic directly in `stage.rs`,
  so every op in a transition's sequence runs fully awaited, in order, in one loop —
  the reason `Engine` now imports `config::Action`/`edit::Edit` at all, unlike
  `chord.rs`/`analog_repeat.rs`, which never need to (their own Action can never be
  `ProfileSwitch`; a dual-stage deep/primary Binding has no such restriction).
- The primary's lone outer `FirePrimary`/`ReleasePrimary` row (no accompanying
  `FireDeep`/`ReleaseDeep` — an ordinary press/release that never touched the deep
  band) is left to the always-present, unmodified `rx_events` path rather than
  actively performed — necessary so a Toggle primary keeps its ordinary "outlives a
  bare release" behavior when the deep band is never engaged. Only a same-report
  double-crossing's longer sequence is walked synchronously by the engine itself,
  which is what actually closes ADR-0007's ordering race; documented as a narrow,
  accepted residual gap (same class as ticket 39's) if the real primary edge for that
  exact crossing happens to arrive afterward.

`/code-review` (three independent finder passes plus a combined reuse/efficiency pass)
caught two real bugs and two real robustness/efficiency gaps, all fixed and covered by
new tests before this ticket closed:

1. **`ReleaseDeep` never stopped a Toggle-mode deep Binding.** It ran the ordinary
   `decide(Up)` + `perform` path — a no-op for Toggle, mirroring a *physical* Up's own
   "Toggle outlives release" rule — so a deep Toggle kept running past `ReleaseDeep`,
   and the next `FireDeep` unconditionally `insert`ed a second, orphaned loop over it
   (`decide`'s `(Toggle, Down)` arm never checks `slot`). Fixed: `ReleaseDeep` now
   mirrors `ReleasePrimary`'s own unconditional `stop_toggle` + `force_release`, not
   `decide`/`perform` — a stage's own release must always fully clear the slot so the
   next `FireDeep` genuinely starts fresh. Covered by
   `dual_stage_deep_toggle_stops_on_release_deep_and_gets_a_fresh_loop_on_refire`.
2. **`stop_all()`'s reset could synthesize a spurious full replay.** Resetting a held
   key's tracked bands to `(Up, Up)` while its real Depth never moved meant the very
   next tick could see a fake `(Up,Up)` → still-deep transition and mechanically
   replay a fresh press through both bands the instant a Layer switch completed, even
   though nothing physically happened. Fixed: `KeyRuntime` gained a `just_reset` flag
   `stop_all()` sets (without discarding the map entries, unlike a truly-untouched
   key); the *next* `update` tick for a flagged key silently re-adopts whatever bands
   Depth currently reads, with no emitted ops, then resumes ordinary delta-based
   tracking. Kept distinct from a genuinely new key's first tick, which still gets
   `advance`'s full-replay treatment — a fast double-crossing landing on a key's very
   first observed sample is a real, tested scenario (the pure tables' own `(Up,Up)`
   rows), not a reset artifact. Covered by
   `dual_stage_layer_switch_mid_press_while_still_deep_does_not_spuriously_refire`.
3. **A `deep_stages` entry's `Binding` was cloned unconditionally on every tick**,
   before the cheap `prev == next` no-crossing check — the same hot sub-millisecond
   per-report path `config::resolved_actuation_point` was split out to avoid a
   redundant rebuild on. Fixed: only checks existence up front now; the clone moved
   to after a real transition is confirmed.
4. **`StageOp::SuppressPrimary => unreachable!()`** depended on an invariant
   (the `QuickSkip` filter) enforced 80 lines away — a future change to that filter
   without revisiting this arm would panic the whole dispatch task, not just the
   offending key. Softened to a `debug_assert!` + graceful no-op, matching `stage.rs`'s
   own pure-core precedent (`unreachable_transition`).

Two more review notes were judged accepted, pre-existing characteristics rather than
bugs to fix in this ticket, and documented in place instead: `rx_depth`'s coalescing
`watch` semantics can in principle widen the same-report race beyond literally one
hidraw report if dispatch falls behind (the same characteristic every `rx_depth`
consumer in this file already has, not specific to this row); and `trigger::
Slots::stop_all`'s `force_release_stuck` can't release a firing `tokio` hasn't polled
yet, the same narrow, pre-existing race plain `force_release` always had.

`cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean throughout.
Daemon suite: 430 (ticket 02) → 441 (12 new `dual_stage_*` integration tests in
`dispatch.rs`, exercised through real `rx_events`/`rx_depth` channels against a
`CommandHarness`-spawned `run()`, plus one new `trigger::Slots::stop_all` addition) —
covering Handoff's full slow walkthrough, a same-report double-crossing in both
directions, Handoff Toggle-primary/Fire-once-deep producing a fresh loop on
`RepressPrimary`, Additive's independent untouched cadences, No-Return's no-repress
row, Digital-mode inertness, QuickSkip's unaffected-primary-only scoping, and the two
teardown/regression fixes above.
