Type: task
Blocked by: 85
Status: resolved

## Question

Live-verify [ticket 85](./85-task-build-controller-button-trigger-mode-restriction.md)'s build against the real Daemon/Tartarus Pro/GUI, and clear the real config gap ticket 78's Answer found.

Checklist:

- **Fix the live config first**: `~/.config/acheron/config.toml`'s **Testing** profile has three Fire-once + `ControllerButton` bindings (`grid_r4c4`→`BTN_START`, `grid_r4c2`→`BTN_SELECT`, `grid_r4c3`→`BTN_MODE`) that the new validation refuses to start against. Hand-fix each one's Trigger mode to Hold-to-repeat or Toggle directly in the live config (confirm the choice with the user first), the same way ticket 53 hand-converted a legacy Macro Binding rather than treating it as a code problem. Confirm the Daemon starts cleanly afterward.
- Confirm a fresh attempt to save a Fire-once + `ControllerButton` Binding is rejected live, both via the GUI (Trigger-mode dropdown no longer offers Fire-once when Action-kind is Controller Button) and via a hand-edited `config.toml` (Daemon refuses to start with the new named error).
- Confirm Hold-to-repeat `ControllerButton` Bindings still work exactly as ticket 76/77 left them (no regression). Confirm Toggle `ControllerButton` Bindings now hold a single sustained press (one KeyDown on the first press, one KeyUp on the second) rather than the old repeat-tap pulse-train loop — a real behavior change found and fixed while building ticket 85 (Toggle had never gotten Hold-to-repeat's own sustained-hold carve-out; ticket 78's Answer had assumed it already worked this way).
- Confirm Analog-repeat on a grid key bound to `Action::ControllerButton` still fires correctly, and — the actual point of ticket 78's dwell change — capture the real pulse timing (e.g. `evtest`/`dbus-monitor` on the injected gamepad device) and confirm each pulse holds for the new 35ms floor rather than the old 15ms, ideally cross-checked against a real game the way ticket 77 used Shantae and the Pirate's Curse.
- Confirm Keypress/mouse-button Analog-repeat is unaffected (still the original 15ms dwell).

## Answer

Live-verified against the real Daemon/Tartarus Pro/GUI, all five checklist items confirmed, no regressions found.

**Live config fix — done differently than the ticket's default suggestion, per the user's explicit choice.** Rather than hand-converting the three Fire-once + `ControllerButton` bindings (`grid_r4c4`→`BTN_START`, `grid_r4c2`→`BTN_SELECT`, `grid_r4c3`→`BTN_MODE`) to Hold-to-repeat/Toggle, the user chose to just clear all three bindings outright (confirmed live before editing). Backed up the live `config.toml` first. Rebuilt `acheron-daemon` (release) against the uncommitted ticket-85 changes, installed it to `~/.local/bin` (the running systemd --user unit was on a pre-ticket-85 binary), restarted the unit — **starts cleanly**, no `InvalidControllerButtonTrigger` refusal, no crash-loop in `journalctl`.

**Fresh Fire-once + `ControllerButton` is rejected live, both ways:**
- Hand-edited `config.toml` (a scratch copy under an isolated `XDG_CONFIG_HOME`, config-validation-only — happens before device/D-Bus setup in `main()`, so no conflict with the running daemon or hardware): a foreground `acheron-daemon` run against it printed `acheron-daemon: refusing to start: ... (config.toml contains an Action::ControllerButton Binding whose trigger is fire_once)` and exited — the new `ConfigError::InvalidControllerButtonTrigger`, confirmed to follow the exact same generic (non-Binding-naming) message shape as `InvalidProfileSwitchTrigger`/`InvalidStepTrigger` already do, despite ticket 78/85's "naming the offending Binding(s)" phrasing — consistent with, not a deviation from, the established precedent.
- GUI: user confirmed live in their already-open `acheron_gui` window — setting a grid key's Action-kind to Controller Button removes Fire-once from the Trigger-mode dropdown.

**No regression, Hold-to-repeat `ControllerButton`:** temporarily bound `grid_r4c4`→`BTN_START` (Hold-to-repeat) on the Testing profile (also had to `SwitchProfile` to Testing live — the daemon was actually running MnM, caught after a first capture attempt hit the wrong bindings). Captured real evdev output on `/dev/input/event25` ("Acheron Virtual Controller") while the user held the physical key ~3.17s: a single `BTN_START` DOWN, one UP on release — no repeat-tap pulses. Matches ticket 75/76 exactly.

**Toggle `ControllerButton` now holds a single sustained press, confirmed as the real fix:** temporarily bound `grid_r4c2`→`BTN_SELECT` (Toggle). Captured: first physical tap → `BTN_SELECT` DOWN; second tap ~2.68s later → UP. One KeyDown, one KeyUp — not the old repeat-tap pulse-train loop.

**Analog-repeat `ControllerButton` fires correctly and holds the new 35ms floor:** the Testing profile's existing `grid_r1c1`→`BTN_SOUTH` (Analog-repeat) binding, pressed lightly (below the hold-solid threshold): captured a DOWN→UP pulse width of 36.0ms (35ms + ~1ms scheduling overhead) — the new `ANALOG_REPEAT_CONTROLLER_PULSE_HOLD`, not the old 15ms. Real-game cross-check (à la ticket 77's Shantae and the Pirate's Curse) was offered and explicitly declined by the user as unnecessary on top of the evtest measurement.

**Keypress/mouse-button Analog-repeat unaffected:** temporarily bound `grid_r4c3` to Keypress `KEY_A` (Analog-repeat). Captured pulse widths of ~16.2ms — the original `ANALOG_REPEAT_PULSE_HOLD` (15ms + overhead), unchanged. The same capture run also incidentally confirmed `ANALOG_REPEAT_HOLD_SOLID` behavior (deep press → single continuous KeyDown with no further tapping until Depth drops back below threshold) is unaffected — a real behavior, not a data-collection bug, verified by reading `run_analog_repeat_loop` directly rather than assumed.

**Cleanup:** removed the three temporary test bindings from the Testing profile and switched the active profile back to MnM (both were live-hardware-verification scaffolding only, undone after data collection); removed the config backup. `grid_r4c4`/`grid_r4c2`/`grid_r4c3` are confirmed absent from the live config, matching the user's chosen end state. Daemon restarted clean on the final config.

Spawned nothing new — this was the last open ticket this map's controller-button/mouse-button sustained-hold strand (tickets 75-86) needed.
