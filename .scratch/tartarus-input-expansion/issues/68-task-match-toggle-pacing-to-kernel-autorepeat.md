Type: task

## Question

Match Toggle-mode's re-fire pacing to the device's own kernel autorepeat rate, the same live source Hold-to-repeat already uses — found live while verifying [Chord recording UX on hardware](./67-task-verify-chord-recording-ux-on-hardware.md): once that ticket's Hold-to-repeat cadence bug was fixed to correctly match the kernel rate, Toggle mode (tested there via a Toggle Chord, but the mechanism is shared with every individual Toggle Binding too — `executor::run_toggle_loop`, keyed by neither Chord nor Input) felt noticeably faster by comparison.

Grounding facts:

- `executor.rs`'s `MIN_TOGGLE_LAP` (currently a hardcoded 20ms) is a **safety floor** established live in [ticket 26](./26-task-build-trigger-point-depth-ux.md) — a Toggle wrapping a plain Keypress compiles to `[KeyDown, KeyUp]` with no `Delay` step, and without a floor a lap ran as fast as the injector channel + `uinput` write allowed, flooding and freezing the input pipeline. It was never tuned to match any particular target cadence, kernel or otherwise — it just needed to be "fast enough to not feel broken, slow enough to not flood."
- Hold-to-repeat's own re-fire cadence (individual and Chord) is sourced live from the real device's kernel autorepeat via `get_auto_repeat()` ([ticket 18](./18-rework-capture-path-for-analog.md) §Hold-to-repeat timing), not hardcoded — so it's the correct reference cadence to match.
- 20ms (50 laps/sec) is very plausibly faster than this device's actual configured kernel repeat rate (commonly ~25-40ms/repeat or slower), which would fully explain the live "too fast" reaction.

Task: make `run_toggle_loop`'s pacing read the same live `get_auto_repeat()` value Hold-to-repeat already consumes, while preserving some hard floor beneath it so ticket 26's original flood-protection guarantee still holds for degenerate cases (an unusually fast configured repeat rate, or a lap whose own compiled `Delay` steps already exceed the target — `MIN_TOGGLE_LAP` today is a floor under the *lap*, not a target, so decide how the two combine). Applies uniformly to every Toggle Binding, not just Chords — no Chord-specific code expected. HITL: needs the real Tartarus Pro to judge whether the tuned cadence actually feels right, the way ticket 67's own live comparison surfaced this in the first place.
