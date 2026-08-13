# 20 — Daemon/device status surface

**What to build:** The GUI reliably tells the user whether the Daemon is running and whether the Tartarus Pro is connected, in both the main window and the tray, and blocks editing when either isn't true — rather than silently letting edits look like they're taking effect when they can't. Build from `prototype/12-daemon-device-status-indicators/prototype.py` directly: it already implements the status chip, tray line, and dimmed disabled-grid overlay against a `DaemonStub` reused from ticket 16's prototype basis — this ticket's job is wiring that to real Daemon-side detection, not redesigning it. See `.scratch/tartarus-keybinder/spec.md` ("Daemon event loop and concurrency" — failure handling, and "GUI information architecture" — status indicators) for the full design.

**Blocked by:** 16

**Status:** ready-for-agent

- [ ] `CaptureSource` failure handling splits by cause: device-absent (nodes don't exist — at startup, or after a mid-run unplug) is non-fatal and polls the known `/dev/input/by-id/...` paths every ~2s until they reopen cleanly, then resumes; genuine capture errors (e.g. a `uinput` write failure) remain fatal-exit.
- [ ] `GetState()` gains a `device_connected: b` field reflecting the poll loop's current view, and a `DeviceConnectionChanged(connected: b)` signal fires on every transition.
- [ ] The GUI detects Daemon presence via a live `NameOwnerChanged` watch on `com.acheron.Daemon` on the session bus — not a one-shot check on window open.
- [ ] A status chip (colour dot + label) appears above Device Overview and a matching line appears in the tray mock, both reflecting all three reachable states (running+connected / running+disconnected / not running) from the same `GetState()`/signal data, per the prototype.
- [ ] Whenever status isn't running+connected, the entire Device Overview grid is disabled (`set_sensitive(False)`) under a dimmed `Gtk.Overlay` with a centered message naming which condition is unmet, matching the prototype's two message strings; the overlay label uses `hexpand=True`/`vexpand=True` alongside `halign`/`valign = CENTER` (the centering pitfall the prototype already caught).
- [ ] Live demo: physically unplug the Tartarus Pro — chip, tray line, and grid overlay all flip to disconnected within ~2s, and editing is blocked; replug it and confirm automatic recovery with no Daemon restart. Separately, kill the Daemon process — chip/tray flip to "not running" and editing is blocked.
- [ ] Automated tests use the fake `CaptureSource` (device-absent scripted) and a fake Daemon D-Bus object to exercise all three status states in the GUI without real hardware or a real Daemon process.
