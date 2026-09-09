<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 18 — Config-edit teardown-effect omissions: the `ClearDeepStage` orphan + the `StopStage` doc/code mismatch

**What to build:** Two small, self-contained `edit.rs` fixes, decoupled from
ticket 19's teardown refactor so the live bug leaves `dev` immediately.

1. **`Edit::ClearDeepStage` pushes no teardown effect.** Clearing a deep stage
   that currently holds a live deep firing — a Toggle, or a single-key
   Hold-to-repeat sitting in `value=1` autorepeat — orphans that firing: the
   `deep_layer` entry is gone, so `stage::Engine::update`'s
   `profile.deep_layer(active_layer).contains_key(&input)` guard skips the
   Input on every subsequent tick and the engine never releases it. A deep
   Toggle survives until the GUI's next focus-`StopAllToggles`; a deep
   Hold-to-repeat stays stuck until a Layer or Profile switch. Fix: push
   `Effect::StopStage(input)` after a successful `remove`, mirroring
   `cascade_orphaned_deep_stage`'s `is_some()` branch.

2. **`Effect::StopStage`'s doc claims `SetBinding` pushes it; the code does
   not.** `edit.rs`'s `Effect::StopStage` doc comment says it is *"pushed by
   `SetBinding`/`ClearBinding` when the edit cascades away an orphaned
   `deep_base`/`deep_held` entry"*, but only the `Edit::ClearBinding` arm calls
   `cascade_orphaned_deep_stage`. `SetBinding` deliberately does **not** — it
   replaces a primary in place, leaving the deep stage legal (and a
   `SetBinding` that would make the deep stage illegal is rejected by
   `config::validate` first). Decide which is right (the code, almost
   certainly) and fix the doc to match: `StopStage` comes from `ClearBinding`'s
   cascade and, as of fix 1, from `ClearDeepStage` directly.

Nothing else changes. No new `Effect` variant, no runtime code, no D-Bus
surface. `Effect::StopStage`'s `run_effects` handler
(`dispatch.rs` → `stage.stop_stage(input, injector)`) and
`stage::Engine::stop_stage` already do exactly the right thing: per-Input,
immediate, `stop_toggle` + `force_release`, and it drops the runtime entry
outright (safe here — the `deep_layer` entry is gone, so `update`'s guard skips
the Input for good, the same justification `cascade_orphaned_deep_stage` relies
on).

## The friction

`edit::plan` is the single point where a config mutation derives the runtime
`Effect`s that keep dispatch's ephemeral state consistent with the just-committed
`Config`. For the **axis** path this is complete — `SetAxisAssignment` and
`ClearAxisAssignment` both push `RecomputeAxes` / `ForgetAxisContribution`. For
the **deep-stage** path it is wired only for the *primary-removal cascade*
(`ClearBinding` → `cascade_orphaned_deep_stage` → `StopStage`); the direct
deep-stage edits — `ClearDeepStage`, `SetStagingMode`, `SetDeepActuation` — are
all bare.

`ClearDeepStage`'s variant doc rationalises the omission:

> Does **not** cascade-clear `deep_stages` or force-release a live slot —
> ticket 06's runtime-teardown sweep covers only a *primary* Binding's removal
> cascading the deep one away … editing the deep Binding directly isn't one of
> spec.md's five listed transitions, so this stays as-is.

That reasoning predates recognising the live-firing case: "editing the deep
Binding" is fine, but *removing* it while it holds a running Toggle or
autorepeat is the same orphan `ClearBinding`'s cascade exists to prevent. The
GUI's "Clear deep stage" button (`binding_editor`) calls this edit directly.

**Deletion test.** There is nothing to delete — this is a missing
`effects.push`. The fix concentrates nothing; it closes a hole. But it is the
*instance* fix that ticket 19's `tear_down(reason)` refactor makes a *class*
fix: once every lifecycle and cascade teardown routes through one contract, a
new `Edit` variant that forgets its effect is visible in one match, not
scattered.

## Scope boundary — what is NOT in this ticket

The fact-finding for ticket 19's grilling surfaced a cluster of related
"an edit mutates config out from under live runtime state without tearing it
down" cases. They are **behaviour decisions**, not oversight fixes, and go to
ticket 20 for a focused grilling:

- `SetStagingMode` mid-press — a live Handoff deep firing is not torn down when
  the mode flips to/from Quick-Skip.
- `SetAxisAssignment` — atomically removes any existing `Binding` / Chord
  membership for `(layer, input)` but pushes no `StopToggle` / `StopStage` /
  chord-firing release for a live one it just orphaned.
- `SetBinding` replacing a live Toggle's binding with a different action —
  no `StopToggle` for the now-stale Toggle.
- `SetChordBinding` / `ClearChordBinding` — push *no* effects at all; deleting
  the binding under a live Chord Toggle leaves it running.

Only the `ClearDeepStage` orphan (a genuine stuck-key bug, reachable from a
shipped GUI button) and the `StopStage` doc/code mismatch (trivial) are in
scope here.

## Tests

- **New — `edit.rs` unit test:** `ClearDeepStage` on a config with a
  `deep_base`/`deep_held` entry asserts
  `outcome.effects == vec![Effect::StopStage(input)]` (the existing
  `ClearDeepStage` tests assert config state only). Mirror the
  `ClearBinding` → `StopStage` assertion already at `edit.rs`'s
  `ClearBinding` test.
- **New — `dispatch.rs` pipeline test:** drive a dual-stage key into a live
  deep Toggle (and, separately, a single-key deep Hold-to-repeat), then a
  `ClearDeepStage` command; assert the injected `uinput` stream shows the deep
  key released (`value=0`) and no further output. This is the regression net
  for the stuck-key symptom.
- **Kept:** all `dual_stage_*` pipeline tests and `stage.rs` pure tests
  unchanged.

## Docs

- **`edit.rs`** — the `Effect::StopStage` doc comment corrected; the
  `ClearDeepStage` variant doc's "this stays as-is" paragraph rewritten to say
  it now force-releases a live deep slot, same as the primary cascade.
- **No ADR** — a bug fix, no decision.
- **No `CONTEXT.md`** — no new concept.
- **`.scratch/README.md`** — extend the `post-release-development` line with
  ticket 18.

## Decisions from the grilling (2026-09-09)

- **Q1 = decouple.** The `ClearDeepStage` fix ships as its own change, not
  carried by ticket 19's refactor — the refactor is judged on teardown
  legibility + the deadline-arm dedup alone.
- **Q6 = this batch is `ClearDeepStage` + the `StopStage` doc/code mismatch
  only.** The `SetStagingMode` / `SetAxisAssignment` / `SetBinding`-replace /
  `SetChordBinding` cases are behaviour calls → ticket 20; don't let the batch
  sprawl.

## Facts dug from the code during the grilling (not asked of the user)

- `Edit::ClearDeepStage` handling: `edit.rs:727–735` — `deep_layer_mut(layer)
  .remove(&input)`, `Err(CommandError::NotFound)` if absent, **no
  `effects.push`**.
- `cascade_orphaned_deep_stage`: `edit.rs:785–798` — same `remove`, then
  `effects.push(Effect::StopStage(input))` on `is_some()`. Called only from
  `Edit::ClearBinding` (`edit.rs:417`).
- `Effect::StopStage` defined `edit.rs:320–326`; its `run_effects` arm
  (`dispatch.rs` ~669) → `self.stage.stop_stage(input, &self.injector).await`.
- `stage::Engine::stop_stage` (`stage.rs` ~1195) → `release_deep_slot`
  (`stop_toggle` + `force_release`, unconditional) and removes the runtime
  entry; its own doc justifies the removal by `update`'s `deep_layer(...)
  .contains_key` guard — which holds for `ClearDeepStage` too.
- Working-path coverage: `edit.rs` `ClearBinding` test asserts
  `outcome.effects == vec![Effect::StopStage(Input::Grid(1, 1))]`; the
  `ClearDeepStage` tests assert config only.

**Blocked by:** None.

**Status:** done — filed 2026-09-09, implemented 2026-09-09 on `dev`.

- [x] `Edit::ClearDeepStage` arm pushes `Effect::StopStage(input)` on a
      successful `remove`.
- [x] `Effect::StopStage` doc comment corrected (`ClearBinding` cascade +
      `ClearDeepStage`, not `SetBinding`); `ClearDeepStage` variant doc
      rewritten.
- [x] `edit.rs` unit test: `ClearDeepStage` → `vec![Effect::StopStage(input)]`
      (`clear_deep_stage_pushes_stop_stage_to_force_release_a_live_deep_slot`).
- [x] `dispatch.rs` pipeline test: `ClearDeepStage` releases a live deep
      Toggle and a live deep Hold-to-repeat
      (`dual_stage_clearing_the_deep_stage_force_releases_a_live_deep_toggle_immediately`,
      `..._hold_to_repeat`).
- [x] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, full
      daemon suite green; GUI suite unaffected.
- [x] `/code-review` on Standards + Spec axes.
- [x] `.scratch/README.md` `post-release-development` line extended.

## Comments

**2026-09-10** — Implemented on `dev`. `Edit::ClearDeepStage` pushes
`Effect::StopStage(input)` after a successful `remove`; the `Effect::StopStage`
and `ClearDeepStage` variant docs plus the D-Bus `clear_deep_stage` method doc
(`dbus/mod.rs`) corrected. New `edit.rs` unit test + two `dispatch.rs` pipeline
tests (deep Toggle + single-key deep Hold-to-repeat). `cargo fmt` / `clippy
-D warnings` clean, daemon suite 548 green (was 546), GUI untouched.

`/code-review` (Standards + Spec) returned two findings: (1) the D-Bus method
doc was stale — **fixed** in this diff; (2) `stage::Engine::stop_stage`'s
unconditional `runtime.remove` can misfire a *cross-layer* clear (a key with a
deep stage on both Base and Held, one cleared while the other is held live) —
a real latent hole that this change widens from `ClearBinding`'s cascade to
`ClearDeepStage`, but the proper fix needs a `stage.rs` change and active-layer
context `edit::plan` can't see. Logged as **ticket 20 case B12** rather than
expanding this bug fix's scope. The common single-layer path (the shipped GUI
button clears the layer being edited) is correct and tested.

**2026-09-09** — Filed from the fifth architecture review
(`research/architecture-review-2026-09-09.html`, candidate 1 of 6). The review
bundled this bug fix into its top candidate ("close the `ClearDeepStage` orphan
bug structurally"); the grilling (Q1) split it out so the live stuck-key bug is
not held hostage to the refactor. Ticket 19 carries the structural half.
