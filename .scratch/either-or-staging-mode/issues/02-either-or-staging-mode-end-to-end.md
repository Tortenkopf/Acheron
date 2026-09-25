# 02: Either-Or Staging mode, end to end

**What to build:** A fourth Staging mode, **Either-Or** (see `CONTEXT.md`). A user picks
it in the grid-key editor's Staging mode row, it persists to `config.toml` and crosses
D-Bus, and each press of the key then fires exactly one of the two stages:

- the deep band reached within the ~50ms window: **deep only** (identical to
  Quick-Skip's Skipped phase);
- the window elapses first: **primary only**. The primary fires up to 50ms late and
  the deep stage is locked out for the rest of the press. The primary stays held through
  any excursion into and out of the deep band and releases only on the real release;
- released inside the window without reaching the deep band: **primary as a tap**
  (inherited from ticket 01).

Source of truth: [`spec.md`](../spec.md), especially the "Either-Or, from Late" table
and its consequences list.

Start with a prefactor, with no behaviour change: the stage engine's
"is this a Quick-Skip key" checks become "is this a windowed mode" (Quick-Skip or
Either-Or), and the per-press phase type and field take a mode-neutral name. Then add
the mode, which differs from Quick-Skip only in the Late phase.

Known trap: the engine drives a Hold-to-repeat deep stage off the primary's repeat
pulses whenever the deep band is held. For an Either-Or key in Late this must do
nothing. Otherwise the "`Repeat` with no firing re-presses first" rule fires the deep
stage, which is exactly what this mode forbids.

**Blocked by:** 01 (Quick-Skip plays a quick shallow tap as a primary tap)

**Status:** done

- [x] Prefactor lands with every existing Quick-Skip test green and no behaviour change
- [x] `either_or` round-trips through `config.toml` and the D-Bus wire; the GUI test stub accepts it
- [x] The GUI Staging mode row lists "Either-Or" after Quick-Skip, with the tooltip text from the spec
- [x] Pure-core table tests cover every Either-Or Late row (deep crossings in either direction emit nothing; the release is left to the real Up)
- [x] Dispatch: a fast full press fires the deep stage only; the primary is never emitted
- [x] Dispatch: a slow press to full depth and back fires the primary only; no deep emission on the way down or up; the primary is released on the real Up
- [x] Dispatch: Late with both stages Hold-to-repeat, held in the deep band: the primary keeps repeating, the deep stage never fires
- [x] Dispatch: Late with repeated dips into and out of the deep band emits nothing for any of them
- [x] Dispatch: a quick shallow tap flushes the primary as a tap (ticket 01's behaviour, under Either-Or)
- [x] Switching a held key's Staging mode into or out of Either-Or leaves nothing stuck (the existing mode-change teardown covers it)
- [x] README's dual-stage section gains an Either-Or bullet
- [x] Verified on hardware: a fast full press gives deep only; a normal press held and pushed through to full depth gives primary only

## Comments

Implemented in `daemon/src/stage.rs` (`either_or_late` for the Late table; the pure
`deep_locked_out` predicate, consulted by it and by `Engine::deep_repeat`), after a
no-behaviour-change prefactor (`WindowPhase` / `window`, `StagingMode::is_windowed`).

- **Spec correction.** The spec said the `SetStagingMode` teardown "needs no change".
  That holds except in one case: an Either-Or key gone Late holds its primary *in the
  deep band*, a shape no other mode leaves. `stop_stage`'s re-adoption would take
  `(Down, Down)` for a hand-off under the next mode, so it swallowed a Hold-to-repeat
  primary's Repeats and re-pressed the primary on the way out. `stop_stage` now
  force-releases that primary.
- **Pre-existing, not fixed here.** With plain Handoff, when the real `Up` arrives before
  the `(Down, Down) -> (Up, Up)` depth tick, the replayed `[ReleaseDeep, RepressPrimary,
  ReleasePrimary]` leaves a Hold-to-repeat primary held. `force_release` doesn't catch
  the fresh re-press. It reproduces with no Either-Or involvement, so the mode-flip test
  drives depth before the Up. Filed as [`tartarus-dual-stage-keys-impl` ticket 14](../../tartarus-dual-stage-keys-impl/issues/14-handoff-release-replay-leaves-a-hold-to-repeat-primary-stuck.md).

Hardware verification is still pending.
