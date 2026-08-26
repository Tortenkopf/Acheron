Type: task
Blocked by: 78
Status: resolved

## Question

Land [ticket 78](./78-decide-controller-button-trigger-mode-applicability.md)'s settled design. AFK, no hardware needed — this is Daemon/config validation plus dead-code removal, mirroring the `21`/`51`/`54` precedent of landing a decided shape mechanically before any hardware pass.

Scope:

- New `ConfigError::InvalidControllerButtonTrigger` (naming the offending Binding(s)), same shape as `InvalidProfileSwitchTrigger`/`InvalidStepTrigger` in `config.rs::parse()` — refuses to start when any Profile has a Fire-once `Action::ControllerButton` Binding. Add the equivalent live-write-path check to `SetBinding`/`SetChordBinding` (mirrors how `InvalidProfileSwitchTrigger` and the Stepper-Toggle check are each enforced in both places).
- Delete `executor::CONTROLLER_BUTTON_FIRE_ONCE_PULSE_HOLD` and its `compile()` dwell-insertion for `Action::ControllerButton` — dead once Fire-once is locked out. Remove its dedicated tests too (or repoint them at whatever they were guarding against, if anything else still needs that coverage).
- New `dispatch::ANALOG_REPEAT_CONTROLLER_PULSE_HOLD = Duration::from_millis(35)` alongside the existing `ANALOG_REPEAT_PULSE_HOLD`; `fire_analog_repeat_pulse` (or wherever the dwell is read) selects between the two based on whether the firing Binding's Action is `ControllerButton`. `ANALOG_REPEAT_MIN_HZ`/`MAX_HZ`/the rate curve itself unchanged.
- GUI: `binding_editor.py`'s Trigger-mode dropdown excludes Fire-once when the current Action-kind is Controller Button, mirroring the existing non-grid-Input/Chord exclusion for Analog-repeat (ticket 39's precedent — `Gtk.DropDown` has no per-item sensitivity, so this is an outright option-list exclusion, not a greyed-out entry).
- CONTEXT.md: already updated (Trigger mode entry) as part of ticket 78's own resolution — confirm it still reads correctly once the code lands, no further edit expected.

Update `daemon_stub.py`/`daemon_client.py` to match if the wire/validation shape needs it, per the usual cross-module-plumbing check this map's later tickets keep catching.

## Answer

Landed ticket 78's design, AFK, no hardware needed — with two corrections to the scope text discovered while building, both grilled live with the user before implementing:

**Correction 1 — the "delete dead code" instruction was wrong.** `compile()`'s `Action::ControllerButton` dwell-insertion is *not* dead once Fire-once is locked out: `dispatch::fire()`'s generic arm (`(TriggerMode::FireOnce, Down) | (HoldToRepeat | AnalogRepeat, Down | Repeat)`) still reaches it for a Digital-sourced Analog-repeat Binding (ticket 20's Digital-Capture-mode fallback) — Hold-to-repeat is carved out ahead of it, but that fallback path isn't. Deleting the arm/constant outright would have been a non-compiling change (a `match` on `Action` can't just drop a variant) or, if worked around, a silent regression of the frame-safety dwell ticket 74/75/76 built for exactly this single-poll-swallow risk. Kept the arm; renamed the constant `executor::CONTROLLER_BUTTON_FIRE_ONCE_PULSE_HOLD` → `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD` and corrected every comment referencing it (including `compile()`'s own doc comment) to name the Digital-Capture-mode Analog-repeat fallback as the real remaining caller, not Fire-once.

**Correction 2 — Toggle for `Action::ControllerButton` was not "unchanged, already correct" as ticket 78 concluded.** Checked `dispatch::fire()`/`fire_chord()` directly: only Hold-to-repeat had a sustained-hold carve-out (ticket 75/76) — Toggle had never gotten one, so it fell through to the generic `(Toggle, Down)` arm, which calls `compile_action` → the same dwell-inserting `compile()` arm above, looped by `ActiveToggle::spawn` — a repeat-tap pulse-train (turbo), not the "latched held button" ticket 78's Answer described. Grilled live with the user (this session): chose to give Toggle a real sustained-hold carve-out, mirroring Hold-to-repeat's own (ticket 75/76) and the mouse-button Toggle fix (ticket 82/83) — `ActiveToggle::spawn_held`, one KeyDown on the first press, one KeyUp when the same key stops it. Added to both `fire()` and `fire_chord()`. This makes `compile()`'s `ControllerButton` dwell arm reachable *only* via the Digital-Capture-mode Analog-repeat fallback (Correction 1) — Fire-once, Hold-to-repeat, and Toggle are now all carved out ahead of it. Updated CONTEXT.md's Toggle glossary entry to describe this.

Everything else landed exactly as scoped:

- **`config.rs`**: `ConfigError::InvalidControllerButtonTrigger`, same shape as `InvalidProfileSwitchTrigger`/`InvalidStepTrigger` — `parse()` refuses to start on a Fire-once `Action::ControllerButton` Binding (checked via `profile_all_bindings`, which already walks Chords too). New test `refuses_to_start_when_a_controller_button_binding_is_fire_once`.
- **`dispatch.rs`**: `validate_binding` (shared by `SetBinding`/`SetChordBinding`) rejects the same live, alongside the existing gamepad-allowlist check. New test `set_binding_rejects_a_fire_once_controller_button_binding`; updated `set_binding_accepts_a_controller_button_in_the_gamepad_allowlist` to use Hold-to-repeat instead of the now-rejected Fire-once.
- **`dispatch.rs`**: new `ANALOG_REPEAT_CONTROLLER_PULSE_HOLD = Duration::from_millis(35)`; `update_analog_repeats` selects it over the existing 15ms `ANALOG_REPEAT_PULSE_HOLD` when the firing Binding's Action is `ControllerButton`, threading `pulse_hold` through `ActiveAnalogRepeat::spawn`/`run_analog_repeat_loop`/`fire_analog_repeat_pulse`. `ANALOG_REPEAT_MIN_HZ`/`MAX_HZ`/the rate curve untouched. New test `analog_repeat_controller_button_uses_the_controller_pulse_hold_floor` (paused-clock, proves the 15ms mark does *not* release the pulse but the 35ms mark does).
- New tests for the Toggle carve-out: `toggle_controller_button_holds_a_single_keydown_and_the_same_key_stops_it` and its Chord mirror `toggle_chord_controller_button_holds_a_single_keydown_and_full_completion_stops_it`.
- **GUI (`binding_editor.py`)**: the Trigger-mode dropdown's model is now rebuilt live in `render_action_editor()` — excludes Fire-once whenever the current Action-kind is Controller Button, restores it otherwise, preserving the current trigger selection across the rebuild where still valid (falls back to Hold-to-repeat when it isn't, e.g. switching a fresh fire_once-default Binding straight to Controller Button). This has to be dynamic (unlike Analog-repeat's grid-only exclusion above it, fixed for the popover's whole lifetime) since Action-kind changes live via the same dropdown session.
- **`daemon_stub.py`**: mirrored the same Fire-once/ControllerButton rejection in `_validate_binding_action` (shared by `set_binding`/`set_chord_binding`). `daemon_client.py` needed no change — it has no validation logic of its own, just forwards to the real Daemon over D-Bus.
- Updated three existing GUI tests that assumed Fire-once was still valid for Controller Button (`test_saving_a_controller_button_binding_calls_set_binding_with_the_chosen_button`, `test_bound_controller_button_shows_the_button_in_the_grid_button_label`) and added `test_fire_once_is_offered_only_when_action_kind_is_not_controller_button`.
- CONTEXT.md's Trigger mode entry already read correctly from ticket 78; the Toggle entry got the Correction 2 update described above.

**Verified**: 356 Rust tests pass (`cargo test`), `cargo fmt --check`/`cargo clippy --all-targets` clean; 291 GUI tests pass (`.venv/bin/python -m pytest`).

Spawned nothing new — [Verify the Controller-button Trigger-mode restriction on hardware](./86-task-verify-controller-button-trigger-mode-restriction-on-hardware.md) already covers this build's hardware pass, now unblocked. Its checklist should also confirm the new Toggle sustained-hold behavior (Correction 2) alongside what it already asked for.
