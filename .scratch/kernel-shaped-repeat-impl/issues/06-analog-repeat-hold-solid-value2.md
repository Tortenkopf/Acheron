# 06 — Analog-repeat hold-solid → genuine `value=2` (surface 5)

**What to build:** Analog-repeat's near-full-travel phase (Depth ≥
`ANALOG_REPEAT_HOLD_SOLID`, 235) stops parking on a bare unbalanced `KeyDown` and
instead emits a self-contained `value=2` stream at the kernel `period_ms` — with
**no initial `REP_DELAY` gap** (this is the top of a hand-driven tapping ramp, not a
fresh press). `value=1` on crossing 235, steady `value=2` at `period_ms`, `value=0`
when Depth leaves hold-solid or the task is cancelled. The tap band below 235 keeps
its current deliberately-human pulse shape (ticket 20) untouched.

This surface does not go through `trigger::decide` — `run_analog_repeat_loop` calls
`injector.repeat_key` directly for the single non-modifier key in its compiled steps.

Source of truth: [`spec-kernel-shaped-repeat.md`](../../humane-output-rate/spec-kernel-shaped-repeat.md)
§4 (surface 5), §5 (envelope table, surface-5 row — "immediately, no `delay_ms`"),
§5.3 (`run_analog_repeat_loop` changes, in full), §5.4 (the `advance_fired_steady`
sibling).

**Blocked by:** 01 — Injector `repeat_key` primitive.

**Status:** ready-for-agent

- [ ] `capture::analog::RepeatSchedule` gains a no-`delay_ms` missed-deadline clamp
      (§5.4): either `advance_fired_steady(elapsed, fired) -> u32`
      = `max(fired + 1, elapsed / period_ms + 1)`, or generalise `advance_fired` with
      a `delay: Duration` argument. Pick one; unit-test it the way `advance_fired` is
      table-tested (on-schedule → `+1`; a multi-period stall → jump to the
      elapsed-time count, next due a full `period_ms` later, no burst; never
      regresses).
- [ ] `run_analog_repeat_loop` gains a `schedule: RepeatSchedule` parameter, read at
      spawn in `dispatch::update_analog_repeats` from the same source
      (`read_repeat_schedule`), and `solid_since: Option<Instant>` / `solid_fired: u32`
      loop state. Thread `schedule` through `ActiveAnalogRepeat::spawn` /
      `Engine::spawn` alongside `pulse_hold`.
- [ ] The `TickPlan::HoldSolid` arm (per §5.3): on first entry press the `KeyDown`
      steps (`value=1`), set `holding_solid`, `solid_since = Some(now)`,
      `solid_fired = 0`. Thereafter `select!` on `cancel` / `depth_rx.changed()` /
      a `sleep` to the next `value=2` due at `solid_since + N * period_ms` (no
      `delay_ms`); on the sleep firing, `solid_fired = advance_fired_steady(...)`,
      then `injector.repeat_key(solid_key)`.
- [ ] `solid_key` is the single non-modifier `KeyDown` code in `steps` (Analog-repeat
      is grid-key-only; its Action compiles to a keypress or single button).
- [ ] Leaving hold-solid (`release_solid_first` true, or cancel): the existing
      `release_solid` / `executor::force_release` fires the `KeyUp` steps (`value=0`)
      and `solid_since` / `solid_fired` reset to `None` / `0`.
- [ ] The tap band (`TickPlan::Tap`, Depth < 235) is untouched —
      `fire_analog_repeat_pulse` still emits balanced `[KeyDown … pulse_hold … KeyUp]`
      pulses paced by `tap_pace_wait` (ticket 06). A `start_paused` test asserts the
      tap→hold-solid→tap transition: pulsed pairs below 235, a `value=1`+`value=2`
      stream at/above 235 with the first `value=2` at `period_ms` (not `delay_ms`),
      pulsed pairs again on dropping back below, with a clean `value=0` at each
      hold-solid exit and no key left down.
- [ ] Analog-repeat + Macro stays rejected at the config layer (ticket 09) — nothing
      to do here; note it.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets`, full daemon suite green.
