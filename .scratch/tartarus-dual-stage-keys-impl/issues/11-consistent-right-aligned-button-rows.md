# 11 — Consistent right-aligned button rows across the three editors

**What to build:** The three binding editors present their action buttons the
same way — right-aligned, with the primary commit action in the bottom-right
corner.

- Grid-key editor: `[Clear Binding] [Apply] [Save]`, right-aligned, Save in
  the corner.
- Non-grid plain editor (Mode key, thumbstick, wheel): `[Clear Binding]
  [Save]`, right-aligned.
- Chord binding dialog: `[Cancel] [Save Chord]`, right-aligned.

Cosmetic only — button order and alignment change, nothing about what the
buttons do. No behaviour change to non-grid Inputs or Chords.

**Blocked by:** 10

**Status:** resolved

- [x] Grid-key editor row is `[Clear Binding] [Apply] [Save]`, right-aligned,
      Save rightmost.
- [x] Non-grid plain editor row is `[Clear Binding] [Save]`, right-aligned.
- [x] Chord dialog row is `[Cancel] [Save Chord]`, right-aligned.
- [x] The button row stays outside the grid-key panel's scroll container
      (always visible regardless of panel height).
- [x] Existing GUI tests for all three editors updated for the new row order;
      no behavioural assertions change.
- [x] Screenshot tools (`shot_binding_editor.py` and any Chord-dialog shot)
      still produce representative output.

## Answer

Cosmetic-only pass, all in `gui/acheron_gui/binding_editor.py`:

- **Grid-key dual-stage editor** (`build_binding_editor`'s grid branch): the
  `btn_row` is now `Gtk.Box(spacing=8, halign=Gtk.Align.END)` with children
  appended `clear_btn`, `apply_btn`, `save_btn` (was `save`/`apply`/`clear`,
  left-aligned). It was already a sibling of `panel_scroller` in the outer
  `box`, not a child of it, so it already stayed outside the scroll container —
  unchanged, now covered by a test.
- **Non-grid / version-skew / Axis-assigned plain editor**
  (`build_action_and_trigger_fields` caller): `btn_row` gets
  `halign=Gtk.Align.END`; `save_btn` is appended after `clear_btn` instead of
  before.
- **Chord dialog** (`build_chord_binding_dialog`): `btn_row` gets
  `halign=Gtk.Align.END`; order is now `cancel_btn`, `save_btn` (was
  `save`/`cancel`).

Tests: `gui/tests/test_binding_editor.py` gains `_button_row` /
`_row_button_labels` / `_has_ancestor_of_type` helpers and three cases
asserting each editor's row order + `halign` + (grid) that the row has no
`Gtk.ScrolledWindow` ancestor. No existing test asserted button order (all
use label-based lookup), so none changed. Full GUI suite 478 pass.
`gui/tools/shot_binding_editor.py` re-run: all six PNGs still produced, the
three rows visibly right-aligned.

## Comments

**Follow-up (830b5b1):** while reviewing the shots Charon noticed the Key
picker always highlighted `A` green on an unbound key, even where that key's
passthrough default is something else. Fixed separately: new
`inputs.default_key_code_for()` / `INPUT_DEFAULT_KEY_CODE` (mirroring
`daemon/src/input.rs::key_code_for_input`); the synthetic primary stage, the
plain non-grid editor's fresh binding, and the non-keypress→Keypress draft
fallback now seed the Key field from the Input's own default (`grid_r1c1` →
`KEY_1`, Mode key → Left Alt, thumbstick Up → `KEY_UP`, wheel middle →
`BTN_MIDDLE`; scroll directions + a Chord's Binding keep `KEY_A`). Saving an
unbound key with no edits now commits that default. GUI suite 481 pass.
