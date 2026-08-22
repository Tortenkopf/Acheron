Type: task

## Question

Live-verify [Build the Device Overview nav-rail restructuring for real](./48-task-build-device-overview-nav-rail.md)'s build against the real, connected Tartarus Pro and GUI — the joint HITL session that ticket 48 itself skipped (no GUI-automation/screenshot tooling in that session to click through the running app, and pressing the physical Mode key is something only a human at the machine can do; per this map's ticket 26/42 precedent, swapping the user's live input/GUI state unasked is out of scope for an unattended session).

Checklist:

- Install the new binary/GUI and open Device Overview against the real Daemon — confirm it opens on the Grid destination by default, with the Profile sidebar, "Grid"/"Library" switcher, separator, real `build_layer_bar`, grid+thumbstick, and the Chords placeholder slot all present.
- Profile sidebar: create a Profile, rename it, switch to it, and delete a non-active one, all against the real Daemon — confirm each round-trips (`CreateProfile`/`RenameProfile`/`SwitchProfile`/`DeleteProfile`) and the active-Profile delete button stays disabled.
- Base/Held tabs: click Held, confirm the grid re-renders that Layer's Bindings; then physically hold and release the real Mode key (with `mode_key_role` at its default Layer-shift) and confirm the tab indicator auto-follows the live `ActiveLayerChanged` push, exactly as before this restructuring.
- Open a real Grid key's per-key Binding editor (`make_input_button`'s modal `Gtk.Window`) and save a real Binding — confirm it persists to `config.toml` and the grid button's label updates.
- Click "Library" and confirm the content area fully replaces with the placeholder (no grid, no layer bar, no Chords slot visible) while the Profile sidebar and switcher stay put; click back to "Grid" and confirm everything returns.
- With the window at a normal (non-maximized) size, measure the Profile sidebar's real allocated width while Grid is showing and again while Library is showing (e.g. via GTK inspector or a quick `get_allocation()` probe) — confirm it's the same in both, matching ticket 47's prototype-measured 197px and its live `set_hexpand(False)` fix.
- Confirm the tray mock (`build_tray_mock`) still renders sensibly alongside the new layout and its "Quick switch" popover still works.
