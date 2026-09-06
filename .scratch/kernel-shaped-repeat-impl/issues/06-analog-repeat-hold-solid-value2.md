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

**Status:** done

- [x] `capture::analog::RepeatSchedule` gains `without_warmup()` — the same live
      envelope with `delay_ms` collapsed to a single `period_ms`, so the Nth repeat is
      due a plain `N * period_ms` in and the first `value=2` lands at `period_ms`, not
      `delay_ms`. `due_offset` / `repeat_due` / `advance_fired` then apply unchanged —
      the missed-deadline clamp (§5.4) included — so it needs no `advance_fired_steady`
      sibling (the ticket's other listed option; this is the DRYer of the two).
      Unit-tested the way `advance_fired` is: first repeat at `period_ms`; the whole
      `due_offset`/`repeat_due` boundary contract on the collapsed schedule; a
      multi-period stall → one repeat, next due within a period, no burst; never
      regresses.
- [x] `run_analog_repeat_loop` gains a `schedule: RepeatSchedule` parameter (threaded
      through `ActiveAnalogRepeat::spawn` / `Engine::spawn` alongside `pulse_hold`) and
      a `Solid { Off | On { since, fired } }` loop-state enum — one value, not a
      hand-synced `bool` + `Option<Instant>` + `u32`. The schedule is the
      daemon-startup-resolved value ticket 05 already threads to `DispatchState`
      (`toggle_autorepeat_schedule`, now read by both self-driven `value=2` emitters);
      an inline blocking read at spawn would break the `tokio::time::pause()` harness,
      exactly as ticket 05/68 found.
- [x] The `TickPlan::HoldSolid` arm (per §5.3): on first entry press the `KeyDown`
      steps (`value=1`), `solid = Solid::On { since: now, fired: 0 }`. Thereafter
      `select!` on `cancel` / `depth_rx.changed()` /
      `sleep_until(since + schedule.due_offset(fired))`; on the sleep firing, bump
      `fired` via `schedule.advance_fired(elapsed, fired)` then
      `injector.repeat_key(solid_key)`.
- [x] `solid_key` is the single non-modifier `KeyDown` code in `steps`
      (`solid_key_in`, unit-tested for plain / modified keypress + controller button +
      the none case). `None` ⇒ the arm just parks (pre-spec behaviour).
- [x] Leaving hold-solid (`release_solid_first` true, or cancel): `leave_solid`
      (one helper, both the Idle and Tap arms) fires the `KeyUp` steps via
      `release_solid` (`value=0`) and resets `solid` to `Solid::Off`; `force_release`
      on task exit covers the cancel path.
- [x] The tap band (`TickPlan::Tap`, Depth < 235) is untouched —
      `fire_analog_repeat_pulse` still emits balanced `[KeyDown … pulse_hold … KeyUp]`
      pulses paced by `tap_pace_wait`. `analog_repeat_tap_to_hold_solid_to_tap_transition`
      asserts the full ramp; `analog_repeat_holds_solid_above_the_hold_threshold`
      rewritten for the `value=1`+`value=2` stream (first `value=2` at `period_ms`,
      clean `value=0` on exit, key not left down).
- [x] Analog-repeat + Macro stays rejected at the config layer (ticket 09) —
      unchanged (`config::tests::refuses_to_start_when_a_chord_binding_is_analog_repeat`
      / `..._on_a_non_grid_input` still green); nothing added here.
- [x] `cargo fmt --check`, `cargo clippy --all-targets`, full daemon suite green (516).
