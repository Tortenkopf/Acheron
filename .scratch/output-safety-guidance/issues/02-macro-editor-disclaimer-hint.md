# 02 — Macro-editor standing disclaimer hint

Status: done — 2026-09-07

**What to build:** When the user opens the Macro editor, a single always-visible advisory
line sits at the top of the editor column: `uinput` output is always identifiable as
synthetic, Acheron paces its own Trigger modes, but a Macro does exactly what you write.
The line is not shown on the Stepper tab, and flipping between the Macro and Stepper tabs
does not shift anything.

**Blocked by:** None — can start immediately.

Source: `.scratch/humane-output-rate/spec-user-facing-output-safety-guidance.md` §2.
This is a **GUI hint** per `CONTEXT.md` → Interface — persistent, condition-bound,
dim, non-blocking, no dismiss.

- [x] An always-visible line appears at the top of the Macro editor's column, above the
      §3 expander slot and above the existing "Changes save automatically." hint.
- [x] Widget matches the `_CONTROLLER_MACRO_HINT` style: `Gtk.Label`, `xalign=0`,
      `wrap=True`, `css_classes=["dim"]`. Renders as one wrapped line, no hard breaks.
- [x] Copy is exactly the three sentences from spec §2, leading with the `⚠️` emoji.
- [x] Not shown on the Stepper tab; not added to `binding_editor.py` (belongs where a
      Macro is written, not where it is assigned).
- [x] Flipping between the Macro and Stepper library tabs stays visually stable — nothing
      above the step list shifts (Stepper side gets a matching inert reserve if needed,
      per the identical-measurements structure in `library_view.py`).

## Comments

**Implemented 2026-09-07.** `library_view.py`: `_MACRO_DISCLAIMER` +
`_macro_disclaimer_label()` (dim wrapped `Gtk.Label`, same style as
`_CONTROLLER_MACRO_HINT`), a `shows_disclaimer` field on `LibraryKind`
(`MACRO=True` / `STEPPER=False`), and a prepend at the top of column 3 in
`build_editor_columns`. The Stepper tab gets the identical label at
`opacity=0` (`set_can_focus(False)`) as the ticket-91 lockstep reserve —
a matching widget rather than a hardcoded height, since the wrapped line's
rendered height depends on column width and theme. Tests: 4 in
`test_library_view.py`, 1 guard in `test_binding_editor.py`. Full GUI suite
green (488 passed).
