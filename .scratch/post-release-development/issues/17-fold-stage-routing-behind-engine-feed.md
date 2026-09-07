<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 17 — Fold the Quick-Skip / deep-repeat routing in `dispatch::handle_event` behind one `stage::Engine::feed`

**What to build:** The two blocks in `dispatch::handle_event` that interpret a
physical edge against a dual-stage key's Staging-mode machine —

- the Quick-Skip primary-suppression divert (`dispatch.rs:246–289`): the
  `quick_skip_key` predicate re-derived from `Config`, then a `match
  event.state` that calls `stage.begin_quick_skip` / `stage.deep_repeat` /
  swallows / falls through by querying `stage.is_late`;
- the general deep-repeat + primary-handed-off swallow
  (`dispatch.rs:291–321`): every mode, on a `Repeat` with Depth, calls
  `stage.deep_repeat` then queries `stage.primary_handed_off` to decide
  whether to swallow the primary's synthetic `Repeat`;

collapse into **one** call, sited right after the `chord::feed` `NotMine`
fall-through:

```rust
// daemon/src/stage.rs
impl Engine {
    /// Route one physical edge on a grid key against its Staging-mode machine.
    /// `NotMine` ⇒ dispatch runs the ordinary Binding path for this event
    /// (any deep-side side effect this call needed — e.g. driving a
    /// Hold-to-repeat deep stage's cadence off a primary `Repeat` — has
    /// already been applied). `Handled(edits)` ⇒ the event is consumed;
    /// commit `edits` (empty in every case but a Quick-Skip `Down` that
    /// fired a deep `ProfileSwitch`).
    pub(crate) async fn feed(
        &mut self,
        deps: EngineDeps<'_>,
        event: PhysicalEvent,
    ) -> io::Result<StageOutcome>;
}

/// Mirrors `chord::ChordOutcome` — same shape, same place in `handle_event`.
pub(crate) enum StageOutcome {
    Handled(Vec<Edit>),
    NotMine,
}
```

`begin_quick_skip`, `is_late`, `primary_handed_off`, `deep_repeat` leave the
`pub(crate)` surface — inlined into `feed`'s `match` on `event.state` + the
key's Quick-Skip phase + its `primary_handed_off` flag, or kept as private
helpers. The interface goes **10 → 7**: `feed`, `update`, `next_deadline`,
`tick`, `stop_all`, `stop_stage`, `stop_all_toggles` — the last two being
review candidate 3's teardown-contract concern (a future ticket), untouched here.

**No behaviour change** — with **one** sanctioned exception added by the
2026-09-07 Addendum below (a dual-stage key's primary press stops carrying the
Fire-once dwell). Every firing, swallow, Quick-Skip `Armed → Skipped → Late`
transition and deep-repeat cadence otherwise resolves identically for every
input sequence. `feed` is the current two blocks' logic relocated, not
rewritten — the routing matrix below is today's behaviour enumerated.

## The friction

`stage::Engine` is a genuinely deep pure core (`advance` / `tick` / the four
Staging-mode transition tables, `stage.rs:132–411`, 11 table tests) wearing a
**10-method** interface, because `handle_event` reaches past the interface to
drive the Quick-Skip and deep-repeat state machines by hand:

- **`handle_event` re-derives a predicate the engine already tracks.**
  `quick_skip_key` (`dispatch.rs:246–253`) is `event.depth.is_some() &&
  profile.deep_stages.get(input).mode == QuickSkip &&
  profile.deep_layer(active_layer).contains_key(input)` — the same
  `deep_stages` / `deep_layer` lookups `Engine::update` does on every depth
  tick (`stage.rs:524–534`).
- **Five methods exist only for this routing.** `begin_quick_skip`
  (`stage.rs:697`), `is_late` (`stage.rs:792`), `primary_handed_off`
  (`stage.rs:805`), `deep_repeat` (`stage.rs:825`) have no other caller;
  `is_late` / `primary_handed_off` are pure getters that exist so
  `handle_event` can branch on the engine's private phase.
- **`EngineDeps` is built inline three times** in `handle_event`
  (`dispatch.rs:258`, `273`, `299`) — once per stage touchpoint.
- **~90 lines** of `dispatch.rs` (`246–321`) are the "interpret this edge
  against the staging machine" logic that ADR-0007 says lives in the `stage`
  module.

**Deletion test.** Deleting `feed` scatters the phase-interpretation match
back into `handle_event`, restores the `Config`-derived `quick_skip_key`
predicate, and re-exposes `begin_quick_skip` / `is_late` /
`primary_handed_off` / `deep_repeat` as an interface only one caller uses.
Concentrating it is the win: the Quick-Skip `Armed → Skipped → Late` routing
becomes testable against `feed` directly instead of through a dispatch-task
harness, the interface halves the methods that carry Quick-Skip's control
flow, and `handle_event`'s stage section becomes one `chord::feed`-shaped
call.

## The routing matrix `feed` implements

Every row is current `handle_event` behaviour. `feed` is called for **every**
event surviving the earlier `handle_event` guards (mode-key, Down-stops-Toggle,
axis, `chord::feed`); it owns the "is this a dual-stage key?" decision.

| Situation | `feed` returns | Was |
|---|---|---|
| No deep stage for `event.input` on the active Layer | `NotMine` (early-out) | the `quick_skip_key` predicate being false + the `deep_repeat` no-op |
| Non-Quick-Skip primary `Down` (Handoff / No-Return / Additive) | `NotMine` | fell straight through to `dispatch_individual_down`; `update` drives the deep stage off Depth |
| Quick-Skip `Down`, resolves `Armed` or (deep already hot) `Skipped` | `Handled(edits)` | `stage.begin_quick_skip(...)` |
| Quick-Skip `Up` / `Repeat` while `Armed` / `Skipped` / `None` | `Handled(vec![])` | `return Ok(Vec::new())` — swallowed |
| Quick-Skip `Up` / `Repeat` once `Late` | `NotMine` | `EventState::Up \| EventState::Repeat => {}` — ran as ordinary Handoff |
| Any mode, `Repeat`, primary currently handed off to deep | drive deep repeat, then `Handled(vec![])` | `stage.deep_repeat(...)` then `if primary_handed_off { return }` |
| Additive `Repeat`, primary **not** handed off | drive deep repeat, then `NotMine` | `stage.deep_repeat(...)` then fall through |

**`NotMine` is not "feed did nothing".** In the last two `Repeat` rows `feed`
has already fired the deep stage's Hold-to-repeat as a side effect; `NotMine`
means "now also run the ordinary path" — exactly the current
`self.stage.deep_repeat(...)` then fall-through.

## Integration in `dispatch.rs`

- **`handle_event`** (`dispatch.rs:158`): delete the `quick_skip_key` block
  (`246–289`) and the deep-repeat block (`291–321`). In their place, right
  after the `chord::feed` `match` arm `ChordOutcome::NotMine => {}`
  (`dispatch.rs:225`) and **before** the `binding` local is bound
  (`dispatch.rs:228`):

  ```rust
  let deps = stage::EngineDeps {
      config,
      active_layer: self.active_layer,
      individual: &mut self.individual,
      injector: &self.injector,
      cursors: &mut self.stepper,
      toggle_lap_target: self.toggle_lap_target,
  };
  match self.stage.feed(deps, event).await? {
      stage::StageOutcome::Handled(edits) => return Ok(edits),
      stage::StageOutcome::NotMine => {}
  }
  ```

  One `EngineDeps` build; edits flow out through `handle_event`'s existing
  `-> io::Result<Vec<edit::Edit>>` return, so the `rx_events` `select!` arm
  (`dispatch.rs:889–895`) is unchanged.

- **The `EventState::Down => self.dispatch_individual_down(...)` tail**
  (`dispatch.rs:336–343`) and the `Repeat | Up` `perform` tail
  (`344–369`) are untouched — a non-Quick-Skip dual-stage key's primary
  still reaches them via `feed` → `NotMine`.

- **The analog-repeat swallow** (`dispatch.rs:329–334`) stays where it is.
  Dual-stage keys can't be Analog-repeat (`ConfigError::AnalogRepeatOnDualStageKey`),
  so its position relative to the new `feed` call is immaterial; placing
  `feed` earlier (before `binding`) is simply less code.

- **No `select!` / deadline-arm changes.** `update_stages` (`dispatch.rs:602`)
  and `tick_stages` (`628`) keep their own `EngineDeps` builds in the
  `rx_depth` and `wait_for_stage_deadline` arms. The 4×-duplicated `if
  !edits.is_empty() { commit_input_edits(...) }` tail and the
  `wait_for_stage_deadline` / `wait_for_chord_deadline` byte-copy
  (`dispatch.rs:1000` / `1013`) belong to review candidate 3 (a future ticket) — not touched here.

## What moves, what stays

- **Into `stage::Engine::feed` (private):** the bodies of
  `begin_quick_skip`, `deep_repeat`, the `is_late` / `primary_handed_off`
  reads (now `match` conditions), and the `quick_skip_key` predicate (now
  `feed`'s early-out: `profile.deep_stages.get(&event.input)` ∧
  `deep_layer(active_layer).contains_key`). `begin_quick_skip`'s
  `rx_events`-vs-`rx_depth` race handling (`stage.rs:719–738`, the `rt.primary
  == KeyState::Down` check) moves **verbatim** — `feed` is still called from
  the `rx_events` path, so the "arm the ~50ms window at the physically
  precise moment" property is preserved.
- **Stays `pub(crate)` on `Engine`:** `feed`, `update`, `next_deadline`,
  `tick`, `stop_all`, `stop_stage`, `stop_all_toggles`.
- **Stays in `stage.rs` unchanged:** the pure core (`Band`, `Bands`,
  `StageOp`, `QuickSkipPhase`, `advance`, `next_deadline`, `tick`, the four
  transition-table fns, `assert_reachable`), `KeyRuntime`, `Engine::update`'s
  own `rx_depth`-driven body including its `quick_skip.is_none()` bypass
  (`stage.rs:555–575`) — that bypass still defers the outer Up→Down edge to
  `feed` (was: to `begin_quick_skip`).
- **Stays in `dispatch.rs`:** everything else — the earlier `handle_event`
  guards, `dispatch_individual_down`, `run_chord_effects`, `update_stages` /
  `tick_stages`, the `select!` loop, every `handle_*`.
- **`EngineDeps`** is unchanged (same six fields).

## Naming note

`dispatch.rs`'s **test harness** already has an `async fn feed(&mut self,
event: PhysicalEvent) -> Vec<edit::Edit>` (`dispatch.rs:1362`) that drives a
synthetic event through the whole pipeline. `stage::Engine::feed` is a
different receiver in a different module; the collision is tolerable and the
`chord::feed` parallel is worth more. If it reads badly in review, `route`
is the fallback name — decide during implementation, not now.

## Behaviour-preservation protocol

This is the latency-critical input path and every dual-stage transition runs
through it:

- **Diff the moved bodies line-by-line against `HEAD`.** Load-bearing:
  `begin_quick_skip`'s `rt.primary == KeyState::Down` short-circuit (trust
  `rt.deep`, not the event's now-stale `depth`, when `update` raced ahead);
  the outer edge `advance((Up, Up), next, QuickSkip, None)` call; `is_late`
  gating a real `Up`/`Repeat` back to the ordinary path; `primary_handed_off`
  swallowing a `Repeat` for Handoff/No-Return but **not** Additive;
  `deep_repeat`'s three guards (`rt.deep == Down`, deep Binding present,
  `trigger == HoldToRepeat`).
- **`cargo test -p acheron-daemon` fully green** — the ~40 `dual_stage_*`
  pipeline tests (`dispatch.rs:4542–5928`) — before any test is added.
- **`/code-review` on both the Standards and Spec axes**, as tickets 05–16 did.

## Tests: keep the net, add the matrix

- **Kept, unchanged:** all ~40 `dual_stage_*` pipeline tests
  (`dispatch.rs:4542–5928`) — they drive `fake` capture → assert injected
  `uinput` writes / emitted signals, at the pipeline seam, unaffected by an
  internal refold. This is the regression net; nothing here is moved or
  deleted.
- **Kept, unchanged:** `stage.rs`'s pure `advance` / `tick` table tests
  (`stage.rs:1017+`).
- **New — `stage::Engine::feed` routing-matrix unit tests** in `stage.rs`'s
  test module: a synthetic `PhysicalEvent` + a hand-built `EngineDeps` (stub
  `Injector`, a small `Config` with one dual-stage key, `Slots::default()`),
  asserting `StageOutcome` + slot state for every row of the matrix above —
  in particular the Quick-Skip `Armed → Skipped → Late` sequence, which is
  currently reachable only through the dispatch task. Async, no D-Bus.
- Net: coverage strictly increases — the Quick-Skip routing decisions get a
  direct test surface; the end-to-end tests stay as integration proof that
  `handle_event` actually routes through `feed`.

## Docs

- **ADR-0007** (`docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md`)
  — append a dated **Refined:** paragraph. Substance (settled in the
  grilling): (1) the per-Quick-Skip-key divert the ADR describes
  (`"for Quick-Skip keys only, handle_event diverts the primary edge to the
  engine"`) is now a single unconditional `stage::Engine::feed` call for
  every physical edge on a grid key, with `begin_quick_skip` / `is_late` /
  `primary_handed_off` / `deep_repeat` no longer on the interface; (2) the
  load-bearing decision — staged-Depth interpretation lives in dispatch's
  `stage` module, not the capture source — is **unchanged**. Exact prose
  written with the implementation.
- **No `CONTEXT.md` change** — `feed` / `StageOutcome` are module-internal
  plumbing; they introduce no domain noun (Actuation stage / primary stage /
  deep stage / Staging mode already cover the concepts). Ticket 14 / 15
  precedent of declining an entry for a locus refactor.
- **`.scratch/README.md`** `post-release-development` line — extend with
  ticket 17.
- **`.scratch/post-release-development/issues/`** has no map (non-wayfinder);
  no map update needed.

## Decisions from the grilling (2026-09-06)

- **Scope: both blocks fold** (Q1=A). The Quick-Skip divert and the general
  deep-repeat/swallow are both "interpret a physical edge against the staging
  machine". Not option C — pulling `dispatch_individual_down` (shared with the
  chord `FireIndividual` executor) across the seam buys no locality.
- **The engine owns the "is this a dual-stage key?" predicate** (Q2=A).
  `handle_event` stops computing `quick_skip_key`; `feed` early-outs to
  `NotMine`. Same map lookups `update` already does per depth tick.
- **Timer methods stay separate** (Q3). `next_deadline` / `tick` are driven
  by the `wait_for_stage_deadline` arm — a timer, not an event — and mirror
  `chord`'s identical pair.
- **Signature: `EngineDeps`, not pre-shaped for a future depth-engine trait** (Q4=A).
  One implementor is a hypothetical seam; revisit when the depth-engine
  contract (review candidate 3) is actually picked up.
- **Amend ADR-0007** (Q5=A), don't write a new one — only the internal call
  shape narrows.
- **Shape + name mirror `chord::feed`** (Q6): `Engine::feed(deps, event) ->
  StageOutcome::Handled(Vec<Edit>) | NotMine`. `Handled(vec![])` vs `NotMine`
  names the passthrough decision instead of encoding it as an empty vec.
- **Four methods leave `pub(crate)`** (Q7): `begin_quick_skip`, `is_late`,
  `primary_handed_off`, `deep_repeat`. Interface 10 → 7.
- **One call site**, right after `chord::feed`, before `binding` is bound
  (Q8). Dual-stage keys are mutually exclusive with Chord membership
  (`ChordMemberDeepStageConflict`) and Analog-repeat
  (`AnalogRepeatOnDualStageKey`) by validation, so order against those guards
  is immaterial.
- **Keep the pipeline tests, add a `feed`-level routing matrix** (Q9). No
  deletion of end-to-end Quick-Skip coverage.
- **No `CONTEXT.md` entry** (Q10).
- **`select!` / commit wiring unchanged** (Q12) — edits flow through
  `handle_event`'s existing return.
- **Filed as `post-release-development` ticket 17** (Q13) — that effort's
  purpose is architecture-review-driven deepening (tickets 03–16); this is
  the review's top candidate. One cohesive ticket: dispatch refold + ADR
  amendment + `feed` tests.

## Facts dug from the code during the grilling (not asked of the user)

- `handle_event` (`dispatch.rs:158`) already calls `chord::feed(...) ->
  ChordOutcome::Handled(effects) | NotMine` at `216–226`, immediately above
  where the new `stage.feed` call lands.
- `Engine::update`'s `quick_skip.is_none()` bypass (`stage.rs:555–575`)
  writes the shadow bands unconditionally but decides no op for a Quick-Skip
  key's outer Up→Down edge — it defers that edge entirely to
  `begin_quick_skip` (→ `feed`). `begin_quick_skip` detects an `update` that
  raced ahead via `rt.primary == KeyState::Down` and then trusts `rt.deep`
  over the event's stale `depth` (`stage.rs:719–738`).
- `deep_repeat` (`stage.rs:825`) is called from **two** sites in
  `handle_event` today — inside the Quick-Skip `Repeat` arm (`dispatch.rs:281`,
  Skipped phase runs as Handoff) and in the general block (`dispatch.rs:307`,
  every mode). Both fold into `feed`.
- `primary_handed_off` (`stage.rs:805`) is set/cleared inside
  `Engine::update` (`stage.rs:581–592`) — tracked across every op a tick
  emits, cleared when the primary band goes Up. `handle_event` only *reads*
  it (`dispatch.rs:318`).
- The `dual_stage_*` pipeline tests span `dispatch.rs:4542–5928` (helpers
  `dual_stage_config` / `toggle_binding` / `hold_to_repeat_binding` /
  `settle` / `event_counts` at `4542–4600`, ~40 `#[tokio::test]` fns after).
- `EngineDeps` (`stage.rs:430`) has six fields: `config`, `active_layer`,
  `individual: &mut Slots<Input>`, `injector: &Injector`, `cursors: &mut
  stepper::Cursors`, `toggle_lap_target: Duration`. `handle_event`'s
  `&mut self` on `DispatchState` lets it borrow `self.stage` + `self.individual`
  + `self.stepper` mutably + `self.injector` immutably at once (disjoint
  fields), exactly as `update_stages` does.
- The `stage::Engine` `pub(crate)` methods today: `update` (507),
  `begin_quick_skip` (697), `is_late` (792), `primary_handed_off` (805),
  `deep_repeat` (825), `next_deadline` (870), `tick` (884), `stop_all` (964),
  `stop_stage` (989), `stop_all_toggles` (1005) — 10.
- `handle_connection_change` (`dispatch.rs:1078`), `handle_layer_switch`
  (`1032`), `handle_capture_mode_change` (`1108`) each tear down a *different*
  subset of the three depth engines — that divergence is review candidate 3
, explicitly out of scope here.
- `dispatch.rs` is 5930 lines (implementation ends ~1185); `stage.rs` is 1270.

**Blocked by:** None — `tartarus-dual-stage-keys-impl` tickets 01–07 (config
schema, pure `stage.rs`, `stage::Engine` in dispatch, Quick-Skip buffer, the
D-Bus surface, teardown, the binding editor) are all resolved; this refolds
their dispatch-side seam.

**Status:** ready-for-agent

- [ ] `stage::StageOutcome { Handled(Vec<Edit>), NotMine }` added, mirroring
      `chord::ChordOutcome`.
- [ ] `stage::Engine::feed(deps, event) -> io::Result<StageOutcome>` — the
      routing matrix above, bodies of `begin_quick_skip` / `deep_repeat`
      moved in verbatim, `is_late` / `primary_handed_off` / `quick_skip_key`
      as `match` conditions. `begin_quick_skip`'s `rx_events`-vs-`rx_depth`
      race handling preserved line-for-line.
- [ ] `begin_quick_skip`, `is_late`, `primary_handed_off`, `deep_repeat`
      removed from the `pub(crate)` surface (inlined or made private helpers).
      `Engine` interface: 10 → 7.
- [ ] `handle_event` (`dispatch.rs`): the `246–289` and `291–321` blocks
      replaced by one `EngineDeps` build + `match self.stage.feed(...).await?`,
      sited right after `chord::feed`'s `NotMine` arm, before `binding` is
      bound. `quick_skip_key` deleted.
- [ ] No `select!` arm changes; no `wait_for_stage_deadline` /
      `wait_for_chord_deadline` change; `update_stages` / `tick_stages`
      unchanged.
- [ ] ADR-0007 — dated **Refined:** paragraph (call shape narrowed;
      dispatch-owns-Depth-interpretation decision unchanged).
- [ ] New `stage::Engine::feed` routing-matrix unit tests in `stage.rs`
      (every table row; the `Armed → Skipped → Late` sequence). All ~40
      `dual_stage_*` pipeline tests kept unchanged and green.
- [ ] `.scratch/README.md` `post-release-development` line extended with
      ticket 17.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, full
      daemon suite green; GUI suite unaffected (no wire/stub change) but run
      to confirm.
- [ ] `/code-review` (Standards + Spec) — no behaviour-change findings *other
      than* the Addendum's sanctioned dual-stage-primary dwell change; moved
      bodies verified verbatim against HEAD.
- [ ] **Addendum:** dual-stage primary uses `PerformDeps::new_machine_sequenced`
      (see below) — the redundant trailing `value=0` from `humane-output-rate`
      ticket 12's sub-dwell force-release path is gone. A `dual_stage_*` test
      pins it.

## Addendum (2026-09-07): fold the Fire-once-dwell × dual-stage-primary interaction

**Why this lands here.** `humane-output-rate` ticket 12 spliced a fixed 40 ms
`executor::FIRE_ONCE_KEY_DWELL` between the edges of a canned one-shot keyboard
press, gated by `PerformDeps` construction: `PerformDeps::new` (individual +
Chord paths) dwells; `PerformDeps::new_machine_sequenced` (every `stage::Engine`
call site) does not. Ticket 12 handled the five `stage::Engine` `perform` sites
but **could not** cleanly handle one case: a dual-stage key's *initial primary
`Down`* flows through `handle_event`'s ordinary individual path
(`dispatch_individual_down` / the `Repeat | Up` `perform` tail), which builds
`PerformDeps::new` — so a dual-stage primary that happens to be a Fire-once
single key **does** get the dwell today. If the user then crosses into the deep
band inside that 40 ms window, `ReleasePrimary` / `ForceReleaseStuck` drains the
firing and force-releases the key, and the still-pending dwell task later emits
its own `KeyUp` — a redundant `value=0` for an already-released key (benign:
kernel-deduplicated, and the same shape as a sub-35 ms `CONTROLLER_BUTTON_
DIGITAL_PULSE_HOLD` Analog-repeat `Up` — but avoidable).

The clean fix needs exactly the predicate this ticket centralises: **"is
`event.input` a dual-stage key on the active Layer?"** Once `feed` owns that
(Q2 of the grilling — `handle_event` stops computing `quick_skip_key`), dispatch
can route the primary path's `PerformDeps` accordingly with no new lookup.

**Decision.** A dual-stage key's primary press is **machine-sequenced input** —
its timing is already subject to depth interpretation — so it does **not** carry
the Fire-once dwell, matching every other `stage::Engine`-driven firing. The
loss (a dual-stage key tapped as a plain Fire-once key, never crossing deep,
forgoes the ~40 ms plausibility dwell on that one press) is negligible: dual-
stage keys are a niche feature and the primary is the light half of an
escalating pair. This is simpler than any alternative that keeps the dwell and
cancels/adopts the in-flight firing on a deep crossing (a `FiringHandle` cancel
token — rejected by ticket 12 for unrelated reasons: it would truncate a running
multi-step Fire-once Macro on physical release).

**Mechanism — constraints binding, exact shape your call (as elsewhere in this ticket).**

1. When `feed` returns `NotMine` for an edge on a key it tracks (matrix rows 2,
   6, 7 — a non-Quick-Skip primary `Down`, and the deep-repeat fall-through
   rows), the subsequent ordinary individual `perform` in `handle_event` for
   that event must use `PerformDeps::new_machine_sequenced`, not
   `PerformDeps::new`.
2. `feed`'s early-out `NotMine` (matrix row 1 — *not* a dual-stage key) must
   still lead to `PerformDeps::new` (dwell on) — this is the ordinary
   non-dual-stage individual press and ticket 12's whole point.
   So `StageOutcome::NotMine` alone is not enough information at the call site;
   options: (a) `feed` returns `NotMine { machine_sequenced: bool }` (keeps one
   fall-through variant, mild deviation from the `chord::feed` mirror);
   (b) a cheap `self.stage.tracks(event.input)` getter (`runtime.contains_key`)
   that `handle_event` calls to pick the constructor — one method *added* to the
   interface, against this ticket's 10→7 goal, but it is a pure `&self` getter
   with a real second purpose (it *is* the "dual-stage key?" predicate);
   (c) fold the two individual `perform` tails in `handle_event` so the
   constructor choice is made once. Prefer whichever keeps `handle_event`
   readable; (a) is the lightest if the `chord::feed` shape can bend.
3. The retroactive `dispatch_individual_down` call from the **Chord** path
   (`run_chord_effects`) stays `PerformDeps::new` — a Chord member can never be
   a dual-stage key (`ChordMemberDeepStageConflict`), so that path is genuinely
   a user one-shot.

**Tests.**

- A `dual_stage_*` pipeline test (Handoff or No-Return, Fire-once single-key
  primary): press the primary, cross into the deep band **within**
  `FIRE_ONCE_KEY_DWELL`, assert the primary emits exactly one `[Down, Up]` pair
  with **no** trailing redundant `value=0`, and that the Up is not deferred by a
  40 ms dwell (i.e. the primary `perform` took the machine-sequenced path).
  `start_paused`.
- A `dual_stage_*` test that a primary press which **never** crosses deep still
  fires correctly (it just no longer holds the 40 ms dwell — assert the pair,
  timing not dwell-shaped).
- Confirm `settle_past_dwell()` retrofits that ticket 12 added to the Handoff /
  No-Return Handoff-walk tests can be **reverted** to plain `settle()` — the
  dual-stage primary no longer dwells, so the deep excursion no longer has to
  wait it out. (If any test still needs it, say why.)
- The `perform`-layer `trigger::slots` dwell tests (ticket 12) are unaffected —
  they exercise `PerformDeps::new` directly.

**Docs.**

- **ADR-0008** "The ceiling" — the ticket-12 paragraph currently says the dwell
  covers "a Fire-once Keypress, and its single-key Macro / Stepper-step / Chord
  equivalents". Add a clause: *a dual-stage key's primary stage is excluded —
  its press is machine-sequenced input, dwelled nowhere `stage::Engine` drives
  a firing.*
- **ADR-0007's** dated **Refined:** paragraph (already in this ticket's scope)
  gains one sentence: folding the routing behind `feed` also moved the
  Fire-once-dwell decision for a dual-stage primary onto the machine-sequenced
  side, closing `humane-output-rate` ticket 12's noted residual.
- **`CONTEXT.md`** — no change (the "Physical-plausibility ceiling" entry's
  "canned one-shot keyboard press carries a fixed ~40 ms dwell" sentence stays
  true for the ordinary case; the dual-stage-primary carve-out is ADR detail,
  not glossary).
- Cross-reference: append a note to `humane-output-rate` ticket 12's "Redundant
  trailing `value=0`" section pointing here (**done** 2026-09-07).

## Comments

**2026-09-06** — Filed from the fourth architecture review
(`research/architecture-review-2026-09-06.html`, candidate 1 of 6, the
"Start here" pick). The review scoped to the just-shipped *Dual-stage keys*
work; this candidate pays down the interface debt that feature added to
`stage::Engine`. Design tree settled over three grilling rounds; see
"Decisions from the grilling". Also noted as the precondition that makes
review candidate 3 (a depth-engine teardown contract) tractable
— once `stage` presents one event entry point instead of ten methods, fitting
it behind a shared contract is far cheaper. Not yet implemented — handed to a
fresh session.
