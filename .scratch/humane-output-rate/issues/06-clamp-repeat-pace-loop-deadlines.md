# Clamp missed deadlines in both repeat pace loops

Type: task
Status: resolved
Blocked by: 01
Parent: [Humane output rate](../map.md)

> Execution ticket — resolved on this map, not handed off (map Notes: "execution is in
> scope"). Graduated from ticket 01's audit (surfaces 2 & 5).

## Question

Two repeat pace loops advance their "next fire" bookkeeping by exactly one step per emitted
event, with no clamp for missed deadlines. Under a scheduler / injector stall longer than
one period they emit a bunched catch-up burst on resume — sub-millisecond spacing for
≈ (stall duration / period) events — which exceeds the kernel-autorepeat bar. The kernel's
own `input_repeat_key` re-arms its timer from the current instant and never bursts; match
that.

### Surface 2 — `RepeatSchedule` catch-up in `capture::analog`

`relay_grid_blocking` (`daemon/src/capture/analog.rs:810`) checks
`schedule.repeat_due(state.started.elapsed(), state.fired)` every `REPORT_POLL_TIMEOUT`
tick and per report, doing `state.fired += 1` on each true. `repeat_due` (`analog.rs:268`)
is `held_for >= delay_ms + fired * period_ms`. If `tx.blocking_send` back-pressures on a
full `event_rx` (cap 256) and the loop stalls, `elapsed` jumps well past `due_at`, and the
loop then fires one Repeat per iteration (each iteration ≈ one sub-ms `hidraw` read) until
`fired * period` catches up.

Fix shape: on firing a Repeat, advance `fired` to `((held_for - delay) / period).floor() + 1`
(skip the missed repeats) rather than `fired + 1`; or re-base `HoldState.started`. Keep the
`RepeatSchedule` type pure and unit-tested — add a table case for "held_for far past
due_at fires exactly one, and the next is due one period later, not immediately".

### Surface 5 — the `Tap` branch in `analog_repeat::run_analog_repeat_loop`

`run_analog_repeat_loop` (`daemon/src/analog_repeat.rs:280`): `TickPlan::Tap` measures
`tick_start.elapsed()` after `fire_analog_repeat_pulse` and only sleeps
`period - elapsed` when `elapsed < period`. If the pulse's own injector round-trips overrun
`period` (backpressure), the loop re-enters immediately and free-runs at ≈ 1 / `pulse_hold`
(≈ 66 Hz keyboard, ≈ 28 Hz controller) until it catches up.

Fix shape: cap the overrun — if `elapsed >= period`, still yield briefly / clamp the
effective rate so a run of overrunning ticks can't free-run above the 20 Hz curve ceiling.
The pure `tick_plan` already returns `period`; the clamp belongs in the loop shell.

### Constraint

Ticket 07 rebuilds the surface-2 path to emit `value=2`. Its replacement scheduler **must
preserve this clamp** — note that in ticket 07's spec. Do this fix now regardless: surface
5's tapping-band loop survives ticket 07 unchanged, and surface 2 wants the clamp whether
or not the rebuild lands first.

## Answer

Both clamps landed on `dev`, each expressed as a pure, table-tested helper with the loop
shell reduced to calling it. All 473 daemon lib tests pass; clippy clean.

### Surface 2 — `RepeatSchedule` catch-up (`daemon/src/capture/analog.rs`)

New pure method **`RepeatSchedule::advance_fired(held_for, fired) -> u32`**: the count to
set `fired` to after emitting one Repeat. On schedule it returns `fired + 1`; after a stall
that slipped N due times past, it jumps straight to
`(held_for - delay) / period + 1` (`.max(fired + 1)` so it never regresses), so the loop
emits **exactly one** Repeat now and the next falls due a full `period` later — no
sub-millisecond burst of the missed repeats. Matches the kernel's `input_repeat_key`, which
re-arms from the current instant.

`relay_grid_blocking`'s repeat-scheduling loop (was `state.fired += 1`) now captures
`held_for` once per key per tick and does `state.fired = schedule.advance_fired(held_for,
state.fired)`. The `if let Some(state) = hold && repeat_due(..)` was split into a
`let ... else { continue }` + `if` so `held_for` is computed once and shared by both
`repeat_due` and `advance_fired` (no double `Instant::now` skew).

Tests (`capture::analog::tests`): `advance_fired_steps_by_one_when_on_schedule`,
`advance_fired_skips_missed_repeats_after_a_stall_and_the_next_is_a_full_period_out` (the
table case the ticket asked for — held 5 s, `fired == 1`, lands on 144 and the next Repeat
is due at 250 + 144·33 ms, not immediately), `advance_fired_never_regresses_the_count`.

### Surface 5 — the `Tap` branch (`daemon/src/analog_repeat.rs`)

New pure helper **`tap_pace_wait(period, pulse_elapsed) -> Duration`**:
`period.checked_sub(pulse_elapsed).unwrap_or(period)`. Normal case → `period -
pulse_elapsed`; on an overrun (`pulse_elapsed >= period`, injector back-pressure or a
runtime stall) → a **full `period`** rather than `0`, so the loop can't re-enter and
free-run above the 20 Hz curve ceiling.

`run_analog_repeat_loop`'s `Tap` arm: the old `let elapsed = ...; if elapsed < period {
select! { sleep(period - elapsed) } }` became `let wait = tap_pace_wait(period,
tick_start.elapsed()); select! { sleep(wait) }` — the sleep (and its `cancel` race) now
always runs, `sleep(ZERO)` being a no-op when exactly on pace.

Tests (`analog_repeat::tests`):
`tap_pace_wait_sleeps_the_remainder_of_the_period_when_the_pulse_fits`,
`tap_pace_wait_re_arms_a_full_period_on_an_overrun_rather_than_bursting`.

### For ticket 07

Its item 5 already carries the constraint. The concrete shape to preserve: the replacement
surface-2 scheduler must re-base its "next due" from the current instant on a stall (the
`advance_fired` behaviour), never emit a catch-up burst of missed repeats.
