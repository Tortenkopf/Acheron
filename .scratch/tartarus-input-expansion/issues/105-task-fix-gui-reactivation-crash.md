Type: task
Status: open

## Question

`AcheronApplication.do_activate` (`gui/acheron_gui/app.py:333`) does all its
setup — `Gtk.ApplicationWindow`, `TrayIcon`, CSS provider, signal
subscriptions, `_ensure_daemon_started_on_launch` — unconditionally, every
time the application is activated, not just the first. `Gio.Application`
emits `activate` on **every** invocation, including a second `acheron-gui`
process or `gtk-launch acheron.desktop` handing off to the running primary.

On that second activation:

- a fresh `Gtk.ApplicationWindow(application=self, …)` is built (`:342`) and
  registered on the app but never shown;
- `TrayIcon(…)` (`:357`) calls `self._bus.publish_object(ITEM_OBJECT_PATH, …)`,
  which collides with the SNI object the first instance already exported and
  raises `gi.repository.GLib.GError: g-io-error-quark: An object is already
  exported for the interface org.kde.StatusNotifierItem at /StatusNotifierItem`;
- the exception aborts `do_activate` before the `win.present()` at `:456`, so
  the **existing** window is never raised and a zombie second `ApplicationWindow`
  is left registered on the app.

Reproduced live during [ticket 104](./104-task-verify-ticket-88-90-code-review-fixes.md)
against the real running GUI + daemon: cold start is clean, but every
subsequent `acheron-gui` from a terminal throws the traceback above on the
primary's stderr and does not focus the running window.

Not caused by the ticket 88/90 fixes — dates to [ticket 36](./36-task-build-tray-icon.md)
(the tray icon was added into `do_activate`). It contradicts
[ticket 90](./90-task-desktop-app-launcher.md)'s verification note ("second
launch just presents the existing window … no second window / no error").
That note held in practice only because a GNOME app-grid **click** on a
running app is serviced by GNOME Shell raising the window itself, never
calling D-Bus `activate` — confirmed again in ticket 104 (the user's
app-grid click focuses the existing window cleanly). The broken path is the
CLI / `gtk-launch` re-invocation, which is exactly what ticket 90's own
"`acheron-gui` ↔ grid reach the same process" check exercises.

### Scope

- Move one-time setup (window, tray, CSS, subscriptions, daemon-start net) to
  `do_startup` or behind a create-once guard; make `do_activate` just
  `self.get_active_window().present()` (or create-then-present if none).
- Decide what re-activation should do to the window when it is currently
  hidden-to-tray (`_wire_window_close_to_hide` means close ≠ quit): re-show it.
- Regression test in `gui/tests/test_app.py`: a second `activate` emits no
  exception, creates no second window, presents the existing one. The existing
  `test_initial_focus_state_is_pushed_once_on_connect`-style idle-pump harness
  already exercises `do_activate`.
- Live check on the real GNOME panel (map's execution discipline): cold start,
  then `acheron-gui` again and `gtk-launch acheron.desktop` again — window
  raised, no traceback, one tray icon, one window.

### Out of scope

- The tray icon's own design / menu behaviour (tickets 36, 98) — untouched.
- The launcher and `.desktop` file (tickets 90, 96, 104) — verified good.
