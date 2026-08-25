Type: task
Blocked by: 76
Status: open

## Question

Verify [ticket 76](./76-task-build-controller-button-pulse-fix.md)'s build against the real Tartarus Pro **and a real game** — not just `evtest`/`jstest` at the device level, which is exactly the gap that let this bug ship unnoticed in the first place ([ticket 45](./45-task-verify-controller-button-picker-ux-on-hardware.md)'s own device-level-only verification).

Checklist:
- A Fire-once `Action::ControllerButton` Binding reliably registers a single press in an actual running game (pick one confirmed to use polled-state gamepad reads if possible — otherwise test against whatever's available and note which consumption model it uses, if determinable).
- A Hold-to-repeat `Action::ControllerButton` Binding, held down, reads to the game as one continuous held button (not a rapid-fire mash) — confirm via the game's own behavior (e.g. a "run"/"charge" style action reacting as held, not tapped).
- A Chord whose Action is `Action::ControllerButton` under both Fire-once and Hold-to-repeat gets the same fixes.
- Existing Keypress/mouse-button/Macro Hold-to-repeat behavior is confirmed unchanged (a quick regression spot-check, not exhaustive — ticket 76's unit tests cover the rest).
- Tune the 35ms Fire-once dwell if the live game test shows it's insufficient or unnecessarily high — record the final value and rationale here even if unchanged, mirroring ticket 73's precedent.
