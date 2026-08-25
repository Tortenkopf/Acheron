Type: task
Blocked by: 39
Status: resolved

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

Live-verified against the real Daemon/Tartarus Pro/GUI, and the five placeholder constants are
**confirmed as-is** — no code change needed, ticket 39's blind defaults hold up.

The user's real "Testing" profile already had grid_r1c1 bound to `Keypress(KEY_A)` with
`trigger = "analog_repeat"` (set up ahead of this session), so no throwaway binding was needed.
Verification combined the user's live feel with objective captures: `evtest` on the virtual
`Acheron Virtual Tartarus Pro` uinput node for tap timing, and `dbus-monitor` on
`com.acheron.Daemon`'s `DepthChanged` signal for raw depth, run in parallel with the user's own
presses.

- **Dropdown gating (item 1)**: confirmed live in the GUI — Analog-repeat is offered for
  grid_r1c1, excluded for the Mode key/thumbstick/wheel, and excluded in the Chord dialog. Also
  traced in code: `binding_editor.py`'s `build_action_and_trigger_fields` (shared by both the
  per-key editor and the Chord dialog) filters `TRIGGER_OPTIONS` by `is_grid_input(inp)` /
  `inp is not None`.
- **Light press taps, not fires-once/holds-solid (item 2)**: confirmed live and via `evtest` —
  ~15ms-wide pulses (matches `ANALOG_REPEAT_PULSE_HOLD`) with ~100-120ms gaps at a light depth.
- **Rate scales across full travel (item 3)**: a `dbus-monitor` capture of one slow, deliberate
  press-to-mechanical-bottom showed a clean, continuous `DepthChanged` ramp from 0 to 255 over
  ~3.5s (134 distinct depth values, no early plateau). An earlier, faster/imprecise press had
  made the rate change feel narrow-banded and prompted a report that the GUI's live depth bar
  "saturates before full mechanical travel" — the raw signal does **not** saturate early (0→255
  smooth); what the user felt past 100%-fill is a small amount of physical switch overtravel
  beyond the HID report's 8-bit ceiling, a hardware property no software constant can extend.
  `ANALOG_REPEAT_HOLD_SOLID` (235) crosses only in the last ~5% of that ramp, right before 255,
  confirming solid-hold engages appropriately late rather than early. Mapped onto
  `rate_hz = MIN_HZ + (MAX_HZ - MIN_HZ) * depth/255`: depth 12 (deadzone) ≈ 2.85Hz, depth 100 ≈
  9Hz, depth 235 (hold-solid) ≈ 18.6Hz — consistent with both live-feel tests once their actual
  starting depths are accounted for.
- **Full/near-full press holds solid (item 4)** and **clean release, no stuck key (item 5)**:
  user-confirmed live; corroborated by the same depth capture (depth held at 254-255 for ~3s
  while the user held the key, then ramped smoothly back to 0 on release).
- **Constants tuned by feel (item 6)**: shown the depth-to-Hz mapping above, the user chose to
  **keep all five constants as shipped** (`ANALOG_REPEAT_DEADZONE=12`, `_MIN_HZ=2.0`,
  `_MAX_HZ=20.0`, `_PULSE_HOLD=15ms`, `_HOLD_SOLID=235`) — no code change.
- **Digital Capture fallback (item 7)**: with "Force digital capture" checked, the same Binding
  produced steady ~34ms-period ticking (`evtest`-confirmed), matching plain Hold-to-repeat at the
  live kernel autorepeat cadence ([ticket 68](./68-task-match-toggle-pacing-to-kernel-autorepeat.md)'s
  ~33ms finding on this device) rather than the analog ramp.
- **Layer/Profile switch stops an in-flight tap (item 8)**: user-confirmed clean stop, no stuck
  key, nothing bleeding into the newly-active Layer/Profile; `evtest` shows the tap sequence end
  abruptly at the switch with no further events until the next deliberate press.
- **Toggling force_digital mid-tap stops cleanly, no double-fire (item 9)**: user-confirmed;
  `evtest` shows the analog ramp end cleanly with a subsequent clean ~34ms-cadence digital burst,
  no overlapping/doubled timings anywhere in either transition.
- **Rate-curve refinement (item 10)**: not needed — the user's verdict after seeing the real
  depth-to-Hz mapping was to keep the linear curve, so no fresh finding is recorded. The map's
  existing "Analog-repeat's rate-curve refinement" fog note stands unchanged for future revisit.

**Session cleanup**: live testing left two real state changes in the user's daily-driver
`config.toml` — `force_digital` flipped to `true` (from the Digital Capture fallback test) and a
`grid_r1c1` actuation override (`actuation=1, release=0`, apparently from an inadvertent drag on
the live depth-bar marker while checking saturation). Both cleared live via the Daemon's own
D-Bus methods (`SetForceDigital(false)`, `ClearActuationPoint("grid_r1c1")`), not by hand-editing
the file. Confirmed the resulting config is semantically identical to a pre-session backup (a
structural TOML diff, since profile/key insertion order differs harmlessly between writes).

No code changes. 339 Rust + 290 Python tests unaffected (none run this session, no source
touched). This closes the map's analog fast-follow strand entirely: capture (12→13→16→17→18→21→
22→23→24), trigger-point UX (19→26→27), and Analog-repeat (20→39→73) are all now built and
live-hardware-verified.
