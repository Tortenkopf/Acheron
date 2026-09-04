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

**Status:** ready-for-agent

- [ ] `stage::Engine`, non-pure, the third depth engine alongside `axis::Engine`/
      `analog_repeat::Engine`. Owns: per-key deep-band `KeyState` fed to the same
      `observe()` capture uses for the primary band; a shadow primary-band
      `KeyState` so the engine tracks the primary band itself off the depth stream
      (never needing the primary `PhysicalEvent`); and the deep stage's own
      `trigger::Slots<StageKey>` (Additive can hold both stages live at once, each a
      fully independent Binding).
- [ ] `DispatchState` gains a `stage: stage::Engine` field, constructed at
      `DispatchState::new` alongside `axis`/`analog_repeat`.
- [ ] `stage::Engine::update(&mut self, config: &Config, active_layer: Layer,
      snapshot: &HashMap<Input, u8>) -> Vec<(target, StageOp)>` runs from the
      existing `rx_depth.changed()` `select!` arm in `run`, third after
      `state.handle_depth_update(...)` and `state.update_analog_repeats(...)`. Deep
      `ActuationPoint` for an Input comes from
      `profile.deep_stages.get(&input).map(|c| c.actuation)`.
- [ ] Deep-stage firing: ops performed directly against `state.stage`'s
      `Slots<StageKey>` via `trigger::decide(&deep_binding, ..)` + `Slots::
      perform(..)` — no synthetic `PhysicalEvent` round trip. `ReleasePrimary`/
      `RepressPrimary` ops reach into the **primary** `Input` keyspace instead
      (`state.individual`'s `Slots::force_release`/`stop_toggle` to release,
      `dispatch_individual_down` to re-press) — precedented by the Chord executor's
      own `FireIndividual`/`ForceReleaseIndividual` effects touching two keyspaces
      from one shell.
- [ ] `stage::Engine::stop_all()` (force-releases every live deep slot + resets
      per-key `KeyState`) is wired into the two pre-existing `analog_repeat.
      stop_all()` call sites: `handle_layer_switch` and `handle_capture_mode_change`'s
      Digital-transition branch. (The two *new* teardown mechanisms — `SwitchProfile`'s
      effect list, cascade-delete, the disconnect hook — are ticket 06's job,
      deliberately deferred.)
- [ ] Ordering is resolved synchronously per-event from the arriving depth report —
      a same-report double-crossing (a fast ramp jumping both bands in one hidraw
      report) walks the full logical op sequence via mechanical replay, exactly as
      if the two crossings had arrived in separate reports, never depending on
      `rx_events`/`rx_depth.changed()` arrival order under `tokio::select!`'s
      unordered tie-break (closing the ordering race spec.md flags ticket 01's
      original design hadn't fully covered).
- [ ] Trigger-mode composition needs no per-combination logic beyond `decide`/
      `perform`'s existing Fire/Release rule: verify with an integration test that
      Handoff primary=Toggle/deep=Fire-once produces a **fresh** Toggle loop on
      `RepressPrimary` (not a resume), and that Additive with both stages
      Hold-to-repeat holds/repeats each stage on its own untouched cadence.
- [ ] `stage::Engine` integration tests exercised through the `rx_depth`/`rx_events`
      channels the way dispatch's other engines are (the harness precedent
      `analog_repeat`/`axis` integration tests already use): a same-report
      double-crossing resolves synchronously and produces the mechanically-replayed
      sequence, not a short-circuited one; each of Handoff/No-Return/Additive's full
      transition tables reproduced end-to-end against real `Slots<StageKey>`/
      `Slots<Input>` state (not just the pure `stage.rs` unit tests from ticket 02).
- [ ] Digital Capture mode: verify (or add, if not already structurally guaranteed)
      that a dual-stage key in Digital mode fires only its primary — the deep stage
      never engages because `stage::Engine::update` only ever runs off `rx_depth`,
      which the Digital-mode capture source never populates.
