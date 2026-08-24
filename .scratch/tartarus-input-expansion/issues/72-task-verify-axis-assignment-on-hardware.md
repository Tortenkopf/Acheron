Type: task
Status: open
Blocked by: 71

## Question

Live-verify [Build the axis-assignment UX and Daemon support for real](./71-task-build-axis-assignment-ux.md)'s build against the real, connected Tartarus Pro and GUI, per ticket 43/45's decide → prototype → build → verify precedent.

Checklist, per ticket 71's own "Live-hardware verification" scope:

- Install the new binary and open the real GUI against the real Daemon; open a Grid key's Binding editor, switch Action to "Axis", and confirm the diagram picker renders inline (sticks with their crosses, LT/RT beside them, the Driving/Flight groups below the rule) — matches the prototype's live-reacted layout, not clipped or mispositioned in the real per-key `Gtk.Window`.
- Assign one unsigned target (e.g. Right Trigger), one signed half (e.g. Left Stick X+), and confirm each round-trips through `config.toml`/D-Bus and produces a real, depth-driven `ABS_*` value on the gamepad `uinput` node end-to-end (e.g. via `evtest`/`jstest`), ramping from 0 at the Release point to full raw Depth at/above the Actuation point.
- Confirm the Digital Capture mode step-increment fallback actually fires when `force_digital` is set or the device is in evdev-only mode — press/release produces a step change in the axis value, not nothing.
- Assign the opposite half of the same signed axis (e.g. Left Stick X−) to a second key and confirm the runtime conflict resolution: whichever key is actively pressed suppresses the other, never both driving the axis simultaneously.
- Assign the *same* half to two different keys and confirm the greater-Depth-wins resolution when both are pressed.
- Confirm cross-key claim awareness: assigning a target already claimed by another key shows the `.toast` banner and the assignment still succeeds (not rejected).
- Confirm the Device Overview grid shows the always-visible purple diagonal stripe on both Axis-assigned keys, and that toggling "Select Chord members" and clicking a striped key surfaces the inline error rather than selecting it.
- Confirm a plain Binding/Chord attempt on an already Axis-assigned Input is rejected with a specific error, and vice versa (assigning Axis to an Input that already has a Binding/Chord membership clears the old one atomically rather than leaving a stale entry).
- Try (and confirm the Daemon rejects at startup) a hand-edited `config.toml` where the same (Layer, Input) carries both a Binding and an axis assignment.
- Confirm the Device Overview grid button's label reads sensibly for a saved axis assignment (whatever format ticket 71 picked for `action_summary`'s Axis branch).

## Answer

_(unresolved)_

