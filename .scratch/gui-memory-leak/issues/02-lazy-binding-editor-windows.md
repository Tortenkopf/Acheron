# 02: Build Binding-editor windows on click and destroy them on close

Spec: [`../spec.md`](../spec.md) — Diagnosis cause 1; Implementation Decisions "Lazy
Binding-editor windows", "Commit-then-close flow preserved", "The ticket-44 rationale moves".

**What to build:** Clicking an Input on Device Overview builds its Binding editor window from the
config snapshot that Input's button was built from, and presents it. The editor looks and behaves
exactly as it does today. Closing the window by any route (Save, the WM close button, Escape)
destroys it instead of hiding it. A Device Overview rebuild while an editor is open destroys that
editor too, so no top-level window can outlive the tree that spawned it. Today about 11,700
widgets are pre-built and pinned in GTK's top-level list on every rebuild; that stops here.

**Blocked by:** 01.

**Status:** done (2026-10-15)

- [x] Building Device Overview creates no top-level windows. `Gtk.Window.get_toplevels()` is 0
      after a rebuild with no editor open.
- [x] Clicking an Input opens an editor with the same title (`profile / layer / input`) and
      content as today. The "Select Chord members" override is still checked first and opens
      no editor.
- [x] Closing an editor destroys it, and the button's accessor returns `None` afterwards.
- [x] Save, or Apply followed by close, still drives exactly one `on_change()`. A pure open/close
      drives none. Save's direct deferred-rebuild call is kept for the unrealized-window case.
- [x] A rebuild while an editor is open leaves 0 top-level windows behind.
- [x] The Actuation & release depth stream still starts when an editor is open and stops when it
      closes.
- [x] The code comment explaining "built once, shown/hidden via present()/close()" (ticket 44)
      is rewritten to explain build-on-open/destroy-on-close.
- [x] Soak test: N rebuilds of the Grid view against `DaemonStub` (the app's own rebuild-closure
      shape, pumping the main context between rebuilds) leave the top-level window count at 0.

## Comments

**2026-10-15, implemented.** `make_input_button`'s click handler builds the window through a new
`_build_binding_editor_window` and presents it. The window isn't hide-on-close any more: Save and
`close-request` both run one `close()`, which destroys the window and clears the accessor before
the deferred `on_change()`. The handler returns `True`, so GTK's own close never runs a second
destroy. A rebuild destroys an open editor through the button's `notify::root`, which fires when
`clear_children` takes the button out of the main window. In that case the editor's own deferred
`on_change()` is skipped, because the rebuild already re-reads the config. Clicking a button whose
editor is already open re-presents that editor.

`gtk_window_destroy` doesn't emit `destroy` while the window's Python wrapper is alive. That's why
the accessor is cleared explicitly in `_destroy_editor_window` and not from a signal handler.

Tests:
- New `gui/tests/test_rebuild_soak.py`. Its `_App` hosts the persistent box in a real
  (never-presented) window, so buttons have a root to leave. It covers 15 Grid rebuilds, 15
  rebuilds each with an editor open, and 15 open/close cycles. Each one asserts that only the
  host window is left in `Gtk.Window.get_toplevels()`.
- A conftest autouse fixture destroys leftover top-level windows after each test.
- A shared `widget_tree.toplevels()` helper.
- Three `test_binding_editor.py` tests used to dismiss the same window twice. Their second
  dismissal now reopens the editor and dismisses the fresh one.

615 GUI tests pass.

Not covered by a test: the depth stream stopping on close. Headless windows are never mapped, so
`destroy()` sends no `unmap`. The map/unmap wiring itself is unchanged. Confirm it live in
ticket 07.

**Notes for 03:**
- The `open_editor_window` docstring flags the Lighting button as still pre-built. Drop that
  note when 03 lands.
- Reuse `_destroy_editor_window`.
