Type: task
Blocked by: 75
Status: open

## Question

Build [ticket 75](./75-decide-controller-button-pulse-fix.md)'s settled design, AFK (no hardware needed for the code itself — hardware/game verification is the follow-up task):

- `Action::ControllerButton` + Fire-once: insert a `MacroStep::Delay` between the compiled `KeyDown`/`KeyUp` steps in `executor::compile()`, scoped to `Action::ControllerButton` only. New constant `CONTROLLER_BUTTON_FIRE_ONCE_PULSE_HOLD = Duration::from_millis(35)` (not shared with `ANALOG_REPEAT_PULSE_HOLD`). The dwell blocks (`await`s) before the `KeyUp` write.
- `Action::ControllerButton` + Hold-to-repeat: dispatch-level change. `KeyDown` once on the physical Down, ignore kernel-autorepeat `Repeat` events (no re-fire), `KeyUp` once on the physical Up — reusing the existing `ActiveToggle`-style held-state mechanism rather than `spawn_fire_once`'s per-event pulse. Applies uniformly for a direct Binding and for a Chord's Action.
- No change to Keypress, mouse-button (`Action::Keypress` with a `BTN_*` code), or Macro output. No change to Stepper (unreachable).
- Add unit tests covering: Fire-once's dwell actually elapses before the Up write; Hold-to-repeat's Down→(N Repeats, no re-fire)→Up sequence for a `ControllerButton` Binding; a Chord whose Action is `ControllerButton` gets the same treatment; existing Keypress/mouse-button/Macro Hold-to-repeat behavior is unchanged (regression coverage).

Spawn [Verify the Controller-button pulse fix on hardware, against a real game](./77-task-verify-controller-button-pulse-fix-on-hardware.md) (already created, currently blocked on this ticket).
