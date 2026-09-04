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

**Status:** ready-for-agent

- [ ] `handle_event` gets one narrow insertion at the binding-lookup point where the
      `AnalogRepeat`-swallow already sits, gated on `event.depth.is_some()` exactly
      like that swallow — so a Digital-mode primary press never diverts (the deep
      stage is inert for free in Digital mode) and Handoff/No-Return/Additive keys
      are entirely unaffected (this insertion only ever fires for a Quick-Skip-mode
      key).
- [ ] `Down` on a Quick-Skip key → `stage.begin_quick_skip(input, depth)`, then the
      event is swallowed (not passed to the ordinary individual-Down path). Resolves
      immediately to *skip* if the deep band is already hot on this same report
      (using the event's own `.depth` field, the same synchronous-resolution trick
      ticket 03 already established for same-report double-crossings); otherwise
      arms the ~50ms deadline.
- [ ] `Up` while the buffered primary never fired → swallowed (unbalanced, tolerated
      the way `force_release_stuck` already tolerates a lingering entry).
- [ ] A new fourth `select!` arm, `wait_for_stage_deadline`, mirroring
      `wait_for_chord_deadline`'s exact shape (`Option<Instant>` → `sleep_until` or
      `pending()`), added to `run`'s `tokio::select!`. On elapse with no deep
      crossing: fires the primary retroactively via `dispatch_individual_down` and
      flips the key to run as ordinary Handoff for the rest of the press.
- [ ] The deep band being reached within the window (via `stage::Engine::update`'s
      ordinary `rx_depth` path) resolves the Armed state to Skipped:
      `SuppressPrimary` (the buffered Down is dropped for good — it never fires, and
      Quick-Skip's own eventual Up doesn't either) → `FireDeep`. From here on for
      the rest of *this* press, every crossing runs as Additive-with-no-primary
      (release path = No-Return), per ticket 02's pure-core semantics — this ticket
      wires that runtime-state transition through `stage::Engine`, it doesn't
      reimplement it.
- [ ] Layer/Profile switch or a capture-mode flip to Digital while Armed cancels the
      buffered primary outright via `stage::Engine::stop_all()` (already wired at
      those call sites from ticket 03) — verify the buffer is included in what
      `stop_all()` clears.
- [ ] Integration tests (paused-time `tokio::test`, mirroring the Chord-deadline test
      harness precedent) covering every row of Quick-Skip's runtime-state table:
      deep reached within the window (including same-report-already-hot); deadline
      elapses with no deep crossing (retroactive primary fire via
      `dispatch_individual_down`, then ordinary Handoff for the rest of the press);
      early Up cancels with nothing emitted; a Layer/Profile/capture-mode change
      while Armed cancels via `stop_all()`.
