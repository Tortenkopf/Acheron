# 04: Break the reference loops inside the Binding editor

Spec: [`../spec.md`](../spec.md). Diagnosis cause 2; Implementation Decisions, "Break
editor-internal cycles".

**What to build:** Opening and closing a Binding editor any number of times retains nothing.
Today a destroyed editor still leaks about 1.4 MB, because signal handlers on its widgets and
event controllers capture their own ancestors. GTK's parent→child ownership and closures on
GObjects without a live Python wrapper are invisible to Python's cycle collector, so it can't
free them. Known instances: the Depth bar's drag-gesture handlers (bound methods on a controller
it owns) and its map/unmap handlers, the editor window's Save/close-request closures, the DropDown
`notify::selected` handlers. Accepted techniques: a weak reference resolved at call time, reading
the target from the signal's emitter (`get_root()`, `get_ancestor()`), or disconnecting on
teardown. The soak test decides what needs fixing, not this list.

**Blocked by:** 02.

**Status:** done (2026-10-15)

- [x] Editor open/close soak test: click a grid-key Input (so the Depth bar is built), close the
      editor, repeat N times. After `gc.collect()`, no widget from any closed editor is alive
      (weak references taken while it was open), and 0 top-level windows remain.
- [x] The same holds for a Save-and-close cycle and for a rebuild while an editor is open.
- [x] The same holds for at least one non-grid Input (no Depth bar) and for each Action kind's
      editor fields (Keypress with the inline key picker, Macro, Stepper, Controller, Switch
      Profile, Axis).
- [x] Depth bar dragging, the live depth fill, the 200 ms resync while mapped, and every DropDown
      still behave as before (existing tests stay green).
- [x] The rule "a handler on a widget must not strongly capture that widget's ancestors" is noted
      where the GUI's widget-building conventions live, so new code doesn't reintroduce it.

## Comments

- 2026-10-15: Done. The soak test (`gui/tests/test_rebuild_soak.py`) found six cycle roots:
  DepthTrack's drag-gesture bound methods (the controller held its own bar), five "render
  closure captures the container it refills" sites reached from the buttons inside that
  container: `build_dual_stage_panel`'s `rebuild` → `panel`, the key picker's `grid_slot`, the
  controller and axis pickers' `panel`, and `build_action_and_trigger_fields`' `sync_analog_hint`
  → `fields`/`trigger_row`; and the name-prompt popover's `popover` capture. The DropDown
  `notify::selected` handlers and the Save/close-request closures were only leaking *through*
  those roots and needed no change of their own. Fixed with the emitter (`gesture.get_widget()`,
  `get_ancestor(Gtk.Popover)`) or a GObject weak ref (`widget.weak_ref()`).
- Gotcha worth knowing for ticket 05: Python's `weakref.ref(widget)` is useless here, since the
  PyGObject wrapper dies as soon as Python drops it while the widget lives on (the first attempt
  broke every in-editor re-render). The soak test likewise uses `w.weak_ref()`, so it checks the
  GTK widget itself rather than its wrapper. Also: `_survivors` collects until nothing more is
  freed, since one collection's disposals free more on the next.
- Rejected: a generic teardown with `GObject.signal_handlers_destroy` over a discarded subtree. It
  also destroys GTK's own internal handlers, which GTK later disconnects by id (fatal critical in
  `gtk_widget_unparent`).
- Beyond the ticket: an Apply soak (in-place panel rebuild while the editor stays open) and the
  kind-switch soak also cover subtrees replaced inside an open editor.
