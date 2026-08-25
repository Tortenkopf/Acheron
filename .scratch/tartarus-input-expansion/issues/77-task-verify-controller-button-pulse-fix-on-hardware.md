Type: task
Blocked by: 76
Status: resolved

## Question

Verify [ticket 76](./76-task-build-controller-button-pulse-fix.md)'s build against the real Tartarus Pro **and a real game** — not just `evtest`/`jstest` at the device level, which is exactly the gap that let this bug ship unnoticed in the first place ([ticket 45](./45-task-verify-controller-button-picker-ux-on-hardware.md)'s own device-level-only verification).

Checklist:
- A Fire-once `Action::ControllerButton` Binding reliably registers a single press in an actual running game (pick one confirmed to use polled-state gamepad reads if possible — otherwise test against whatever's available and note which consumption model it uses, if determinable).
- A Hold-to-repeat `Action::ControllerButton` Binding, held down, reads to the game as one continuous held button (not a rapid-fire mash) — confirm via the game's own behavior (e.g. a "run"/"charge" style action reacting as held, not tapped).
- A Chord whose Action is `Action::ControllerButton` under both Fire-once and Hold-to-repeat gets the same fixes.
- Existing Keypress/mouse-button/Macro Hold-to-repeat behavior is confirmed unchanged (a quick regression spot-check, not exhaustive — ticket 76's unit tests cover the rest).
- Tune the 35ms Fire-once dwell if the live game test shows it's insufficient or unnecessarily high — record the final value and rationale here even if unchanged, mirroring ticket 73's precedent.

## Answer

Verified live against the real Tartarus Pro and Shantae and the Pirate's Curse (the
same game used in prior hardware sessions), running ticket 76's freshly-built daemon
binary (installed to `~/.local/bin/acheron-daemon`, replacing the pre-fix build).
Temporary test bindings added to the `Testing` profile for the session (grid keys
distinct from the existing analog-repeat test bindings on `grid_r1c1`-`grid_r1c4`),
config restored to its pre-session state afterward:

- **Fire-once, single press** (`grid_r4c3` → `BTN_SOUTH`/A, fire_once): confirmed
  registering reliably as one press.
- **Hold-to-repeat, sustained hold** (`grid_r4c1` → `BTN_EAST`/B, hold_to_repeat):
  confirmed reading as one continuous held button, not a rapid-fire mash.
- **Chord + `Action::ControllerButton`, both Trigger modes**: Fire-once
  (`grid_r1c5`+`grid_r4c5` → `BTN_WEST`/X) and Hold-to-repeat (`grid_r2c1`+`grid_r2c5`
  → `BTN_NORTH`/Y) both confirmed matching their direct-Binding counterparts.
- **Regression spot-check**: Keypress Hold-to-repeat (`grid_r3c1` → `KEY_A`) still
  autorepeats normally at the OS level; mouse-button Hold-to-repeat (`thumbstick_right`
  → `BTN_LEFT`) still reads as a sustained held click, not a mash. Both unchanged, as
  expected from ticket 76's scoping (only `Action::ControllerButton` match arms
  touched).

All five checklist items passed on the first pass — no bugs found live that ticket
76's own unit tests and code review had missed. **Buttons chosen matter**: the
original test plan used `BTN_MODE`/`BTN_TR`/`BTN_TL`/`BTN_THUMBL` (mode/shoulder/
stick-click), which the user flagged mid-session as unused by this game; rebound to
`BTN_SOUTH`/`BTN_EAST`/`BTN_WEST`/`BTN_NORTH` (A/B/X/Y) before testing, worth keeping
in mind for any future live-game verification ticket — pick face buttons the target
game actually binds, not just any distinct button.

**Dwell tuning**: 35ms left unchanged. No missed or double registrations observed on
repeated Fire-once taps against Shantae's polled reads; the value ticket 75 picked
(and ticket 74's research flagged as a reasonable middle ground against AntiMicroX's
100ms/sc-controller's 10ms prior art) held up live with no adjustment needed.

This closes the map's Controller-button pulse-fix strand (decide → build → verify),
built, live-hardware-verified, and matching ticket 76's own scoping claims exactly.
