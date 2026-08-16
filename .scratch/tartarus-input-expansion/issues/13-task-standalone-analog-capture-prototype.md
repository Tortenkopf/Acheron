Type: task
Blocked by: 12

## Question

Build a standalone Linux proof-of-concept — independent of Acheron's codebase, living under `prototype/analog-grid-capture/` rather than `daemon/`/`gui/` — that attempts to actually read live per-grid-key analog depth values off the real, connected Tartarus Pro via `hidraw`, using the concrete translation [the research ticket](./12-research-linux-analog-grid-key-protocol.md) produces.

This is a feasibility test, not a build task: the question it answers is "does this work at all on Linux, on our actual hardware," not "how should this integrate into Acheron." HITL — needs the physical device and a human watching results in real time (and the caution flagged in ticket 12 about a possible firmware reset/reconnect side effect from the unlock command).

Settle at least:

- Does sending the documented unlock (HID Feature Report) actually put the device into streaming mode, observably, on this hardware?
- Do the resulting Report `0x06` reads actually carry plausible per-key depth values that change as a grid key is pressed harder/softer — not just on/off?
- Any deviation from the documented byte layout/report IDs found on this specific unit/firmware revision.
- Whether the reported firmware-reset risk is reproduced.

## Answer

