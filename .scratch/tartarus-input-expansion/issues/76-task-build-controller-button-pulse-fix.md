Type: task
Blocked by: 75
Status: resolved

## Question

Build [ticket 75](./75-decide-controller-button-pulse-fix.md)'s settled design, AFK (no hardware needed for the code itself — hardware/game verification is the follow-up task):

- `Action::ControllerButton` + Fire-once: insert a `MacroStep::Delay` between the compiled `KeyDown`/`KeyUp` steps in `executor::compile()`, scoped to `Action::ControllerButton` only. New constant `CONTROLLER_BUTTON_FIRE_ONCE_PULSE_HOLD = Duration::from_millis(35)` (not shared with `ANALOG_REPEAT_PULSE_HOLD`). The dwell blocks (`await`s) before the `KeyUp` write.
- `Action::ControllerButton` + Hold-to-repeat: dispatch-level change. `KeyDown` once on the physical Down, ignore kernel-autorepeat `Repeat` events (no re-fire), `KeyUp` once on the physical Up — reusing the existing `ActiveToggle`-style held-state mechanism rather than `spawn_fire_once`'s per-event pulse. Applies uniformly for a direct Binding and for a Chord's Action.
- No change to Keypress, mouse-button (`Action::Keypress` with a `BTN_*` code), or Macro output. No change to Stepper (unreachable).
- Add unit tests covering: Fire-once's dwell actually elapses before the Up write; Hold-to-repeat's Down→(N Repeats, no re-fire)→Up sequence for a `ControllerButton` Binding; a Chord whose Action is `ControllerButton` gets the same treatment; existing Keypress/mouse-button/Macro Hold-to-repeat behavior is unchanged (regression coverage).

Spawn [Verify the Controller-button pulse fix on hardware, against a real game](./77-task-verify-controller-button-pulse-fix-on-hardware.md) (already created, currently blocked on this ticket).

## Answer

Landed both fixes, AFK, no hardware needed:

- **`executor::compile()`** (`daemon/src/executor.rs`): `Action::ControllerButton`'s
  compiled steps gained a `MacroStep::Delay(CONTROLLER_BUTTON_FIRE_ONCE_PULSE_HOLD)`
  (35ms, a new constant, deliberately not shared with dispatch's
  `ANALOG_REPEAT_PULSE_HOLD`) between the `KeyDown`/`KeyUp` — unconditional on the
  Action, per the ticket's own compile()-level scoping (applies to every caller of
  `compile()` for this Action, i.e. Fire-once and any Digital-fallback Analog-repeat
  Binding; Hold-to-repeat never reaches `compile()` for this Action at all, see below).
  The existing blocking-`await` step-walker (`run_once`) already made this a genuine
  dwell with no further change needed.
- **`dispatch::fire()`/`fire_chord()`**: a new match arm, guarded on
  `Action::ControllerButton` and `TriggerMode::HoldToRepeat`, carved out ahead of the
  general Hold-to-repeat arm — `Down` fires a bare, unbalanced `vec![MacroStep::
  KeyDown(button)]` (not `compile_action`'s pulse) via the existing `spawn_fire_once`/
  `in_flight` (`chord_in_flight` for a Chord) machinery, `Repeat` is a hard no-op, and
  the pre-existing generic `Up` arm's `force_release_stuck` call (ticket 33) — already
  shared by both direct Bindings and Chord members via `release_chord_firing` — needed
  no change at all to release it. No new task type, map, or primitive: this is the
  exact "unbalanced firing, force-released on physical Up" mechanism ticket 33 already
  built, just deliberately produced instead of only defended against.
- **Blast radius confirmed as scoped**: only `Action::ControllerButton` triggers either
  branch (`matches!` guards on the match arms); Keypress/mouse-button (`Action::Keypress`
  with a `BTN_*` code)/Macro/Stepper output take the pre-existing paths unchanged. Both
  a direct Binding and a Chord's Action get identical treatment (mirrored match arms in
  `fire`/`fire_chord`).
- **Tests** (all in `daemon/src/executor.rs`/`daemon/src/dispatch.rs`, 343 Rust tests
  green, `cargo clippy`/`cargo fmt --check` clean): `compile_controller_button_is_a_
  down_up_pair_with_a_dwell` (updated from the old bare-pair assertion) and a new
  paused-clock `fire_once_controller_button_dwell_actually_elapses_before_the_up_write`
  proving the dwell genuinely blocks the Up write, not just appears in the compiled
  sequence; `hold_to_repeat_controller_button_ignores_repeat_and_releases_on_physical_up`
  and its Chord mirror `hold_to_repeat_chord_controller_button_ignores_repeat_and_
  releases_on_member_up` (Down→3×Repeat→Up nets exactly one KeyDown/KeyUp pair each);
  `hold_to_repeat_mouse_button_still_refires_on_every_repeat` as explicit regression
  coverage that the carve-out doesn't bleed onto `Action::Keypress` with a `BTN_*` code.
  Existing suites (`hold_to_repeat_fires_on_down_and_every_repeat_but_not_up`,
  `hold_to_repeats_unbalanced_macro_is_force_released_on_physical_up`, the Macro/Stepper/
  Toggle/AnalogRepeat coverage) pass unchanged, confirming no regression.
- Not done here, deliberately: hardware/real-game verification and dwell tuning — that's
  [ticket 77](./77-task-verify-controller-button-pulse-fix-on-hardware.md), now unblocked.
