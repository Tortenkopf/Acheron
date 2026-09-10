<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 22 — Config edits that orphan a live individual / Chord firing or Toggle (ticket 20 cases B9, B10, B11)

**What to build:** `edit::plan` additions so an edit that removes or replaces
a `Binding` / Chord Binding also pushes the teardown `Effect` for whatever
live runtime state it just orphaned — mirroring what `cascade_orphaned_deep_
stage` / `Effect::StopStage` already do for the deep-stage path (ticket 18),
extended to the individual-Toggle and Chord keyspaces. One new `Effect`
variant (`StopChord`), one new cascade helper, and wiring into four `edit`
arms.

Ticket 20's grilling decided all three are **fix**, not keep.

## The cases

### B11 — `SetChordBinding` / `ClearChordBinding` push no effects at all

`ClearChordBinding` (`edit.rs`) removes the key from `chords_mut(layer)` and
pushes nothing. A live **Chord Toggle** for that key is then **permanently
unstoppable**: a Chord Toggle stops *only* via a fresh full-member completion
routed through `chord::feed`, whose `stopping` filter iterates
`chords.keys()` — and the key is gone. (Unlike an individual Toggle, which
`handle_event`'s unconditional `stop_toggle`-on-`Down` at `dispatch.rs:190`
always catches on the next physical press.) A live Chord **firing**
(Hold-to-repeat's `value=1` held key) is likewise stranded — the completed
member's `Up` that would `ReleaseChordFiring` can't reach it. This is the
`SetModeKeyRole` situation exactly ("a still-running one would become
permanently unstoppable via that key" → `Effect::StopToggle(Input::ModeKey)`).

`SetChordBinding` that *replaces* an existing Chord Binding has the same
problem when the trigger mode or Action changes under a live slot.

**Fix:**

- New `Effect::StopChord(ChordKey)` — `run_effects` arm →
  `self.chord_slots.stop_toggle(&key).await;` then force-release any firing
  for that key (`chord_slots` has `stop_toggle`; add a targeted firing
  force-release or reuse `drain_firings`-style logic scoped to one key — the
  `Slots<K>` surface may need a `stop_firing(&key, injector)` sibling to
  `stop_toggle`, matching `stage::Engine::release_deep_slot`'s shape).
- `Edit::ClearChordBinding` pushes `Effect::StopChord(key)` after a successful
  `remove` (unconditional, like `ClearDeepStage`).
- `Edit::SetChordBinding` pushes `Effect::StopChord(key)` when the `.insert`
  returns `Some` (a replacement) **and** the new binding differs from the old
  in trigger mode or Action. (A pure re-Save of an identical binding — the GUI
  does this — must not stop a live Toggle.)

### B9 — `SetAxisAssignment` orphans a live Binding / Chord / Toggle

`SetAxisAssignment` (`edit.rs`) atomically removes any existing `Binding` and
any Chord membership for `(layer, input)`, then pushes only
`Effect::RecomputeAxes { layer }`. It relies on `config::validate` rejecting
illegal combinations — but a live **individual Toggle** on that Input is not
illegal, and neither is a live **Chord firing/Toggle** on a chord the Input
was a member of. Both are orphaned with no release effect.

- The individual Toggle: `dispatch.rs:190`'s `stop_toggle`-on-`Down` still
  fires on the key's next press (even an Analog `Down` from driving the new
  axis), so it is *recoverable* — but it runs its stale Action (looping /
  held) until then. Ticket 20 decided (B10 = "push StopToggle"): a Binding
  removed or replaced under a live individual Toggle should push the release,
  not wait for the next press. **Fix applies here too.**
- The Chord memberships removed: same permanent-orphan problem as B11.

**Fix:** the `SetAxisAssignment` removed-`Binding` branch runs the same
individual-Toggle cascade as B10 (below); its Chord-membership-removal loop
pushes `Effect::StopChord(key)` for each removed `ChordKey` (reusing B11's
effect). A deep stage the removed primary carried is already handled — no:
check whether `SetAxisAssignment`'s `layer_mut(layer).remove(&input)` triggers
the deep cascade. It does **not** today. Add `cascade_orphaned_deep_stage(
&mut next, layer, input, &mut effects)` to the `SetAxisAssignment` arm so a
deep stage on the now-axis key is force-released too (an axis key can't carry
a deep stage — `validate` would reject the end state — so the deep `Binding`
must be cascaded away here, exactly as `ClearBinding` does).

### B10 — `SetBinding` replacing a live individual Toggle's binding

`SetBinding` (`edit.rs`) overwrites `layer_mut(layer).insert(input, binding)`
and pushes no `StopToggle`. A live Toggle keeps running its **old** Action
until the key is next pressed (which stops it via `dispatch.rs:190`). Ticket
20 decided **push StopToggle** (more aggressive than the keybinder `spec.md`
"stops on next press" model, chosen for consistency with B9/B11 — a Binding
change under a live slot releases it now).

**Fix:** new helper

```rust
// edit.rs — sibling of cascade_orphaned_deep_stage
/// A `SetBinding`/`SetAxisAssignment` that removes or replaces `input`'s
/// primary Binding on `layer` pushes `Effect::StopToggle(input)` so a live
/// individual Toggle pinned to that key is released on commit rather than
/// lingering with a stale Action until the key's next press (ticket 20 B10).
/// `edit::plan` is pure — it cannot see `individual` liveness — so the push
/// is unconditional and `run_effects`' `stop_toggle` no-ops when absent,
/// matching `Effect::StopStage`'s own contract.
fn cascade_replaced_toggle(input: Input, effects: &mut Vec<Effect>) {
    effects.push(Effect::StopToggle(input));
}
```

- `Edit::SetBinding` calls it when `insert` returns `Some` (a replacement,
  not a fresh bind) **and** the new binding differs (trigger or Action) from
  the old — same "don't punish a GUI re-Save" guard as B11.
- `Edit::SetAxisAssignment` calls it when `layer_mut(layer).remove(&input)`
  returns `Some`.

### Consistency note — `ClearBinding`

`ClearBinding` today pushes only `cascade_orphaned_deep_stage`; it does **not**
push `StopToggle` for a live individual Toggle on the cleared key (relies on
`dispatch.rs:190`). Once B10 makes `SetBinding`-replace push `StopToggle`,
`ClearBinding` is the odd one out. **Recommendation:** extend
`cascade_replaced_toggle` to `ClearBinding` too, for one rule ("any edit that
removes or replaces a primary Binding releases a live Toggle on that key").
Flagged for the implementer — it is a one-line addition to the `ClearBinding`
arm and keeps the four arms uniform. Not a separate ticket.

## Effects summary

| edit | pushes today | adds |
|---|---|---|
| `SetBinding` (replace, changed) | `ReconcileStepperCursor?` | `StopToggle(input)` |
| `ClearBinding` | deep cascade | `StopToggle(input)` *(consistency)* |
| `SetAxisAssignment` (had a Binding) | `RecomputeAxes` | `StopToggle(input)`, deep cascade, `StopChord(key)` ×N |
| `SetChordBinding` (replace, changed) | — | `StopChord(key)` |
| `ClearChordBinding` | — | `StopChord(key)` |

`Effect::StopChord(ChordKey)` is the one new variant. `Effect::StopToggle`
already exists (`SetModeKeyRole` uses it).

## Tests

- **`edit.rs` unit tests** (assert `outcome.effects`, matching the existing
  `ClearBinding` → `vec![Effect::StopStage(..)]` test):
  - `ClearChordBinding` → `vec![Effect::StopChord(key)]`.
  - `SetChordBinding` replacing a differing binding → includes
    `Effect::StopChord(key)`; re-Saving an identical binding → does **not**.
  - `SetBinding` replacing a differing binding → includes
    `Effect::StopToggle(input)`; fresh bind or identical re-Save → does not.
  - `SetAxisAssignment` over a key that had a Binding + was a Chord member +
    had a deep stage → `RecomputeAxes` + `StopToggle` + `StopChord(key)` +
    `StopStage(input)`.
- **`dispatch.rs` pipeline tests** (the stuck-output regression net):
  - a live Chord Toggle, then `ClearChordBinding` for its key → chord key
    released, no further output, `GetState().active_toggles`-equivalent for
    chords empty.
  - a live individual Toggle, then `SetBinding` replacing its Action → the
    old Action's output stops immediately (not on the next press).
  - a live Chord Hold-to-repeat, then `SetAxisAssignment` on a member → key
    released.
- **Kept unchanged:** the `dual_stage_*` tests, `chord.rs` / `trigger.rs`
  pure tests, existing `edit.rs` config-state assertions.

## Docs

- **`edit.rs`** — `Effect::StopChord` doc (mirror `StopStage`'s); the
  `SetChordBinding` / `ClearChordBinding` / `SetAxisAssignment` / `SetBinding`
  variant docs note the new pushes; `SetStagingMode`'s "No `Effect`, same
  reasoning" is **not** touched here (ticket 23 owns it).
- **`dispatch.rs`** — `run_effects` gains the `StopChord` arm.
- **No ADR** — ticket 20's grilling is the record; these close holes, they
  don't decide architecture.
- **No `CONTEXT.md`.**
- **`.scratch/README.md`** — extend the `post-release-development` line.

## Cases from ticket 20 NOT in this ticket

- **A1–A6** — ticket 21 (lifecycle-teardown matrix).
- **B7 / B12** — ticket 23 (`stop_stage` reset-and-keep + `SetStagingMode`).
- **B8** — decided keep; `edit.rs` rationale comment added by ticket 20.

**Blocked by:** None. Independent of tickets 21 / 23 (different `edit` arms,
different effects). Lands cleanly in any order.

**Status:** done — filed 2026-09-10 from ticket 20's grilling; implemented
2026-09-10 on `dev`.

## Comments

**2026-09-10** — Implemented. `trigger::Slots::stop_firing(&key, injector)`
(single-key `drain_firings` — force-release **and remove**) added as the
sibling to `stop_toggle`. New `Effect::StopChord(ChordKey)` → `run_effects`
runs `chord_slots.stop_toggle` then `chord_slots.stop_firing`. New pure
helper `cascade_replaced_toggle(input, effects)` (sibling of
`cascade_orphaned_deep_stage`) pushes `Effect::StopToggle(input)`. Wiring:
`ClearChordBinding` pushes `StopChord` unconditionally after a successful
remove; `SetChordBinding` pushes it when `.insert` returns `Some` and the
new binding differs (`old != binding`); `SetBinding` pushes `StopToggle` on
the same changed-replace guard; `ClearBinding` pushes `StopToggle` too (the
consistency recommendation — one rule: any edit that removes or replaces a
primary Binding releases a live Toggle on that key); `SetAxisAssignment`
pushes `StopToggle` (primary removed), `cascade_orphaned_deep_stage`'s
`StopStage` (an axis key can't keep a deep stage — this arm didn't trigger
the deep cascade before), and `StopChord(key)` for every Chord membership it
removes. Effect order in `SetAxisAssignment`: `StopToggle`, `StopStage`,
`StopChord`×N, `RecomputeAxes`.

Tests: 4 new `edit.rs` unit tests (`ClearChordBinding` → `[StopChord]`;
`SetChordBinding` changed-replace vs identical re-Save; `SetBinding`
changed-replace vs fresh/identical; `SetAxisAssignment` over a key with a
Binding + Chord membership + deep stage → the full ordered set); 3 new
`dispatch.rs` pipeline tests (Chord Toggle + `ClearChordBinding` → released
+ no further output; individual Toggle + `SetBinding`-replace → old Action
released without a second press; Chord Hold-to-repeat + `SetAxisAssignment`
on a member → key released). Updated the 3 existing tests whose asserted
effect vectors gained `StopToggle`. daemon 572 green, clippy clean. No ADR,
no `CONTEXT.md`, no wire/GUI change (`Effect` is daemon-internal).

**2026-09-10** — Filed from ticket 20. The three B-cases here all have the
same shape: a config edit mutates `Config` out from under a live firing/Toggle
and `edit::plan` forgets the teardown `Effect`. Ticket 18 fixed the
`ClearDeepStage` instance of this; ticket 19 made the omission visible in one
match; this is the individual-Toggle and Chord-keyspace remainder. Decision
carried from ticket 20's `AskUserQuestion` round: B10 = push `StopToggle` on a
Binding replace (not the spec's "stops on next press" model), chosen so B9 /
B10 / B11 share one rule.
