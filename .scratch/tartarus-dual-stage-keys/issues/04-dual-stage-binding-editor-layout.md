Type: prototype
Blocked by: 02, 03
Status: resolved (Charon, 2026-09-04)

## Question

Find the **binding-editor layout** for a grid key that carries a deep stage. Build a
throwaway prototype under `prototype/` (like `tartarus-status-leds` ticket 01's harness,
on the `dev` branch) — a layout to react to, not a step toward the shipped editor.

Invoke `/prototype`.

### Hard constraints (do not violate)

- **Never two key/controller-button pickers on screen at once** (Q12). The picker
  (`key_picker.py` / `controller_picker.py`) is large. The deep stage needs *a* way to
  choose its Action without a second full picker visible simultaneously — a disclosure, a
  tab, a "swap which stage I'm editing" toggle, an inline summary that expands one at a
  time, etc. This is the central layout question.
- The deep-stage controls are **greyed with a "requires analog" note** in Digital Capture
  mode (mirrors how the Actuation section already behaves).
- Deep-stage affordance is **disabled until a primary Binding exists** (Q6).

### Explore

- **The Actuation bar** — one bar with 4 markers (primary green/amber + deep in a second
  colour pair), collision-constrained so `deep.release > primary.actuation` (extend
  `binding_editor.py:298`'s marker-collision logic)? Or two stacked bars? Live Depth fill
  still shown.
- **The staging-mode selector** — 4-way (Handoff / No-Return / Additive / Quick-Skip).
  Radio group, dropdown, segmented control? Where it sits relative to the two binding
  rows and the bar. Each mode needs a one-line explanation surface (tooltip / helper
  text).
- **Disclosure / progressive reveal** — how "Add deep stage" appears and how the panel
  reflows when it is added; how "Remove deep stage" works and warns.
- How the whole thing fits the existing `build_binding_editor` scroller
  (`binding_editor.py:992`, `max_content_height=320`).

### Output

The prototype linked as an asset, plus an `## Answer` recommending one layout with the
key trade-offs, ready for ticket 06 to spec in text.

## Answer

**Variant A — Swap toggle.** Live-reviewed against the real Key/Controller-button
pickers (below), refined over several rounds, and confirmed by Charon: *"This is the
layout I want the current binding editor to be replaced with once the deep bindings
feature lands."*

**Prototype:** [`prototype/04-dual-stage-binding-editor-layout/prototype.py`](../../../prototype/04-dual-stage-binding-editor-layout/prototype.py)
(standalone GTK4, `python3 prototype/04-dual-stage-binding-editor-layout/prototype.py`).
Lives under `prototype/` on `dev`, same precedent as `tartarus-status-leds` ticket 01 —
the release rebuild keeps `prototype/` and `.scratch/` out of `main`, so no separate
throwaway branch is needed. Built all three variants from the ticket's Explore list
(A — swap toggle, B — stacked bars + mutually-exclusive accordion, C — tabs via
`Gtk.Stack`); B and C stay in the file for reference but lost the review.

### The settled layout

Top to bottom, one flat panel (no disclosure, no tabs):

1. **One shared 4-marker Actuation bar** — primary green/amber + deep in a second
   colour pair (blue actuation / purple release), fixed-width (not hexpand — a live
   `get_width()` read during drag made every marker jump the instant one was picked up,
   even with the real `DepthTrack`'s 200ms resync mitigation ported in; a fixed width
   removes the mismatch outright). Width is tuned to the real `_labeled("Key",
   key_picker)` row's own natural width (measured via `Gtk.Widget.measure()`: the real
   key picker is 573px natural, wider than the controller picker's 440px; +100px label
   column +8px spacing = 680px) — the bar lines up flush with the picker row beneath it
   rather than tracking window width live.
   - Marker order left-to-right matches the enforced hysteresis order exactly
     (`p_rel < p_act < d_rel < d_act`, Q4's disjoint stacked bands): primary release,
     primary actuation, deep release, deep actuation.
   - Legend uses real colour swatches (Pango markup spans), same order as the bar, not
     colour-name text.
   - Greys with a "No depth — analog capture unavailable" note in Digital mode (mirrors
     the real Actuation section's existing `apply_mode` treatment) — the deep markers
     grey too since they only exist once a deep stage does.
2. **Primary/Deep row.** A `[Primary — <summary>]` toggle, and in the *same slot* next to
   it: **`+ Add deep stage`** until one exists, replaced by **`[Deep — <summary>]`** once
   it does, with a square **`✕`** button (red, `.destructive-action`) immediately after it
   — "Remove deep stage" moved off the button label into its tooltip. Both toggles share
   one `Gtk.ToggleButton` group (mutually exclusive), so at most one stage is ever
   "selected" for editing.
3. **Staging-mode row** (Handoff / No-Return / Additive / Quick-Skip, one-line tooltip
   each) sits *below* the Primary/Deep row — only rendered once a deep stage exists (it
   has nothing to hand off between until then). Greys with a "Requires analog capture"
   note in Digital mode.
4. **Editor slot** — the real Trigger-mode dropdown, Action-kind dropdown (Keypress /
   Controller Button / Profile Switch), and the **real, unmodified**
   `key_picker.build_inline_key_picker` / `controller_picker.build_inline_controller_picker`
   for whichever stage is currently selected in step 2. Only one stage's fields (and so
   only one picker) are ever mounted in the tree — this is what actually answers Q12's
   hard constraint, structurally, not by hiding a second picker behind CSS.
   - The deep stage's picker highlights its current selection in the **same blue as the
     deep-actuation marker** (`#3498db`) rather than the theme's generic
     `.suggested-action` accent, so which stage's picker is on screen is unambiguous at a
     glance. Needed `background-image: none` alongside `background-color` — the theme
     paints `.suggested-action` with an accent `background-image` layer that otherwise
     sits on top and masks a plain colour override regardless of CSS provider priority.
5. Bind-primary-first gate (Q6): with no primary Binding, the whole panel below the bar
   collapses to one line ("Bind a primary Action first…") — no Add-deep-stage affordance,
   no editor.

### Window behaviour (a prototype-harness concern, not part of the layout itself, but
worth carrying into the real popover's own sizing)

The window is not user-resizable (`Gtk.Window.set_resizable(False)`) and re-hugs its
content's natural size on every rebuild, so it visibly grows/shrinks as a deep stage is
added/removed or Action kind changes. The variant panel sits in its own
`Gtk.ScrolledWindow` (`hscrollbar_policy=NEVER`, `propagate_natural_height=True`,
`max_content_height` = the active monitor's height minus a chrome allowance) — the real
bound-by-screen-edge fallback once content would run taller than the display, matching
the real `build_binding_editor`'s existing `actuation_scroller` pattern
(`binding_editor.py:992`) but sized to the live monitor instead of a fixed 320px.

### For ticket 06 (spec)

- Spec the panel structure in section order 1-5 above, including the exact Add/Remove
  button placement (in the Primary/Deep row, not floating elsewhere) and the staging-mode
  row's position below it.
- Spec the deep-picker blue-highlight treatment as a small, explicit visual rule (ties
  picker selection state to which stage's bar markers it corresponds to).
- The bar's *fixed* width and the window's auto-sizing/scroll-bound behaviour are
  prototype-harness specifics, not literal implementation instructions — the real popover
  already has its own sizing constraints (`build_binding_editor`'s window,
  `device_overview.make_input_button`); carry forward the *intent* (bar width matches the
  picker row beneath it; deep-stage growth stays bounded by screen, falling back to
  scrolling) rather than the exact pixel numbers.
