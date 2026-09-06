# Audit every holding/repeating output path against the kernel-autorepeat bar

Type: grilling
Status: resolved
Blocked by: —
Parent: [Humane output rate](../map.md)

## Question

For each synthetic-output path that **holds a key/button down** or **repeats a keystroke**,
measure the actual event cadence it can produce and give a verdict against the bar:

> Acheron never emits faster than the Linux input stack does for a physically held
> key/button — the kernel's configured autorepeat delay/period for keys (live via
> `analog::read_kernel_auto_repeat`, fallback 250/33 ms), and exactly one Down/Up with no
> repeat for a held mouse/gamepad button. Steady-state **rate** only; initial-repeat delay
> judged per surface.

Produce a **per-surface verdict table**: surface → mechanism & constants (file:line) →
worst-case cadence → verdict (OK / exceeds bar / needs a call) → if flagged, the shape of
the fix. Ratify the table with Charon in the grilling session; flagged surfaces graduate
into fix tickets on the map.

### Inventory to cover

- **Hold-to-repeat, keyboard** — Analog mode synthesizes via `RepeatSchedule`
  (`daemon/src/capture/analog.rs`, seeded from the live kernel delay/period, fallback
  `DEFAULT_REPEAT_DELAY_MS` 250 / `DEFAULT_REPEAT_PERIOD_MS` 33); Digital mode uses
  kernel-native autorepeat. Confirm the synthesized path can't outrun the kernel path, and
  that `REPORT_POLL_TIMEOUT` (8 ms) / report cadence don't let repeats bunch up.
- **Hold-to-repeat via Chord** — `chord::feed` emits `Down`/`Repeat`; rides the same
  schedule. Confirm no extra firing per member.
- **Hold-to-repeat via Stepper** — same schedule, advancing the cursor per repeat.
- **Analog-repeat** — `daemon/src/analog_repeat.rs`: `ANALOG_REPEAT_MIN_HZ` 2.0 /
  `ANALOG_REPEAT_MAX_HZ` 20.0 lerped on Depth; `ANALOG_REPEAT_PULSE_HOLD` 15 ms
  (`ANALOG_REPEAT_CONTROLLER_PULSE_HOLD` 35 ms). Judge 20 Hz tapping against "what a human
  interlacing keypresses could do"; confirm the deliberate lack of an initial delay is
  intended and documented.
- **Toggle → Keypress / Controller button / mouse button** — `executor::ActiveToggle::spawn_held`:
  one `KeyDown`, one `KeyUp`, nothing between. Confirm it is genuinely a sustained hold on
  every button-shaped Action.
- **Toggle → Macro loop** — `executor::run_toggle_loop`, floored by `MIN_TOGGLE_LAP` 20 ms
  or the live kernel period (`resolve_toggle_lap_target`). This loops a *Macro*, which is
  the map's declared exception — but the loop cadence when the macro has near-zero delays
  is exactly the self-DoS vector (ticket 26). Record its behaviour; the *guardrail* call is
  ticket 03, not here.
- **Controller-button digital pulse-hold** — `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD` 35 ms
  minimum hold so per-frame polling catches it. Direction is *slower* than a real press —
  confirm it never shortens below a plausible tap.
- **Confirm-in-passing (out of scope, note only)**: passthrough (`injector::inject_physical`,
  1:1), axis writes, staging-mode handoff / Quick-Skip (`QUICK_SKIP_WINDOW` 50 ms — adds
  latency).

### Also decide

- Whether any surface that follows the *live* kernel autorepeat config needs an independent
  absolute floor against a pathologically fast kernel setting (e.g. `kbrate` set to 100/s+).
  `MIN_TOGGLE_LAP` already does this for Toggle→Macro; Hold-to-repeat and Analog-repeat's
  endpoints may not. If yes, graduate a decision (or fold into ticket 03).

## Answer

Audit ran against the daemon source and was **ratified with Charon** in a `/grilling`
session (2026-09-06). Every surface in the inventory was traced to its mechanism and
constants; the verdict table below stands.

### Per-surface verdict table

The bar: never emit key repeats faster than the live kernel autorepeat period
(`analog::read_kernel_auto_repeat`, fallback 250 / 33 ms); a held mouse/gamepad button is
exactly one Down/Up, no repeat. Steady-state *rate* only.

| # | Surface | Mechanism (file:line) | Worst-case cadence | Verdict |
|---|---------|----------------------|--------------------|---------|
| 1 | Hold-to-repeat, keyboard — **Digital** capture | kernel `value=2` → `EventState::Repeat` (`capture/evdev_source.rs:368`) → `trigger::decide`→`SpawnFireOnce` per Repeat → `keypress_steps` = `[KeyDown,KeyUp]` once (`trigger.rs:153`, `executor.rs:82`) | exactly kernel `REP_PERIOD` | **OK** — it *is* the kernel's own repeat stream, gated 1:1 |
| 2 | Hold-to-repeat, keyboard — **Analog** capture (grid keys) | synthesized: `RepeatSchedule` seeded from live `get_auto_repeat()` (`capture/analog.rs:376`), `repeat_due(held_for, fired)` checked every `REPORT_POLL_TIMEOUT` 8 ms + per report (`capture/analog.rs:810`) | kernel period steady-state; **catch-up burst** under stall | **FLAGGED → ticket 06.** `fired` bumps by 1 per emit with no missed-tick clamp; if `event_rx` (cap 256) back-pressures `blocking_send` and the loop stalls > 1 period, on resume it emits a bunched catch-up burst (≈ stall_ms / period events, sub-ms apart). Kernel's `input_repeat_key` re-arms from *now* and never bursts. |
| 3 | Hold-to-repeat via **Chord** | `chord::feed_repeat` — only `BTreeSet`-first "leader" member's Repeat re-fires, `HoldToRepeat` + live slot only (`chord.rs:213`); ticket 67 N×-fix present | = leader member's Repeat cadence (surface 1/2) | **OK** (inherits 1/2, incl. the surface-2 caveat) |
| 4 | Hold-to-repeat via **Stepper** | each Repeat → `SpawnFireOnce` → `compile_action` advances `stepper::Cursors::step` by one + item fired once (`trigger.rs:404`) | one item per Repeat = Repeat cadence | **OK** (inherits 1/2) |
| 5 | **Analog-repeat** | `analog_repeat.rs`: `lerp(2 Hz, 20 Hz, depth/255)` hardcoded, ignores kernel config; `ANALOG_REPEAT_PULSE_HOLD` 15 ms kbd / 35 ms controller; no initial delay (deliberate, ticket 20); `ANALOG_REPEAT_HOLD_SOLID` 235 → solid hold | ≤ 20 Hz steady; free-run under injector backpressure | **FLAGGED → ticket 06** (missed-deadline clamp on the `Tap` branch — if `fire_analog_repeat_pulse` overruns `period`, the loop re-enters with no sleep and free-runs at ≈ 1/`pulse_hold` ≈ 66 Hz kbd until it catches up). Ceiling of 20 Hz **ratified as OK** (below kernel ~30 Hz, below butterfly-clicking; the feature deliberately simulates a hand interlacing keypresses). Deliberate lack of an initial delay: **recorded, intended.** The hold-solid phase presenting as a bare stuck `KeyDown` rather than genuine autorepeat → ticket 07. |
| 6 | **Toggle → Keypress (keyboard)** | `executor::run_toggle_loop`, `target_lap = combine_toggle_lap_target(live kernel period) = max(kernel, MIN_TOGGLE_LAP 20 ms)` (`executor.rs:340,355`), lap measured from `lap_start` | max(kernel, 20 ms); no burst (lap from start) | **OK** |
| 7 | **Toggle → Keypress (mouse button)** | `is_mouse_button` → `sustained_hold_key` → `decide`→`StartToggleHeld` → `run_toggle_held`: one `KeyDown`, wait, one `KeyUp` (`executor.rs:415`) | one Down/Up, no repeat | **OK** — matches bar exactly |
| 8 | **Toggle → Controller button** | `Action::ControllerButton` → `sustained_hold_key` → `StartToggleHeld` → `run_toggle_held` (ticket 78) | one Down/Up, no repeat | **OK** |
| 9 | **Toggle → Macro loop** | `run_toggle_loop` w/ compiled macro steps, lap floored at `target_lap` | ~50 Hz keystroke loop w/ a zero-delay macro | **bar N/A** (Macro = declared exception). Behaviour recorded for **ticket 03**; the *loop-lap* cadence is a candidate for the generalised floor. |
| 10 | **Hold-to-repeat → Macro** | each Repeat → `SpawnFireOnce` runs the whole macro once; overlap guard (`Slot::FiringUnfinished`→`Nothing`) drops a Repeat while the prior run still walks | macro-runs ≤ kernel rate, but N keystrokes/run → N × kernel-rate aggregate during a run | **bar N/A** (Macro exception). **Distinct unfloored vector** from Toggle→Macro — `MIN_TOGGLE_LAP` does not cover the re-fire cadence *or* the within-run burst. Recorded for **ticket 03** (both the re-fire cadence and the within-run nuance). |
| 11 | **Controller-button digital pulse-hold** | `controller_button_steps` = `[KeyDown, Delay(CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD 35 ms), KeyUp]` (`executor.rs:100`); reached only via Digital-mode Analog-repeat fallback now | one press per kernel Repeat, +35 ms floor | **OK** — direction is *slower* than a real tap |
| 12 | **Confirm-in-passing** | passthrough `injector::translate` 1:1 value-preserving (`injector.rs:368`); `set_axis_value` continuous `ABS_*`; Quick-Skip `QUICK_SKIP_WINDOW` 50 ms adds latency; non-grid Inputs (Mode key / thumbstick / wheel) native evdev passthrough even in Analog mode; wheel = one `Down` per notch | — | **OK** — none holds or repeats; every deviation is in the safe (slower) direction |

### Cross-cutting finding

Acheron's Hold-to-repeat output is **always `[KeyDown,KeyUp]` pairs with ~0 ms dwell, never
`value=2` autorepeat events**, in *both* capture modes (the grabbed physical device's kernel
`value=2` never reaches apps; the uinput device does not advertise `EV_REP` — verified:
`evdev` 0.13.2's `VirtualDeviceBuilder` has no repeat method, `injector::build_device`
declares only keys + relative axes, so the kernel does not softrepeat a held key on it).
Rate-compliant, but the 1/0-pair shape + near-zero dwell is the single most concrete
"synthetic" tell in the ticket-02 research (§4.2). The bar was deliberately scoped
rate-only — so this **passes as written** — but Charon's call (Q5) is to fix it, not just
document it: → **ticket 07** produces a handoff spec for emitting genuine `value=2`.

### Decisions taken in the ratification session

1. **Surfaces 1, 3, 4, 6, 7, 8, 11, 12 — OK, accepted.** The floors (`MIN_TOGGLE_LAP`,
   `combine_toggle_lap_target`, the 35 ms pulse-hold) and the 1:1 Repeat→fire gating are
   sound and already regression-tested. No work.
2. **Missed-deadline clamp (surfaces 2 & 5) → ticket 06**, an inline fix on this map. Add
   missed-tick clamping to `RepeatSchedule` catch-up in `relay_grid_blocking` and to
   `run_analog_repeat_loop`'s `Tap` branch. The ticket-07 rebuild of the surface-2 path
   must preserve the clamp.
3. **Analog-repeat 20 Hz ceiling — kept.** Below kernel and butterfly-click rates; the
   feature explicitly simulates hand-interlaced keypresses. Additional requirement from
   Charon: as Depth approaches the hold-solid threshold the output must *look like* an
   automated ramp leading into genuine kernel autorepeat, with the ramp's variance still
   visibly hand-driven → shape work in **ticket 07**; a one-time **selection toast**
   (Analog-repeat can produce inhumanly fast keypresses near full travel; single-player /
   known-safe games only, never competitive multiplayer) → copy + placement folded into
   **ticket 05** (broadened).
4. **`value=2` genuine autorepeat → ticket 07**, a new `grilling` ticket producing a
   handoff spec `spec-kernel-shaped-repeat.md`. Covers: synthesized + Digital + Analog
   Hold-to-repeat all emit `value=2` at the kernel rate with the real delay→period
   envelope; Analog-repeat's hold-solid phase presents as that autorepeat; the
   injector-`value=2` vs `EV_REP`/`EVIOCSREP` implementation fork; and a written
   heuristics-report rationale ("this virtual device plays by the rules — rate stays
   physically plausible and the ramp is hand-driven, i.e. human-in-the-loop, not
   automation"). Blocked by 01 + 03.
5. **Pathologically fast kernel autorepeat config — out of scope (Q4).** Following the live
   kernel rate *is* the bar as written; a `kbdrate` set to 100/s+ is the user's own OS
   misconfiguration and their physical held keys would flood identically. Not Acheron's to
   guard against. This closes that half of the map's "absolute-rate floor" fog item.
6. **Macro paths (surfaces 9 & 10) → ticket 03.** Charon's lean: generalise
   `MIN_TOGGLE_LAP` so the *loop-lap / re-fire cadence* of a Macro **repeated by a Trigger
   mode** (Toggle→Macro loop, Hold-to-repeat→Macro re-fire) is floored to
   `combine_toggle_lap_target`'s value; a Macro fired **once**, and the keystroke cadence
   *within* one run of a multi-step macro, stay unrestricted. If `value=2` is being built
   anyway (ticket 07), a trigger-looped single-key "turbo" macro should ride the same
   `value=2` path so it looks like a held key. Ticket 03 still owns the final
   soft-warning-vs-hard-floor call. The map's "steady-state speed ceiling on Macros"
   out-of-scope line is reframed accordingly (see map).

### Map changes made on resolution

- **Out of scope** — "steady-state speed ceiling on Macros" reframed: once-fired macros and
  the within-single-run keystroke cadence stay out; a ceiling on *trigger-driven macro
  repetition* moves in, pending ticket 03. New line: a pathologically fast kernel autorepeat
  config is the user's own OS misconfiguration, not Acheron's to guard against.
- **Not yet specified** — the absolute-rate-floor fog item's pathological-kernel half is
  closed (decision 5); its remaining half folds into ticket 03; the per-surface-fix-tickets
  item resolves to just ticket 06.
- **Destination** — expanded to name `spec-kernel-shaped-repeat.md` (ticket 07) as a second
  handed-off deliverable alongside ticket 05's guidance spec.
- **Tickets rewired** — 06 (new, `Blocked by: 01`), 07 (new, `Blocked by: 01, 03`); 03
  gains the decision-6 lean + feeds 07; 04 gains `Blocked by: 07`; 05 broadened + gains
  `Blocked by: 06, 07`.
