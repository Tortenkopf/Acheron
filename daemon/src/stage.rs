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
//! Ticket 03 adds the non-pure `Engine` further down — the third depth
//! engine on `DispatchState`, alongside `axis::Engine`/`analog_repeat::
//! Engine` — which owns the per-key deep/shadow-primary `KeyState`, the deep
//! stage's own `Slots<StageKey>`, and drives the pure core above off the live
//! `rx_depth` stream. The pure core itself (everything above `Engine`) keeps
//! the same discipline it always has: no tokio, no injector, no `Config`,
//! importing only the lightweight `StagingMode` config type, the same way
//! `chord.rs` imports `Binding`/`ChordKey`/`TriggerMode` from `config`.
//! `Engine` is deliberately exempt from that restriction — same shape as
//! `analog_repeat.rs`, whose file-level pure/non-pure split doesn't stop its
//! own `Engine` from importing `Injector`/`config::Action`.
//!
//! Source of truth: `.scratch/tartarus-dual-stage-keys/spec.md`
//! §"Staging-mode state machine" and §"Pipeline architecture", and
//! `docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md`.
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

use std::collections::HashMap;
use std::hash::Hash;
use std::io;
use std::time::Duration;

use tokio::time::Instant;

use crate::capture::EventState;
use crate::capture::analog::{self, KeyState};
use crate::config::{Action, Binding, Config, Layer, StagingMode};
use crate::edit::Edit;
use crate::injector::Injector;
use crate::input::Input;
use crate::stepper;
use crate::trigger::{self, PerformDeps, Slots};

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
/// currently Armed — mirroring `chord::next_deadline`'s exact shape.
/// `Engine::next_deadline` (ticket 04) calls this once per tracked key and
/// takes the earliest, since — unlike Chord's single global window — every
/// Quick-Skip key arms its own independent deadline.
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
/// (`RepressPrimary`, performed via `dispatch_individual_down`'s exact logic
/// by `Engine::tick`, ticket 04) and the key flips to Late — plain Handoff
/// for the rest of the press. `now` guards a spurious call before the
/// deadline, same as `chord::tick`.
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

// ─────────────────────────────────────────────────────────────────────────
// `Engine` — ticket 03's non-pure dispatch-side shell, completed by ticket
// 04's Quick-Skip primary-suppression buffer (`begin_quick_skip`/`tick`
// below).
// ─────────────────────────────────────────────────────────────────────────

/// One key's combined runtime state the `Engine` tracks across depth ticks:
/// a shadow primary-band `KeyState` (fed by the same raw `rx_depth` stream
/// as the deep band, never the primary's own `PhysicalEvent` — ADR-0007),
/// the deep-band `KeyState`, and Quick-Skip's own per-press phase (`None` for
/// every mode but `QuickSkip`, and for a `QuickSkip` key at rest between
/// presses — ticket 04's `begin_quick_skip`/`tick` are the only writers of a
/// `Some` value here). `Default` is a fresh key that has never crossed
/// either band — equivalent to `(Band::Up, Band::Up)`.
#[derive(Debug, Clone, Copy, Default)]
struct KeyRuntime {
    primary: KeyState,
    deep: KeyState,
    quick_skip: Option<QuickSkipPhase>,
    /// Set by `Engine::stop_all()` for every key it resets — the *next*
    /// `Engine::update` tick for that key silently re-adopts whatever bands
    /// Depth currently reads, with no emitted ops, rather than treating a
    /// Depth that never moved as `(Up, Up)` and mechanically replaying a
    /// spurious full press through it the instant a Layer/Profile switch
    /// completes (code-review finding, ticket 03). A key with no
    /// `KeyRuntime` yet at all (this flag's default, via `or_default`) is
    /// genuinely new — a fast double-crossing landing on a key's very first
    /// observed tick is a real, tested scenario (the mechanical-replay
    /// tables' own `(Up, Up)` rows), not a reset artifact, so it still gets
    /// `advance`'s ordinary full-replay treatment.
    just_reset: bool,
    /// The stage machine currently holds this key's primary *released* — a
    /// `ReleasePrimary` op not yet followed by `FirePrimary`/`RepressPrimary`
    /// (Handoff/No-Return have handed the press to the deep stage). The
    /// primary band is still physically Down through that hand-off, so
    /// `capture::analog` keeps synthesizing `Repeat`s for it — `handle_event`
    /// queries `Engine::primary_handed_off` to swallow them, so a
    /// Hold-to-repeat primary genuinely stops rather than machine-gunning
    /// under the deep stage. Cleared the moment the primary band itself
    /// crosses back Up (the press is over). Additive never emits an inner
    /// `ReleasePrimary`, so its primary keeps repeating for free.
    primary_handed_off: bool,
}

/// `dispatch_individual_down`'s exact Down-side logic — get, short-circuit
/// `Action::ProfileSwitch` into a returned `Edit` (`decide` documents that
/// `ProfileSwitch` never reaches it), else `decide` + `perform` — generalized
/// over the slot keyspace (`K = Input` for the primary, `K = StageKey` for
/// the deep stage) so `Engine::update` applies it identically to
/// `FirePrimary`/`RepressPrimary` (against `individual`) and `FireDeep`
/// (against `self.slots`). See `Engine::update`'s own doc for why this lives
/// here rather than deferring to `DispatchState`, unlike the Chord path.
async fn fire<K: Eq + Hash + Clone>(
    slots: &mut Slots<K>,
    key: K,
    binding: &Binding,
    injector: &Injector,
    config: &Config,
    cursors: &mut stepper::Cursors,
    toggle_lap_target: Duration,
) -> io::Result<Option<Edit>> {
    if let Action::ProfileSwitch { target } = &binding.action {
        return Ok(Some(Edit::SwitchProfile {
            name: target.clone(),
        }));
    }
    let slot = slots.slot(&key);
    let decision = trigger::decide(binding, EventState::Down, slot);
    let deps = PerformDeps::new(injector, config, cursors, toggle_lap_target);
    slots.perform(decision, key, binding, deps).await?;
    Ok(None)
}

/// `capture::analog::KeyState` <-> this module's local `Band` — the two are
/// isomorphic (both a bare Up/Down hysteresis state); this is the one seam
/// where `Engine` bridges capture's own hysteresis primitive into the pure
/// core's vocabulary.
fn to_band(state: KeyState) -> Band {
    match state {
        KeyState::Up => Band::Up,
        KeyState::Down => Band::Down,
    }
}

/// The non-pure third depth engine on `DispatchState`, alongside `axis::
/// Engine`/`analog_repeat::Engine` (spec.md "Pipeline architecture"). Owns
/// every dual-stage key's runtime band tracking plus the deep stage's own
/// `trigger::Slots<StageKey>` — Additive can hold both stages live at once,
/// each a fully independent Binding, so this is never shared with
/// `DispatchState::individual`.
#[derive(Default)]
pub(crate) struct Engine {
    runtime: HashMap<Input, KeyRuntime>,
    slots: Slots<StageKey>,
}

/// `Engine::update`'s scattered dependencies, bundled into one parameter to
/// stay under clippy's `too_many_arguments` — the same reason `trigger::
/// PerformDeps` bundles `Slots::perform`'s own. Built at the one call site
/// (`DispatchState::update_stages`) from `&Config` plus four disjoint
/// `DispatchState` field borrows.
pub(crate) struct EngineDeps<'a> {
    pub config: &'a Config,
    pub active_layer: Layer,
    pub individual: &'a mut Slots<Input>,
    pub injector: &'a Injector,
    pub cursors: &'a mut stepper::Cursors,
    pub toggle_lap_target: Duration,
}

impl Engine {
    /// Runs from the existing `rx_depth.changed()` `select!` arm in `run`,
    /// third after `handle_depth_update`/`update_analog_repeats` (dispatch.rs).
    /// For every `Input` the active Profile configures a deep stage on (and
    /// only those already carrying an active-Layer deep Binding — a
    /// `deep_stages` entry with no matching `deep_base`/`deep_held` entry is
    /// legally inert, per spec.md's "Config schema"), feeds `depth` through
    /// the same `capture::analog::observe` capture uses for the primary band
    /// to advance both this key's shadow-primary and deep `KeyState`, then
    /// `advance`s the pure core on any resulting `Bands` transition.
    ///
    /// Every op in a transition's emitted sequence is performed **in order,
    /// fully awaited before the next**, in this one loop body — `FireDeep`/
    /// `ReleaseDeep` against this `Engine`'s own `Slots<StageKey>`,
    /// `ReleasePrimary` against `individual` (`DispatchState::individual`,
    /// borrowed in) via `force_release`/`stop_toggle` (unconditional, which
    /// is exactly what lets `RepressPrimary` on a Toggle start a genuinely
    /// *fresh* loop later, per spec.md's own note), and `FirePrimary`/
    /// `RepressPrimary` also against `individual`, via the `fire` helper
    /// below. Keeping every op — deep and primary alike — inside one
    /// sequential loop (rather than deferring the primary-keyspace ops to
    /// the caller, as an earlier draft did) matters: the 1-report-skip rows
    /// interleave `ReleaseDeep`/`FireDeep` between `RepressPrimary` and a
    /// trailing `ReleasePrimary` (e.g. `[ReleaseDeep, RepressPrimary,
    /// ReleasePrimary]`), and deferring the primary ops to run *after* this
    /// method returns would let that trailing `ReleasePrimary` race ahead of
    /// the `RepressPrimary` it's meant to immediately follow.
    ///
    /// `fire` (both `FireDeep` and `FirePrimary`/`RepressPrimary`) short-
    /// circuits an `Action::ProfileSwitch` Binding into a returned `Edit`
    /// instead of reaching `decide` — which documents that `ProfileSwitch`
    /// never reaches it — mirroring `dispatch_individual_down`'s own
    /// interception. Unlike the Chord path (whose own Action can never be
    /// `ProfileSwitch`, enforced by `config::validate`), a dual-stage key's
    /// deep — and, via `RepressPrimary`, primary — Binding has no such
    /// restriction (spec.md: "carries any Action a Binding can carry today,
    /// including firing a Profile switch from the deep stage"), so `Engine`
    /// genuinely needs this awareness itself rather than deferring to
    /// `DispatchState`, unlike `chord.rs`/`analog_repeat.rs`, which never
    /// import `edit::Edit` at all.
    ///
    /// **Ordering / the same-report double-crossing race (ADR-0007):** a
    /// transition whose emitted sequence is *only* `[FirePrimary]` or only
    /// `[ReleasePrimary]` — the ordinary, non-crossing outer edge of a press
    /// — is left alone entirely: the always-present, unmodified `rx_events`
    /// primary edge already fires/releases it, and *actively* performing it
    /// here too would (for a Toggle primary) incorrectly force-stop a loop
    /// that ordinary Toggle semantics leave running past a bare release.
    /// A same-report double-crossing's sequence is longer than one op
    /// (`[FirePrimary, ReleasePrimary, FireDeep]` and its mirror), and *is*
    /// walked in full here, synchronously, rather than left to race the
    /// separately-arriving real primary edge under `tokio::select!`'s
    /// unordered tie-break between the `rx_events` and `rx_depth.changed()`
    /// arms — closing the ordering race spec.md flags ticket 01's original
    /// design hadn't fully covered. Accepted residual gap, narrow enough
    /// that it isn't specially engineered around (same class as ticket 39's
    /// own accepted gap): if the real primary edge for that same crossing
    /// arrives *after* this synchronous handling, a Toggle primary can pick
    /// up a second, unwanted loop. Reachable by a single hidraw report
    /// jumping from fully released past the deep Actuation point in one
    /// sample — and, since `rx_depth` is a coalescing `watch` channel (the
    /// latest snapshot only, never a queue) while `rx_events` is a
    /// non-lossy `mpsc`, also by *separate* reports whose depth snapshots
    /// happen to coalesce into one `rx_depth.changed()` tick because
    /// dispatch's `select!` loop was busy handling something else across
    /// them — the same coalescing-under-load characteristic every
    /// `rx_depth` consumer in this file already has (Axis resolution,
    /// Analog-repeat's rate curve), not something specific to this row.
    pub(crate) async fn update(
        &mut self,
        deps: EngineDeps<'_>,
        snapshot: &HashMap<Input, u8>,
    ) -> io::Result<Vec<Edit>> {
        let EngineDeps {
            config,
            active_layer,
            individual,
            injector,
            cursors,
            toggle_lap_target,
        } = deps;
        let profile = config
            .active_profile()
            .expect("load_or_seed validates active_profile names a real profile");
        let mut edits = Vec::new();
        for (&input, deep_cfg) in &profile.deep_stages {
            // Existence only, no clone yet (code-review finding: the actual
            // `Binding` — cloned below, only once a real transition is
            // confirmed — carries a heap-allocated `String` for
            // `Action::ProfileSwitch`, and this loop runs on the same
            // sub-millisecond hot per-report path `config::
            // resolved_actuation_point` was split out to avoid a redundant
            // rebuild on).
            if !profile.deep_layer(active_layer).contains_key(&input) {
                continue;
            }
            let Some(&depth) = snapshot.get(&input) else {
                continue;
            };
            let primary_point = profile.resolved_actuation_point(input);
            let rt = self.runtime.entry(input).or_default();
            let (new_primary, _) = analog::observe(rt.primary, depth, primary_point);
            let (new_deep, _) = analog::observe(rt.deep, depth, deep_cfg.actuation);
            if rt.just_reset {
                rt.primary = new_primary;
                rt.deep = new_deep;
                rt.just_reset = false;
                continue;
            }
            let prev = (to_band(rt.primary), to_band(rt.deep));
            let next = (to_band(new_primary), to_band(new_deep));
            rt.primary = new_primary;
            rt.deep = new_deep;
            if prev == next {
                continue;
            }
            if deep_cfg.mode == StagingMode::QuickSkip && rt.quick_skip.is_none() {
                // Ticket 04: a Quick-Skip key's outer Up->Down edge is owned
                // entirely by `begin_quick_skip` — armed directly off the
                // real primary `Down` event (`rx_events`), not this
                // coalescing `rx_depth` tick, so the ~50ms window starts at
                // the physically precise moment and the same-report
                // double-crossing ordinarily resolves synchronously from the
                // event's own depth field (see `begin_quick_skip`'s doc). No
                // op is ever decided here for this edge — the same way the
                // other three modes leave their own lone `FirePrimary`/
                // `ReleasePrimary` transitions to the real event path below
                // rather than double-performing them — but shadow-band
                // tracking above still stays live regardless, unconditionally
                // (`rt.primary`/`rt.deep` were just written above), precisely
                // so `begin_quick_skip` can detect when *this* loop has
                // raced ahead of that still-queued `Down` event (`rx_events`
                // vs. the coalescing `rx_depth` watch can reorder under
                // load) and defer to `rt.deep` instead of the event's own,
                // by-then-stale depth reading.
                continue;
            }
            let (ops, quick_skip) = advance(prev, next, deep_cfg.mode, rt.quick_skip);
            rt.quick_skip = quick_skip;
            // Track the primary hand-off across every op this tick emits
            // (last write wins), then clear it whenever the primary band
            // itself went Up. See `KeyRuntime::primary_handed_off`.
            for op in &ops {
                match op {
                    StageOp::ReleasePrimary => rt.primary_handed_off = true,
                    StageOp::FirePrimary | StageOp::RepressPrimary => {
                        rt.primary_handed_off = false;
                    }
                    _ => {}
                }
            }
            if new_primary == KeyState::Up {
                rt.primary_handed_off = false;
            }
            if ops.len() == 1 && matches!(ops[0], StageOp::FirePrimary | StageOp::ReleasePrimary) {
                continue;
            }
            // A deep Binding on this Layer is confirmed to exist (checked
            // above); a real transition just occurred — now worth the clone.
            let deep_binding = profile
                .deep_layer(active_layer)
                .get(&input)
                .cloned()
                .expect("checked present above");
            for op in ops {
                match op {
                    StageOp::Nothing => {}
                    StageOp::FirePrimary | StageOp::RepressPrimary => {
                        // A primary must exist for a deep stage to exist at
                        // all (`ConfigError::DeepStageWithoutPrimary`), so
                        // this is always `Some` post-`validate`; the `else`
                        // is defensive, not a reachable production path.
                        let Some(primary_binding) =
                            profile.layer(active_layer).get(&input).cloned()
                        else {
                            continue;
                        };
                        if let Some(edit) = fire(
                            individual,
                            input,
                            &primary_binding,
                            injector,
                            config,
                            cursors,
                            toggle_lap_target,
                        )
                        .await?
                        {
                            edits.push(edit);
                        }
                    }
                    StageOp::ReleasePrimary => {
                        individual.stop_toggle(&input).await;
                        individual.force_release(&input, injector).await;
                    }
                    StageOp::FireDeep => {
                        if let Some(edit) = fire(
                            &mut self.slots,
                            StageKey(input),
                            &deep_binding,
                            injector,
                            config,
                            cursors,
                            toggle_lap_target,
                        )
                        .await?
                        {
                            edits.push(edit);
                        }
                    }
                    // Unconditional, exactly mirroring `ReleasePrimary` above
                    // (code-review finding: the ordinary `decide(Up)` +
                    // `perform` path this used to take is a no-op for a
                    // Toggle-mode deep Binding — `decide`'s `(Toggle, Up)`
                    // arm falls to `Nothing`, matching a real physical Up's
                    // own deliberate "Toggle outlives release" rule — so a
                    // deep Toggle never stopped on `ReleaseDeep`, and the
                    // next `FireDeep` unconditionally `insert`ed a second,
                    // orphaned loop over it via `decide`'s own Toggle+Down
                    // arm, which never checks `slot`). A stage's own release
                    // must always fully clear the slot so the following
                    // `FireDeep` genuinely starts fresh, the same reasoning
                    // that makes `ReleasePrimary` unconditional.
                    StageOp::ReleaseDeep => {
                        let key = StageKey(input);
                        self.slots.stop_toggle(&key).await;
                        self.slots.force_release(&key, injector).await;
                    }
                    // Ticket 04: the buffered primary `Down` is dropped for
                    // good — genuinely a no-op here, since `begin_quick_skip`
                    // swallowed it before it ever reached `individual` at
                    // all (unlike `ReleasePrimary`, there is nothing live to
                    // force-release).
                    StageOp::SuppressPrimary => {}
                }
            }
        }
        Ok(edits)
    }

    /// The dispatch-side half of Quick-Skip's primary-suppression buffer
    /// (`tartarus-dual-stage-keys` ticket 04) — `handle_event`'s divert calls
    /// this on a fresh Quick-Skip primary `Down`, off the real `rx_events`
    /// edge rather than waiting for `update`'s own coalescing `rx_depth`
    /// tick, so the ~50ms window is armed at the exact moment the primary
    /// physically crosses. `depth` is that same `Down` `PhysicalEvent`'s own
    /// raw depth reading, ordinarily enough on its own to resolve whether the
    /// deep band is already hot (ADR-0007's synchronous-resolution trick,
    /// the same one `advance`'s mechanical-replay tables rely on for a
    /// same-report double-crossing) — this is the only place this key's
    /// outer Up->Down edge is ever *decided* (`update`'s own `quick_skip.
    /// is_none()` bypass defers to it entirely). But `depth` can still be
    /// *stale* by the time this runs: `rx_depth` is a coalescing `watch`
    /// while `rx_events` is a non-lossy `mpsc`, so under load `update`'s own
    /// shadow-band tracking can race ahead and observe a later, larger depth
    /// sample first (code-review finding on this ticket) — handled below by
    /// deferring to `rt.deep` whenever that's happened, rather than trusting
    /// `depth` unconditionally.
    pub(crate) async fn begin_quick_skip(
        &mut self,
        deps: EngineDeps<'_>,
        input: Input,
        depth: u8,
    ) -> io::Result<Vec<Edit>> {
        let EngineDeps {
            config,
            active_layer,
            injector,
            cursors,
            toggle_lap_target,
            ..
        } = deps;
        let profile = config
            .active_profile()
            .expect("load_or_seed validates active_profile names a real profile");
        let deep_cfg = profile
            .deep_stages
            .get(&input)
            .copied()
            .expect("handle_event's divert only calls this for a configured Quick-Skip key");
        let rt = self.runtime.entry(input).or_default();
        // Structurally the key was at rest (`(Up, Up)`) the instant before
        // this real primary `Down` — *unless* `update`'s own `rx_depth`-
        // driven shadow tracking has already raced ahead of this `rx_events`
        // message and observed a later, larger depth sample first (`rx_depth`
        // is a coalescing `watch`, `rx_events` a non-lossy `mpsc` — the two
        // can reorder under load, code-review finding on this ticket).
        // `update`'s own `quick_skip.is_none()` bypass still writes the
        // shadow bands unconditionally even while it declines to decide an
        // op, so `rt.primary` already reading `Down` here is exactly that
        // signal: trust its already-tracked `rt.deep` (strictly more recent
        // than this event's own, now-stale `depth` field) rather than
        // recomputing from scratch, which would otherwise roll a genuine
        // deep-band crossing back to `Up` and wrongly Arm instead of
        // resolving Skipped.
        let deep_state = if rt.primary == KeyState::Down {
            rt.deep
        } else {
            analog::observe(KeyState::Up, depth, deep_cfg.actuation).0
        };
        rt.primary = KeyState::Down;
        rt.deep = deep_state;
        let next = (Band::Down, to_band(deep_state));
        let (ops, quick_skip) = advance((Band::Up, Band::Up), next, StagingMode::QuickSkip, None);
        rt.quick_skip = quick_skip;
        let mut edits = Vec::new();
        for op in ops {
            match op {
                // The buffered Down is dropped for good — nothing to
                // release, since it was never given to `individual`.
                StageOp::SuppressPrimary => {}
                StageOp::FireDeep => {
                    let deep_binding = profile
                        .deep_layer(active_layer)
                        .get(&input)
                        .cloned()
                        .expect("handle_event's divert only calls this for a live deep stage");
                    if let Some(edit) = fire(
                        &mut self.slots,
                        StageKey(input),
                        &deep_binding,
                        injector,
                        config,
                        cursors,
                        toggle_lap_target,
                    )
                    .await?
                    {
                        edits.push(edit);
                    }
                }
                StageOp::Nothing
                | StageOp::FirePrimary
                | StageOp::RepressPrimary
                | StageOp::ReleasePrimary
                | StageOp::ReleaseDeep => debug_assert!(
                    false,
                    "quick_skip_advance's `None`-phase branch only ever emits an \
                     empty Vec (Armed) or [SuppressPrimary, FireDeep] (already hot), \
                     got {op:?}"
                ),
            }
        }
        Ok(edits)
    }

    /// Whether `input`'s Quick-Skip runtime state is currently `Late` — the
    /// deadline already fired the primary retroactively, so this press now
    /// runs as ordinary Handoff. `handle_event`'s divert queries this to
    /// decide whether a real `Up`/`Repeat` should keep being swallowed (still
    /// `Armed`/`Skipped`/`None`) or must instead reach the ordinary
    /// individual-dispatch path below it, exactly like an ordinary Handoff
    /// key's own real events would.
    pub(crate) fn is_late(&self, input: Input) -> bool {
        matches!(
            self.runtime.get(&input).and_then(|rt| rt.quick_skip),
            Some(QuickSkipPhase::Late)
        )
    }

    /// Whether the stage machine currently holds `input`'s primary released
    /// (a Handoff/No-Return hand-off to the deep stage). `handle_event`
    /// swallows the primary's capture-synthesized `Repeat`s while this is
    /// set — the primary band is still physically Down, so they keep
    /// arriving, but a Hold-to-repeat primary must stay silent under the
    /// deep stage rather than machine-gun. See `KeyRuntime::primary_handed_off`.
    pub(crate) fn primary_handed_off(&self, input: Input) -> bool {
        self.runtime
            .get(&input)
            .is_some_and(|rt| rt.primary_handed_off)
    }

    /// The earliest still-armed Quick-Skip deadline across every tracked
    /// key, or `None` if none is currently `Armed` — the `run` loop's fourth
    /// `select!` arm (`wait_for_stage_deadline`, ticket 04) arms on this.
    /// Unlike Chord's single global window, every Quick-Skip key arms its
    /// own independent deadline, so this takes the minimum rather than
    /// reading one shared value.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.runtime
            .values()
            .filter_map(|rt| next_deadline(rt.quick_skip))
            .min()
    }

    /// Fires every Quick-Skip key whose deadline has elapsed by `now`
    /// (ticket 04) — the `wait_for_stage_deadline` `select!` arm's handler.
    /// A spurious call before any deadline has actually elapsed (or after
    /// `stop_all`/an early `Up` already cancelled it) touches nothing, the
    /// same tolerance `chord::tick` extends. Collects the elapsed keys
    /// first, then mutates `self.runtime` per key, to avoid borrowing it
    /// both immutably (the scan) and mutably (the update) at once.
    pub(crate) async fn tick(
        &mut self,
        deps: EngineDeps<'_>,
        now: Instant,
    ) -> io::Result<Vec<Edit>> {
        let EngineDeps {
            config,
            active_layer,
            individual,
            injector,
            cursors,
            toggle_lap_target,
        } = deps;
        let profile = config
            .active_profile()
            .expect("load_or_seed validates active_profile names a real profile");
        let elapsed: Vec<Input> = self
            .runtime
            .iter()
            .filter_map(|(&input, rt)| match rt.quick_skip {
                Some(QuickSkipPhase::Armed { deadline }) if now >= deadline => Some(input),
                _ => None,
            })
            .collect();
        let mut edits = Vec::new();
        for input in elapsed {
            let rt = self
                .runtime
                .get_mut(&input)
                .expect("just collected this key from the same map");
            let (ops, phase) = tick(rt.quick_skip, now);
            rt.quick_skip = phase;
            for op in ops {
                match op {
                    StageOp::RepressPrimary => {
                        // A primary must exist for a deep stage to exist at
                        // all (`ConfigError::DeepStageWithoutPrimary`); the
                        // `else` is defensive, not a reachable production
                        // path.
                        let Some(primary_binding) =
                            profile.layer(active_layer).get(&input).cloned()
                        else {
                            continue;
                        };
                        if let Some(edit) = fire(
                            individual,
                            input,
                            &primary_binding,
                            injector,
                            config,
                            cursors,
                            toggle_lap_target,
                        )
                        .await?
                        {
                            edits.push(edit);
                        }
                    }
                    other => debug_assert!(
                        false,
                        "the pure `tick` only ever emits [RepressPrimary] or an \
                         empty Vec, got {other:?}"
                    ),
                }
            }
        }
        Ok(edits)
    }

    /// Force-releases every live deep firing/Toggle and resets every per-key
    /// `KeyState`/Quick-Skip runtime state — wired into the same call sites
    /// as `analog_repeat::Engine::stop_all()` (`handle_layer_switch`,
    /// `handle_capture_mode_change`'s Digital-transition branch). The other
    /// Layer's stage for the same Input, if any, never picks up mid-Depth:
    /// `Engine::update`'s own "freshly reset" handling silently re-adopts
    /// wherever Depth currently sits (even mid-band) on the very next tick,
    /// with no ops emitted for it — not a synthetic full mechanical replay
    /// through whatever bands sit between `(Up, Up)` and there, which would
    /// otherwise misfire the instant a Layer/Profile switch completes for a
    /// key that never physically moved.
    pub(crate) async fn stop_all(&mut self, injector: &Injector) {
        self.slots.stop_all(injector).await;
        self.slots = Slots::default();
        for rt in self.runtime.values_mut() {
            *rt = KeyRuntime {
                just_reset: true,
                ..KeyRuntime::default()
            };
        }
    }

    /// Force-releases one key's live deep firing/Toggle and drops its
    /// runtime tracking entirely — `tartarus-dual-stage-keys` ticket 06's
    /// `Effect::StopStage`, wired into `run_effects` for the cascade-delete
    /// case (`edit::plan`'s `SetBinding`/`ClearBinding` arms, when the edit
    /// orphans a live `deep_base`/`deep_held` entry). Scoped to `input`
    /// alone, unlike `stop_all`'s whole-`Engine` sweep — every other key's
    /// tracking is untouched. Runs **immediately** on commit rather than
    /// waiting for `input`'s next Up: nothing guarantees one ever arrives
    /// once the deep Binding backing it is gone from `Config`. The runtime
    /// entry is removed outright (not reset-and-kept, unlike `stop_all`'s
    /// per-key `just_reset` dance) because `Engine::update`'s own `profile.
    /// deep_layer(active_layer).contains_key(&input)` guard already skips
    /// this `input` for good the moment the cascade lands — there's no next
    /// tick left to hand a stale entry to.
    pub(crate) async fn stop_stage(&mut self, input: Input, injector: &Injector) {
        let key = StageKey(input);
        self.slots.stop_toggle(&key).await;
        self.slots.force_release(&key, injector).await;
        self.runtime.remove(&input);
    }

    /// Drains every live deep Toggle without touching firings or per-key
    /// `KeyState`/Quick-Skip tracking — the GUI-focus `Command::
    /// StopAllToggles` escape hatch's own share of ticket 06's runtime
    /// teardown, extending it to also drain `Slots<StageKey>` alongside the
    /// individual path's `Slots<Input>`. Deliberately more aggressive than
    /// the Chord-toggle-survives-a-Profile-switch precedent (this is a
    /// manual, not automatic, teardown) but narrower than `stop_all` — a
    /// manual "attention just moved to the GUI" tap must not also reset a
    /// still-physically-held key's band tracking out from under it.
    pub(crate) async fn stop_all_toggles(&mut self) {
        self.slots.stop_all_toggles().await;
    }
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
