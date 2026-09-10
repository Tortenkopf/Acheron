<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 23 — `stage::Engine::stop_stage` reset-and-keep + `SetStagingMode` mid-press teardown (ticket 20 cases B12, B7)

**What to build:** One `stage.rs` change (`stop_stage` stops removing the
runtime entry) plus one `edit.rs` addition (`SetStagingMode` pushes
`Effect::StopStage`). B7's fix depends on B12's, so they ship together.

## B12 — `stop_stage`'s unconditional `runtime.remove` misfires a cross-layer clear

`stage::Engine::stop_stage` (`stage.rs`) is:

```rust
pub(crate) async fn stop_stage(&mut self, input: Input, injector: &Injector) {
    self.release_deep_slot(input, injector).await;
    self.runtime.remove(&input);
}
```

Its doc justifies the `remove` by `Engine::update`'s
`profile.deep_layer(active_layer).contains_key(&input)` guard "already skips
this `input` for good" — **true only when the cleared layer is the active
layer.**

Failure case: a grid key carries a deep stage on **both** Base and Held. One
layer's deep Binding is cleared (`Edit::ClearDeepStage`, or `Edit::ClearBinding`'s
`cascade_orphaned_deep_stage`) while the key is physically held into the deep
band on the **other**, still-valid layer. `stop_stage`:

1. `release_deep_slot` force-releases the live firing on the *still-valid*
   layer — a genuine, wrong release of a stage the user is actively holding.
2. `runtime.remove(&input)` drops the band tracking.
3. Next `Engine::update` tick: the guard `deep_layer(active_layer)
   .contains_key(&input)` is **still true** (the active layer's deep Binding
   was not the one cleared) → `runtime.entry(input).or_default()` recreates
   the entry with `just_reset = false` → `advance((Up, Up), (Down, Down), …)`
   mechanically replays a full press through every band → **re-fires** a deep
   stage that never physically moved (release-then-re-press).

Introduced for `ClearDeepStage` by ticket 18; pre-existing for `ClearBinding`'s
cascade. Reachable via the D-Bus `layer` argument or the GUI editing the
non-active layer. Ticket 18's `/code-review` logged it here rather than
widening that bug fix (the fix needs a `stage.rs` change + a pure test, and
active-layer context `edit::plan` can't see).

**Fix:** `stop_stage` resets-and-keeps the entry, exactly as `stop_all` does
per key:

```rust
pub(crate) async fn stop_stage(&mut self, input: Input, injector: &Injector) {
    self.release_deep_slot(input, injector).await;
    if let Some(rt) = self.runtime.get_mut(&input) {
        *rt = KeyRuntime { just_reset: true, ..KeyRuntime::default() };
    }
}
```

- **Cross-layer case:** the next `update` tick sees `just_reset = true`,
  silently re-adopts wherever Depth currently sits (mid-band included) with
  **no ops emitted** — the key the user is still holding keeps working, no
  phantom re-press. (It *is* still briefly force-released by
  `release_deep_slot` — see "Residual" below.)
- **Same-layer case (the common one):** the guard goes false on the very next
  tick and the stale `just_reset = true` entry is never ticked again — a
  harmless inert `HashMap` entry, bounded by grid size, identical to what
  `stop_all` leaves behind.

**Residual (acceptable, note in the doc):** `release_deep_slot` still runs
unconditionally, so the cross-layer held stage *is* force-released for one
frame before `update` re-adopts it. Fully fixing that needs active-layer
context in the effect (which layer was cleared vs. which is live) — out of
scope here; the `just_reset` change removes the *re-fire*, which is the
user-visible double-actuation. A follow-up could pass the cleared `layer`
through `Effect::StopStage(Input, Layer)` and skip `release_deep_slot`
when it isn't the active layer — flagged, not required.

### `stop_stage` doc rewrite

The current doc's "The runtime entry is removed outright (not reset-and-kept,
unlike `stop_all`'s per-key `just_reset` dance) because `Engine::update`'s own
… guard already skips this `input` for good" paragraph is **wrong for the
cross-layer case** and is replaced: `stop_stage` now resets-and-keeps like
`stop_all`, so a cross-layer clear silently re-adopts and a same-layer clear
leaves an inert entry.

## B7 — `SetStagingMode` mid-press pushes no teardown

`Edit::SetStagingMode` (`edit.rs`) does
`deep_stages.entry(input).or_default().mode = mode` and pushes nothing (doc:
"No `Effect`, same reasoning" as `SetDeepActuation`).

Flipping to/from `StagingMode::QuickSkip` under a live deep firing is unsafe
in a way `SetDeepActuation` is not: Quick-Skip carries its own per-press phase
(`rt.quick_skip: Option<QuickSkipPhase>`), and the next `advance(prev, next,
new_mode, rt.quick_skip)` runs the **new** mode's transition table against a
phase value the **old** mode wrote (or a `None` the new Quick-Skip logic
doesn't expect). The deep slot can be left in an inconsistent phase — no
clean "the next tick reconciles" guarantee, unlike `SetDeepActuation` where
`analog::observe` just re-thresholds.

Ticket 20 decided **force-release the slot**.

**Fix:** `Edit::SetStagingMode` pushes `Effect::StopStage(input)`
unconditionally after the `mode =` write (like `ClearDeepStage`). With B12's
`stop_stage` now reset-and-keep:

- a key **not** currently staged: `release_deep_slot` no-ops, the runtime
  entry (if any) resets to `just_reset = true`, next tick re-adopts at rest —
  fine.
- a key **currently** holding a live deep firing: force-released, runtime
  reset, next tick re-adopts at current Depth under the **new** mode with a
  clean `quick_skip = None` — the mode change takes effect from a known state.

This is only safe *because* B12 changes `stop_stage` to reset-and-keep — a
`SetStagingMode` on the active layer with the key held would otherwise hit the
exact re-fire bug B12 describes (the deep Binding is untouched, so the guard
stays true). Hence one ticket.

### `SetStagingMode` doc rewrite

"No `Effect`, same reasoning" → it now pushes `Effect::StopStage(input)`
because a mode change (unlike an actuation-point change) can strand the
Quick-Skip phase machine; the live slot is force-released so the new mode
starts from a known state (ticket 20 B7).

## Tests

- **`stage.rs` pure test (B12):** the cross-layer scenario, driven through
  `Engine` directly — seed a `KeyRuntime` with `deep = KeyState`-in-band and a
  live slot, call `stop_stage(input, injector)`, then an `update` tick with
  the same in-band Depth and the guard still true; assert the tick emits **no
  ops** (`just_reset` re-adoption) rather than a `FireDeep` replay. A second
  case: guard goes false → entry stays inert, no panic, no ops.
- **`stage.rs` pure test (B7):** `stop_stage` on a key with
  `quick_skip = Some(Skipped)` resets it to `None`.
- **`edit.rs` unit test (B7):** `SetStagingMode` →
  `outcome.effects == vec![Effect::StopStage(input)]`.
- **`dispatch.rs` pipeline test (B12):** a dual-stage key with a deep stage on
  both Base and Held, physically held into the deep band on Held (active),
  `ClearDeepStage` for **Base** → the `uinput` stream shows **no**
  release-then-re-press of the Held deep key (one brief `release_deep_slot`
  drop is tolerable; a full `value=0` then `value=1` re-press is the
  regression).
- **`dispatch.rs` pipeline test (B7):** a live deep Hold-to-repeat, then
  `SetStagingMode` → deep key released, no further output until a fresh
  physical deep crossing.
- **Kept unchanged:** every existing `dual_stage_*` test, `stage.rs` handoff /
  no-return / quick-skip transition tables, ticket 18's
  `dual_stage_clearing_the_deep_stage_force_releases_a_live_deep_toggle_immediately`
  (still passes — a same-layer clear still force-releases; only the runtime
  bookkeeping changed).

## Docs

- **`stage.rs`** — `stop_stage` doc rewritten (above); the `KeyRuntime
  ::just_reset` doc gains `stop_stage` alongside `stop_all` as a writer.
- **`edit.rs`** — `SetStagingMode` variant doc + `Effect::StopStage` doc
  (add `SetStagingMode` to the list of sources: `ClearBinding` cascade,
  `ClearDeepStage`, `SetStagingMode`).
- **`dbus/mod.rs`** — if the `set_staging_mode` method doc mentions "no
  runtime effect", correct it (ticket 18 hit the same stale-doc issue on
  `clear_deep_stage`).
- **No ADR / no `CONTEXT.md`.**
- **`.scratch/README.md`** — extend the `post-release-development` line.

## Cases from ticket 20 NOT in this ticket

- **A1–A6** — ticket 21. **B9 / B10 / B11** — ticket 22. **B8** — keep,
  rationale comment landed with ticket 20.

**Blocked by:** None. Independent of tickets 21 / 22. (B12 touches only
`stage.rs`; B7 adds one `edit.rs` push. No overlap with ticket 22's
`edit` arms.)

**Status:** done — 2026-09-10.

## Comments

**2026-09-10** — Filed from ticket 20. B12 is the latent hole ticket 18's
`/code-review` surfaced and deferred here; B7 is a ticket-20 "decision needed"
that came back **force-release the slot** (`AskUserQuestion` round), and its
fix is only sound once B12 makes `stop_stage` reset-and-keep — so they are one
ticket. Both are `stage.rs`-local plus one `edit.rs` line, disjoint from
ticket 22.

**2026-09-10 — implemented.**

- **B12** — `stage::Engine::stop_stage` now resets-and-keeps the runtime
  entry (`KeyRuntime { just_reset: true, ..default() }`, exactly as
  `stop_all` does per key) instead of `runtime.remove`. `stop_stage` doc
  rewritten (the stale "removed outright because the `deep_layer` guard
  already skips this input" paragraph replaced with the cross-Layer /
  same-Layer split + the accepted one-frame `release_deep_slot` residual);
  `KeyRuntime::just_reset` doc gains `stop_stage` as a second writer.
- **B7** — `Edit::SetStagingMode` pushes `Effect::StopStage(input)` after the
  `mode =` write **when the mode actually changed** (an idempotent same-mode
  re-apply is a config no-op and pushes nothing — `/code-review` finding: an
  unconditional push would drop a deep firing the user is holding on a
  redundant D-Bus/GUI re-set). Runtime state is still never inspected — the
  guard is purely `entry.mode != mode`. Variant doc + `Effect::StopStage` doc
  (now lists three sources: `ClearBinding` cascade, `ClearDeepStage`,
  `SetStagingMode`) + `dbus::set_staging_mode` method doc updated.
- **Tests** — `stage.rs`: `stop_stage_reset_and_keep_lets_a_cross_layer_held_stage_re_adopt_without_re_firing`,
  `stop_stage_same_layer_clear_leaves_an_inert_entry_no_panic_no_ops`,
  `stop_stage_resets_a_stranded_quick_skip_phase_to_none`. `edit.rs`: the
  existing `set_staging_mode_*` test now asserts `Effect::StopStage`, plus
  `set_staging_mode_pushes_stop_stage_even_on_an_existing_entry` and
  `set_staging_mode_to_the_same_mode_pushes_no_effect`. `dispatch.rs`:
  `dual_stage_cross_layer_clear_does_not_re_press_the_still_held_deep_stage`
  (B12 pipeline), `dual_stage_set_staging_mode_mid_press_releases_the_live_deep_stage`
  (B7 pipeline). All prior `dual_stage_*` / `stage.rs` transition-table tests
  unchanged and green (daemon 565, clippy clean).
- **Residual (flagged, not done):** `release_deep_slot` still runs
  unconditionally in `stop_stage`, so a cross-Layer held deep stage is
  briefly force-released for one frame before `update` re-adopts it.
  Removing that needs the cleared `Layer` threaded through
  `Effect::StopStage` — a follow-up, out of scope here.
- **Not touched:** A1–A6 (ticket 21, done), B9/B10/B11 (ticket 22), B8 (keep,
  rationale in `edit.rs` `SetDeepActuation` doc).
