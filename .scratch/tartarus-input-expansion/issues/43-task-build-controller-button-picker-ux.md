Type: task

## Question

Build `Action::ControllerButton` and its picker for real, against [Design Controller/Joystick output emulation](./14-decide-controller-joystick-output-emulation.md)'s settled Daemon model and [Design the controller-button picker GUI](./38-prototype-controller-button-picker-ux.md)'s settled shape (variant A — Gamepad Diagram). No open design questions remain — this is implementation + live verification, per this map's "resolving a ticket means actually building and testing against the real, connected Tartarus Pro" discipline.

Scope, per ticket 14's Answer:

- **New `Action::ControllerButton { button: KeyCode }`** variant, reusing the existing Binding/Trigger-mode/dispatch/executor pipeline exactly as `Action::Keypress` does — only the target `uinput` device differs.
- **Second `uinput` device** in `injector.rs`, distinct from the existing keyboard device, advertising the full standard Linux Gamepad Spec capability set: the named `BTN_GAMEPAD` range (`BTN_SOUTH/EAST/NORTH/WEST/TL/TR/TL2/TR2/SELECT/START/MODE/THUMBL/THUMBR`), `BTN_TRIGGER_HAPPY1`–`BTN_TRIGGER_HAPPY40`, and `BTN_DPAD_UP/DOWN/LEFT/RIGHT` — mirroring the existing device's "advertise everything, curate in the GUI" precedent.
- **Allowlist validation** in `SetBinding` against that same 57-entry set (curated gamepad `KeyCode`s only — reject anything outside it, same shape as ticket 02's mouse-button precedent).
- **Macro step support**: `KeyDown`/`KeyUp` step values may carry a `BTN_*` gamepad code the same way they already carry mouse-button codes — no new step-type work, just confirm the existing step editor's key field (once ticket 42 lands) doesn't need to special-case gamepad codes.

Scope, per ticket 38's Answer:

- **Gamepad diagram picker widget**: a reusable component — a collapsed "`<button label>` ▸ Change" summary button that expands into the visual controller face (d-pad diamond, ABXY diamond, shoulders/triggers, stick clicks, Select/Start/Mode) plus a separate collapsed "Extra buttons (Trigger-Happy 1-40) ▸" grid — driven off the real curated gamepad allowlist above, not the prototype's hand-listed catalog. Port the prototype's live-corrected geometry (`prototype/38-controller-button-picker-ux`, `_PAD_LAYOUT`/`_OFFSET_Y` in `prototype_38_controller_button_picker_ux.py`) as a starting point, re-tuned against the real popover's actual space budget rather than copied blindly.
- **Wire into `render_action_editor`**: `Action` dropdown gains a "Controller Button" option alongside Keypress/Macro, showing the diagram picker in `editor_slot` when selected — independent picker under the existing Action-kind selector, per ticket 38's settled answer (no shared container with the Keypress picker beyond the same component shape).
- **No suggested-default chip, no freedom note** — ticket 38 explicitly cut both after live reaction; the picker opens on a plain default with zero built-in opinion about what a given Input "should" be, and needs no explanatory text since nothing is ever restricted.
- **CONTEXT.md**: confirm the Controller glossary entry (added in ticket 14) doesn't need a picker-specific addendum; likely doesn't.

Live-hardware verification: assign a controller button to a physical Input, confirm the second `uinput` device shows up as a distinct gamepad node (`/dev/input/jsX` via `joydev`, per ticket 37's closed research) and fires real button events end-to-end against the real Tartarus Pro, for at least one entry per category (a face button, a shoulder, a stick click, a d-pad direction, Select/Start/Mode, and one Trigger-Happy extra).
