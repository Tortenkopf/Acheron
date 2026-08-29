Type: task
Status: resolved

## Question

Make the Stepper and Macro library editors visually interchangeable, so nothing "jumps
around" when the user flips between the two tabs (user's `/wayfinder` Round 1, Q5 + Q6 +
list-item #9). GUI-only — `gui/acheron_gui/library_view.py` (+ possibly `gtk_utils.py` /
`app.py` window size). Build and **live-verify with screenshots in the same session** against
the running GUI — "jumps around" is a visual-fidelity judgement that has to be seen, not a
question answerable from the source.

Ticket 70 already put both editors on the same three-column shape (pinned sidebar / name+list
column / hint-error-add-controls column) with parallel structure. What remains is that shared
elements are not built to *identical* measurements, so switching tabs shifts things by a few
pixels.

### 1. Unify the measurements (Q5)

The Stepper editor is the reference — **adjust the Macro editor to match it**, not the other
way round. Both keep every function they have today; only sizing/spacing/padding of the shared
GUI elements changes. Go through the two editors side by side and make identical:

- Column widths (columns 2 and 3), and the pinned-sidebar width (already shared via
  `build_pinned_sidebar_box` — confirm it actually renders equal in both destinations).
- The name row, the browse list, and the steps/items list: row height, spacing, internal
  padding, scroll-viewport height (`_vscrollable` / `set_vexpand` behavior from ticket 70).
- The pinned column-3 header (Macro: `[error_label, add_btn]`; Stepper:
  `[toast, error_label, add_btn, assignment_row, separator]`) — the Stepper header is taller
  because of the assignment row and toast. Make the Macro header reserve the **same vertical
  space** (or otherwise ensure the "+ Add step" button and the picker below it sit at the same
  y-position as their Stepper counterparts) so the editor body doesn't shift when switching.
- The "New step" / "New item" area: the kind dropdown row (Macro only), the key-picker mount,
  and the modifier-checkbox row (see #3) must leave the picker itself at the same position in
  both.
- Button labels/sizes ("+ Add step" vs "+ Add item" — keep the wording, match the size).

Prefer extracting shared spacing/size constants (or a shared builder) in `library_view.py` /
`gtk_utils.py` over hand-tuning each call site, so they can't drift again.

### 2. "Value" / "New item" → "Key" (Q6)

In `library_view.py`:

- Stepper item editor (`:810`): `labeled_row("New item", value_picker)` → `labeled_row("Key", ...)`.
- Macro step editor, key branch (`:398`): `labeled_row("Value", value_picker)` → `labeled_row("Key", ...)`.
- Macro step editor, delay branch (`:387`): `labeled_row("Value", ms_entry)` →
  `labeled_row("Delay (ms)", ms_entry)` (the field genuinely isn't a key here — the step-kind
  dropdown selects KeyDown / KeyUp / Delay).

Matches the grid-view key-picker's own "Key" label. Update affected tests.

### 3. Stepper modifier checkboxes above the fold (#9)

Today the Stepper "New item" editor puts the Ctrl/Shift/Alt/Super `mod_box` (`:815`–`:832`,
ticket 62/63) below the key picker, and it currently requires scrolling down "a tiny bit" to
see it. Fix by whichever of these the live view shows is cleanest:

- Reclaim vertical space elsewhere in the column (the #1 homogenization pass may already do
  this), and/or
- Reorder so the checkboxes sit directly under the "Key" row, above the picker grid, and/or
- Bump `app.py`'s `win.set_default_size(1400, 860)` height so the default window shows the
  whole editor without scrolling.

The column-3 body is already wrapped in a `Gtk.ScrolledWindow` (ticket 70's `_vscrollable`),
so nothing is *unreachable* — this is purely about the default view not hiding a control the
user needs on first glance. Note that ticket 92/93 (controller-button picker switcher) will
add another row to this same area — leave the layout with a little headroom, and flag in the
answer whether a further window-size bump will be needed once that lands.

### Verification (fold into this session)

- Side-by-side screenshots of both editors with the same list selected; flip the tab and
  confirm no shared element moves.
- The three relabelled fields render correctly, and a Macro delay step still round-trips.
- The Stepper modifier checkboxes are visible without scrolling at the default window size.
- Full GUI suite green.

## Answer

Done, GUI-only (`gui/acheron_gui/library_view.py` + `gui/tests/test_library_view.py`), no
`gtk_utils.py` / `app.py` change needed — the default window size (`1400×860`, ticket 88)
was left alone. Built and live-verified against the running GUI this session via a
self-screenshot harness (see the tooling note at the end).

### What changed

**1. Unified measurements (Q5).** The two editor-column builders
(`build_macro_editor_columns` / `build_stepper_editor_columns`) are now structured in
lockstep, via shared helpers and one spacing constant (`_EDITOR_COL_SPACING = 6`) rather
than hand-tuned call sites. **No hardcoded pixel dimensions** — every "reserve the same
space" is done by building the same widget stack, or by measuring a real dropdown row's
height on the current theme (`_dropdown_row_height()`), never a magic number (a shared
`Gtk.SizeGroup` can't span the two editors since only one is realized at a time):

- `_build_editor_col2(name, scroller)` — column 2 is now built identically for both: a
  name heading (**newly `ellipsize=END`** so a long Macro/Stepper name can't grow the
  column and shift the split) above the shared `_vexpanding_list_scroller` (unchanged
  `vexpand`, `hscrollbar=NEVER`).
- Column 3 is now the same skeleton on both tabs: `error_label` → `+ Add step`/`+ Add
  item` → a middle sized to two dropdown rows (Stepper: the real Forward/Backward
  `build_stepper_assignment_row`; Macro: `_header_middle_reserve()`, a blank `Box` sized
  to `2 * _dropdown_row_height() + _EDITOR_COL_SPACING`) → `Gtk.Separator()` →
  `_vscrollable` body. The Macro editor previously had neither the middle nor the
  separator, so its body started ~87px higher — now "Changes save automatically." lands
  at the **same y on both tabs** (measured y=164/164).
- `_mount_editor_columns(root, col2, col3)` — column 3 is now `hexpand=False` (was
  `True`), so it holds its natural width, which is **identical on both tabs** (driven by
  the shared inline key picker). Column 2 (`hexpand=True`) absorbs all the width variation
  that used to come from the step/item text differing between tabs. Column 3's left edge
  is now pinned at the same x on both tabs (measured x=729/729) — the picker and keyboard
  grid do not move at all when flipping tabs.
- Result: `+ Add step`/`+ Add item` (y=42/42), the name heading (y=37/37), the hint
  (y=164/164) and the `Selected: …` picker summary + keyboard grid (y=228/228) are all
  pixel-aligned across the two tabs. The only things that visibly differ are the
  legitimately-different controls — the step list vs. item list, the step-kind dropdown
  vs. the modifier checkboxes, and the reserved blank vs. the Forward/Backward row.

**2. "Value" / "New item" → "Key" (Q6).**

- Stepper item editor: `labeled_row("New item", value_picker)` → `labeled_row("Key", …)`.
- Macro step editor, key branch: `labeled_row("Value", value_picker)` → `labeled_row("Key", …)`.
- Macro step editor, delay branch: `labeled_row("Value", ms_entry)` →
  `labeled_row("Delay (ms)", ms_entry)`. Live-verified the Delay row still renders and a
  delay step still round-trips (`test_adding_a_step_calls_set_macro_steps_and_appends`,
  updated).

**3. Stepper modifier checkboxes above the fold (#9).** The Ctrl/Shift/Alt/Super `mod_box`
now renders **above** the "Key" picker row instead of below it, as `labeled_row("Modifiers",
mod_box)`. The key picker owns a tall on-screen keyboard grid; below it the checkboxes sat
at the very bottom edge of the default window (the reported "scroll down a tiny bit").
Above the grid they're visible on first glance with no scrolling — verified live at
`1400×860`. This also does double duty for #1: it's the one `labeled_row` between the hint
and the picker row, structurally the same as the Macro editor's step-kind `labeled_row`;
`mod_box` gets `size_request(-1, _dropdown_row_height())` + `valign=CENTER` so the checkbox
row (shorter than a dropdown row) is floored at the dropdown-row height and the picker
lands at the same y on both tabs (measured y=228/228).

### Findings / notes for later tickets

- **Ticket 92/93 headroom:** column 3's body scrolls (`_vscrollable`), so ticket 92/93's
  key↔controller switcher row can be added to the "New step"/"New item" area without a
  window-size bump — but it should go in the same one-row slot as the step-kind dropdown /
  `mod_box` (keeping the "exactly one ~one-row control before the picker" structure), and
  both editors should get it so the lockstep holds. If 92 instead makes "controller
  button" a third step-kind for Macro (its Q3 option), only the Stepper editor gains a
  real switcher and the Macro side stays a plain dropdown — that still fits.
- **Pinned-sidebar width (Q5 "confirm it renders equal"):** `build_pinned_sidebar_box`'s
  `set_size_request(220, -1)` is a floor, not a clamp — a long Macro/Stepper name in the
  column-1 browse list (`build_macro_row`/`build_stepper_row`'s `Gtk.Button(label=name,
  hexpand=True)`) can widen column 1 by ~30px, which shifts column 2 with it. With
  realistic names it's ~2px (247 vs 249, imperceptible), and column 3 (the editor + picker,
  the thing this ticket is about) is pinned regardless. Left as-is: a real clamp needs
  either `Gtk.Button.set_can_shrink` (GTK 4.12+, a version floor this project hasn't
  taken) or replacing the row buttons' label with an ellipsizing `Gtk.Label` child (breaks
  `get_label()` and ~15 tests). Flagging as a possible small follow-up if the ~2px bothers
  the user in practice — it also affects the Grid↔Library Profile-sidebar comparison
  (ticket 69/70's turf), not just Steppers↔Macros.
- **Toast row:** the Stepper editor's one-shot `stepper_toast` label (shown once after a
  cross-list steal, gone on the next render) still adds a transient row the Macro editor
  has no equivalent for. Not reserved — it's ephemeral and reserving permanent space for a
  near-never-shown element wastes it; the ticket's "reserve the same space" was about the
  always-present assignment row, which is handled.

### Tooling

`gui/tools/shot_library.py` (new, committed) — a reusable screenshot harness: it drives
the real `AcheronApplication` against a `DaemonStub`, clicks through to each Library tab,
and screenshots the running window **from inside its own process** (the toplevel's GSK
renderer → `Gdk.Texture` → PNG). No external screenshot binary, xdg portal, or compositor
cooperation — it works headlessly under the session's own Wayland/X display. This is the
same technique the prototype tickets used; earlier "no screenshot tooling here" claims
(tickets 61, 89's spawn of 95, etc.) were mistaken. Tickets 92-95's own screenshot
verification can reuse or adapt it.

### `/code-review`

Ran (low effort, GUI diff only). Two PLAUSIBLE findings, both the same point: the first
pass used hardcoded pixel constants (`_HEADER_MIDDLE_H = 74`, `mod_box.set_size_request(-1,
34)`) for cross-tab alignment, which are theme/font/GTK-version fragile. **Addressed** by
replacing both with `_dropdown_row_height()` — a value measured from a throwaway
`labeled_row(Gtk.DropDown)` probe at build time, so it tracks whatever height a real
dropdown row renders at on the running theme. No magic dimensions remain.

### Tests

`5` new tests (`Key`/`Delay (ms)` labels on both editors, `mod_box` renders above the
"Key" row, column 3 mounted `hexpand=False`), `4` updated for the relabels. Full GUI
suite **300 passed** (was 295). No Rust changes.

Status: resolved
