Type: task
Blocked by: 39

## Question

Live-verify and tune [Build and tune Analog-repeat on hardware](./39-task-build-analog-repeat.md)'s build against the real, connected Tartarus Pro and GUI — ticket 39 landed the whole architecture AFK (no physical device access that session), so both the hardware-verification half and the actual live-tuning half of ticket 20/39's scope still need doing here.

Checklist:

- Install the new binary, bind a grid key's Trigger mode to "Analog-repeat" (Keypress Action) via the real GUI, and confirm the Trigger-mode dropdown offers it for a Grid Input and excludes it for a non-grid one (Mode key/thumbstick/wheel) and inside the Chord dialog.
- Press the bound key lightly, past the deadzone but well short of full travel, and confirm it taps repeatedly — not a single fire, not a solid hold.
- Vary press depth continuously (light to firm) and confirm the tap rate visibly speeds up with deeper travel, roughly linearly, feeling like a real analog axis across the key's *full* physical travel (not just the band between its own tunable Actuation/Release points).
- Press to full/near-full travel and confirm the key holds down solid (continuous, no further tapping) rather than continuing to tap at the fastest rate.
- Release fully and confirm tapping/holding stops cleanly, with no stuck key.
- Tune the five `dispatch.rs` constants (`ANALOG_REPEAT_DEADZONE`, `ANALOG_REPEAT_MIN_HZ`, `ANALOG_REPEAT_MAX_HZ`, `ANALOG_REPEAT_PULSE_HOLD`, `ANALOG_REPEAT_HOLD_SOLID`) against real hands-on feel — pick values that feel right for the driving-sim/steer-or-accelerate use case, not the placeholders ticket 39 shipped blind.
- With `force_digital` set (or the device otherwise in Digital Capture mode), confirm the same Binding falls back to plain Hold-to-repeat at the kernel-autorepeat cadence, matching how every other grid key already behaves there.
- Confirm a Layer switch and a Profile switch each cleanly stop an in-flight Analog-repeat task (no stuck key, no output bleeding into the newly-active Layer/Profile's own Binding for that Input).
- Confirm toggling `force_digital` off/on (or an unplug/replug Analog↔Digital transition) while a tap is in flight stops it cleanly rather than double-firing alongside the Digital fallback.
- If a live feel for the rate curve suggests linear isn't good enough (per the map's "Analog-repeat's rate-curve refinement" fog note), record that as a fresh finding rather than silently reworking the curve here.

## Answer

_(unresolved)_
