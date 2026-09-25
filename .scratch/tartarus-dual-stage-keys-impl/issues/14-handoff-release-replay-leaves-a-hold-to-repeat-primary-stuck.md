<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 14 — Handoff's release replay can leave a Hold-to-repeat primary stuck down

**What to fix:** When a Handoff dual-stage key is released straight from the deep
band and the real primary `Up` event is handled *before* the depth tick that sees
`(Down, Down) -> (Up, Up)`, a Hold-to-repeat primary is left held down. That means
kernel autorepeat until a Layer/Profile switch or disconnect tears it down.
The primary is re-pressed by the replay and never released.

Found while implementing `either-or-staging-mode` ticket 02. The bug reproduces
with plain Handoff and no Either-Or involvement, so it is filed here instead.
Daemon-only.

**Blocked by:** None — can start immediately.

**Status:** resolved

## Repro

Paused-time dispatch test (add to `daemon/src/dispatch.rs` tests; it currently
ends with `[A1, A0, B1, B0, A1]`: the last `KEY_A` down is never released):

```rust
#[tokio::test(start_paused = true)]
async fn dual_stage_handoff_real_up_before_the_release_depth_tick_leaves_nothing_stuck() {
    let config = dual_stage_config(
        StagingMode::Handoff,
        hold_to_repeat_binding(evdev::KeyCode::KEY_A),
        hold_to_repeat_binding(evdev::KeyCode::KEY_B),
    );
    let harness = CommandHarness::spawn(config);
    harness.press_analog(Input::Grid(1, 1), 150).await;
    harness.push_depth([(Input::Grid(1, 1), 150)]);
    settle().await;
    harness.push_depth([(Input::Grid(1, 1), 250)]);
    settle().await;
    // The real Up lands first; the depth tick for the same release second.
    harness.release_analog(Input::Grid(1, 1), 0).await;
    settle().await;
    harness.push_depth([(Input::Grid(1, 1), 0)]);
    settle().await;
    let events = events_of(&harness.shut_down().await);
    // Expect every KEY_A value=1 balanced by a value=0.
}
```

On hardware: a Handoff key, both stages Hold-to-repeat, pressed into the deep band and
released fast. `rx_events` (non-lossy mpsc) and `rx_depth` (coalescing `watch`) have
no ordering guarantee, so the bug is intermittent.

## Root cause

The depth tick runs Handoff's 1-report-skip release row, `[ReleaseDeep,
RepressPrimary, ReleasePrimary]`, in `stage::Engine::update`:

1. `RepressPrimary` → `fire` → `decide((HoldToRepeat, Down))` → `D::HoldKeyDown` →
   `executor::spawn_fire_once(held_key_down_steps(..))`. The key's `value=1` is
   written **by the spawned task**, which also records it in the firing's `held` set.
2. `ReleasePrimary` → `individual.stop_toggle` + `individual.force_release` runs
   straight after, **before that task has run**. `FiringHandle::force_release_stuck`
   drains a still-empty `held` set and releases nothing.
3. The task then runs and presses `KEY_A`. Nothing else releases it: the real `Up` is
   already consumed.

When the depth tick comes first instead, the later real `Up`'s ordinary
`ForceReleaseStuck` runs after the task has populated `held`, so the key is released.
That ordering is why the tests and hardware have mostly been fine.

## Scope notes

- **Affected:** any primary whose Down spawns a firing that holds a key, meaning
  Hold-to-repeat keyboard, mouse-button and Controller-button primaries. Fire-once
  balances itself; Toggle is stopped by `stop_toggle`.
- **Likely also affected (unverified):** the mirror row `(Up, Up) -> (Down, Down)`,
  `[FirePrimary, ReleasePrimary, FireDeep]`, has the same spawn-then-immediately-
  force-release shape. That row's `Engine::update` doc already accepts a residual gap
  for Toggle; check whether a Hold-to-repeat primary sticks there too.
- No-Return does not re-press on release, so its `(Down, Down) -> (Up, Up)` row is not
  affected.

## Fix direction (implementer's call)

Either make `force_release` see a firing whose task has not run yet (for example,
record `held` synchronously at spawn time for the single-held-key shape), or skip
performing a `RepressPrimary` that the same op sequence releases at once. The skip
is spec-visible (spec.md's mechanical-replay rule says rows are never
short-circuited), so it needs a spec note. The first option fixes the race at its
source.

## Acceptance

- [x] The repro test above passes: every primary `value=1` is balanced
- [x] Same check for the `(Up, Up) -> (Down, Down)` row with the real `Down` arriving after its depth tick (or documented as not affected)
- [x] `either-or-staging-mode`'s `dual_stage_set_staging_mode_out_of_either_or_mid_press_leaves_nothing_stuck` can drive the real Up before the depth tick (it currently orders them the other way to dodge this bug)
- [x] Existing dual-stage tests stay green

## Comments

### 2026-09-26 — fix

- **The release row (the ticket's repro).** Fixed at the source, as suggested.
  - `executor::FiringKeys` puts `held` and a `force_released` latch under one mutex.
  - `FiringHandle::force_release_stuck` sets the latch. A firing force-released before its task ran then force-releases whatever it still holds once its steps finish. This works on the multi-thread runtime too.
  - A balanced Fire-once or Macro holds nothing at the end, so nothing changes for it.
- **The mirror row `(Up, Up) -> (Down, Down)` was affected, by a different mechanism.**
  - With the real `Down` after the depth tick, the late `Down` fired the primary a second time under the deep stage. The release row's `RepressPrimary` then replaced that firing's `firings` entry, so its held key had no owner. This was the same hole as `Engine::update`'s old "Toggle picks up a second loop" residual gap.
  - Fix: `stage::Engine::feed` swallows a non-windowed real `Down` while the primary is handed off, since that edge was already performed by the replay.
  - Any real `Up` clears the hand-off before `feed`'s early returns, so a quick re-press is never swallowed.
  - Recorded in spec.md under the Handoff table.
- **Hardening.** `trigger::Slots::perform` force-releases and removes a firing it is about to replace, before spawning the new one (`release_displaced_firing`), so an entry still holding a key can't be dropped.
  - Found by a quick re-press whose `Down` beat a coalesced 250→150 depth tick: that tick's `[ReleaseDeep, RepressPrimary]` re-pressed over the live real press.
  - It only ever acts on a finished firing (`decide`'s overlap guard drops a spawn over an unfinished one).

**Residuals.** Both are pre-existing, neither leaves a key stuck, and neither is fixed here:
- **Re-press dropped by the overlap guard.** A re-press `Down` landing after the release row but before its `RepressPrimary` task has run meets `FiringUnfinished`, so the overlap guard drops it. The press's `Repeat`s then emit `value=2` against a key the latch already released.
- **Primary and deep stage autorepeat together.** A quick `Up` + re-press `Down` can both land before a depth tick that coalesces to `(Down, Down)`. The primary is then held alongside a live deep stage with no hand-off recorded, so both autorepeat until the next depth tick.
