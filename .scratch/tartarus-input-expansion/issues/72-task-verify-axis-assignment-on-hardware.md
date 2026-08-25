Type: task
Status: resolved
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

Every checklist item live-verified against the real, connected Tartarus Pro and GUI, jointly with the user. Ticket 71's build held up almost entirely as shipped; found and fixed one real GUI bug.

**Install**: the running binary predated ticket 71's commit — rebuilt (`cargo build --release`) and reinstalled to `~/.local/bin/acheron-daemon`, service restarted.

**Bug found and fixed — Save never re-enabled after picking an Axis target**: `binding_editor.py`'s `render_action_editor` only set `save_btn`'s sensitivity once, at render time, based on whatever `draft["axis"]["target"]` was *before* the user touched the picker (`None` on a fresh Axis selection). `on_axis_changed` updated the draft but never re-armed Save, so picking a target in the diagram never enabled it — only a full Action-dropdown rebuild (switching away and back) happened to re-evaluate it, explaining the user's "toggling things helps but not reliably." Fixed by re-arming Save directly in `on_axis_changed` (`binding_editor.py`). New regression test `test_save_becomes_enabled_after_picking_an_axis_target` (the existing coverage clicked the target then called `Save.emit("clicked")` directly, which bypasses GTK's sensitivity gate and never caught this). 284 Python tests passing (283 + 1 new).

**Picker rendering**: inline, correctly positioned, no clipping — confirmed by the user against the real per-key `Gtk.Window`.

**Unsigned + signed round-trip, live-depth-driven `ABS_*` output**: assigned Right Trigger (unsigned, `ABS_RZ`) and Left Stick X+ (signed half, `ABS_X`) via the GUI; both round-tripped through `config.toml`/`GetConfig()` correctly. `evtest` against the `/dev/input/eventNN` "Acheron Virtual Controller" node confirmed a clean linear ramp on both: 0 at/below the Release point (112), linearly ramping to raw Depth at the Actuation point (128) — output values landing on exact multiples of 8 in the ramp zone, matching `resolve_axis_value`'s formula precisely — and raw-depth pass-through above Actuation, symmetric on release. An initial read of the very first capture looked like the release clamp wasn't firing (non-zero values below the Release threshold); turned out to be a misreading of correctly-computed ramp values (112 is itself a valid ramp output, from depth 126) — re-confirmed correct via temporary `eprintln!` instrumentation in `handle_depth_update` (later fully reverted) and a second clean `evtest` capture showing every value 8–255 appearing exactly twice (once per direction) with real hardware. No code changes needed here.

**Digital Capture mode step-increment fallback**: confirmed working via `Config.force_digital` (the GUI's "Force digital capture" checkbox in the Actuation section). Verified via temporary instrumentation and `evtest` on two independent clean runs (a cold Digital-mode daemon start, and a live Analog→Digital swap driven directly over D-Bus): Down/Repeat steps `ABS_RZ` by the build-time `AXIS_DIGITAL_STEP` (64), saturating; Up resets to 0. Three earlier attempts via the GUI checkbox produced zero output — traced to leftover device/daemon state from this session's own repeated foreground-debug-process churn (starting/killing standalone debug daemon builds competing for the hidraw device), not a real Daemon defect; a clean daemon start resolved it. No code changes needed here.

**Runtime conflict resolution — opposite halves**: assigned Left Stick X− to a second key (`grid_r2c1`); with `grid_r2c1` pressed first (becoming the current owner), pressing `grid_r3c4` (X+) simultaneously never flipped the output positive — stayed negative down to −255 the whole time, confirmed via `evtest` across an explicit press/hold/release sequence coordinated step-by-step with the user. Releasing the non-owner had no effect; releasing the owner ramped smoothly back to 0.

**Runtime conflict resolution — same half, greater Depth wins**: assigned Left Stick X+ to a third key (`grid_r2c2`, sharing the target with `grid_r3c4`). Holding `grid_r2c2` at full depth while briefly tapping `grid_r3c4` (a weaker press) produced one clean, uninterrupted ramp with no glitches — the shared-code max-picking logic (already exhaustively unit-tested) confirmed correctly plumbed for multiple real Inputs on real hardware.

**Cross-key claim toast**: re-picking Left Stick X+ for `grid_r2c2` (already claimed by `grid_r3c4`) showed the exact expected toast text.

**Device Overview grid**: all four Axis-assigned keys (`grid_r1c1`, `grid_r2c1`, `grid_r2c2`, `grid_r2c4`, `grid_r3c4` — five, not four, once the mutual-exclusion test's reassignment is counted) show the always-visible purple diagonal stripe; toggling Chord-member selection and clicking a striped key shows the inline error rather than selecting it.

**Mutual exclusion, both directions**: a plain Keypress Save attempt on the already-Axis-assigned `grid_r2c4` was rejected with the specific error (`"{input} already has an Axis assignment on this Layer — clear it first"`); a Chord-membership attempt on the same key was rejected the same way. Reverse direction: assigning Axis to `grid_r1c1` (which already carried a plain Keypress Binding) atomically cleared the old Binding — confirmed via `GetConfig()` showing `grid_r1c1` absent from `base` and present only in `axis_base`, no stale entry.

**Hand-edited `config.toml` startup rejection**: backed up the real config, hand-added a `[profiles.Testing.base.grid_r2c4]` Binding alongside the existing Axis assignment for the same Input — the Daemon refused to start with `"config.toml contains both an Axis assignment and a Binding for \"grid_r2c4\" on the same Layer"`. Restored the backup (byte-identical, diffed) and restarted cleanly.

**Label check**: Device Overview buttons read `"Axis: Right Trigger"` / `"Axis: Left Stick X+"` — confirmed the `"Axis: "` prefix is present (the user's first report omitted it in shorthand, but the actual button text has it).

**No further code changes** beyond the one Save-button fix. 329 Rust + 284 Python tests passing. The "Testing" profile's five test axis assignments (`grid_r1c1`→Throttle, `grid_r2c1`→Left Stick X−, `grid_r2c2`/`grid_r3c4`→Left Stick X+, `grid_r2c4`→Right Trigger) are left in place at the user's request as a standing test sandbox, `force_digital` restored to `false` (Analog mode).

The axis-assignment strand (tickets 59 → 60 → 71 → 72) is now fully built and live-verified end to end.

