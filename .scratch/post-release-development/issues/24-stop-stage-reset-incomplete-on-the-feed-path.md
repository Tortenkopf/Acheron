<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 24 — `stop_stage`'s reset-and-keep is incomplete: `deep_repeat` and a handed-off primary still machine-gun after it

**What to fix:** `stage::Engine::stop_stage` (ticket 23's B12 reset-and-keep)
quiesces the `Engine::update` (`rx_depth`) path but not the `Engine::feed`
(`rx_events` `Repeat`) path. After `Effect::StopStage` force-releases a live
deep stage and resets the key's `KeyRuntime`, a still-physically-held key on a
*still-valid* deep stage resumes emitting synthetic output on the next
primary `Repeat` pulse:

- **B12-residual** — a **Hold-to-repeat deep** stage: `deep_repeat` re-fires
  it as a phantom `value=2` autorepeat stream.
- **B7-residual** — a **Hold-to-repeat primary** that had been handed off
  (Handoff / No-Return): the primary resumes machine-gunning, the exact thing
  `KeyRuntime::primary_handed_off` exists to prevent.

Both surfaced by ticket 22's `/code-review`. Neither is reachable by ticket
22's own new push (`SetAxisAssignment` → `cascade_orphaned_deep_stage`)
*except* in the same cross-Layer / deep-stage-on-both-Layers shape as the
cross-Layer `ClearDeepStage` case; the dominant triggers are ticket 23's
`SetStagingMode` and cross-Layer `ClearDeepStage` / `ClearBinding`-cascade.

## Root cause

`stop_stage` does:

```rust
self.release_deep_slot(input, injector).await;   // stop_toggle + force_release
if let Some(rt) = self.runtime.get_mut(&input) {
    *rt = KeyRuntime { just_reset: true, ..KeyRuntime::default() };
}
```

and `Engine::update`'s `just_reset` re-adoption (`stage.rs:552`) rebuilds
**only** `rt.primary` / `rt.deep` from the current Depth, then `continue`s:

```rust
if rt.just_reset {
    rt.primary = new_primary;
    rt.deep = new_deep;
    rt.just_reset = false;
    continue;
}
```

It never reconstructs `rt.primary_handed_off`, and it does nothing to keep the
`feed` path in step with the reset. Two concrete failures:

### 1. `deep_repeat` re-fires a Hold-to-repeat deep stage (phantom `value=2`)

`release_deep_slot` = `stop_toggle` + `force_release`. For a deep
Hold-to-repeat single key the slot holds a `HoldKeyDown` firing;
`force_release` releases the held key (the `value=0` ticket 23 intends) but
**keeps the entry** (`trigger::Slots::force_release`'s documented contract),
so `slots.slot(&StageKey(input))` is now `Some(Slot::FiringFinished)`, not
`None`.

Then, once an `update` tick has consumed `just_reset` and re-adopted
`rt.deep = Down` (the key is still held in the deep band), the next
capture-synthesized primary `Repeat` reaches `feed` → `deep_repeat`
(`stage.rs:1027`):

- `rt.deep == KeyState::Down` ✓ (re-adopted)
- the deep Binding still exists on the active Layer ✓ (`SetStagingMode`
  never touched it; a cross-Layer clear only removed the *other* Layer's)
- `deep_binding.trigger == HoldToRepeat` ✓
- `slot == Some(FiringFinished)` → `trigger::decide((HoldToRepeat, Repeat),
  Some(FiringFinished))` returns `D::RepeatKey(code)` (`trigger.rs:242` — the
  `FiringFinished` branch is **not** `guarded`)
- `perform(RepeatKey)` → `injector.repeat_key(code)` → one `value=2`

…once per primary `Repeat` pulse, for the whole remaining hold. The deep key
was logically released by `stop_stage`; it now types phantom characters until
the user lets go.

(If `release_deep_slot` were changed to *remove* the firing entry so the slot
reads `None`, `decide` takes the `else` branch — `guarded(D::HoldKeyDown)` —
which is a phantom **`value=1` re-press**, worse. The slot state is a trap
either way; the real fix is elsewhere — see below.)

### 2. A handed-off Hold-to-repeat primary resumes machine-gunning

Handoff / No-Return, primary = Hold-to-repeat, held into the deep band:
`update` performed `ReleasePrimary` (`individual.stop_toggle` +
`individual.force_release` — entry kept → primary slot `Some(FiringFinished)`)
and set `rt.primary_handed_off = true`, so `feed` swallows the primary's
`Repeat` pulses (`stage.rs:797`).

`stop_stage` resets `rt.primary_handed_off = false`. The `just_reset`
re-adoption never restores it. Now every primary `Repeat`:

- `feed`: `event.state == Repeat` → `deep_repeat` (failure 1), then
  `self.primary_handed_off(input)` → **false** → `feed` returns
  `NotMine { machine_sequenced: true }`
- `handle_event` runs the ordinary primary path →
  `decide((HoldToRepeat, Repeat), Some(FiringFinished))` → `D::RepeatKey` →
  the primary key emits `value=2` while the finger is still in the deep band

i.e. the primary autorepeats *underneath* the deep stage — the machine-gun
`primary_handed_off` was added to stop.

## Why ticket 23's tests miss it

- `stop_stage_reset_and_keep_lets_a_cross_layer_held_stage_re_adopt_without_re_firing`
  (`stage.rs`) and `dual_stage_cross_layer_clear_does_not_re_press_the_still_held_deep_stage`
  (`dispatch.rs`) use a deep **Toggle** — `release_deep_slot`'s `stop_toggle`
  removes the toggle entry outright, so the slot genuinely reads `None` and
  `deep_repeat`'s Hold-to-repeat-only guard is never entered. A deep
  **Hold-to-repeat** leaves the `FiringFinished` entry that trips `RepeatKey`.
- `dual_stage_set_staging_mode_mid_press_releases_the_live_deep_stage`
  (`dispatch.rs`, B7) *does* use a deep Hold-to-repeat, but after
  `SetStagingMode` it drives `repeat_analog` **without a preceding
  `push_depth`**, so `just_reset` is never consumed, `rt.deep` stays `Up`, and
  `deep_repeat`'s `rt.deep == Down` guard returns early. In production
  `capture::analog` sends a fresh Depth snapshot (→ `update` → `just_reset`
  consumed, `rt.deep = Down`) *and* `Repeat` pulses per report — the ordering
  the test omits.
- Every ticket-23 test uses a Fire-once primary, so `primary_handed_off` is
  never exercised (failure 2).

## Fix options (pick during grilling)

1. **Thread the cleared `Layer` through `Effect::StopStage`** — ticket 23
   already flags this ("`release_deep_slot` still runs unconditionally …
   removing that needs the cleared `Layer` threaded through the effect").
   `stop_stage(input, layer)` becomes a no-op (no `release_deep_slot`, no
   `rt` reset) when `layer != active_layer`, killing the cross-Layer
   `ClearDeepStage` / `ClearBinding`-cascade / `SetAxisAssignment` route
   entirely. Does **not** help `SetStagingMode` (same-Layer, deep Binding
   unchanged) — that still needs option 2 or 3.
2. **Make the `just_reset` re-adoption reconstruct the full band state**, not
   just `primary`/`deep`: re-derive `primary_handed_off` from
   `(new_primary, new_deep, mode)` (Handoff/No-Return with primary Up-in-band
   and deep Down ⇒ handed off), and have `feed`'s `Repeat` path skip
   `deep_repeat` on the tick(s) immediately after a reset (a `just_reset`
   check in `feed`, or a dedicated `deep_repeat`-suppression flag cleared by
   the same re-adoption).
3. **`stop_stage` fully clears the deep slot** (`release_deep_slot` →
   `stop_firing`-style remove) **and** the re-adoption re-fires the deep
   stage cleanly (a `FireDeep` on re-adopt when `new_deep == Down`, replacing
   the current silent `continue`) so the stage is genuinely re-established
   rather than left half-alive. Heavier; changes the "silent re-adopt"
   contract B12 introduced.

Option 1 + a narrow version of option 2 (just the `SetStagingMode` case) is
likely the smallest sound fix.

## Tests to add

- **`stage.rs`** — `stop_stage` then an `update` tick (consuming `just_reset`,
  `rt.deep → Down`) then a `feed(Repeat)`: a deep **Hold-to-repeat** must emit
  **nothing** (no `RepeatKey`, no `HoldKeyDown`).
- **`stage.rs`** — Handoff, Hold-to-repeat primary handed off, `stop_stage`,
  `update` re-adopt, `feed(Repeat)`: the primary must **not** emit
  `RepeatKey` / re-press; `primary_handed_off` is restored (or the pulse is
  otherwise swallowed).
- **`dispatch.rs` pipeline** — the B7 test's own sequence with a `push_depth`
  at the held Depth **between** `SetStagingMode` and the `repeat_analog`, plus
  a Hold-to-repeat primary variant: no phantom `KEY_B` / `KEY_A` output after
  the release until a genuine physical re-crossing.
- **`dispatch.rs` pipeline** — cross-Layer `ClearDeepStage` (deep
  Hold-to-repeat on both Layers, held into the active-Layer band): after the
  clear + a further held-Depth report + a `Repeat`, no phantom deep output.

## Docs

- **`stage.rs`** — `stop_stage` doc's "Residual (accepted)" paragraph is
  rewritten: the accepted residual is *only* the one-frame `release_deep_slot`
  drop **for a deep Toggle**; the `feed`/`deep_repeat` and `primary_handed_off`
  gaps are real bugs closed here, not residuals.
- **`KeyRuntime::just_reset`** doc — note that the re-adoption now also
  reconstructs `primary_handed_off` (if option 2 is taken).
- **`edit.rs`** — `Effect::StopStage` doc gains the `Layer` argument note (if
  option 1 is taken); `SetAxisAssignment` variant doc unchanged in substance.
- **No ADR / no `CONTEXT.md`.**
- **`.scratch/README.md`** — extend the `post-release-development` line.

## Facts dug from the code (not asked of the user)

- `trigger::Slots::force_release` (`trigger.rs:585`) releases but **keeps** the
  firing entry; `trigger::Slots::stop_firing` (`trigger.rs`, added by ticket
  22) releases **and removes** it.
- `trigger::decide` `(HoldToRepeat, Repeat)` (`trigger.rs:239`): `RepeatKey`
  for `Some(FiringUnfinished | FiringFinished)` (un-`guarded`),
  `guarded(HoldKeyDown)` for `None`.
- `Engine::update` `just_reset` branch: `stage.rs:552`. `primary_handed_off`
  writes: `stage.rs:593`–`602`. `feed` `Repeat` / `primary_handed_off` /
  `deep_repeat`: `stage.rs:789`–`800`, `stage.rs:1007`, `stage.rs:1027`.
- `stop_stage`: `stage.rs:1221`. Its doc already flags the `Layer`-threading
  follow-up (`stage.rs:1214`–`1220`).

**Blocked by:** None. `stage.rs`-local plus (option 1) one `Effect::StopStage`
signature change touched in `edit.rs` + `dispatch::run_effects`.

**Status:** done — 2026-09-10.

## Resolution

**Fix taken: option 2 only** (self-contained in `stage.rs`; option 1's
`Effect::StopStage(Input, Layer)` threading was declined — it churns four
`edit` arms + `run_effects` and still can't cover `SetStagingMode`, which
has no single Layer). One mechanism covers both trigger families.

- **New `KeyRuntime::deep_repeat_suppressed`** — set (alongside
  `just_reset`) *only* by `stop_stage`, never `stop_all` (a Layer/Profile
  switch genuinely re-establishes on the new Layer, and `decide`'s
  "`Repeat` with no firing re-presses first" rule covers it). While set,
  `deep_repeat` is a no-op, so a still-valid Hold-to-repeat deep stage
  can't resurrect off the `Slot::FiringFinished` entry `release_deep_slot`
  keeps. Cleared by `Engine::update` the moment it observes the deep band
  physically lift (`new_deep == Up`) — a genuine re-crossing then re-fires
  the stage cleanly.
- **`primary_handed_off` is carried across the `stop_stage` reset, not
  cleared** (`/code-review` finding). `stop_stage` force-releases the
  *deep* slot and resets band tracking — it never re-presses the primary,
  so a primary the machine handed to the deep stage is *still* released;
  clearing the flag was a plain bug. The `feed` swallow now works the
  instant `stop_stage` returns, with no dependence on `rx_depth` /
  `Engine::update` timing — which matters because the two channels have no
  ordering guarantee and `update` may not tick at all during a steady hold
  that stops producing depth reports (`capture::analog` synthesizes
  `Repeat`s off a wall clock regardless).
- **The `just_reset` re-adoption re-confirms `primary_handed_off`** — gated
  on `deep_repeat_suppressed` (scopes it to the `stop_stage` path):
  `matches!(mode, Handoff | NoReturn) && new_primary == Down && new_deep ==
  Down`. This *corrects* the carried flag to `false` if the finger left the
  deep band before the first `update` tick after the reset.
- The ticket's option-2 sketch ("skip `deep_repeat` on the tick(s)
  immediately after a reset" / a `just_reset` check in `feed`) was
  under-specified — `just_reset` is already consumed by the time the
  `Repeat` pulses arrive, so suppression has to persist until the physical
  re-cross. That's what `deep_repeat_suppressed` does.
- Bands (`rt.primary` / `rt.deep`) are *not* also carried — the
  `deep_repeat` re-fire is already blocked in the pre-`update` window by
  the band-tracking reset to `Up`, and after the re-adopt by
  `deep_repeat_suppressed`, so carrying them would only risk perturbing
  ticket 23's silent-re-adopt contract for no gain.

**Tests added** (`daemon` 578, clippy clean):

- `stage.rs` — `stop_stage_then_re_adopt_does_not_resurrect_a_held_deep_hold_to_repeat`
  (deep Hold-to-repeat: `stop_stage` → `update` re-adopt `deep = Down` →
  `feed(Repeat)` emits nothing; a fresh crossing re-fires),
  `stop_stage_carries_primary_handed_off_across_the_reset_no_update_tick_needed`
  (Handoff, Hold-to-repeat primary handed off: `feed(Repeat)` swallowed
  with *no* intervening `update` tick, re-confirmed on the held-Depth
  re-adopt, handed back on deep-band exit),
  `stop_stage_re_adopt_corrects_primary_handed_off_when_the_deep_band_was_left`
  (the carried flag corrected to `false` when the re-adopt sees the finger
  back in the primary band). `StageFixture` gained a `sink` handle + a
  `settle()` helper so stage-level tests can assert on injected output.
- `dispatch.rs` pipeline —
  `dual_stage_set_staging_mode_mid_press_stays_silent_after_a_held_depth_report`
  (the ticket-23 B7 sequence *with* the interleaved `push_depth` it
  omitted, deep Hold-to-repeat),
  `dual_stage_set_staging_mode_mid_press_keeps_a_handed_off_primary_silent`
  (Hold-to-repeat primary variant),
  `dual_stage_cross_layer_clear_leaves_the_held_deep_hold_to_repeat_silent_on_repeat`
  (deep Hold-to-repeat on both Layers, held into the active band, Base
  cleared, then held-Depth report + `Repeat` run). All three reproduce the
  phantom `value=2` / machine-gun with the fix reverted.
- All prior `dual_stage_*` / `stage.rs` tests unchanged and green;
  ticket 23's `dual_stage_set_staging_mode_mid_press_releases_the_live_deep_stage`
  (the no-interleaved-`push_depth` ordering) still passes as-is.

**Docs:** `stop_stage` doc's "Residual (accepted)" paragraph rewritten
(the accepted residual is now *only* the one-frame `release_deep_slot`
drop; the `feed`/`deep_repeat` and `primary_handed_off` gaps are closed,
not residuals); `KeyRuntime::just_reset` + new `deep_repeat_suppressed`
docs; `Engine::feed` routing-matrix doc notes the suppression; `edit.rs`
`Effect::StopStage` doc gains the ticket-24 note (no `Layer` argument —
option 1 not taken). No ADR, no `CONTEXT.md`.

## Comments

**2026-09-10** — Filed from ticket 22's `/code-review`. Ticket 23's B12
reset-and-keep fixed the `Engine::update` mechanical-replay re-fire but the
`Engine::feed` (`rx_events` `Repeat`) path was never brought in step: a
still-valid Hold-to-repeat deep stage re-fires via `deep_repeat` →
`RepeatKey`, and a handed-off Hold-to-repeat primary machine-guns because the
`just_reset` re-adoption drops `primary_handed_off`. Both traced through the
code; ticket 23's tests miss them (deep Toggle instead of Hold-to-repeat;
`repeat_analog` with no interleaved `push_depth`; Fire-once primary only).
Ticket 22's `SetAxisAssignment` → `cascade_orphaned_deep_stage` push reaches
the same `stop_stage` in the cross-Layer / deep-on-both-Layers shape but is
not the main trigger — `SetStagingMode` and cross-Layer `ClearDeepStage` are.
