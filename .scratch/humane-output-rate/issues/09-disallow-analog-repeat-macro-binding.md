# Disallow the Analog-repeat + Macro Binding

Type: task
Status: open
Blocked by: 01
Parent: [Humane output rate](../map.md)

> Execution ticket — resolved on this map, not handed off (map Notes: "execution is in
> scope"). Graduated from ticket 03's decision 4. An **audit-surfaced simplification**,
> not a rate fix — Analog-repeat + Macro is already ≤ 20 Hz; it is banned because it has
> no coherent meaning, not because it is fast.

## Question

Firing a Macro through an Analog-repeat Binding is nonsensical:
`analog_repeat::fire_analog_repeat_pulse` collapses a multi-step Macro to a single
simultaneous pulse (all `KeyDown`s, hold `ANALOG_REPEAT_PULSE_HOLD`, all `KeyUp`s
reversed), **ignoring every embedded `MacroStep::Delay`**. Ticket 03 decided the
combination should be disallowed rather than given semantics.

Implement the ban, following the existing precedent in `daemon/src/config.rs`
(`InvalidChordAnalogRepeat`, `AnalogRepeatOnDualStageKey`, the Fire-once
`ControllerButton` refusal, the Toggle `Step` refusal):

1. **New `ConfigError` variant** — e.g. `AnalogRepeatMacro(String)` carrying the Input
   name. `Display`: `an analog_repeat trigger on a Macro Binding is not allowed — Analog-repeat collapses a multi-step Macro to a single simultaneous pulse (its Delay steps ignored); change the trigger mode`.

2. **Reject at `SetBinding`** — the D-Bus edit path refuses it, so the GUI cannot save it.

3. **Reject at `load_or_seed`** — the Daemon refuses to start on a hand-edited
   `config.toml` carrying the combo (a `TriggerMode::AnalogRepeat` Binding whose `Action`
   is `Action::Macro { .. }`, in `base`/`held`). **No silent coercion.** Add the
   `expect_err` load test alongside the sibling cases.

4. **GUI** — the trigger-mode selector stops offering Analog-repeat when the Action is a
   Macro (or surfaces the same validation error the sibling restrictions do). Mirror in
   `daemon_stub.py` / `rules.py` if the GUI pre-validates there (as the dual-stage
   `ConfigError` set does).

5. **CONTEXT.md — Trigger mode entry.** Append a third exception to the existing "except…"
   list, after the Controller-button/Fire-once clause:

   > …and an Analog-repeat Binding, which disallows a Macro Action — Analog-repeat
   > collapses a multi-step Macro to a single simultaneous pulse (its `Delay` steps
   > ignored), so the combination has no coherent meaning (ticket 09).

   No ADR — the sibling restrictions were recorded as CONTEXT.md + ticket ref, not ADRs.

Feeds ticket 04 (the ADR notes Analog-repeat cannot wrap a Macro) and ticket 05 (the tips
no longer need to warn about this combo — it is structurally impossible).
