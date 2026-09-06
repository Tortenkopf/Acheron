# Lock the trigger-driven Macro-repetition floor with regression tests

Type: task
Status: open
Blocked by: 01
Parent: [Humane output rate](../map.md)

> Execution ticket — resolved on this map, not handed off (map Notes: "execution is in
> scope"). Graduated from ticket 03's decision 1. **Test-only — no production logic
> change, no new constant.**

## Question

Ticket 03 established that the *inter-run* cadence of a Trigger mode repeating a whole
Macro is **already** floored to `max(live kernel period, MIN_TOGGLE_LAP 20 ms)` on every
path — `executor::run_toggle_loop`'s `target_lap` for Toggle → Macro, and kernel-`Repeat`
arrival plus the `Slot::FiringUnfinished` overlap guard in `trigger::decide` for
Hold-to-repeat → Macro. "Generalise `MIN_TOGGLE_LAP`" therefore adds no floor that isn't
already present. The risk is a future refactor silently removing that guarantee.

Lock it:

1. **Regression test — Toggle → Macro.** A Toggle wrapping a Macro whose compiled steps
   are `[KeyDown(k), KeyUp(k)]` with no `Delay` paces at `target_lap`, not as fast as the
   injector drains. (Extends the existing `run_toggle_loop` / `MIN_TOGGLE_LAP` tests in
   `executor.rs` if one doesn't already cover the Macro-shaped step vec explicitly.)

2. **Regression test — Hold-to-repeat → Macro.** Assert that a mid-run kernel `Repeat` is
   dropped by the `FiringUnfinished` overlap guard (so a fast Macro cannot re-fire faster
   than kernel `Repeat` arrival), and that a Macro run longer than one `REP_PERIOD`
   re-fires no faster than one run per completion. Locate the right seam — `trigger`'s
   `decide` truth-table tests already exercise `SpawnFireOnce` under
   `Some(Slot::FiringUnfinished)`; this may only need an assertion sharpening + a comment.

3. **Comments.** In `run_toggle_loop` (at the `target_lap` floor) and at the overlap-guard
   arm in `trigger::decide` / `Slots::perform`, a line naming ticket 03's decision 1: this
   is the load-bearing mechanism for the humane-output-rate invariant on trigger-driven
   Macro *repetition*; a Macro fired once and the keystroke cadence within one run are
   deliberately unrestricted (ticket 03 decision 2).

Feeds ticket 04 (the ADR cites the mechanism, not a new constant).
