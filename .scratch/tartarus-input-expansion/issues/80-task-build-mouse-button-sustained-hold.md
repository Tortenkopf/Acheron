Type: task
Blocked by: 79
Status: resolved

## Question

Build [ticket 79](./79-decide-mouse-button-sustained-hold-drag.md)'s settled design, AFK (no hardware needed for the code itself — hardware verification is the follow-up task):

- Add `input::is_mouse_button(code: KeyCode) -> bool` to `daemon/src/input.rs`, mirroring `is_gamepad_button`'s shape/placement, matching evdev's `BTN_LEFT..=BTN_TASK` range (`0x110`–`0x117`, 8 codes: Left/Right/Middle/Side/Extra/Forward/Back/Task).
- `dispatch::fire()`/`fire_chord()`: a new match arm guarded on `(TriggerMode::HoldToRepeat, ...)` and `is_mouse_button(key)` where `Action::Keypress { key, .. }` — carved out ahead of the general Hold-to-repeat arm, mirroring ticket 76's `ControllerButton` carve-out shape exactly: `Down` fires a bare, unbalanced `KeyDown` via the existing `spawn_fire_once`/`in_flight` (`chord_in_flight` for a Chord) machinery, `Repeat` is a hard no-op, physical `Up` releases via the existing force-release path (ticket 33) with no further change needed.
- No change to keyboard-key Keypress, `Action::ControllerButton`, Macro, or Stepper output. No change to Fire-once for any Action.
- Add unit tests covering: `is_mouse_button` accepts the full `BTN_LEFT..=BTN_TASK` range and rejects keyboard codes, gamepad codes, and codes just outside the range (`BTN_0`–`BTN_9`, `BTN_SOUTH`); Hold-to-repeat's Down→(N Repeats, no re-fire)→Up sequence for a mouse-button Keypress Binding; a Chord whose Action is a mouse-button Keypress gets the same treatment; existing keyboard-key Keypress and `ControllerButton` Hold-to-repeat behavior is unchanged (regression coverage, mirroring ticket 76's own `hold_to_repeat_mouse_button_still_refires_on_every_repeat`-style negative test but in the other direction).

Spawn [Verify the mouse-button sustained-hold fix on hardware](./81-task-verify-mouse-button-sustained-hold-on-hardware.md) (already created, currently blocked on this ticket).

## Answer

Landed both pieces, AFK, no hardware needed:

- **`input::is_mouse_button`** (`daemon/src/input.rs`): mirrors `is_gamepad_button`'s
  shape/placement, a range check on `KeyCode::BTN_LEFT.code()..=KeyCode::BTN_TASK.code()`
  (`0x110`–`0x117`, 8 codes). Two new tests: `is_mouse_button_accepts_the_full_btn_left_
  to_btn_task_range` (all 8 named codes) and `is_mouse_button_rejects_keyboard_gamepad_
  and_just_out_of_range_codes` (`KEY_A`, `BTN_SOUTH`, the full `BTN_0`–`BTN_9` block, and
  one code past `BTN_TASK`).
- **`dispatch::fire()`/`fire_chord()`**: a new match arm pair, guarded on
  `(TriggerMode::HoldToRepeat, EventState::Repeat | EventState::Down)` and
  `matches!(binding.action, Action::Keypress { key, .. } if is_mouse_button(key))`,
  carved out ahead of the general Hold-to-repeat arm — structurally identical to ticket
  76's `ControllerButton` carve-out, but guarded on the `Keypress`'s `key` field rather
  than the Action variant, since mouse buttons and keyboard keys both ride
  `Action::Keypress`. `Down` fires a bare, unbalanced `vec![MacroStep::KeyDown(key)]`
  (not `compile_action`'s pulse) via the existing `spawn_fire_once`/`in_flight`
  (`chord_in_flight` for a Chord) machinery; `Repeat` is a hard no-op; the pre-existing
  generic `Up` arm's `force_release_stuck` call (ticket 33) needed no change to release
  it. Applies uniformly to a direct Binding and to a Chord's Action (mirrored arms in
  `fire`/`fire_chord`, same as the `ControllerButton` precedent).
- **Blast radius confirmed as scoped**: only a `Keypress` whose `key` is a mouse-button
  code triggers either branch; keyboard-key Keypress, `ControllerButton`, Macro, and
  Stepper output take the pre-existing paths unchanged. Modifiers on a mouse-button
  Keypress are not part of this fix (the ticket's own scoping only ever mentions the
  bare `key`, mirroring `ControllerButton`, which has no modifiers field at all) — an
  unbalanced modifier-down would be a separate, un-scoped design question if it ever
  matters in practice.
- **Tests** (`daemon/src/input.rs`/`daemon/src/dispatch.rs`, 347 Rust tests green,
  `cargo clippy --all-targets`/`cargo fmt --check` clean): replaced the now-outdated
  `hold_to_repeat_mouse_button_still_refires_on_every_repeat` (asserted the *old*
  mash-click behavior, which this ticket deliberately changes) with
  `hold_to_repeat_mouse_button_ignores_repeat_and_releases_on_physical_up` (Down→3×
  Repeat→Up nets exactly one KeyDown/KeyUp pair) and its Chord mirror
  `hold_to_repeat_chord_mouse_button_ignores_repeat_and_releases_on_member_up`; added
  `hold_to_repeat_keyboard_key_still_refires_on_every_repeat` as explicit regression
  coverage that the carve-out doesn't bleed onto an ordinary keyboard-key Keypress
  (mirrors ticket 76's own mouse-button negative test, in the other direction).
  Existing `ControllerButton`/Macro/Toggle/AnalogRepeat Hold-to-repeat suites pass
  unchanged, confirming no regression.
- Not done here, deliberately: hardware verification and click-and-drag feel — that's
  [ticket 81](./81-task-verify-mouse-button-sustained-hold-on-hardware.md), now unblocked.
