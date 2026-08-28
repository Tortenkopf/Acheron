Type: task

Blocked by: 87

Status: open

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
