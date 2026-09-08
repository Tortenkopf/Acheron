# 13 — Quick-Skip's Armed window is not disarmed on a quick shallow tap

**What to fix:** A Quick-Skip dual-stage key that is tapped quickly and released
*without ever reaching the deep band* can leave its primary stage stuck down —
emitting keypresses forever (kernel autorepeat) until a Layer/Profile switch or
GUI focus fires `stage::Engine::stop_all`. Reported by Charon from testing:
"if both stages are bound to a hold-to-repeat action and the key is tapped
quickly, never crossing into the deep band, the output can sometimes become
stuck and keep emitting keypresses."

Post-ship follow-up from Charon's testing. Daemon-only. Touches
`tartarus-dual-stage-keys/spec.md` too (see below).

**Blocked by:** — (04 landed the Quick-Skip buffer this fixes; 17 relocated the
routing into `stage::Engine::feed`)

**Status:** resolved

## Repro

- A grid key in Analog capture mode, primary + deep stage, `StagingMode::QuickSkip`.
- Both Bindings `Hold-to-repeat` (any single keyboard key). Hold-to-repeat is
  the config that makes it *permanent* — see "Why Hold-to-repeat" — but the
  deadline misfire itself is trigger-mode-independent.
- Tap the key: cross the primary Actuation point, stay shallow (never reach the
  deep band), release — all within ~50 ms is not required; the bug is a race,
  so it is intermittent ("sometimes").
- Sometimes: the primary key latches and autorepeats with no release.

## Root cause

Quick-Skip's outer **press** and outer **release** are handled asymmetrically.

The press is owned directly off the real `rx_events` edge:
`stage::Engine::feed` calls `begin_quick_skip` (`daemon/src/stage.rs`, the
`EventState::Down` arm under `deep_cfg.mode == StagingMode::QuickSkip`), which
arms the ~50 ms deadline at the physically precise moment — deliberately *not*
waiting for `Engine::update`'s coalescing `rx_depth` tick.

The release, while the phase is still `Armed`, is just **swallowed** — the
`EventState::Up if !self.is_late(...)` arm returns
`Ok(StageOutcome::Handled(Vec::new()))` and does nothing else. It never
disarms the window and never records that the key came back up. Cancelling the
`Armed` deadline is delegated *entirely* to the other path: `Engine::update`
observing a `(Down,Up) → (Up,Up)` band transition off `rx_depth` and running
the pure-core row `((Down,Up),(Up,Up)) => (vec![], None)` (tested in isolation
as `quick_skip_cancels_outright_on_an_early_up`).

That delegation is not sound. `rx_events` is a non-lossy mpsc; `rx_depth` is a
**coalescing `watch`**. `capture::analog::relay_grid_blocking` sends, per hidraw
report, the Down/Up event first and then `depth_tx.send_replace(<all 20 keys>)`.
On a quick tap (two reports: press→~150, release→0) the mpsc holds `[Down, Up]`
but the depth watch coalesces to `{KEY: 0}`. If dispatch's `select!` services
the `rx_depth.changed()` arm before draining both queued events (arm order is
random under `tokio::select!`):

1. `update` runs with a fresh `KeyRuntime`: `{KEY: 0}` → `(Up,Up)→(Up,Up)`,
   `prev == next`, no-op. The excursion to ~150 is already coalesced away and
   never recorded in the shadow bands.
2. `Down` dequeues → `begin_quick_skip`: `rt.primary` is still the default `Up`,
   so it resolves from the event's own depth field (shallow) and **arms**;
   `rt.primary = Down`, `rt.quick_skip = Some(Armed { deadline })`.
3. `Up` dequeues → swallowed by the `Armed` arm. Nothing cleared.
4. No further `rx_depth` change is pending — the `{KEY: 0}` snapshot was already
   consumed in step 1 — so `update` never gets another tick to run the cancel
   row.
5. The deadline elapses → `Engine::tick` emits `RepressPrimary` → the primary
   Binding is fired `Down` on the machine-sequenced path, phase → `Late`.

The primary's only real `Up` was consumed in step 3, and `feed` will swallow the
*next* press's `Up` too (it re-arms via `begin_quick_skip`). So nothing ever
releases the primary.

This is exactly the documented `rx_events` ↔ `rx_depth` reordering hazard that
`begin_quick_skip`'s own doc-comment describes — but `begin_quick_skip` only
defends the *arming* decision (is the deep band already hot?) against it, not
the *cancellation*.

### Why Hold-to-repeat makes it permanent

`RepressPrimary` fires the primary `Down`. With `kernel-shaped-repeat`, a
single-key Hold-to-repeat primary is injected as a genuine kernel autorepeat
(`value=1`, then the kernel repeats). No `value=0` will ever follow, so the
kernel autorepeats indefinitely. A Fire-once primary would emit a single stray
keystroke instead — still wrong, but self-limiting.

### Spec gap

`tartarus-dual-stage-keys/spec.md` §"Quick-Skip's primary suppression":
"`Up` while the buffered primary never fired → swallowed (unbalanced, tolerated
…)". That reasoning only covered the *primary keyspace* (nothing to
force-release there) and missed that the **deadline still needs disarming**.
The spec text should be corrected as part of this fix.

## Proposed fix

Handle the Quick-Skip outer release with the same discipline `begin_quick_skip`
gives the outer press — resolve it in the pure core directly off the `rx_events`
edge in `Engine::feed`, rather than swallowing and relying on `rx_depth`:

- `Up` while `Armed` (buffered Down never fired) → drop the phase to `None`
  (the buffered Down is discarded — `quick_skip_advance`'s
  `Some(Armed) … ((Down, Up), (Up, Up)) => (Vec::new(), None)` row already
  encodes this; call it, or a dedicated "outer release" entry, and apply the
  resulting phase + shadow bands to `rt`).
- `Up` while `Skipped` → the No-Return release path
  (`ReleaseDeep` if the deep band is still held, then phase `None`; no
  `RepressPrimary`).
- Keep the `rx_depth` cancel row as a harmless idempotent double-confirm (it
  already tolerates `prev == next`).
- Mirror the same care for a same-report skip that arrives as `Up` in one
  report from the deep band while `Skipped` — `((Down,Down),(Up,Up))`.

`feed` already owns `rt` mutably (it is `&mut self`), so writing the phase and
shadow `KeyState`s back is in reach — this is the same shape as
`begin_quick_skip`.

Watch the interaction with `primary_handed_off` / `deep_repeat` on the same
edge, and with the `Late` fall-through (once `Late`, the real `Up` must still
reach the ordinary path to release the retroactively-fired primary — that path
is correct today and must stay correct).

## Acceptance

- [x] `feed` disarms / resolves the Quick-Skip phase on a real outer `Up`
      itself, without depending on an `rx_depth` tick arriving afterward.
- [x] Regression test at the `stage::Engine` level (the ticket-17 `feed`
      routing-matrix test block): arm via `feed(Down, shallow)`, then
      `feed(Up, …)` → phase is `None`, `next_deadline()` is `None`, a
      subsequent `tick` past the old deadline emits nothing.
      (`feed_quick_skip_shallow_tap_disarms_the_window_on_the_up_edge`)
- [x] Regression test for the race ordering: an `update({KEY: 0})` tick (fresh
      runtime, no-op) *before* `feed(Down)` / `feed(Up)` — the window must still
      end disarmed.
      (`feed_quick_skip_up_disarms_even_when_a_depth_zero_tick_landed_first`;
      end-to-end `dual_stage_quick_skip_quick_shallow_tap_disarms_the_window_on_the_up_edge`)
- [x] `Skipped` + outer `Up` regression: deep stage released, primary never
      touched, phase `None`.
      (`feed_quick_skip_skipped_then_outer_up_releases_the_deep_stage` +
      dispatch `dual_stage_quick_skip_skipped_then_outer_up_releases_the_deep_stage`)
- [x] `tartarus-dual-stage-keys/spec.md` §"Quick-Skip's primary suppression"
      corrected.
- [x] Existing `dual_stage_*` dispatch tests + `stage.rs` unit tests green
      (daemon 545 pass); `cargo clippy --all-targets` / `cargo fmt --check` clean.
- [x] Follow-up (see Comments): `end_quick_skip` also force-releases the primary
      on every non-`Late` outer `Up`, closing the `Late → None` depth-race hole
      where the retroactively-fired primary was left latched.

## Answer

Added `stage::Engine::end_quick_skip(&mut self, injector, input)`, called from
`feed`'s `EventState::Up if !self.is_late(...)` arm before it returns
`Handled(vec![])`. It mirrors `begin_quick_skip`'s discipline: a no-op unless a
Quick-Skip phase is live (`None` ⇒ an `rx_depth` cancel tick already ran, or
never armed), otherwise it drives the pure core with `next == (Band::Up,
Band::Up)`, writes the resulting phase + shadow `KeyState`s back to `rt`, and
performs any `ReleaseDeep` (unconditional `stop_toggle` + `force_release`, same
as `update`'s arm). Phase drops to `None`, which disarms `next_deadline()`.

The `rx_depth` cancel row in `update` is untouched — it stays a harmless
idempotent double-confirm. The `Late` fall-through is unaffected: `feed`'s
`is_late` guard still routes a `Late` key's real `Up` to the ordinary path.

Spec §"Quick-Skip's primary suppression" updated; also dropped the stale
"Additive" naming in the sibling bullet (mode removed by ADR-0009).

## Comments

### 2026-09-08 — follow-up: a second hole (Charon retested, primary still stuck)

The first fix disarmed the `Armed` deadline but Charon's retest still showed a
Hold-to-repeat primary latching. Root cause of the remainder: `feed` swallows
**every** non-`Late` Quick-Skip edge for the ordinary Binding path, so its
"leave a lone `FirePrimary` / `ReleasePrimary` to the real `rx_events` edge"
contract (what `Engine::update` relies on when it `continue`s past a 1-op
primary transition) does not hold for a Quick-Skip key. Race:

1. deadline elapses first → `RepressPrimary` fires the buffered primary
   (`value=1`, phase `Late`).
2. the coalescing `rx_depth` `{KEY: 0}` tick is serviced before the queued real
   `Up`: `update` runs the `Late → None` row, whose lone `ReleasePrimary` it
   `continue`s past.
3. real `Up` reaches `feed` → phase is now `None` → not `Late` → old
   `end_quick_skip` early-returned on `quick_skip.is_none()` → `Handled(vec![])`.
   The `value=1` is never balanced. **Stuck.**

Fix: `end_quick_skip` now takes `&mut Slots<Input>` and **always**
`individual.force_release(&input, injector)` after the phase/deep handling —
the same op the ordinary `(_, Up)` path runs (`ForceReleaseStuck`), a no-op
when the primary never fired (`Armed` / `Skipped`), idempotent otherwise.
`update`'s lone-edge skip is left alone; `feed`'s edge is now the reliable
release point. New regression test
`dual_stage_quick_skip_late_primary_is_released_when_a_depth_tick_beats_the_real_up`
(verified red without the `force_release`). daemon 545 green, clippy + fmt clean.
Spec bullet expanded.
