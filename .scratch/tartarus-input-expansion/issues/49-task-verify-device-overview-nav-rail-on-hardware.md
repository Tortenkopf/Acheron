Type: task
Status: resolved

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

## Answer

Live-verified against the real Daemon (already running HEAD, no rebuild needed — ticket 48 made no Rust changes) and the connected Tartarus Pro. No GUI-automation/screenshot tooling exists (confirmed, same gap ticket 48 hit), so the GUI was launched (`gui/.venv/bin/python3 gui/main.py`) for the user to drive directly; the whole checklist was run by the user at the machine and reported back as passing with no bugs found — the joint HITL session ticket 48 itself skipped came back clean, unlike its ticket-42→44 sibling. All seven items confirmed: default Grid destination with sidebar/switcher/separator/layer-bar/grid+thumbstick/Chords-placeholder all present; Profile CRUD (create/rename/switch/delete) round-trips against the real Daemon with the active-Profile delete button staying disabled; Base/Held tab auto-follow works on a real physical Mode-key hold/release; a real grid-key Binding saves through the modal editor and persists to `config.toml`; Library fully replaces the content area and Grid returns intact; the Profile sidebar's width holds steady across both destinations (ticket 47's 197px, `set_hexpand(False)` fix confirmed live); the tray mock and its Quick-switch popover still work.

Checked `config.toml` afterward for session cruft per ticket 34's cleanup precedent: it showed the "Testing" profile as active with a leftover `grid_r1c3` binding from the editor-save step. Flagged to the user rather than assumed — "Testing" turned out to be their actual daily-driver profile, not a session artifact; the throwaway profile created for this ticket's own Profile-CRUD check had already been deleted by the user during the session. No cleanup needed, no code changes, no test changes. Ticket 48's build is now fully live-hardware-verified.
