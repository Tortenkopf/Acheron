<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 25 — Config-edit runtime-orphan teardown is one diff, not a per-arm push

**What to build:** One pure `edit::reconcile_teardowns(before: &Config, after:
&Config) -> Vec<Effect>` that diffs the two `Config`s and derives every
`StopToggle` / `StopStage` / `StopChord` an edit orphaned — called **once** at
the end of `edit::plan`, replacing the per-arm teardown pushes and the two
`cascade_*` effect helpers. No `edit` arm pushes a teardown `Effect` after
this. Plus **ADR-0011**, a CONTRIBUTING rewrite, and an annotation on ticket
20's decision table.

Filed from the sixth architecture review + its grilling (2026-09-10).

## Why

Tickets **18**, **22** (B9/B10/B11), and **23-B7** are three consecutive
fixes for one class: an `edit::plan` arm mutates a `Binding` / Chord Binding /
deep stage / mode-key role and **forgets to push the teardown `Effect`** for
the live runtime slot it just orphaned. Ticket 19 made the omissions visible
in one `dispatch::tear_down` match; ticket 20 grilled each and decided
"fix in the arm" — and the class kept leaking (23-B7 after 22, then 23-B12,
then 24). The rule ("edit X orphans slot Y, push effect Z") has no single
home: it is smeared across ~8 arms plus `cascade_replaced_toggle` /
`cascade_orphaned_deep_stage`.

`dispatch::tear_down` + ADR-0010 already did exactly this consolidation for
the **lifecycle** axis (Layer / Profile / disconnect / Digital-flip). This
ticket is the same move for the **config-edit** axis.

**Not in scope** (different classes, already handled or out of reach of a
config diff):

- **21** — the `dispatch::tear_down` matrix holes (lifecycle axis, done).
- **23-B12 / 24** — `stage::Engine::stop_stage`'s reset-and-keep semantics
  and the `feed`-path re-press residual. These are `stage.rs` internals
  *downstream* of `StopStage` being delivered — `reconcile_teardowns` decides
  *whether* to emit `StopStage`, not what it then does.

## Shape

```rust
// edit.rs — sited with take_stepper_direction_elsewhere / profile_switch_references
/// Every teardown `Effect` a committed edit implies, derived by diffing the
/// active Profile's runtime-bearing maps before vs. after. `plan` calls this
/// once, just before `config::validate(&next)?`; its output is prepended to
/// the arm's own operation-specific effects. Pure and liveness-blind by the
/// same contract the arms had — `run_effects`' `stop_*` handlers no-op when
/// nothing is live (`Effect::StopStage` / `StopChord` docs).
///
/// Active-Profile only: every binding / Chord / deep / axis / mode edit goes
/// through `active_profile_mut`, and the D-Bus surface has no Profile arg.
/// Returns empty when `before.active_profile != after.active_profile` — the
/// only edit that changes it is `SwitchProfile`, which owns
/// `Effect::TearDown(TeardownReason::ProfileSwitch)`; an active-Profile
/// rename leaves the maps byte-identical.
pub(crate) fn reconcile_teardowns(before: &Config, after: &Config) -> Vec<Effect>;
```

Call site in `plan`:

```rust
    }
    let mut effects = reconcile_teardowns(config, &next);
    effects.extend(arm_effects);          // the operation-specific ones the arm collected
    config::validate(&next)?;
    Ok((next, Outcome { effects, created }))
```

(Mechanically: the arms keep collecting their operation-specific effects into
the existing `effects` local; rename that to `arm_effects`, and build the
returned vec as `reconcile_teardowns(...)` then `.extend(arm_effects)` so
teardowns lead.)

### Diff matrix

Inspects `base`, `held`, `chords_base`, `chords_held`, `deep_base`,
`deep_held`, `deep_stages` (the `.mode` field only), `mode_key_role`.
Ignores `axis_base` / `axis_held`, `actuation_overrides`, `default_actuation`,
`status_leds`, `macros`, `steppers`.

| before → after (active Profile) | emit |
|---|---|
| `base` / `held`[input] removed, or `Binding` differs | `StopToggle(input)` |
| `deep_base` / `deep_held`[input] removed | `StopStage(input)` |
| `chords_base` / `chords_held`[key] removed, or `Binding` differs | `StopChord(key)` |
| `deep_stages`[input]`.mode` differs (actuation-only change → nothing) | `StopStage(input)` |
| `mode_key_role` `Bound → LayerSwitch` | `StopToggle(Input::ModeKey)` |

**Output contract:** fixed effect-type order `StopToggle` → `StopStage` →
`StopChord`; keys **sorted** within each type; deterministic (the current
`SetAxisAssignment` chord loop iterates `HashMap::keys()` unsorted — its test
already copes). Prepended before the arm's `RecomputeAxes` / `AssertStatusLeds`
/ `RepublishActuation` / … so a key is released before an edit re-asserts an
axis or LED state on it.

### `SetAxisAssignment` falls out for free

Its whole teardown trio is a consequence of the maps it mutates: it removes
the primary `Binding` (→ `base`/`held` diff → `StopToggle`), removes Chord
memberships (→ `chords_*` diff → `StopChord` ×N), and its
`drop_orphaned_deep_binding` call removes `deep_*[input]` (→ `deep_*` diff →
`StopStage`). The arm keeps only its `Config` mutations and the
operation-specific `Effect::RecomputeAxes { layer }`.

## Two deliberate behaviour deltas

The uniform rule is strictly more correct than today's code in two spots.
Both get a `reconcile_teardowns` table row and a one-line note in ADR-0011.

1. **`SetDeepStage` replacing an existing deep `Binding`** pushes **nothing**
   today (the arm is a bare `.insert`). Uniform rule → `StopStage(input)` when
   `deep_*[input]` changes under a live slot — matching `SetBinding`-replace →
   `StopToggle` and `SetChordBinding`-replace → `StopChord`. Closes a latent
   instance of the same class.
2. **`SetModeKeyRole`** pushes `StopToggle(ModeKey)` today whenever the *new*
   role is `LayerSwitch`, including a `LayerSwitch → LayerSwitch` re-apply
   (a harmless no-op push). Uniform rule → only on the `Bound → LayerSwitch`
   transition.

## Code removed

- **`cascade_replaced_toggle`** — deleted (pure effect push, fully subsumed by
  the `base`/`held` diff).
- **`cascade_orphaned_deep_stage`** — keeps its `Config` mutation (drop
  `deep_*[input]` from `next` so `config::validate` accepts a primary removal),
  loses the `&mut Vec<Effect>` param and the `StopStage` push. Rename to
  **`drop_orphaned_deep_binding(next, layer, input)`**.
- Per-arm guards now done by the diff: `SetBinding`'s
  `if replaced.is_some_and(|old| old != binding)`; `SetStagingMode`'s
  `mode_changed` local; `SetModeKeyRole`'s `if role == LayerSwitch`.
- Per-arm pushes now done by the diff: `StopChord` in `SetChordBinding` /
  `ClearChordBinding` / `SetAxisAssignment`; `StopStage` in `ClearDeepStage` /
  `SetStagingMode`; `StopToggle` in `ClearBinding` / `SetBinding` /
  `SetAxisAssignment` / `SetModeKeyRole`.

Arms keep **every `Config` mutation** unchanged — including
`SetAxisAssignment`'s `chords.remove(&key)` loop and its
`drop_orphaned_deep_binding` call, and `ClearBinding`'s
`drop_orphaned_deep_binding` call.

`SetStagingMode` collapses to:

```rust
Edit::SetStagingMode { input, mode } => {
    active_profile_mut(&mut next).deep_stages.entry(input).or_default().mode = mode;
}
```

## Effects summary

| edit | operation-specific (stays in arm) | teardown (now from `reconcile_teardowns`) |
|---|---|---|
| `SetBinding` | — | `StopToggle` iff Binding changed under a live slot |
| `ClearBinding` | — | `StopToggle`, `StopStage` iff a deep stage was orphaned |
| `SetChordBinding` | — | `StopChord` iff Chord Binding changed |
| `ClearChordBinding` | — | `StopChord` |
| `SetAxisAssignment` | `RecomputeAxes { layer }` | `StopToggle`, `StopStage`, `StopChord` ×N (all via the maps it mutates) |
| `ClearAxisAssignment` | `ForgetAxisContribution`, `RecomputeAxes` | — |
| `SetDeepStage` | — | `StopStage` iff replacing a differing deep Binding *(delta 1)* |
| `ClearDeepStage` | — | `StopStage` |
| `SetDeepActuation` | — | — (B8 keep: re-thresholds cleanly) |
| `SetStagingMode` | — | `StopStage` iff `.mode` changed |
| `SetModeKeyRole` | — | `StopToggle(ModeKey)` iff `Bound → LayerSwitch` *(delta 2)* |
| `SwitchProfile` | `TearDown(ProfileSwitch)`, `RepublishActuation`, `AssertStatusLeds`, `AnnounceProfileChange` | — (`reconcile_teardowns` returns empty on an active-Profile change) |

No new `Effect` variant — `StopToggle` / `StopStage` / `StopChord` all exist.

## Tests

- **Kept unchanged** — all ~13 scattered `edit::tests` `#[test]` fns that
  assert `outcome.effects == vec![…]` for a specific arm (`clear_deep_stage_*`,
  `set_staging_mode_*`, the `SetAxisAssignment` combined-effects test,
  `SetChordBinding` changed-replace vs identical re-Save, `SetBinding`
  changed-replace vs fresh, `SetModeKeyRole → [StopToggle(ModeKey)]`, …).
  They exercise `plan` + `reconcile_teardowns` end-to-end — the guarantee
  that real callers see no regression. Additive only; no deletions.
  - Two of them shift with the deltas: add a `SetDeepStage`-replace →
    `[StopStage]` assertion; narrow the `SetModeKeyRole` test to the
    `Bound → LayerSwitch` transition and add a `LayerSwitch → LayerSwitch`
    → `[]` case.
- **New** — one focused `reconcile_teardowns` truth-table test: every matrix
  row, both deltas, and the negative cases (`Binding` present and byte-equal →
  nothing; `deep_stages` actuation-only change → nothing; active-Profile
  change → empty). Table shape like `trigger::tests` / `analog_repeat::tests`.
- **`dispatch.rs` pipeline tests** — unchanged; the existing stuck-output
  regression nets (ticket 22's three, plus the `dual_stage_*` set) already
  cover the observable behaviour and must stay green byte-for-byte.

## Docs

- **ADR-0011 — "Config-edit runtime-orphan teardown is one diff, not a
  per-arm push."** Sibling to ADR-0010:
  - **Context** — the leak history (18 → 22 → 23-B7 → 23-B12 → 24); the rule
    had no single home; ticket 20 chose per-arm and the class kept leaking.
  - **Decision** — one pure `reconcile_teardowns(before, after)` diff, called
    once in `plan`; active-Profile only; bail on `SwitchProfile`; the matrix;
    the deterministic output contract; the two behaviour deltas.
  - **Why a diff, not lint-the-arms** — a per-arm helper every arm must
    *remember* to call has the same failure mode; a diff removes the arm from
    the loop entirely. `plan` is already pure and liveness-blind, already
    emits `stop_*` unconditionally, `run_effects` already no-ops on absent —
    the diff has exactly that shape.
  - **Scope boundary** — decides *whether* to emit `StopStage` etc.; the
    engine internals behind those effects (ADR-0007 / tickets 23-B12, 24) are
    unaffected. Adjacent to ADR-0010 (lifecycle axis); this is the config-edit
    axis of the same story.
  - **Supersedes** the "fix in an arm" framing of ticket 20's decision table
    (cases B7 / B9 / B10 / B11).
- **`docs/adr/0010-…`** — add a one-line pointer to 0011 (the two axes).
- **`CONTRIBUTING.md`**:
  - "Adding a new mutating D-Bus method" **step 4** shrinks — orphan-teardown
    (`StopToggle` / `StopStage` / `StopChord`) is automatic from
    `reconcile_teardowns`; step 4 covers only a side effect that is *not* a
    runtime orphan: republish actuation, recompute axes, signal the
    supervisor, reconcile a stepper cursor, emit a signal.
  - New Conventions bullet **"Changing config-edit teardown"** →
    `edit::reconcile_teardowns` + ADR-0011, mirroring the existing "Changing
    lifecycle teardown" bullet.
- **`edit.rs`** — the `reconcile_teardowns` doc (above); the variant docs for
  the arms lose their teardown-effect paragraphs (the effect is no longer
  theirs) and gain a one-line "teardown effects: see `reconcile_teardowns`";
  the `Effect::StopToggle` / `StopStage` / `StopChord` docs note
  `reconcile_teardowns` as the sole `plan`-side source.
- **No `CONTEXT.md`** — `reconcile_teardowns` is internal plumbing, no domain
  term (ADR-0010 precedent: `tear_down` / `TeardownReason` got none).
- **`.scratch/README.md`** — extend the `post-release-development` line.
- **`.scratch/post-release-development/issues/20-…`** — append to the
  decision table / Comments: "B7 / B9 / B10 / B11 re-solved structurally by
  ticket 25 (`reconcile_teardowns` + ADR-0011); the per-arm pushes those
  cases added are deleted."

## Blocked by

None. Tickets 18 / 22 / 23 are `done` — this rewrites the code they landed,
on `dev`, with their tests as the regression net. Independent of 24.

## Status

**Status:** done — filed 2026-09-10 from the sixth architecture review's
grilling; implemented 2026-09-10 on `dev`.

## Comments

**2026-09-10** — Implemented. `edit::reconcile_teardowns(before, after) ->
Vec<Effect>` added next to `plan`, called once just before
`config::validate(&next)?` with its output prepended to the arm's
now-`arm_effects` local. Diffs the active Profile's `base` / `held` /
`deep_base` / `deep_held` / `chords_base` / `chords_held` (removed **or**
`Binding` differs), `deep_stages[input].mode` (an absent entry reads as the
default mode, so a fresh `.or_default()` entry with a non-default mode counts
as a change — matching the old `entry.mode != mode` guard), and
`mode_key_role` (`Bound → LayerSwitch` only). Bails to empty on an
`active_profile` change (covers `SwitchProfile` and an active-Profile rename).
Deterministic output: `StopToggle` → `StopStage` → `StopChord`, keys
`sort_unstable` + `dedup` within each type.

Deleted `cascade_replaced_toggle`; `cascade_orphaned_deep_stage` →
`drop_orphaned_deep_binding` (keeps only the `deep_*[input]` drop so
`config::validate` accepts a primary removal). Removed the per-arm teardown
pushes / guards from `SetBinding`, `ClearBinding`, `SetModeKeyRole`,
`SetChordBinding`, `ClearChordBinding`, `SetAxisAssignment`, `ClearDeepStage`,
`SetStagingMode` (collapsed to the one-line `.entry().or_default().mode =
mode`). `SwitchProfile` keeps its own effect chain; `SetAxisAssignment` keeps
`RecomputeAxes` and every `Config` mutation (the `chords.remove` loop, the
`drop_orphaned_deep_binding` call).

Two behaviour deltas, both with truth-table rows and a note in ADR-0011:
`SetDeepStage`-replace → `StopStage` (was a bare `.insert`); `SetModeKeyRole`
narrowed to `Bound → LayerSwitch` (a `LayerSwitch → LayerSwitch` re-apply
pushed a harmless `StopToggle(ModeKey)` before).

Tests: all pre-existing `edit::tests` / `dispatch::tests` effect + pipeline
assertions pass **unchanged** except the two the ticket names —
`set_deep_stage_*` gained a replace → `[StopStage]` + identical-re-Save → `[]`
assertion, `set_mode_key_role_*` gained a `LayerSwitch → LayerSwitch` → `[]`
case. New `edit::tests::reconcile_teardowns_covers_every_matrix_row_both_deltas_and_the_negatives`
(every row, both deltas, the byte-equal / actuation-only / active-Profile-change
negatives, the sorted-output contract). `daemon` 579 green (was 578), `cargo
fmt` clean on the touched code, `cargo clippy --all-targets -D warnings`
clean.

Docs: **ADR-0011** written (sibling to ADR-0010), ADR-0010 gains a pointer to
it; `CONTRIBUTING.md` step 4 shrunk + new "Changing config-edit teardown"
Conventions bullet + the dual-stage bullet's `StopStage` line repointed;
`edit.rs` variant docs lost their teardown paragraphs (one-line "see
`reconcile_teardowns`" instead) and the `Effect::Stop*` docs name it as the
sole `plan`-side source; ticket 20's decision table annotated as superseded
for B7/B9/B10/B11; `.scratch/README.md` line extended. No `CONTEXT.md` change
(internal plumbing, ADR-0010 precedent). No wire/GUI change. Hardware
verification not required — pure refactor of effect *derivation*, behaviour
held bar the two unit-tested deltas.

Refined by a `/code-review` pass (Standards + Spec axes). Both axes flagged a
further structural consequence: an arm that removes an active-Profile map
entry as a *side effect* — `SetBinding` / `SetChordBinding` stealing a `Step`
direction off its old owner (`take_stepper_direction_elsewhere`),
`RenameProfile` rewriting an active-Profile `SwitchProfile` binding
(`cascade_rename_profile_switch_targets`) — now emits a `StopToggle` /
`StopChord` for that key where the old per-arm code emitted nothing. Inert in
practice (both Actions are `FireOnce`-validated, so no live Toggle), and
consistent with "teardown is a consequence of the mutation", so documented as
a fall-out rather than guarded: ADR-0011 gained a paragraph and the
`set_binding_with_a_step_action_steals_...` test now asserts the
`StopToggle`. Also from the review: the three identical map-diff loops
collapsed behind a `removed_or_changed(was, now)` helper; the `reconcile_teardowns`
internal profile bindings renamed off the shadowed `before`/`after`; the
per-arm "see `reconcile_teardowns`" notes moved uniformly after the mutation;
`set_mode_key_role_*` test renamed to name the `Bound → LayerSwitch`
narrowing.

**2026-09-10** — Filed. Sixth architecture review
(`research/architecture-review-2026-09-10.html`, candidate 1 of 3) surfaced
the cluster; the grilling that followed settled the shape. Key decisions from
the grilling rounds:

- **Central diff, not a centralized per-arm helper** — the only form that
  makes the omission structurally impossible (the arm is no longer in the
  teardown loop at all).
- **Teardown trio only** (`StopToggle` / `StopStage` / `StopChord`).
  `RecomputeAxes` / `RepublishActuation` / `ReconcileStepperCursor` stay
  hand-pushed — each fights a pure diff (a layer arg + a deferred
  active-layer check; fires on non-orphan actuation edits; keys off a
  changed `StepperId`). Revisit if it pays.
- **Active-Profile only, bail on `SwitchProfile`** — every orphan-bearing
  edit targets the active Profile (no Profile arg on the wire); `SwitchProfile`
  owns `TearDown(ProfileSwitch)`.
- **Two behaviour deltas accepted** (`SetDeepStage`-replace → `StopStage`;
  `SetModeKeyRole` narrowed to the `Bound → LayerSwitch` transition) — both
  strictly more correct, both get table rows.
- **All scattered per-arm effect tests kept** — the regression guarantee;
  the new `reconcile_teardowns` table test is additive.
- **ADR-0011** — ticket 20 deliberately wrote no ADR and the class kept
  leaking; recording the reversal now is the closing bracket.
