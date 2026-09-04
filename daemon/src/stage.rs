// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The Staging-mode state machine (`tartarus-dual-stage-keys` ticket 02) — a
//! pure core deciding, for every `(primary band, deep band, StagingMode)`
//! combination, exactly which ops fire and in what order, including
//! same-report double-crossings (mechanical replay, never short-circuited)
//! and Quick-Skip's own Armed/Skipped/Late per-press runtime state. Mirrors
//! the discipline `chord`/`axis`/`analog_repeat`'s own pure cores already
//! hold to: no tokio, no injector, no `Config`, hardware-tuned band/timing
//! rules table-tested without spawning `run`.
//!
//! **No runtime behavior change yet — nothing here is wired into
//! `dispatch`.** This is the prefactor ticket 03's dispatch integration
//! builds on.
//!
//! Source of truth: `.scratch/tartarus-dual-stage-keys/spec.md`
//! §"Staging-mode state machine", and
//! `docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md`.
//!
//! Imports nothing from `dispatch`/`edit`/`chord`/`config::Config` — only the
//! lightweight `StagingMode` config type, the same discipline `chord.rs`
//! holds to importing `Binding`/`ChordKey`/`TriggerMode` from `config`.
//!
//! ## `StageOp::Nothing` vs. an empty `Vec`
//!
//! `advance`/`tick` distinguish two different "no ops" outcomes deliberately:
//! a genuine **no-crossing** row (`prev == next`, or a `tick` before its
//! deadline) returns `vec![StageOp::Nothing]` — literally the table's own
//! "no crossing" entry, so a caller can match on it the same way it matches
//! any other row. A **defined crossing whose designed effect is silence**
//! (Quick-Skip's early-Up cancellation, or the primary's permanently-inert
//! final release once Skipped) returns an *empty* `Vec` — a crossing did
//! happen, the state machine just has nothing to perform for it. Collapsing
//! these into one convention would blur "nothing crossed" with "something
//! crossed but is deliberately silent," which spec.md treats as distinct.

// Nothing in this module has a caller yet — ticket 03's `stage::Engine`
// wires it into `dispatch` next. Remove this once that lands; until then it
// keeps `cargo clippy --all-targets -- -D warnings` (CONTRIBUTING.md) green
// for a deliberately-unwired prefactor module.
#![allow(dead_code)]

use std::time::Duration;

use tokio::time::Instant;

use crate::config::StagingMode;
use crate::input::Input;

/// Quick-Skip's fixed suppression window (spec.md "Solution": "~50ms") — a
/// Rust constant, not a persisted `Config` value, the same status
/// `chord::CHORD_WINDOW` has.
const QUICK_SKIP_WINDOW: Duration = Duration::from_millis(50);

/// One stage's simple Up/Down hysteresis band — the same shape
/// `capture::analog::KeyState` gives a single key, defined locally here so
/// `stage.rs` never imports from `capture` (only the lightweight `config`
/// types this module is allowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Band {
    Up,
    Down,
}

/// The combined `(primary band, deep band)` state a transition moves
/// between. `(Up, Down)` is structurally impossible (spec.md
/// "Staging-mode state machine": the disjoint-stacked-band constraint means
/// Depth can never be low enough to cross `primary.release` while still
/// `>= deep.release`) — asserted, not defensively handled, everywhere this
/// module matches on one.
pub(crate) type Bands = (Band, Band);

/// A newtype distinguishing the deep slot's own keyspace from the primary's
/// bare `Input` keyspace (spec.md "Pipeline architecture") — dispatch-internal
/// only, unlike `config::ChordKey`, no `Profile` map is ever keyed by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct StageKey(pub Input);

/// One emitted op (spec.md "Staging-mode state machine"). Data-only — no
/// payload, since the caller (`stage::Engine`, ticket 03) already knows
/// which key it's driving and reaches into the right keyspace
/// (`Slots<StageKey>` for `FireDeep`/`ReleaseDeep`, `Slots<Input>` for
/// everything else) per op kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StageOp {
    FirePrimary,
    ReleasePrimary,
    RepressPrimary,
    FireDeep,
    ReleaseDeep,
    SuppressPrimary,
    /// The "no crossing" table row itself — see the module doc's note on
    /// `Nothing` vs. an empty `Vec`.
    Nothing,
}

/// Quick-Skip's own per-press runtime state (spec.md "Quick-Skip" table),
/// layered on top of Handoff's mechanics. `None` (outside this type, carried
/// by the caller as `Option<QuickSkipPhase>`) means "not currently mid a
/// Quick-Skip press" — every other Staging mode, or a Quick-Skip key at rest
/// between presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuickSkipPhase {
    /// The primary's real Down is buffered (ticket 04's dispatch-side
    /// buffer swallows it) — waiting to see whether the deep band is
    /// reached before `deadline`.
    Armed { deadline: Instant },
    /// The deep band was reached in time — the primary is permanently
    /// suppressed for the rest of this press; every further crossing runs
    /// Additive-with-no-primary, release path = No-Return.
    Skipped,
    /// The deadline elapsed first — `tick` already emitted the retroactive
    /// `RepressPrimary`; the rest of this press runs plain Handoff.
    Late,
}

/// Advances one key's combined `(primary, deep)` band state by one
/// transition, producing the ordered `Vec<StageOp>` spec.md's four
/// Staging-mode tables specify, plus the Quick-Skip phase this transition
/// leaves the key in (always `None` for every mode but `QuickSkip`). A
/// same-report double-crossing is handled by mechanical replay: `prev`/
/// `next` may differ by more than one band at once (the 1-report-skip
/// rows), resolved as a single ordered op sequence, never short-circuited.
pub(crate) fn advance(
    prev: Bands,
    next: Bands,
    mode: StagingMode,
    quick_skip: Option<QuickSkipPhase>,
) -> (Vec<StageOp>, Option<QuickSkipPhase>) {
    assert_reachable(prev);
    assert_reachable(next);
    match mode {
        StagingMode::Handoff => (handoff(prev, next), None),
        StagingMode::NoReturn => (no_return(prev, next), None),
        StagingMode::Additive => (additive(prev, next), None),
        StagingMode::QuickSkip => quick_skip_advance(prev, next, quick_skip),
    }
}

/// The Quick-Skip timeout's deadline, or `None` if no Quick-Skip press is
/// currently Armed — mirroring `chord::next_deadline`'s exact shape. The
/// `run` loop's `select!` timeout branch (ticket 04) arms on this.
pub(crate) fn next_deadline(quick_skip: Option<QuickSkipPhase>) -> Option<Instant> {
    match quick_skip {
        Some(QuickSkipPhase::Armed { deadline }) => Some(deadline),
        _ => None,
    }
}

/// Fires the window timeout — mirroring `chord::tick`'s exact shape (same
/// `Instant` / outcome-producing signature convention). A no-op unless
/// Armed and `now` has reached `deadline`: on the real deadline elapsing
/// with the deep band never reached, the primary fires retroactively
/// (`RepressPrimary`, performed via `dispatch_individual_down` by ticket
/// 03's `Engine`) and the key flips to Late — plain Handoff for the rest of
/// the press. `now` guards a spurious call before the deadline, same as
/// `chord::tick`.
pub(crate) fn tick(
    quick_skip: Option<QuickSkipPhase>,
    now: Instant,
) -> (Vec<StageOp>, Option<QuickSkipPhase>) {
    match quick_skip {
        Some(QuickSkipPhase::Armed { deadline }) if now >= deadline => {
            (vec![StageOp::RepressPrimary], Some(QuickSkipPhase::Late))
        }
        other => (Vec::new(), other),
    }
}

/// **Handoff** (spec.md's default mode): crossing into the deep band
/// releases the primary and fires the deep stage; crossing back out
/// releases the deep and re-presses the primary.
fn handoff(prev: Bands, next: Bands) -> Vec<StageOp> {
    use Band::{Down, Up};
    match (prev, next) {
        ((Up, Up), (Up, Up)) | ((Down, Up), (Down, Up)) | ((Down, Down), (Down, Down)) => {
            vec![StageOp::Nothing]
        }
        ((Up, Up), (Down, Up)) => vec![StageOp::FirePrimary],
        ((Down, Up), (Down, Down)) => vec![StageOp::ReleasePrimary, StageOp::FireDeep],
        ((Down, Down), (Down, Up)) => vec![StageOp::ReleaseDeep, StageOp::RepressPrimary],
        ((Down, Up), (Up, Up)) => vec![StageOp::ReleasePrimary],
        ((Up, Up), (Down, Down)) => vec![
            StageOp::FirePrimary,
            StageOp::ReleasePrimary,
            StageOp::FireDeep,
        ],
        ((Down, Down), (Up, Up)) => vec![
            StageOp::ReleaseDeep,
            StageOp::RepressPrimary,
            StageOp::ReleasePrimary,
        ],
        _ => unreachable_transition(prev, next),
    }
}

/// **No-Return**: identical to Handoff except the down-direction release
/// never represses — the key stays quiet until fully released and pressed
/// again.
fn no_return(prev: Bands, next: Bands) -> Vec<StageOp> {
    use Band::{Down, Up};
    match (prev, next) {
        ((Down, Down), (Down, Up)) => vec![StageOp::ReleaseDeep],
        ((Down, Down), (Up, Up)) => vec![StageOp::ReleaseDeep, StageOp::ReleasePrimary],
        _ => handoff(prev, next),
    }
}

/// **Additive**: the primary is never touched by the inner crossings — only
/// `FireDeep`/`ReleaseDeep` at the inner transitions, real Primary Down/Up
/// at the outer edges.
fn additive(prev: Bands, next: Bands) -> Vec<StageOp> {
    use Band::{Down, Up};
    match (prev, next) {
        ((Up, Up), (Up, Up)) | ((Down, Up), (Down, Up)) | ((Down, Down), (Down, Down)) => {
            vec![StageOp::Nothing]
        }
        ((Up, Up), (Down, Up)) => vec![StageOp::FirePrimary],
        ((Down, Up), (Down, Down)) => vec![StageOp::FireDeep],
        ((Down, Down), (Down, Up)) => vec![StageOp::ReleaseDeep],
        ((Down, Up), (Up, Up)) => vec![StageOp::ReleasePrimary],
        ((Up, Up), (Down, Down)) => vec![StageOp::FirePrimary, StageOp::FireDeep],
        ((Down, Down), (Up, Up)) => vec![StageOp::ReleaseDeep, StageOp::ReleasePrimary],
        _ => unreachable_transition(prev, next),
    }
}

/// **Quick-Skip**: a per-press Armed→Skipped/Late runtime state layered on
/// Handoff's mechanics (spec.md "Quick-Skip" table). `quick_skip == None`
/// means "at rest between presses" — the only transition it accepts is a
/// fresh outer Down, which either arms the window or (a same-report or
/// already-arrived deep crossing) resolves synchronously to Skipped, per
/// the ADR's "this can't produce an ordering hazard" note: mechanical
/// replay of the 1-report-skip row already covers "already hot."
fn quick_skip_advance(
    prev: Bands,
    next: Bands,
    quick_skip: Option<QuickSkipPhase>,
) -> (Vec<StageOp>, Option<QuickSkipPhase>) {
    use Band::{Down, Up};
    match quick_skip {
        None => match (prev, next) {
            ((Up, Up), (Up, Up)) => (vec![StageOp::Nothing], None),
            ((Up, Up), (Down, Up)) => (
                Vec::new(),
                Some(QuickSkipPhase::Armed {
                    deadline: Instant::now() + QUICK_SKIP_WINDOW,
                }),
            ),
            ((Up, Up), (Down, Down)) => (
                vec![StageOp::SuppressPrimary, StageOp::FireDeep],
                Some(QuickSkipPhase::Skipped),
            ),
            _ => (unreachable_transition(prev, next), None),
        },
        Some(QuickSkipPhase::Armed { deadline }) => match (prev, next) {
            ((Down, Up), (Down, Up)) => (
                vec![StageOp::Nothing],
                Some(QuickSkipPhase::Armed { deadline }),
            ),
            ((Down, Up), (Down, Down)) => (
                vec![StageOp::SuppressPrimary, StageOp::FireDeep],
                Some(QuickSkipPhase::Skipped),
            ),
            ((Down, Up), (Up, Up)) => (Vec::new(), None),
            _ => (
                unreachable_transition(prev, next),
                Some(QuickSkipPhase::Armed { deadline }),
            ),
        },
        Some(QuickSkipPhase::Skipped) => match (prev, next) {
            ((Down, Up), (Down, Up)) | ((Down, Down), (Down, Down)) => {
                (vec![StageOp::Nothing], Some(QuickSkipPhase::Skipped))
            }
            ((Down, Up), (Down, Down)) => (vec![StageOp::FireDeep], Some(QuickSkipPhase::Skipped)),
            ((Down, Down), (Down, Up)) => {
                (vec![StageOp::ReleaseDeep], Some(QuickSkipPhase::Skipped))
            }
            ((Down, Up), (Up, Up)) => (Vec::new(), None),
            ((Down, Down), (Up, Up)) => (vec![StageOp::ReleaseDeep], None),
            _ => (
                unreachable_transition(prev, next),
                Some(QuickSkipPhase::Skipped),
            ),
        },
        Some(QuickSkipPhase::Late) => {
            let ops = handoff(prev, next);
            let phase = if next == (Up, Up) {
                None
            } else {
                Some(QuickSkipPhase::Late)
            };
            (ops, phase)
        }
    }
}

/// The disjoint-stacked-band invariant, asserted rather than defensively
/// handled (spec.md's own reasoning: `RepressPrimary` fires unconditionally,
/// never re-checking the primary's own hysteresis, because `(Up, Down)` can
/// never physically arise).
fn assert_reachable(bands: Bands) {
    debug_assert_ne!(
        bands,
        (Band::Up, Band::Down),
        "structurally impossible band state: deep can never be Down while primary is Up"
    );
}

/// A transition this Staging mode's table has no row for — reachable only if
/// `(Up, Down)` slipped past `assert_reachable` in a release build (assertions
/// compiled out) or another impossible jump was constructed by a caller bug.
/// Documented unreachable rather than defensively producing a guess.
fn unreachable_transition(prev: Bands, next: Bands) -> Vec<StageOp> {
    debug_assert!(false, "no table row for transition {prev:?} -> {next:?}");
    vec![StageOp::Nothing]
}

#[cfg(test)]
mod tests {
    use super::*;
    use Band::{Down, Up};

    // ── Handoff ──────────────────────────────────────────────────────────

    #[test]
    fn handoff_table() {
        let cases: &[(Bands, Bands, &[StageOp])] = &[
            ((Up, Up), (Up, Up), &[StageOp::Nothing]),
            ((Down, Up), (Down, Up), &[StageOp::Nothing]),
            ((Down, Down), (Down, Down), &[StageOp::Nothing]),
            ((Up, Up), (Down, Up), &[StageOp::FirePrimary]),
            (
                (Down, Up),
                (Down, Down),
                &[StageOp::ReleasePrimary, StageOp::FireDeep],
            ),
            (
                (Down, Down),
                (Down, Up),
                &[StageOp::ReleaseDeep, StageOp::RepressPrimary],
            ),
            ((Down, Up), (Up, Up), &[StageOp::ReleasePrimary]),
            (
                (Up, Up),
                (Down, Down),
                &[
                    StageOp::FirePrimary,
                    StageOp::ReleasePrimary,
                    StageOp::FireDeep,
                ],
            ),
            (
                (Down, Down),
                (Up, Up),
                &[
                    StageOp::ReleaseDeep,
                    StageOp::RepressPrimary,
                    StageOp::ReleasePrimary,
                ],
            ),
        ];
        for &(prev, next, expected) in cases {
            let (ops, phase) = advance(prev, next, StagingMode::Handoff, None);
            assert_eq!(ops, expected, "handoff {prev:?} -> {next:?}");
            assert_eq!(phase, None);
        }
    }

    // ── No-Return ────────────────────────────────────────────────────────

    #[test]
    fn no_return_table() {
        let cases: &[(Bands, Bands, &[StageOp])] = &[
            // Rows shared with Handoff.
            ((Up, Up), (Up, Up), &[StageOp::Nothing]),
            ((Up, Up), (Down, Up), &[StageOp::FirePrimary]),
            (
                (Down, Up),
                (Down, Down),
                &[StageOp::ReleasePrimary, StageOp::FireDeep],
            ),
            ((Down, Up), (Up, Up), &[StageOp::ReleasePrimary]),
            (
                (Up, Up),
                (Down, Down),
                &[
                    StageOp::FirePrimary,
                    StageOp::ReleasePrimary,
                    StageOp::FireDeep,
                ],
            ),
            // No-Return's own two rows: no repress.
            ((Down, Down), (Down, Up), &[StageOp::ReleaseDeep]),
            (
                (Down, Down),
                (Up, Up),
                &[StageOp::ReleaseDeep, StageOp::ReleasePrimary],
            ),
        ];
        for &(prev, next, expected) in cases {
            let (ops, phase) = advance(prev, next, StagingMode::NoReturn, None);
            assert_eq!(ops, expected, "no_return {prev:?} -> {next:?}");
            assert_eq!(phase, None);
        }
    }

    // ── Additive ─────────────────────────────────────────────────────────

    #[test]
    fn additive_table() {
        let cases: &[(Bands, Bands, &[StageOp])] = &[
            ((Up, Up), (Up, Up), &[StageOp::Nothing]),
            ((Down, Up), (Down, Up), &[StageOp::Nothing]),
            ((Down, Down), (Down, Down), &[StageOp::Nothing]),
            ((Up, Up), (Down, Up), &[StageOp::FirePrimary]),
            ((Down, Up), (Down, Down), &[StageOp::FireDeep]),
            ((Down, Down), (Down, Up), &[StageOp::ReleaseDeep]),
            ((Down, Up), (Up, Up), &[StageOp::ReleasePrimary]),
            (
                (Up, Up),
                (Down, Down),
                &[StageOp::FirePrimary, StageOp::FireDeep],
            ),
            (
                (Down, Down),
                (Up, Up),
                &[StageOp::ReleaseDeep, StageOp::ReleasePrimary],
            ),
        ];
        for &(prev, next, expected) in cases {
            let (ops, phase) = advance(prev, next, StagingMode::Additive, None);
            assert_eq!(ops, expected, "additive {prev:?} -> {next:?}");
            assert_eq!(phase, None);
        }
    }

    // ── Quick-Skip ───────────────────────────────────────────────────────

    #[test]
    fn quick_skip_arms_the_window_on_a_fresh_primary_down() {
        let (ops, phase) = advance((Up, Up), (Down, Up), StagingMode::QuickSkip, None);
        assert!(ops.is_empty(), "the real Down is buffered, not fired");
        assert!(matches!(phase, Some(QuickSkipPhase::Armed { .. })));
        assert!(next_deadline(phase).is_some());
    }

    #[test]
    fn quick_skip_resolves_synchronously_when_already_hot() {
        // A same-report double-crossing straight from (Up,Up): the ADR's
        // "resolves immediately to skip if the deep band is already hot"
        // path is just mechanical replay of the 1-report-skip row.
        let (ops, phase) = advance((Up, Up), (Down, Down), StagingMode::QuickSkip, None);
        assert_eq!(ops, vec![StageOp::SuppressPrimary, StageOp::FireDeep]);
        assert_eq!(phase, Some(QuickSkipPhase::Skipped));
    }

    #[test]
    fn quick_skip_waits_quietly_while_armed_and_still_shallow() {
        let armed = Some(QuickSkipPhase::Armed {
            deadline: Instant::now() + QUICK_SKIP_WINDOW,
        });
        let (ops, phase) = advance((Down, Up), (Down, Up), StagingMode::QuickSkip, armed);
        assert_eq!(ops, vec![StageOp::Nothing]);
        assert_eq!(phase, armed);
    }

    #[test]
    fn quick_skip_becomes_skipped_when_the_deep_band_is_reached_within_the_window() {
        let armed = Some(QuickSkipPhase::Armed {
            deadline: Instant::now() + QUICK_SKIP_WINDOW,
        });
        let (ops, phase) = advance((Down, Up), (Down, Down), StagingMode::QuickSkip, armed);
        assert_eq!(ops, vec![StageOp::SuppressPrimary, StageOp::FireDeep]);
        assert_eq!(phase, Some(QuickSkipPhase::Skipped));
    }

    #[test]
    fn quick_skip_cancels_outright_on_an_early_up() {
        let armed = Some(QuickSkipPhase::Armed {
            deadline: Instant::now() + QUICK_SKIP_WINDOW,
        });
        let (ops, phase) = advance((Down, Up), (Up, Up), StagingMode::QuickSkip, armed);
        assert!(ops.is_empty(), "buffered Down dropped, nothing emitted");
        assert_eq!(phase, None);
    }

    #[test]
    fn quick_skip_tick_before_the_deadline_is_a_no_op() {
        let deadline = Instant::now() + QUICK_SKIP_WINDOW;
        let armed = Some(QuickSkipPhase::Armed { deadline });
        let (ops, phase) = tick(armed, deadline - Duration::from_millis(1));
        assert!(ops.is_empty());
        assert_eq!(phase, armed);
    }

    #[test]
    fn quick_skip_tick_after_the_deadline_represses_and_goes_late() {
        let deadline = Instant::now() + QUICK_SKIP_WINDOW;
        let armed = Some(QuickSkipPhase::Armed { deadline });
        let (ops, phase) = tick(armed, deadline + Duration::from_millis(1));
        assert_eq!(ops, vec![StageOp::RepressPrimary]);
        assert_eq!(phase, Some(QuickSkipPhase::Late));
    }

    #[test]
    fn quick_skip_tick_with_no_armed_press_is_a_no_op() {
        let (ops, phase) = tick(None, Instant::now());
        assert!(ops.is_empty());
        assert_eq!(phase, None);
        assert_eq!(next_deadline(None), None);
    }

    #[test]
    fn quick_skip_late_runs_the_rest_of_the_press_as_plain_handoff() {
        let late = Some(QuickSkipPhase::Late);
        let (ops, phase) = advance((Down, Up), (Down, Down), StagingMode::QuickSkip, late);
        assert_eq!(ops, vec![StageOp::ReleasePrimary, StageOp::FireDeep]);
        assert_eq!(phase, Some(QuickSkipPhase::Late));

        let (ops, phase) = advance((Down, Down), (Down, Up), StagingMode::QuickSkip, late);
        assert_eq!(ops, vec![StageOp::ReleaseDeep, StageOp::RepressPrimary]);
        assert_eq!(phase, Some(QuickSkipPhase::Late));

        // The rest of the press ends normally — Late's tracking clears once
        // the key is fully released, ready for a fresh press to Arm again.
        let (ops, phase) = advance((Down, Up), (Up, Up), StagingMode::QuickSkip, late);
        assert_eq!(ops, vec![StageOp::ReleasePrimary]);
        assert_eq!(phase, None);
    }

    #[test]
    fn quick_skip_skipped_runs_additive_with_no_primary_release_path_no_return() {
        let skipped = Some(QuickSkipPhase::Skipped);

        // Every dip into/out of the deep band: deep-only ops, primary
        // untouched (it never fired).
        let (ops, phase) = advance((Down, Up), (Down, Down), StagingMode::QuickSkip, skipped);
        assert_eq!(ops, vec![StageOp::FireDeep]);
        assert_eq!(phase, skipped);
        let (ops, phase) = advance((Down, Down), (Down, Up), StagingMode::QuickSkip, skipped);
        assert_eq!(ops, vec![StageOp::ReleaseDeep]);
        assert_eq!(phase, skipped);

        // Final release from shallow: primary permanently inert — no
        // ReleasePrimary at all, unlike No-Return's own equivalent row.
        let (ops, phase) = advance((Down, Up), (Up, Up), StagingMode::QuickSkip, skipped);
        assert!(ops.is_empty());
        assert_eq!(phase, None);

        // Final release via a 1-report skip straight from the deep band:
        // ReleaseDeep only, no repress (No-Return's release shape) and no
        // ReleasePrimary (Skipped's own primary-inert rule).
        let (ops, phase) = advance((Down, Down), (Up, Up), StagingMode::QuickSkip, skipped);
        assert_eq!(ops, vec![StageOp::ReleaseDeep]);
        assert_eq!(phase, None);
    }

    #[test]
    fn quick_skip_state_does_not_leak_across_a_cancelled_press() {
        // Models ticket 03's `stage::Engine::stop_all()` external-cancel
        // path (a Layer/Profile switch or capture-mode flip while Armed):
        // the Engine simply drops the per-key phase back to `None`. A fresh
        // press afterward arms cleanly, exactly as if the key had never
        // been touched.
        let cancelled: Option<QuickSkipPhase> = None;
        let (ops, phase) = advance((Up, Up), (Down, Up), StagingMode::QuickSkip, cancelled);
        assert!(ops.is_empty());
        assert!(matches!(phase, Some(QuickSkipPhase::Armed { .. })));
    }

    // ── Disjoint-stacked-band invariant ─────────────────────────────────

    #[test]
    #[should_panic(expected = "structurally impossible band state")]
    fn up_down_is_asserted_unreachable() {
        advance((Up, Down), (Up, Up), StagingMode::Handoff, None);
    }
}
