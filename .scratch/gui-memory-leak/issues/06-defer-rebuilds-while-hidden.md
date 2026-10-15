# 06: Skip rebuilds while the window is hidden in the tray

Spec: [`../spec.md`](../spec.md). Diagnosis cause 3; Implementation Decisions, "Defer rebuilds
while hidden".

**What to build:** While the main window is hidden to the tray, Daemon signals (Layer changes on
every Mode-key press and release, Profile switches, Daemon up/down, device connect/disconnect,
Capture-mode changes) still update the GUI's state and the tray icon. They only mark Device
Overview as stale instead of rebuilding it. When the window is shown again (from the tray, or by
a second launch), it rebuilds once if anything changed, so it opens showing current state. While
the window is visible, nothing changes.

**Blocked by:** None (can start immediately).

**Status:** done (2026-10-15)

- [x] A new `_wire_*`-style function, taking the duck-typed window (`get_visible()` +
      `connect()`) and the real rebuild, gates rebuilds on visibility and listens for
      `notify::visible`.
- [x] While hidden, triggers call the rebuild 0 times but still update `ui_state["selected_layer"]`
      and the status dict.
- [x] Becoming visible after any trigger calls the rebuild exactly once. Becoming visible with
      nothing changed calls it 0 times.
- [x] While visible, triggers rebuild immediately, as today.
- [x] The tray icon's `update()` (Profile list, status dot) runs on every trigger regardless of
      visibility.
- [x] Fake-window tests in the style of the existing `_wire_focus_tracking` /
      `_wire_window_close_to_hide` tests cover all of the above.

## Comments

- 2026-10-15: Done. `app.py`'s old `rebuild()` is now two functions. `refresh()` re-reads
  `GetConfig()`/`GetState()` into `last_known`/`status` and updates the tray icon. `rebuild_view()`
  is the costly widget rebuild. `_wire_rebuild_deferral_while_hidden(window, refresh, rebuild_view)`
  returns the `on_change` every trigger (and every committed edit) calls: it always refreshes,
  rebuilds the view only while the window is visible, and otherwise marks the view stale for the
  next `notify::visible`. The gate also takes `refresh`, so the "tray always updates" criterion can
  be tested with the fake window. The startup build is now deferred to `_present_window`'s
  `set_visible(True)`, which fires before `present()`. That was checked against a real
  `Gtk.Window`. 7 new fake-window tests; 655 GUI tests pass.
- Not pinned by a test: `on_layer_changed` sets `ui_state["selected_layer"]` before the gated call.
  The code is correct, but that closure lives in the untested `_build_main_window`. The same goes
  for `refresh()` actually calling the tray's `update()`.
- Open for ticket 07 to weigh: while hidden, each Mode-key edge still costs two synchronous D-Bus
  calls plus a tray `update()`. That is cheap and leaks nothing, but it isn't "free" as user
  story 2 says.
