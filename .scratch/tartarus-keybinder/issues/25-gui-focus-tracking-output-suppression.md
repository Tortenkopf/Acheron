# 25 — GUI: focus-scoped output suppression wiring

**What to build:** The real Acheron GUI actually drives ticket 24's Daemon-side suppression capability from its own window's live focus state, so the ticket 22 freeze and the stray-macro-output-in-a-text-field risk (ticket 23) are closed end-to-end — not just reachable via a manual D-Bus call. Whenever the GUI's main window has keyboard focus, it tells the Daemon to suppress output; whenever it doesn't, it tells the Daemon output is fine to resume. This includes the moment the GUI first connects, in case it starts out already focused.

**Blocked by:** 24

**Status:** implemented-pending-live-verification

- [x] `app.py` wires GTK focus tracking (e.g. `notify::is-active` on the main `Gtk.ApplicationWindow`) to call ticket 24's new Daemon method on every focus change, passing the window's current focus state.
- [x] Initial focus state is pushed once on GUI startup/connect, not just on subsequent transitions — covers the GUI launching while already the focused window.
- [x] `gui/acheron_gui/daemon_client.py`'s `DaemonClient`/`DBusDaemonClient` gains the corresponding client method, following the existing call shape (e.g. `switch_profile`'s pattern).
- [x] Test coverage at the existing `DaemonStub` D-Bus client seam: simulated focus-state changes call the new method with the correct value, including the one-shot initial-state call on startup.
- [ ] Live/manual verification against the real Daemon + real GUI + real hardware (per ticket 22/23's repro): starting a Toggle while the GUI window is already focused no longer freezes it, and focusing the GUI while a Toggle is already running suppresses its output without stopping the Toggle (confirm via `GetState()`/`active_toggles` that the Toggle is still reported active throughout). Record the result as a comment on this ticket.

## Comments

Implemented in `gui/acheron_gui/app.py` (`_wire_focus_tracking`), `gui/acheron_gui/daemon_client.py` (`set_output_suppressed` on both `DaemonClient` and `DBusDaemonClient`, calling `SetOutputSuppressed(b)`), and `gui/acheron_gui/daemon_stub.py` (`DaemonStub.set_output_suppressed`, recorded to `.calls` for test assertions).

`notify::is-active` carries no value of its own and `is-active` is a read-only, WM-computed property (`Gtk.Window.set_property("is-active", ...)` raises) — nothing can force it in a headless test. `_wire_focus_tracking` is duck-typed against `is_active()` + `connect()` rather than annotated to a real `Gtk.Window`, so `gui/tests/test_app.py` drives it with a small fake window exposing a `simulate_focus_change()` seam, the same shape as `DaemonStub`'s own `simulate_*` methods. The same handler is both connected to the signal and called once immediately for the initial push, so there's one code path instead of two that could drift.

`/code-review` (medium) caught a real reentrancy hazard before commit: the first pass called `client.set_output_suppressed` straight through from the `notify::is-active` handler, which can fire while some other blocking `call_sync` (e.g. `switch_profile`) is still in flight on the same `GMainContext` — nesting a second blocking D-Bus round-trip inside the first one's still-unfinished wait, the same class of hang `app.py`'s own module docstring documents for `SwitchProfile`/`ActiveProfileChanged` (tickets 18/19). Fixed by deferring the call via `GLib.idle_add`, matching `on_layer_changed`/`on_profile_changed`'s existing guard. `test_app.py` pumps the default `GMainContext` after each simulated focus change to observe the deferred call, plus a regression test asserting the signal handler never calls through synchronously.

Not yet verified against the real Daemon + real GUI + real hardware (no hardware access in this session) — see the open live-verification checklist item above.
