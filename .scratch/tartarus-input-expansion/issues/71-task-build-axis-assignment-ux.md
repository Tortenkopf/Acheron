Type: task
Status: open

## Question

Build axis assignment for real, against [Design Controller/Joystick axis output](./59-decide-controller-axis-output.md)'s settled Daemon model and [Prototype the axis-assignment GUI](./60-prototype-axis-assignment-ux.md)'s settled shape (variant B's fork/grid treatment, variant A's diagram picker, refined over five rounds of live reaction). No open design questions remain — this is implementation + live verification, per this map's "resolving a ticket means actually building and testing against the real, connected Tartarus Pro" discipline.

Scope, per ticket 59's Answer:

- **`Profile.axis_base` / `Profile.axis_held: HashMap<Input, AxisTarget>`** — a new parallel per-Layer map alongside the existing `bindings_*`/`chords_*` maps, not a new `Action` variant. `AxisTarget` is one of the 17 settled targets (5 unsigned: `ABS_Z`/`ABS_RZ`/`ABS_THROTTLE`/`ABS_GAS`/`ABS_BRAKE`; 6 signed axes × independently-assignable +/- halves: `ABS_X`/`Y`/`RX`/`RY`/`RUDDER`/`WHEEL`).
- **Mutual exclusion**: an Input present in a Layer's axis map is structurally excluded from that Layer's `bindings_*` map *and* from Chord membership on that Layer — `SetAxisAssignment` must clear any existing Binding/Chord-membership on the same (Layer, Input) atomically (mirrors `SetBinding`'s existing atomic-persist precedent), and `SetBinding`/`SetChordBinding` must reject a grid key that already has an axis assignment on that Layer with a specific error, not a silent overwrite.
- **Two new D-Bus methods**: `SetAxisAssignment(input, layer, target)` / `ClearAxisAssignment(input, layer)`, active-Profile-scoped and atomically persisted like `SetBinding`. `GetConfig()` gains the `axis_base`/`axis_held` maps.
- **`uinput` capability**: the existing single gamepad-identity `uinput` device (`injector.rs::build_gamepad_device`, from ticket 43) gains the 17 targets' underlying `ABS_*` codes (`ABS_Z`/`RZ`/`THROTTLE`/`GAS`/`BRAKE`/`X`/`Y`/`RX`/`RY`/`RUDDER`/`WHEEL` — 11 codes, the +/- halves share one `ABS_*` code each per ticket 59 §3) — no second device.
- **Live-depth-driven axis resolution**: reuses the key's existing Actuation/Release-point thresholds (tickets 17/19/26) as the axis's start/end thresholds — 0 output below Release, linear ramp to raw Depth above Actuation, per ticket 59 §4. Implement as its own seam, `(Depth, edge_event) → axis_value` (ticket 59 §7's forward-looking note), even though "Live/linear" is the only resolver built now.
- **Runtime conflict resolution** (ticket 59 §5): opposite halves of one signed axis — whichever key is already actively outputting suppresses the other. Same-half sharing — take the greater of the two Depths when both are pressed.
- **Digital Capture mode fallback** (ticket 59 §6): press/release step-increment; exact step size a build-time-tuned constant, same precedent as Analog-repeat's four TBD constants.
- **CONTEXT.md**: confirm the Axis assignment glossary entry (added in ticket 59) doesn't need a build-specific addendum.

Scope, per ticket 60's Answer:

- **`binding_editor.py`**: `ACTION_TYPES` gains a 6th entry, `"Axis"`, offered only when `is_grid_input(inp)` — non-grid Inputs never see it. Trigger-mode dropdown locks insensitive with a tooltip ("Axis output has no Trigger mode") when Axis is selected, mirroring the existing Profile Switch lock.
- **Diagram picker** (`prototype/60-axis-assignment-ux`'s `build_axis_picker_diagram`, ported against the real 17-target catalog, not the prototype's hand-listed one): Left Stick / Right Stick each a 4-direction cross with the stick's label directly above it; Left Trigger beside the Left stick, Right Trigger beside the Right stick; a horizontal rule below the sticks; **Driving** (Wheel +/- inline with Gas, Brake) and **Flight** (Rudder +/- inline with Throttle) as two named groups below the rule.
- **Cross-key claim toast**: reuses the real app's existing `.toast` CSS convention (ticket 55) — "Also assigned to `<key>` — allowed, both keys will drive this axis," shown when the picked target is already claimed by another key on the same Layer.
- **Device Overview grid**: Axis-assigned grid keys carry an always-visible purple diagonal-stripe treatment (new `.axis-stripe` CSS class, `#8e44ad` accent — same color used for the picker's current-target highlight) — visible regardless of Chord-selection mode. The button stays fully clickable at all times; clicking a striped key while selecting Chord members surfaces an inline error line ("`<key>` is Axis-assigned — can't join a Chord") rather than disabling the button or toggling it into the selection.
- **`action_summary`**: needs an Axis-assignment rendering path for the grid button label — the prototype didn't model this (it has no `action_summary` equivalent), so pick a concise format (e.g. `"Axis: Right Trigger"` or `"Axis: Left Stick X+"`) consistent with the existing `"Btn: <label>  [<trigger>]"`/`"Ctrl+A  [1x]"` conventions, minus the Trigger-mode suffix (Axis has none).

Live-hardware verification: assign an axis target to a physical grid key, confirm the `uinput` device reports the new `ABS_*` capability and produces a real depth-driven axis value end-to-end against the real Tartarus Pro, for at least one unsigned target, one signed half, and the Digital-mode step-increment fallback; confirm the Chord-recording grid correctly excludes an Axis-assigned key; confirm a hand-edited `config.toml` conflict (a Binding and an axis assignment on the same Input/Layer) is rejected at startup. Deliberately deferred to [Verify axis-assignment on hardware](./72-task-verify-axis-assignment-on-hardware.md), not done in this ticket — matches ticket 43/45's split.

## Answer

_(unresolved)_
