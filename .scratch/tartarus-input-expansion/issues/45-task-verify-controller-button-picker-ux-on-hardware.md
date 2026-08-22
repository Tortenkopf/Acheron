Type: task
Status: resolved

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

## Answer

Every checklist item live-verified working, jointly with the user against their real daily-driver Daemon/Tartarus Pro/GUI — unlike ticket 44's sibling session, no bugs were found this time; ticket 43's build held up exactly as shipped.

**Install**: the running Daemon binary predated this ticket's changes (last built before `injector.rs`'s gamepad-device commit), so it was rebuilt (`cargo build --release`) and reinstalled to `~/.local/bin/acheron-daemon`, then the live `acheron-daemon.service` was stopped/restarted onto it — briefly dropping the user's real keyboard/mouse emulation, with their explicit go-ahead first.

**Picker rendering**: opened the real GUI against the real Daemon, opened a Grid key's Binding editor, switched Action to "Controller Button" — the gamepad diagram rendered inline immediately, nothing clipped or mispositioned, confirmed by the user visually. This also covers the space-budget sanity check (ticket 38's `_PAD_LAYOUT`/`_OFFSET_Y` geometry): held up as-is in the real per-key `Gtk.Window`, no re-tuning needed.

**Second `uinput` device**: `/proc/bus/input/devices` and `/dev/input/by-id` confirmed a distinct `"Acheron Virtual Controller"` node (`event27` + `js0`/joydev) alongside the existing `"Acheron Virtual Tartarus Pro"` keyboard node (`event24`) — exactly ticket 37's predicted shape.

**Per-category round-trip + real device output**: the user bound one Input per category (South, Left-Trigger/TL2, Left-stick click/THUMBL, D-pad Up, Start, Trigger-Happy1) plus one plain Keypress (`KEY_K`) on a separate Input, saved through the GUI, and physically pressed all seven. `evtest`/`jstest` were installed (`apt install evtest joystick`) and run concurrently against `event24`, `event27`, and `js0`:
- `event27` recorded exactly the six expected `BTN_*` codes, each a clean down/up pulse, in the order pressed, and never `KEY_K`.
- `event24` recorded exactly `KEY_K` down/up, and never any `BTN_*` code.
- `js0`'s `jstest --event` recorded matching button-index down/up pairs (index 0/6/11/13/9/17 = BtnA/BtnTL2/BtnThumbL/dpad-up/BtnStart/TriggerHappy1).

This confirms both the config.toml/D-Bus round-trip (`config.toml` showed all seven `[profiles.Default.base.grid_*]` entries with the correct `type`/`button`/`key` values) and that `input::is_gamepad_button`-based routing in the injector never crosses a keyboard code onto the gamepad sink or vice versa — real, concurrent, device-level confirmation, not just the Rust routing test's synthetic `RecordingSink`s.

**Extra-buttons grid**: the user expanded "Extra buttons (Trigger-Happy 1-40)" in the picker — expands correctly, and the already-saved Trigger-Happy1 selection showed highlighted, matching the real device-level capture above.

**Labels**: both the Device Overview grid button and the Action Table row show a readable label ("Btn: A / South", not "BTN_SOUTH") for the saved South binding, confirmed by the user.

**Allowlist guard against a real startup**: backed up the user's `config.toml`, hand-edited the South binding's `button` to `"KEY_A"` (outside the 57-entry gamepad allowlist), and restarted the Daemon — it refused to start with a specific error naming the exact bad value (`config.toml contains an Action::ControllerButton Binding whose button "KEY_A" is not a valid gamepad button`), confirming `parse()`'s `InvalidControllerButton` guard protects a real Daemon startup, not just the test suite. Restored the real config from backup (byte-identical, diffed) and restarted cleanly (one `systemctl --user reset-failed` needed first, since the induced failure had tripped systemd's own restart rate-limit — not a Daemon or Acheron bug).

No code changes this session — ticket 43's build was correct as shipped. Test suite untouched (199 Rust / 121 Python, unchanged from ticket 43).
