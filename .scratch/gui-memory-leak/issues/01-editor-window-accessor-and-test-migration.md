# 01: Expose an editor's open window through one accessor and move tests onto it

Spec: [`../spec.md`](../spec.md). Prefactor for tickets 02 and 03.

**What to build:** A Device Overview Input button and a Lighting colour-picker button each expose
"the editor window it currently has open" through one accessor. While windows are still pre-built,
the accessor returns that window. Every test that reads the pre-built `binding_editor_window` /
`colour_picker_window` attribute (8 call sites, mostly through the `widget_tree` helpers) now
activates the button and then reads the accessor. That is the path tickets 02 and 03 keep once
windows are built lazily. Nothing the user sees changes.

**Blocked by:** None (can start immediately).

**Status:** done (2026-10-15)

- [x] Input buttons and colour-picker buttons expose one accessor for their currently open
      window. It returns `None` when nothing is open, once lazy construction lands.
- [x] No test reads `binding_editor_window` or `colour_picker_window` directly. Each one clicks
      the button first and then uses the accessor, including the close-request/commit test and
      the helper that finds a button by its editor window's title.
- [x] Clicking a button in a headless test doesn't require a real mapped window (whatever
      `present()` does without a compositor is tolerated, or isolated behind the accessor).
- [x] The full GUI test suite passes with no assertion changes beyond how the window is reached.

## Comments

**2026-10-15, implemented.** The accessor is `device_overview.open_editor_window(btn)`, shared by Input
buttons and Lighting colour buttons. The click handler sets the private `btn._open_editor_window`
just before `present()`, so it returns `None` until the first click. Tests reach windows through
`widget_tree.open_window(btn)`, which clicks only when nothing is open yet. A conftest autouse
fixture makes `Gtk.Window.present` a no-op, so test clicks never map a real window. 608 GUI tests
pass, and the `gui/tools/shot_*.py` scripts were migrated and smoke-run.

One deviation: `_grid_button` finds a button by the bold Input label on its face, not by clicking
it and reading the editor title. It's also used in "Select Chord members" mode, where a click
records a Chord member instead of opening an editor. Its unused `profile`/`layer` params were
dropped.

**Notes for 02/03:**
- Clear `btn._open_editor_window` when the window is destroyed. Otherwise the button keeps a
  destroyed window alive, which is the same pinning this effort removes.
- Because `present()` is a no-op under test, a click never maps the window. A test for "the
  depth stream starts while an editor is open" has to emit `map`/`unmap` itself.
- `test_binding_editor._dismiss` asserts an editor is open rather than opening one.
