Type: task

Blocked by: 87

Status: resolved

## Question

Build [ticket 87's settled fixed-size-button design](./87-prototype-fixed-size-device-overview-buttons.md) (variant A, plus the two live-refinements) for real in `device_overview.py`/`app.py`, against the real Daemon and Tartarus Pro — matching this map's "resolving a ticket means actually building and testing against the real, connected hardware" discipline.

Scope:

- **`make_input_button`** (`device_overview.py`): replace today's `btn.set_size_request(w, h)` "floor, not cap" sizing with a genuine cap on both dimensions, following the prototype's mechanism — `Gtk.Label.set_wrap(True)` + `set_wrap_mode(Pango.WrapMode.WORD_CHAR)` (unchanged) plus `set_max_width_chars()` + `set_lines(3)` + `set_ellipsize(Pango.EllipsizeMode.END)` (new), tuned to the prototype's snug values (`max_width_chars` 8 for 100px-wide buttons, 14 for key 20's 150px — re-measure live against the real font/theme rather than assuming the prototype's GTK defaults carry over exactly). Every button gets a tooltip set unconditionally to its full untruncated two-line text (label + binding summary, joined with two spaces) — not conditional on whether it actually truncated.
- **Bold Input label.** The label's first line (`input_label(inp)` — grid number/"Mode"/arrow glyph) renders bold; the second line (`action_summary(...)`) stays regular weight. Switch the label from plain `label=` construction to `Gtk.Label()` + `set_markup()`, with `GLib.markup_escape_text()` applied to *both* lines before building the markup string (the binding-summary line can contain user-influenced content — Chord member text, Macro/Stepper names — so it isn't safe to interpolate unescaped).
- **New sizes**, all as a genuine cap now, not a floor:
  - Grid buttons (all 16 numbered grid keys), the three wheel buttons (`wheel_scroll_up`/`wheel_middle`/`wheel_scroll_down`), the four thumbstick-diamond lobes, and the Mode key: **100×100** (`input_btn`'s default `w`/`h`, and the diamond/Mode-key call sites that currently pass `52, 40` explicitly).
  - Key 20 (`grid_input(4, 5)`, the paddle below the diamond): **150×100** (currently also `52, 40`).
- **Mode key**: change its size to 100×100 alongside the rest (no longer a special case) — keep its existing `.mode-key` CSS class (`border-radius: 999px`) and `add_css_class` call exactly as today; the square footprint alone is what turns the existing oval into a true circle, per ticket 87's Answer. No CSS changes needed.
- **Positions unchanged**: this is a sizing-only change to `make_input_button`'s call sites in `build_main_view` — don't restructure the grid/diamond/stick-column layout itself, only the `w`/`h` arguments passed in and the two call sites (`mode_btn`, `grid_input(4, 5)`) that currently hardcode `52, 40`.
- **Window default size** (`app.py`, `win.set_default_size(920, 680)`): grow enough that the larger device row (100×100 grid cells vs. today's 76×99, plus the 100×100 diamond/Mode-key/wheel vs. today's 52×40, plus key 20's 150×100 vs. today's 52×40) fits comfortably at first launch without GTK needing to shrink any button below its new fixed floor. Measure live against the real running window rather than computing from box-model arithmetic — margins/spacing/the Profile sidebar's pinned 220px all factor in and are easiest to get right by looking at the real thing.
- **Confirm nothing regresses**: every existing Device Overview interaction (opening the per-key Binding editor, Chord-selection click-override, the axis-assignment stripe, the insensitive/disabled Mode-key state, Profile/Layer switching) still works unchanged — this ticket only touches sizing and label markup, not click handling or state.

Live-hardware verification in scope for this ticket: visually confirm, against the real running GUI, that (1) every button (including a hand-edited pathological Binding, e.g. a 4-modifier Chord) stays at its fixed size with no growth, (2) the tooltip surfaces the full text on hover, (3) the Mode key renders as a true circle, (4) the bold/regular label split reads correctly at the real theme's default font, and (5) the initial window comfortably fits everything with no button undersized.

Once this lands, close the map's fixed-size-button strand (decide → build, tickets 87-88) — no separate verify-on-hardware ticket is needed unless this ticket's own live check surfaces something that needs a second pass (this map's Notes already fold live-hardware verification into every build ticket rather than always spawning a dedicated one, per e.g. ticket 06's own direct-build precedent).

## Answer

Built into `device_overview.py` and `app.py`, live-verified against the real Daemon +
Tartarus Pro (daemon started for the session, GUI launched, screenshotted, daemon stopped
and the user's profiles restored afterward — a throwaway `zz-ticket88-scratch` Profile
held the pathological test bindings and was deleted). 294 GUI tests green (3 new in
`test_device_overview.py`); no daemon changes.

**`make_input_button` (`device_overview.py`):**
- Label switched from plain `label=` to `Gtk.Label()` + `set_markup()`: first line
  (`input_label(inp)`) bold, second line (`action_summary(...)`) regular.
  `GLib.markup_escape_text()` on *both* lines.
- Genuine size cap: `set_lines(3)` + `set_ellipsize(Pango.EllipsizeMode.END)` applied
  after the existing `set_wrap(True)` + `WORD_CHAR`, plus **`set_width_chars(chars)` ==
  `set_max_width_chars(chars)`** where `chars = 8 if w <= 100 else 14`. The
  `width_chars == max_width_chars` pinning is a deviation from the prototype's
  `max_width_chars`-only sketch: the prototype's combination produced an intermittent
  GTK `natural size must be >= min size` warning live (the `wrap` + `ellipsize` pair
  reporting a natural width below the wrap-minimum during transient allocation). Pinning
  the width request to a fixed value removed it entirely and is itself the predictability
  the ticket wants; the button's own `w`/`h` still bounds it.
- Tooltip: `btn.set_tooltip_text(f"{label_line}  {summary_line}")` set unconditionally
  (newline flattened to two spaces), *then* overridden by the existing
  `insensitive_reason` / `chord_tooltip` branches when either applies — a more specific
  tooltip still wins. (The prototype set the full-text tooltip on the inner label; moved
  to the button so it doesn't mask the disabled-Mode-key reason / Chord-membership
  tooltips, which the prototype had no equivalent of.)

**Sizes (all a genuine cap now):** grid (16) + wheel (3) + thumbstick-diamond lobes (4) +
Mode key → 100×100 (`make_input_button`/`input_btn` defaults changed 76×99 → 100×100, and
the `mode_btn` / diamond call sites changed from explicit `52, 40`). Key 20
(`grid_input(4, 5)`) → 150×100. Mode key keeps its `.mode-key` class untouched — the
square footprint alone turns the existing oval into a true circle (confirmed live). No CSS
changes. Positions unchanged — only `w`/`h` args and the two hardcoded call sites touched.

**Window default (`app.py`):** `920×680` → `1400×860`, measured against the real running
window — fits the 100×100 device row + 220px Profile sidebar + 220px Chords section with
margin, no button shrunk below its fixed size.

**Live checklist — all five confirmed:**
1. Fixed size holds — a hand-seeded `Ctrl+Shift+Alt+Super+KBDILLUMTOGGLE [hold]` stayed
   exactly 100×100, wrapped-then-ellipsized (`per+KB…`), no button/column growth.
2. Tooltip surfaces the full untruncated text on hover (set unconditionally, not gated on
   whether the label truncated).
3. Mode key renders as a true circle.
4. Bold Input line / regular summary line reads correctly at the real Yaru theme font.
5. Initial 1400×860 window fits everything with no button undersized.

**Observations, not defects (no second pass needed):**
- The GTK size-warning found live was fixed in-session (the `width_chars` pinning above).
- With 100×100 lobes and an empty 100px centre cell, the thumbstick diamond reads as a
  loose plus rather than the tight diamond the 52×40 version gave — an accepted
  consequence of the ticket's "sizing-only, don't restructure the layout" constraint, not
  fixed here.
- A genuinely pathological binding shows the bold line + 3 wrapped/ellipsized summary
  lines (4 total; `set_lines(3)` limits the summary paragraph, the explicit newline adds
  the label line) — matches the prototype's own `set_lines(3)` behavior the user
  approved in ticket 87.

Char caps kept at the prototype's snug 8/14. The fixed-size-button strand (tickets 87–88)
is closed — no separate verify-on-hardware ticket, per this ticket's own instruction and
the map's Notes.
