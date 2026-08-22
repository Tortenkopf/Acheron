Type: task

## Question

Live-verify [Build the controller-button picker UX and Action::ControllerButton for real](./43-task-build-controller-button-picker-ux.md)'s build against the real, connected Tartarus Pro and GUI — the joint HITL session that ticket 43 itself skipped (no physical device access from this unattended session, matching ticket 42/44's precedent for the sibling key/mouse-button picker).

Checklist, per ticket 43's own "Live-hardware verification" scope:

- Install the new binary and open the real GUI against the real Daemon; open a Grid key's Binding editor, switch Action to "Controller Button", and confirm the gamepad diagram renders inline (not a text entry, not a broken/mispositioned popover — ticket 44 found the sibling picker's original collapsed-popover shape broken on this GTK4/Wayland stack, so this picker skipped straight to always-inline; confirm that choice actually holds up live, not just in headless widget-tree tests).
- Confirm the second `uinput` device shows up as a distinct gamepad node — `/dev/input/jsX` via `joydev` (ticket 37's closed research) and/or `evtest`/`ls /dev/input/by-id` showing "Acheron Virtual Controller" alongside the existing "Acheron Virtual Tartarus Pro" keyboard device.
- Assign a controller button to a physical Input for at least one entry per category (a face button, a shoulder/trigger, a stick click, a d-pad direction, Select/Start/Mode, and one Trigger-Happy extra) and confirm each one round-trips through `config.toml`/D-Bus and fires a real button event on the gamepad device end-to-end — e.g. via `evtest`/`jstest` on the new node, or a real game/controller-test utility.
- Confirm a keyboard/mouse Binding on a different Input still fires correctly on the *original* device at the same time — the injector's code-based routing (`input::is_gamepad_button`) must never cross-route a keyboard code onto the gamepad device or vice versa.
- Confirm the Extra-buttons grid (Trigger-Happy 1-40) expands correctly and a picked extra button round-trips the same way as a named one.
- Confirm the Device Overview grid button and Action Table row both show a readable label ("Btn: A / South", not "BTN_SOUTH") for a saved ControllerButton Binding.
- Sanity-check the gamepad diagram's real popover/window space budget — ticket 38's prototype geometry (`_PAD_LAYOUT`/`_OFFSET_Y`) was tuned via live reaction against the *prototype's* own host mock, not the real per-key `Gtk.Window`; re-tune if it clips or looks cramped in the real container.
- Try (and confirm the Daemon rejects) a hand-edited `config.toml` `Action::ControllerButton` binding whose `button` is outside the 57-entry gamepad allowlist (e.g. `"KEY_A"`) — confirms `parse()`'s `InvalidControllerButton` guard actually protects a real Daemon startup, not just the test suite.
