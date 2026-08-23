Type: task
Status: open

## Question

The Library view's Macro/Stepper editor grows the whole window taller than the screen once a Macro or Stepper has more than a few steps/items — there is no `Gtk.ScrolledWindow` anywhere in this GTK4 GUI (confirmed by grep across `gui/acheron_gui/`), and the main window's `set_default_size(920, 680)` (`app.py:325`) is only an initial hint, not a cap. Four `Gtk.Box` lists in `gui/acheron_gui/library_view.py` grow unbounded and need a scrollable, height-capped container instead:

- `build_macro_editor`'s `steps_list` (a Macro's steps)
- `build_stepper_editor`'s `items_list` (a Stepper's items)
- `build_macros_panel`'s `list_box` (the Macro browse list — same root cause, fewer Macros needed in practice to trigger it, but the same unbounded-`Gtk.Box` bug)
- `build_steppers_panel`'s `list_box` (the Stepper browse list, same as above)

Settled during charting (see [the map](../map.md)'s Decisions so far), so no open design question remains — this is scoped as a direct build, not a grilling/prototype ticket:

- **Minimal fix only** — wrap each of the four lists in a `Gtk.ScrolledWindow` with a sane bounded height, keeping today's two-column layout (browse list | editor) exactly as-is otherwise. No sidebar/tab-switcher restructuring, and no conflict with [ticket 48](./48-task-build-device-overview-nav-rail.md)'s settled "Profile sidebar stays exactly as it is, in both destinations" decision — that decision stands untouched.
- **Both editors in scope** — Macro and Stepper share the identical shape (a growing item list stacked above "add new item" controls), so both get the same treatment in the same session.
- **All four lists in scope**, not just the two step/item lists — the browse lists share the same root cause.
- Pick a reasonable bounded height for each ScrolledWindow by eyeballing it live (mirrors this map's existing precedent for tuning pixel values by hand, e.g. [ticket 06](./06-gui-polish-grid-sizing-default-labels-mode-key-width.md)'s grid-button height) — no specific pixel value was settled during charting.
- Confirm nothing regresses: reorder (↑/↓), remove (×), rename, delete-gating, the "+ New"/"+ Add step"/"+ Add item" controls, and autosave all still work with the list wrapped.

Live-hardware verification is not required for this ticket — nothing here touches the Daemon or physical device; the GUI test suite is the bar, consistent with this map's other GUI-only tickets (e.g. ticket 52/55's own scope notes).

## Comments
