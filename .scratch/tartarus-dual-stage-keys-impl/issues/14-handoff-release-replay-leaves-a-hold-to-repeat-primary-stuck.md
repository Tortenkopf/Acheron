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

**Status:** ready-for-agent

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

- [ ] The repro test above passes: every primary `value=1` is balanced
- [ ] Same check for the `(Up, Up) -> (Down, Down)` row with the real `Down` arriving after its depth tick (or documented as not affected)
- [ ] `either-or-staging-mode`'s `dual_stage_set_staging_mode_out_of_either_or_mid_press_leaves_nothing_stuck` can drive the real Up before the depth tick (it currently orders them the other way to dodge this bug)
- [ ] Existing dual-stage tests stay green
