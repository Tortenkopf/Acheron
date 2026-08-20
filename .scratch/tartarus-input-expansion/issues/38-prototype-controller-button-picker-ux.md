Type: prototype

## Question

Design the look and feel of the controller-button picker GUI, building on the settled model from [Design Controller/Joystick output emulation](./14-decide-controller-joystick-output-emulation.md): `Action::ControllerButton { button: KeyCode }`, no hardcoded physical-Input-to-button-code correspondence — the user picks freely, from the full standard Linux Gamepad Spec capability set (`BTN_SOUTH/EAST/NORTH/WEST/TL/TR/TL2/TR2/SELECT/START/MODE/THUMBL/THUMBR`, the full `BTN_TRIGGER_HAPPY1`–`40` extra-button range, and `BTN_DPAD_UP/DOWN/LEFT/RIGHT`).

Settle at least:

- **Layout**: a labeled gamepad-diagram picker (visual controller face, click a button on the diagram) vs. a category-sorted list/menu (Face buttons / Shoulders / Sticks / D-pad / Extra 1-40), similar in spirit to [ticket 32](./32-prototype-key-mouse-button-picker-ux.md)'s two-candidate approach for the key/mouse-button picker. A ~40-slot `BTN_TRIGGER_HAPPY` range in particular needs a layout that doesn't overwhelm — likely the category/list approach for that part even if the named buttons get a visual diagram.
- **Where it lives** relative to `binding_editor.py`'s existing controls — same question ticket 32 settles for its own picker; check whether these two pickers should share a container/pattern (e.g. an Action-kind selector that swaps in Key/Mouse picker vs. Controller picker) or stay fully independent.
- **Sane defaults without hardcoding**: ticket 14 deliberately left button assignment fully free-form (no forced physical→button correspondence), but a picker with zero defaults is a bad first-run experience. Does the picker *suggest* a default (e.g. Mode key's assignment field opens pre-scrolled to `BTN_MODE`, thumbstick directions pre-scrolled to `BTN_DPAD_*`) without restricting the choice?
- **Whether this reuses any of ticket 32's picker infrastructure** — both pickers assign a bare `KeyCode` to a Binding's Keypress-shaped field, just from different allowlists (keyboard/mouse vs. gamepad). Check for a shared underlying widget before building two independent pickers from scratch.
- **Discoverability of the "any Input, any button" freedom** — since there's no hardcoded correspondence, the UI should make it obvious this is intentional (not a bug/gap) rather than confusing about why, say, a grid key can be assigned `BTN_START`.

Use the `/prototype` skill. Once resolved, spawn the real build ticket (Daemon: new `Action::ControllerButton` variant + second `uinput` device in `injector.rs` + allowlist validation in `SetBinding`; GUI: the real picker), matching the decide → prototype → build pattern used for Chord (01 → 30 → build) and the trigger-point UX (17 → 19 → 26 → 27).
