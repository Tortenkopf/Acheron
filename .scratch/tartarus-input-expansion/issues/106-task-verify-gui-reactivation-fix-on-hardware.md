Type: task
Status: open
Blocked by: 105

## Question

Verify [ticket 105](./105-task-fix-gui-reactivation-crash.md)'s re-activation fix live on
the real GNOME panel, the same build→verify discipline as every other pair on this map
(26→27, 93→94, 89→95, 102→103). The fix is a testable seam (`_activate_window` /
`_present_window` in `gui/acheron_gui/app.py`) with `gui/tests/test_app.py` coverage, but the
whole point of the bug was that it only showed up against a real second `Gio` activation
through GNOME Shell — which the headless suite can't exercise.

Needs the user to re-run `install.sh` first (the installed `~/.local/bin/acheron-gui` is a
pre-fix snapshot), then:

Checklist:

- **Cold start is still clean**: `gtk-launch acheron.desktop` (or the app-grid icon) with no
  GUI running — window opens, one tray icon appears, no traceback on stderr.
- **CLI re-invocation** (the path ticket 104 caught the crash on): with the GUI already
  running, `acheron-gui` from a terminal — the existing window is raised/focused, **no**
  `g-io-error-quark ... StatusNotifierItem` traceback on the primary's stderr, still exactly
  one tray icon, still exactly one window (check the window list / alt-tab).
- **`gtk-launch` re-invocation**: same again via `gtk-launch acheron.desktop` while running —
  existing window raised, no traceback, one icon, one window.
- **Re-activation while hidden to tray**: close the main window (ticket 36 hides it to the
  tray), then `acheron-gui` again — the window comes back visible and focused, not just
  raised-while-invisible.
- **Tray "Show" still works**: with the window hidden, the tray menu's Show item restores it
  (its callback now routes through the same `_present_window` helper — confirm no regression).
- **GNOME app-grid click** still focuses the running window cleanly (this path was always
  fine — shell-serviced, never hits D-Bus `activate` — but confirm the refactor didn't
  disturb it).

Capture the stderr of the primary across a few re-invocations (should be silent). GUI +
Daemon suites green. This closes the ticket 104 finding.

## Answer
