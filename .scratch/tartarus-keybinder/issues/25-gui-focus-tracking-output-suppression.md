# 25 — GUI: focus-scoped output suppression wiring

**What to build:** The real Acheron GUI actually drives ticket 24's Daemon-side suppression capability from its own window's live focus state, so the ticket 22 freeze and the stray-macro-output-in-a-text-field risk (ticket 23) are closed end-to-end — not just reachable via a manual D-Bus call. Whenever the GUI's main window has keyboard focus, it tells the Daemon to suppress output; whenever it doesn't, it tells the Daemon output is fine to resume. This includes the moment the GUI first connects, in case it starts out already focused.

**Blocked by:** 24

**Status:** ready-for-agent

- [ ] `app.py` wires GTK focus tracking (e.g. `notify::is-active` on the main `Gtk.ApplicationWindow`) to call ticket 24's new Daemon method on every focus change, passing the window's current focus state.
- [ ] Initial focus state is pushed once on GUI startup/connect, not just on subsequent transitions — covers the GUI launching while already the focused window.
- [ ] `gui/acheron_gui/daemon_client.py`'s `DaemonClient`/`DBusDaemonClient` gains the corresponding client method, following the existing call shape (e.g. `switch_profile`'s pattern).
- [ ] Test coverage at the existing `DaemonStub` D-Bus client seam: simulated focus-state changes call the new method with the correct value, including the one-shot initial-state call on startup.
- [ ] Live/manual verification against the real Daemon + real GUI + real hardware (per ticket 22/23's repro): starting a Toggle while the GUI window is already focused no longer freezes it, and focusing the GUI while a Toggle is already running suppresses its output without stopping the Toggle (confirm via `GetState()`/`active_toggles` that the Toggle is still reported active throughout). Record the result as a comment on this ticket.
