# Config-edit runtime-orphan teardown is one diff, not a per-arm push

A committed `Edit` can strand live dispatch runtime state: `SetBinding` replacing a
primary `Binding` under a running individual Toggle, `ClearDeepStage` removing a deep
Binding while its slot is held into the deep band, `SetChordBinding` replacing a Chord
Binding under a live Chord Toggle, `SetAxisAssignment` clearing a key's Binding /
Chord memberships / deep stage all at once. In each case the stranded slot has no
release edge — `dispatch`'s own stop-on-next-press only fires if the key is pressed
again — so `edit::plan` has to emit the matching `Effect::StopToggle` / `StopStage` /
`StopChord` at commit time.

Before this decision that rule was **smeared across ~8 `edit::plan` arms** plus two
effect helpers (`cascade_replaced_toggle`, `cascade_orphaned_deep_stage`), and every
new arm had to *remember* to push. It leaked repeatedly: `post-release-development`
ticket 18 (`ClearDeepStage`), ticket 22 (B9/B10/B11 — `SetAxisAssignment` /
`SetBinding` / `SetChordBinding` / `ClearChordBinding`), then ticket 23-B7
(`SetStagingMode`) — and even after ticket 20 grilled every case and chose "fix in
the arm", the class kept leaking (23-B12, then 24). The sixth architecture review
(2026-09-10) surfaced the cluster; the grilling that followed settled the shape.

## Decision

Config-edit orphan teardown is **one pure function**,
`edit::reconcile_teardowns(before: &Config, after: &Config) -> Vec<Effect>`, called
**once** at the end of `edit::plan` — just before `config::validate(&next)?` — with its
output prepended to the arm's own operation-specific effects. No `edit::plan` arm
pushes a teardown `Effect` any more. This is the config-edit axis of the same move
**ADR-0010** made for the lifecycle axis (`dispatch::tear_down` — one `match` over
`TeardownReason`, no per-call-site fan-out).

`reconcile_teardowns` diffs the **active Profile's** runtime-bearing maps:

| before → after (active Profile) | emit |
|---|---|
| `base` / `held`[input] removed, or `Binding` differs | `StopToggle(input)` |
| `deep_base` / `deep_held`[input] removed, or `Binding` differs | `StopStage(input)` |
| `chords_base` / `chords_held`[key] removed, or `Binding` differs | `StopChord(key)` |
| `deep_stages`[input]`.mode` differs (an absent entry reads as the default mode) | `StopStage(input)` |
| `mode_key_role` `Bound → LayerSwitch` | `StopToggle(Input::ModeKey)` |

It ignores `axis_base` / `axis_held`, `actuation_overrides`, `default_actuation`,
`status_leds`, `macros`, `steppers` — none back a stoppable per-key runtime slot.

**Active-Profile only, and it bails when `before.active_profile != after.active_profile`.**
Every orphan-bearing edit reaches the active Profile through `active_profile_mut` (the
D-Bus surface has no Profile arg), so a non-active Profile can hold no live slot to
orphan. The only edit that changes `active_profile` is `SwitchProfile`, which already
owns `Effect::TearDown(TeardownReason::ProfileSwitch)`; an active-Profile *rename*
leaves the maps byte-identical. Without the bail the diff would read the entire active
layer as "changed".

**Deterministic output contract:** fixed effect-type order `StopToggle` → `StopStage`
→ `StopChord`; keys sorted and de-duplicated within each type; independent of `HashMap`
iteration order. The teardowns lead the arm's own `RecomputeAxes` / `AssertStatusLeds`
/ `RepublishActuation` so a key is released before an edit re-asserts an axis or LED
state on it.

The two effect helpers are gone: `cascade_replaced_toggle` (a pure push, fully
subsumed) is deleted, and `cascade_orphaned_deep_stage` keeps only its `Config`
mutation — dropping `deep_*[input]` from `next` so `config::validate` accepts a primary
removal — renamed to `drop_orphaned_deep_binding`.

### Two deliberate behaviour deltas

The uniform rule is strictly more correct than the old per-arm code in two spots; both
are covered by unit tests:

1. **`SetDeepStage` replacing an existing deep `Binding`** was a bare `.insert` that
   pushed nothing. The diff now emits `StopStage(input)` on a changed deep Binding,
   matching `SetBinding`-replace → `StopToggle` and `SetChordBinding`-replace →
   `StopChord`. Closes a latent instance of the same class.
2. **`SetModeKeyRole`** used to push `StopToggle(ModeKey)` whenever the *new* role was
   `LayerSwitch`, including a `LayerSwitch → LayerSwitch` re-apply (a harmless no-op
   push). The diff emits it only on the `Bound → LayerSwitch` transition.

Because the diff is purely structural, an arm that removes an active-Profile map entry
as a *side effect* now also gets a teardown for that key, where the old per-arm code
emitted nothing: `SetBinding` / `SetChordBinding` assigning a `Step` Action already
bound elsewhere (`take_stepper_direction_elsewhere{,_from_chords}` drops the old
owner's entry → `StopToggle` / `StopChord` for it), and `RenameProfile` rewriting an
active-Profile `SwitchProfile` binding's target (`cascade_rename_profile_switch_targets`
→ `StopToggle` for that key). Both are inert in practice — a `Step` or `ProfileSwitch`
Action is `FireOnce`-validated and can never hold a live Toggle, so `run_effects`
no-ops — and both are consistent with "teardown is a consequence of the mutation";
they are noted here rather than guarded against, since a guard would re-introduce the
per-arm special-casing this decision exists to remove.

## Why a diff, not lint-the-arms

Ticket 20 already tried the per-arm discipline ("edit X orphans slot Y, push effect
Z") and wrote no ADR; the class kept leaking. A *centralized per-arm helper* every arm
must still call has the identical failure mode — the arm can forget the call. A diff
removes the arm from the teardown loop **entirely**: the teardown is a consequence of
the `Config` mutation the arm already makes, derived structurally after the fact.

`edit::plan` is already pure and liveness-blind — it cannot see `individual` /
`chord_slots` / `stage` runtime state — and already emitted `stop_*` unconditionally,
with `run_effects`' handlers no-op'ing when nothing is live. The diff has exactly that
shape: it emits on a *config* difference and lets dispatch sort out what is actually
running.

## Scope boundary

`reconcile_teardowns` decides *whether* to emit `StopStage` etc. — not what the engine
then does with it. The `stage::Engine::stop_stage` internals behind the effect
(reset-and-keep, `deep_repeat` suppression, `primary_handed_off` carry — ADR-0007,
tickets 23-B12 / 24) are unaffected.

The teardown trio is the whole of it. `RecomputeAxes`, `RepublishActuation`,
`ReconcileStepperCursor`, `AssertStatusLeds`, `AnnounceProfileChange`,
`SignalCaptureMode`, and the `CreateMacro` / `CreateStepper` id all stay hand-pushed in
their arms — each fights a pure diff (a `layer` arg plus a deferred active-layer check;
fires on non-orphan actuation edits; keys off a changed `StepperId`). Revisit only if
it pays.

## Consequences

- The "edit X orphans slot Y" rule has one home. A new mutating `Edit` gets orphan
  teardown for free as long as it goes through the active Profile's binding / Chord /
  deep / `deep_stages.mode` / `mode_key_role` maps; `CONTRIBUTING.md`'s "Adding a new
  mutating D-Bus method" step 4 shrinks to cover only non-orphan side effects.
- The ~13 scattered `edit::tests` per-arm effect assertions are kept unchanged (bar the
  two delta rows) — they now exercise `plan` + `reconcile_teardowns` end to end and are
  the guarantee that real callers see no regression. A new `reconcile_teardowns`
  truth-table test covers every matrix row, both deltas, and the negatives.
- **Supersedes** the "fix in an arm" framing of ticket 20's decision table for cases
  B7 / B9 / B10 / B11 — the per-arm pushes those cases added are deleted.

Adjacent to **ADR-0010** (lifecycle-axis teardown) — the two ADRs are the two axes of
one story: what ephemeral runtime state is released, and where the rule for releasing
it lives. Adjacent to **ADR-0007** (dual-stage Depth interpretation) — untouched here;
this is about effect *derivation*, not engine behaviour.
