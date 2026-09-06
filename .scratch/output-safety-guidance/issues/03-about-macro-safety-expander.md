# 03 — "About macro safety" expander

Status: done — 2026-09-07

**What to build:** Directly under the §2 disclaimer line in the Macro editor, a
collapsed-by-default expander labelled `About macro safety`. Expanding it reveals two
themed blocks of compact best-practice tips — "Staying plausible to a game" and "Not
locking up your own system" — followed by a link row to the README's full guide. Collapsed
or expanded, flipping to the Stepper tab shifts nothing.

**Blocked by:** 01 (the README `#output-safety` anchor the link row targets), 02 (the
disclaimer line this expander sits directly beneath).

Source: `.scratch/humane-output-rate/spec-user-facing-output-safety-guidance.md` §3.

- [x] A `Gtk.Expander`, collapsed by default, labelled `About macro safety`, directly
      under the §2 disclaimer line in the Macro editor column.
- [x] Body renders each of the two themes as a bold sub-label followed by a bulleted
      dim `Gtk.Label` list; content is exactly the two blocks in spec §3 ("Staying
      plausible to a game" — 5 bullets; "Not locking up your own system" — 4 bullets
      including the "To stop a runaway" line).
- [x] Below the two themes, a link row: a `Gtk.LinkButton` labelled
      `Full guide: Output safety` pointing at the README's `#output-safety` anchor, with
      the `Gtk.Label` fallback (`Full guide: the "Output safety" section of the README`)
      when no stable public README URL is known at build time.
- [x] Flipping between the Macro and Stepper library tabs stays visually stable — the
      implementer either mirrors the height with an inert reserve on the Stepper side or
      places the expander below the vexpanding step list; the requirement is only that
      nothing above shifts.

## Comments

**Implemented 2026-09-07.** `library_view.py`: `_MACRO_SAFETY_EXPANDER_LABEL` +
`_MACRO_SAFETY_THEMES` (spec §3 verbatim, markdown emphasis flattened for the
label render — 5 + 4 bullets), `_macro_safety_expander()` (a collapsed
`Gtk.Expander`; each theme a `["heading"]` bold sub-label over a dim wrapped
bulleted `Gtk.Label` list; `_full_guide_link_row()` last), and a `col3` prepend
directly after the §2 disclaimer in `build_editor_columns`. The link row is a
real `Gtk.LinkButton` → `{about_dialog.REPO_URL}#output-safety`
(`_OUTPUT_SAFETY_README_URL`); set that constant to `None` and it falls back to
the `Gtk.Label`. Tab-flip lockstep reuses the ticket-02 pattern: the Stepper tab
builds the same expander and `_make_inert_reserve()`s it (opacity 0, insensitive,
non-focusable, a11y-hidden) — extracted from the disclaimer's inline
opacity/HIDDEN pair so both share it. A collapsed `Gtk.Expander` reserves a fixed
height regardless of child, and every rebuild (every tab flip) starts it
collapsed, so nothing shifts. Gated on the existing `LibraryKind.shows_disclaimer`
field — no new adapter field. Tests: 6 in `test_library_view.py`, 1 guard
extended in `test_binding_editor.py` (no `Gtk.Expander` leaks into the assignment
editor). Full GUI suite green (502 passed).
