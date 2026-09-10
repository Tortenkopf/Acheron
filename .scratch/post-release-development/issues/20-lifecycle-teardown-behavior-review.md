<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 20 — Decide the lifecycle-teardown matrix holes and the config-edit-vs-live-state gaps

**What to decide:** Ticket 19 makes the lifecycle-teardown matrix explicit
without changing behaviour — every `—` cell becomes a `//`-marked skip. This
ticket grills each skip and each related "an edit mutates config out from under
live runtime state" gap, decides keep-or-fix per case, and spawns focused fix
tickets for the ones that change. Charting only; no code here.

**Why a separate ticket.** These are behaviour decisions, some touching the
dual-stage `spec.md`'s explicit "Out of Scope" list. Bundling them into ticket
18 (a bug fix) or ticket 19 (a behaviour-neutral refactor) would muddy both
diffs and skip the reasoning each one needs.

## The cases

### A — Lifecycle-teardown matrix holes (from ticket 19's `//`-marked skips)

1. **`axis` not reset on device disconnect.** A grid key Axis-assigned and
   pushed to full travel when the device drops: its last `ABS_*` value stays
   asserted on the gamepad `uinput` device with no key to release it. Layer
   switch and Profile switch both `axis.reset()`; disconnect does not.
   *Likely fix:* `axis.reset()` in `tear_down(Disconnect)`.

2. **`analog_repeat` not torn down on disconnect.** The dual-stage `spec.md`
   records this as a pre-existing gap, explicitly Out of Scope ("Analog-repeat's
   pre-existing lack of dropout handling … is explicitly out of scope as a
   separate effort"). A live Analog-repeat task keeps its `depth_rx` watch and
   its pace loop running against a stale snapshot after the device drops.
   *Decision needed:* is "a separate effort" this ticket, or does it stay
   parked? If fixed, `analog_repeat.stop_all()` in `tear_down(Disconnect)` is
   the one-liner — but confirm the spawned loop's `depth_rx.changed()` +
   `cancel` select handles a dropped sender cleanly first.

3. **`axis` not reset on the flip to Digital.** Digital mode has no Depth, so a
   live continuous axis output has no further input driving it and no release.
   `analog_repeat` and `stage` are both `stop_all`'d on this flip; `axis` is
   not. *Likely fix:* `axis.reset()` in `tear_down(CaptureModeToDigital)`.
   (Cross-check: does the axis step-digital fallback take over for an
   Axis-assigned key in Digital mode, making a reset wrong? `axis::Engine
   ::step_digital` exists — resolve the interaction.)

4. **`chord_machine` window not reset on a Layer switch.** If a chord
   simultaneity window is open (first member down, waiting ≤50 ms for the rest)
   when the Mode key toggles the Layer, the window keeps ticking and
   `chord::tick` fires whatever the window recorded — a Chord that may not be
   bound on the new Layer, whose member Inputs' individual Bindings on the new
   Layer are also being suppressed. *Likely fix:* reset the window (a
   `chord::ChordMachine::reset()` — new) in `tear_down(LayerSwitch)` and
   probably `ProfileSwitch` / `Disconnect` / `CaptureModeToDigital` too.

5. **`chord_slots` firings not drained on a Layer switch.** A live Chord
   Hold-to-repeat firing survives a Layer switch with no release edge (the
   member release that would end it is on the old Layer's suppression path).
   Individual firings *are* drained (`individual.drain_firings`). *Decision
   needed:* should `chord_slots.drain_firings()` join `tear_down(LayerSwitch)`?
   (Chord *Toggles* surviving is arguably deliberate, matching individual
   Toggles — but firings are not Toggles.)

6. **`chord_slots` on Profile switch.** `edit.rs` documents "an active Chord
   Toggle survives a Profile switch today" as current behaviour, but not as a
   *decision*. Individual Toggles are drained on a Profile switch
   (`StopAllToggles`). *Decision needed:* is the asymmetry intended, or should
   `chord_slots.stop_all_toggles()` join `tear_down(ProfileSwitch)`?

### B — Config edits that mutate live runtime state without tearing it down
(from ticket 18's scope boundary)

7. **`SetStagingMode` mid-press.** Flipping a key's Staging mode to/from
   Quick-Skip while it holds a live Handoff deep firing pushes no effect. The
   next `stage::Engine::update` tick re-reads `Config` and may leave the deep
   slot in an inconsistent phase. *Decision needed:* push `Effect::StopStage`
   (force-release the live slot on a mode change), or is "the next tick
   reconciles" actually fine?

8. **`SetDeepActuation` under a live slot.** Changing the deep band's
   actuation/release point while the deep stage is held — the next tick
   re-thresholds against the new point. *Likely keep* (no orphan; behaves like
   moving the primary actuation point under a held key), but confirm.

9. **`SetAxisAssignment` orphaning a live Binding / Chord / Toggle.** The edit
   atomically removes any existing `Binding` and Chord membership for
   `(layer, input)` but pushes no `StopToggle` / `StopStage` / chord-firing
   release for a live one it just deleted. It relies on `RecomputeAxes` and on
   `config::validate` rejecting the illegal combinations — but a live
   *individual Toggle* on that Input is not illegal and is not torn down.
   *Likely fix:* the removed-Binding branch pushes the matching release effect.

10. **`SetBinding` replacing a live Toggle's binding.** Overwriting a Toggle's
    Binding with a different Action pushes no `StopToggle` for the now-stale
    Toggle — it keeps running the old Action until pressed again. *Decision
    needed:* push `Effect::StopToggle(input)` on a `SetBinding` that overwrites
    a Binding with a live Toggle, or is "the Toggle stops on the next press,
    now running the new Action" acceptable?

11. **`SetChordBinding` / `ClearChordBinding` push no effects at all.**
    Deleting a Chord's Binding while it holds a live Chord Toggle leaves the
    Toggle running with nothing to stop it (its key is gone from
    `chords(layer)`). *Likely fix:* `ClearChordBinding` pushes a
    chord-firing/Toggle release for that `ChordKey`; `SetChordBinding` that
    replaces one does too.

12. **`Effect::StopStage`'s unconditional `runtime.remove` misfires a
    cross-layer clear.** `stage::Engine::stop_stage` drops the per-key runtime
    entry outright, justified by "`update`'s `deep_layer(active_layer)
    .contains_key` guard skips this Input for good" — true only when the
    cleared layer *is* the active layer. If a grid key carries a deep stage on
    *both* Base and Held and one layer's is cleared (`ClearDeepStage`, or
    `ClearBinding`'s cascade) while the key is physically held into the deep
    band on the *other*, still-valid layer, the force-release stops that live
    firing and the next `update` tick — guard still true — recreates the entry
    with `just_reset=false` and re-fires it (release-then-re-press of a stage
    that never physically moved). Introduced for `ClearDeepStage` by ticket 18;
    pre-existing for `ClearBinding`'s cascade. Reachable via the D-Bus `layer`
    arg / the GUI editing the non-active layer. *Likely fix:* `stop_stage`
    resets-and-keeps the entry with `just_reset=true` (like `stop_all`) instead
    of removing it, so the cross-layer case silently re-adopts with no op and
    the same-layer case is still skipped by the guard — needs a `stage.rs`
    change + a pure test, hence deferred here rather than folded into 18.

## Process

Grill each case (A1–6, B7–11) — keep or fix, and if fix, the exact effect /
call and its tests. Cases that keep get a one-line rationale added next to
ticket 19's `//` skip or the `edit.rs` variant doc. Cases that fix graduate to
their own small tickets (or a single "teardown-gap fixes" ticket if they
cluster). Update the dual-stage `spec.md` "Out of Scope" list if A2 moves.

## Docs

- Per-case: either a rationale comment (keep) or a fix ticket (change).
- **`spec.md`** (`tartarus-dual-stage-keys/`) — its "Out of Scope: Analog-repeat
  dropout handling" line is revisited if A2 is taken.
- **ADR?** Only if a *keep* decision is load-bearing and surprising enough that
  a future review would re-raise it (e.g. "Chord Toggles deliberately survive
  everything" might warrant one line in an ADR or CONTEXT.md). Decide per case.
- **`.scratch/README.md`** — extend the `post-release-development` line.

## Facts dug from the code during the grilling (not asked of the user)

- Matrix holes A1/A2/A3: see ticket 19's fact list — `handle_connection_change`
  (`dispatch.rs:1085`) and `handle_capture_mode_change` (`dispatch.rs:1122`)
  signatures take no `&mut axis::Engine`; disconnect also takes no
  `&mut analog_repeat::Engine`.
- A4/A5: `chord_machine` is reset by no lifecycle event (touched only at field
  decl, `DispatchState::new`, `chord::feed` in `handle_event`, the
  `chord::next_deadline`/`chord::tick` deadline arm). `chord_slots` swept by no
  lifecycle event.
- A2: dual-stage `spec.md` — "Analog-repeat's pre-existing lack of dropout
  handling, surfaced while resolving the reconnect question, is explicitly out
  of scope as a separate effort" (also in `.scratch/README.md`'s dual-stage
  line).
- B7: `Edit::SetStagingMode` (`edit.rs:747–753`) — `.entry(input).or_default()
  .mode = mode`, no effect; variant doc "No `Effect`, same reasoning" (as
  `SetDeepActuation`: "`stage::Engine` … reads `Config` directly each tick").
- B8: `Edit::SetDeepActuation` (`edit.rs:736–746`) — no effect (doc
  `edit.rs:265–270`).
- B9: `Edit::SetAxisAssignment` (`edit.rs:670–694`) — removes existing
  `Binding` + Chord membership for `(layer, input)` (`edit.rs:678–689`), pushes
  `Effect::RecomputeAxes { layer }` only.
- B10: `Edit::SetBinding` (`edit.rs:378–408`) — overwrites the primary; comment
  (400–407) explains it deliberately does not cascade the deep stage (a primary
  stays in place); no `StopToggle`.
- B11: `Edit::SetChordBinding` / `ClearChordBinding` (`edit.rs:640–668`) — no
  `effects.push` on either arm.
- B12: `stage::Engine::stop_stage` (`stage.rs:1195–1198`) — `release_deep_slot`
  then `self.runtime.remove(&input)`; its doc justifies the remove by
  `update`'s `deep_layer(active_layer).contains_key` guard, which is
  active-layer-scoped. `stop_all` (`stage.rs:1170–1179`) keeps entries with
  `just_reset=true` instead. Surfaced by ticket 18's `/code-review`.

**Blocked by:** ticket 19 (this ticket grills the skips ticket 19 makes
visible). Ticket 18 clears the one case that is an unambiguous bug.

**Status:** done (charting) — filed 2026-09-09, grilled 2026-09-10 on `dev`.
All twelve cases decided; three fix tickets filed (21, 22, 23), one keep
(B8) documented in `edit.rs`.

## Decisions (2026-09-10)

| # | case | decision | lands in |
|---|---|---|---|
| A1 | `axis` not reset on disconnect | **fix** — `reset_axis_outputs()` in `tear_down(Disconnect)` | ticket 21 |
| A2 | `analog_repeat` not torn down on disconnect | **fix here** — `analog_repeat.stop_all()` in `tear_down(Disconnect)`; the spec.md "Out of Scope" line moves (user decision: this ticket *is* the "separate effort") | ticket 21 |
| A3 | `axis` not reset on the Digital flip | **fix** — `reset_axis_outputs()` in `tear_down(CaptureModeToDigital)` | ticket 21 |
| A4 | `chord_machine` window not reset on a lifecycle event | **fix** — new `chord::ChordMachine::reset()` in all four `tear_down` arms | ticket 21 |
| A5 | `chord_slots` firings not drained on lifecycle sweeps | **fix** — `chord_slots.drain_firings()` in all four arms (user decision: full match with individual teardown) | ticket 21 |
| A6 | `chord_slots` Toggles survive a Profile switch | **fix** — `chord_slots.stop_all_toggles()` in `tear_down(ProfileSwitch)` (user decision: full match; keybinder spec.md "every active Toggle") | ticket 21 |
| B7 | `SetStagingMode` mid-press | **fix** — `SetStagingMode` pushes `Effect::StopStage(input)` (user decision: force-release the slot); depends on B12 | ticket 23 |
| B8 | `SetDeepActuation` under a live slot | **keep** — `analog::observe` re-thresholds cleanly next tick; a band exit emits `ReleaseDeep`; no orphan. Rationale comment added to the `edit.rs` `SetDeepActuation` variant doc. | (doc only) |
| B9 | `SetAxisAssignment` orphans a live Binding / Chord / Toggle | **fix** — removed-Binding branch pushes `StopToggle` + `cascade_orphaned_deep_stage`; removed-Chord loop pushes `StopChord(key)` ×N | ticket 22 |
| B10 | `SetBinding` replacing a live Toggle's binding | **fix** — push `Effect::StopToggle(input)` on a replace that changes the binding (user decision: not the spec's "stops on next press" model — chosen for consistency with B9/B11) | ticket 22 |
| B11 | `SetChordBinding` / `ClearChordBinding` push nothing | **fix** — new `Effect::StopChord(ChordKey)`; `ClearChordBinding` unconditional, `SetChordBinding` on a changed replace | ticket 22 |
| B12 | `Effect::StopStage`'s unconditional `runtime.remove` misfires a cross-layer clear | **fix** — `stop_stage` resets-and-keeps the entry (`just_reset = true`) like `stop_all` instead of removing it | ticket 23 |

**Clustering rationale.** A1–A6 share one diff (`DispatchState::tear_down` +
one new `chord` method) and one test file (`dispatch::tests::tear_down_*`) →
ticket 21. B9/B10/B11 share the "an `edit` arm forgot its teardown `Effect`"
shape and a new `Effect::StopChord` → ticket 22. B12 + B7 are both
`stage.rs`-local and B7's fix is unsound without B12's → ticket 23.

**ADR?** No. ADR-0010 already records the teardown *mechanism*; ticket 20's
grilling + this decision table is the behaviour record. No *keep* decision is
surprising enough to warrant one (B8 is the only keep and it is unremarkable).

**Follow-on:** fix tickets 21 / 22 / 23 are mutually independent and can land
in any order.

**2026-09-10 — superseded for B7 / B9 / B10 / B11.** Ticket 25
(`reconcile_teardowns` + ADR-0011) re-solves these four structurally: the
per-arm `StopToggle` / `StopStage` / `StopChord` pushes those cases added
(plus `cascade_replaced_toggle` / `cascade_orphaned_deep_stage`) are deleted,
replaced by one pure diff of the active Profile's runtime-bearing maps called
once at the end of `edit::plan`. The "fix in an arm" disposition above kept
leaking (23-B7 after 22, then 23-B12, then 24); a diff removes the arm from
the teardown loop entirely. B8 (keep), B12 and 24 (`stage.rs` internals
downstream of the effect) are unaffected.

## Comments

**2026-09-10** — Grilled all twelve cases against the code on `dev`
(`dispatch::tear_down`, `stage.rs`, `edit.rs`, `chord.rs`, `axis.rs`,
`analog_repeat.rs`, `trigger::Slots`, both spec.md files). Decision table
above. Ten fixes, one keep (B8), and B9's individual-Toggle sub-case folded
into B10's decision. Four calls went to the user (`AskUserQuestion`): A2 =
take the Analog-repeat disconnect fix here rather than leaving it a future
"separate effort"; A5 + A6 = full match between Chord and individual teardown;
B7 = force-release the deep slot on a mode change; B10 = push `StopToggle` on
a Binding replace (not the spec's "stops on next press"). Fix tickets filed:
**21** (A1–A6, the lifecycle-teardown matrix), **22** (B9/B10/B11, config
edits that orphan individual / Chord runtime state), **23** (B12 + B7,
`stop_stage` reset-and-keep + `SetStagingMode` teardown). B8's keep rationale
added to the `edit.rs` `SetDeepActuation` variant doc. No ADR. `spec.md`'s
"Out of Scope" Analog-repeat line moves when ticket 21 lands (not now — the
gap is real until the fix ships). Charting complete.

**2026-09-09** — Filed from the fifth architecture review's grilling. The
review's candidate 1 grilling (see tickets 18, 19) surfaced this cluster while
mapping the teardown matrix: ticket 18 took the one clear bug, ticket 19 makes
every remaining hole *visible* without deciding it, and this ticket is where
each hole gets decided. Deliberately a charting ticket — the cases don't share
a fix, only a cause.
