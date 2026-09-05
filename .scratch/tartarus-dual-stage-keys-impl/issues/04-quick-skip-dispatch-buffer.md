# 04 — Quick-Skip's dispatch-side primary-suppression buffer

**What to build:** A grid key configured with a deep stage in Quick-Skip mode now
behaves per spec — a fast full press within ~50ms skips the primary's Down (and its
eventual Up) entirely and fires only the deep stage; a slower press fires the primary
(delayed by up to that ~50ms) and the key runs as ordinary Handoff for the rest of
that press. All four Staging modes are now fully live end-to-end.

Source of truth: [`spec.md`](../../tartarus-dual-stage-keys/spec.md) §"Pipeline
architecture" ("Quick-Skip's primary suppression") and §"Staging-mode state machine"
(the Quick-Skip table).

**Blocked by:** 03

**Status:** resolved

- [x] `handle_event` gets one narrow insertion at the binding-lookup point where the
      `AnalogRepeat`-swallow already sits, gated on `event.depth.is_some()` exactly
      like that swallow — so a Digital-mode primary press never diverts (the deep
      stage is inert for free in Digital mode) and Handoff/No-Return/Additive keys
      are entirely unaffected (this insertion only ever fires for a Quick-Skip-mode
      key).
- [x] `Down` on a Quick-Skip key → `stage.begin_quick_skip(input, depth)`, then the
      event is swallowed (not passed to the ordinary individual-Down path). Resolves
      immediately to *skip* if the deep band is already hot on this same report
      (using the event's own `.depth` field, the same synchronous-resolution trick
      ticket 03 already established for same-report double-crossings); otherwise
      arms the ~50ms deadline.
- [x] `Up` while the buffered primary never fired → swallowed (unbalanced, tolerated
      the way `force_release_stuck` already tolerates a lingering entry).
- [x] A new fourth `select!` arm, `wait_for_stage_deadline`, mirroring
      `wait_for_chord_deadline`'s exact shape (`Option<Instant>` → `sleep_until` or
      `pending()`), added to `run`'s `tokio::select!`. On elapse with no deep
      crossing: fires the primary retroactively via `dispatch_individual_down` and
      flips the key to run as ordinary Handoff for the rest of the press.
- [x] The deep band being reached within the window (via `stage::Engine::update`'s
      ordinary `rx_depth` path) resolves the Armed state to Skipped:
      `SuppressPrimary` (the buffered Down is dropped for good — it never fires, and
      Quick-Skip's own eventual Up doesn't either) → `FireDeep`. From here on for
      the rest of *this* press, every crossing runs as Additive-with-no-primary
      (release path = No-Return), per ticket 02's pure-core semantics — this ticket
      wires that runtime-state transition through `stage::Engine`, it doesn't
      reimplement it.
- [x] Layer/Profile switch or a capture-mode flip to Digital while Armed cancels the
      buffered primary outright via `stage::Engine::stop_all()` (already wired at
      those call sites from ticket 03) — verify the buffer is included in what
      `stop_all()` clears.
- [x] Integration tests (paused-time `tokio::test`, mirroring the Chord-deadline test
      harness precedent) covering every row of Quick-Skip's runtime-state table:
      deep reached within the window (including same-report-already-hot); deadline
      elapses with no deep crossing (retroactive primary fire via
      `dispatch_individual_down`, then ordinary Handoff for the rest of the press);
      early Up cancels with nothing emitted; a Layer/Profile/capture-mode change
      while Armed cancels via `stop_all()`.

## Comments

**2026-09-05** — Implemented as specified. `stage::Engine` gains `begin_quick_skip`
(the `Down`-side divert, called off the real `rx_events` edge rather than waiting for
a coalescing `rx_depth` tick so the ~50ms window is armed at the physically precise
moment), `is_late` (queried by `handle_event`'s divert to decide whether a real
`Up`/`Repeat` still gets swallowed or must reach the ordinary path once this press
has gone Late), `next_deadline`/`tick` (the earliest-armed-deadline aggregate and its
`select!`-arm handler, both generalizing `chord`'s single-global-window shape to
Quick-Skip's one-independent-deadline-per-key model). `Engine::update`'s own blanket
"skip every QuickSkip input" filter from ticket 03 is gone, replaced by a narrower
`rt.quick_skip.is_none()` bypass on just the outer Up→Down edge — every other
transition (the deep engaging/disengaging while Armed, the final release out of
Skipped/Late) now runs through the same `advance` call the other three modes already
use, per ticket 02's tables.

One code-review finding, fixed before landing: `begin_quick_skip` originally assumed
`(prev primary, prev deep) == (Up, Up)` unconditionally and recomputed the deep band
fresh from the triggering event's own `depth` field. `rx_depth` (a coalescing
`watch`) and `rx_events` (a non-lossy `mpsc`) can reorder under load — the same
characteristic ADR-0007 already documents for the other three modes' same-report
race — so `Engine::update`'s own bypassed tick can observe a *later*, larger depth
sample (and correctly mark the shadow bands `Down`/`Down`) before this key's
still-queued primary `Down` event (carrying an *earlier*, smaller depth) is ever
drained. Blindly trusting the event's own stale depth in that case rolled a genuine
deep-band crossing back to `Up`, wrongly Arming instead of resolving Skipped — a fast
full press landing in this specific interleaving would silently drop the deep Binding
entirely and fire the primary Late instead, exactly the outcome Quick-Skip exists to
prevent. Fixed: `begin_quick_skip` now defers to `rt.deep` whenever `rt.primary` is
already `Down` (the signal that `update`'s own tracking raced ahead), and only
recomputes from the event's own `depth` when it's genuinely first. Verified by
temporarily reverting the fix and confirming the new regression test
(`dual_stage_quick_skip_reordered_depth_tick_still_resolves_skipped_not_late`, which
drives `DispatchState` directly through the `Seam` seam so the ordering is
deterministic rather than left to `tokio::select!`'s fairness draw) fails without it.

The same review also flagged a coverage gap against this ticket's own checklist: the
"Layer/Profile switch or a capture-mode flip ... while Armed" row had only a
Layer-switch test. Added
`dual_stage_quick_skip_capture_mode_flip_while_armed_cancels_the_buffered_primary`,
the capture-mode-flip call site's sibling (raw `run()` setup mirroring ticket 03's own
`dual_stage_digital_mode_flip_mid_press_...`). A dedicated Profile-switch test is not
yet meaningful — `Edit::SwitchProfile` doesn't call `stage::Engine::stop_all()` at all
yet; that wiring (`Effect::StopAllStages`) is ticket 06's job.

`cargo fmt --check` and `cargo clippy --all-targets` clean throughout. Daemon suite:
441 (ticket 03) → 447 (7 new `dual_stage_quick_skip_*` integration tests replacing the
one ticket-03 placeholder test that documented QuickSkip's old unaffected-primary-only
scoping) — covering the fast-full-press synchronous skip, deep-reached-within-window,
deadline-elapse-then-Late-then-plain-Handoff, early-Up cancellation, Layer-switch and
capture-mode-flip cancellation while Armed, and the reordered-depth-tick regression
above.
