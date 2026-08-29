Type: task
Status: resolved

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

## Answer

Fixed with a create-once guard, not `do_startup`. `Gio.Application` re-emits
`activate` on every secondary launch, so the entire build was moved out of
`do_activate` into a new one-time `AcheronApplication._build_main_window()`;
`do_activate` now only calls a dispatch helper.

**`gui/acheron_gui/app.py`:**

- **`_activate_window(existing, build, present)`** (module function) — `win =
  existing if existing is not None else build(); present(win); return win`.
  `do_activate` is now just
  `self._main_window = _activate_window(self._main_window, self._build_main_window, _present_window)`.
  The `self._main_window` instance attr (init `None`) is the guard — chosen
  over `get_active_window()` because a window hidden to the tray
  (`_wire_window_close_to_hide`) isn't reliably "active", which would let a
  second activation build a second window anyway.
- **`_present_window(win)`** (module function) — `win.set_visible(True);
  win.present()`. The explicit show is the ticket's "re-show it when
  hidden-to-tray" decision: re-activation while the window is tray-hidden
  makes it visible again, not just raised-while-invisible.
- **`_build_main_window(self)`** — the old `do_activate` body verbatim
  (CSS provider, `Gtk.ApplicationWindow`, `TrayIcon`, all D-Bus
  subscriptions, `_ensure_daemon_started_on_launch`, the idle-drain, first
  `rebuild()`), ending `return win` instead of `win.present()`. Now runs
  exactly once per process, so the duplicate CSS-provider registration and
  duplicate subscriptions on re-activation are gone too, not just the SNI
  crash.
- `TrayIcon(...)` construction: `on_show_window` callback changed from
  `win.present` to `lambda: _present_window(win)` so the tray "Show" item
  also un-hides a tray-hidden window through the one helper. `__init__`
  gained an injectable `tray_bus=None` (mirrors `client`/`systemd_client`)
  so a test can hand `TrayIcon` a fake bus — the tray's real
  `SessionMessageBus` was the main thing blocking a headless `do_activate`
  test.
- Module + `_ensure_daemon_started_on_launch` docstrings updated for the
  `do_activate` → `_build_main_window` move.

**`gui/tests/test_app.py`:** four new tests against the seam (the suite's
established pattern — every `_wire_*` helper is tested this way, never the
live `do_activate`, which needs a registered `Gtk.Application` + live
session bus + a mapped top-level):

- `test_present_window_shows_a_tray_hidden_window_before_presenting_it`
- `test_first_activation_builds_the_window_and_presents_it`
- `test_second_activation_reuses_the_window_without_rebuilding` — the core
  regression: `build()` runs once across two activations, `present()` runs
  twice.
- `test_reactivation_reshows_the_window_when_it_was_hidden_to_the_tray`

336 Python tests green (was 332). Daemon suite untouched.

**Live GNOME check not done this session** — the installed
`~/.local/bin/acheron-gui` is a pre-fix snapshot (needs `install.sh` re-run)
and exercising a real second `Gio` activation through GNOME Shell is
inherently HITL. Spawned
[Verify the GUI re-activation fix on hardware](./106-task-verify-gui-reactivation-fix-on-hardware.md)
(blocked on this ticket) for the cold-start / `acheron-gui` re-invoke /
`gtk-launch` re-invoke / hidden-to-tray checklist on the user's panel.
