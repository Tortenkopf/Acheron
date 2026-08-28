Type: prototype

Status: resolved

## Question

Revisit [ticket 06](./06-gui-polish-grid-sizing-default-labels-mode-key-width.md)'s "floor, not cap" decision for Device Overview's buttons (`make_input_button` in `gui/acheron_gui/device_overview.py`): the user wants every button to render at a genuinely fixed height and width regardless of its current label content, rather than growing past its `set_size_request` floor (76×99 for grid buttons, 52×40 for the Mode key/diamond lobes/key-20 paddle) when text doesn't fit — which is what currently produces tall-and-narrow buttons once the window is narrower than the grid's natural width and GTK squeezes columns down toward the floor.

Ticket 06 tried a hard width cap (`max-width-chars`) once already and rejected it live because it mid-word-split ordinary short labels ("passthr"/"ough"). A real fixed size reopens that exact problem, so the core design question is: **what happens to label content that doesn't fit inside a fixed box?** Candidates to react to, live, via the `/prototype` skill:

- Truncate with an ellipsis, full text available via tooltip/hover.
- Shrink the font size to fit (dynamic/auto-scaling label).
- Shorten the label content itself (e.g. abbreviate modifier names, drop the Trigger-mode tag from the button and surface it another way).
- Some combination (e.g. normal case wraps within the fixed box at a smaller font; only the genuinely pathological case truncates).

Also settle scope: does "every button" mean truly uniform dimensions across all button kinds on the screen (grid buttons, Mode key, diamond lobes, key-20 paddle — today four different sizes), or just that each kind stops growing past its own current floor? The user's framing ("always the same size... regardless of what text is currently on it") suggests the latter (no growth) is the actual ask, not visual uniformity across kinds — confirm live rather than assuming.

Use the `/prototype` skill against real production widgets and realistic Binding content (mirroring ticket 06's own worst-case strings — multi-modifier chords, Trigger-mode tags), the same discipline ticket 47 used.

## Answer

Prototyped on `prototype/87-fixed-size-device-overview-buttons` (real content — `input_label`/`action_summary`/`INPUT_DEFAULT_LABEL`, the real `.bound`/`.empty`/`.mode-key` CSS — driven by the real `DaemonStub` seeded with ticket 06's own worst-case strings, since `make_input_button`'s current "floor, not cap" sizing is exactly what's being replaced so it wasn't reused directly). Three variants, round 1: **A won outright** — the user's own words, "I like A best," no runner-up.

- **A — Tight ellipsis + tooltip.** Both dimensions are a genuine cap for the first time (`Gtk.Label.set_max_width_chars()` + `set_lines()` + `set_ellipsize(Pango.EllipsizeMode.END)` together, applied *after* ordinary word/char wrap rather than instead of it) — the missing half of ticket 06's own rejected `max-width-chars` attempt, which paired a width cap with wrapping alone and mid-word-split "passthrough" for lack of a line limit + ellipsis fallback. Cap tuned snug on purpose (`max_width_chars` 8 for 100px-wide buttons, 14 for key 20's 150px), so ordinary content sometimes truncates too, not just the pathological cases — every button always carries a tooltip with the full untruncated text (both label lines, newline replaced with two spaces) regardless of whether it actually truncated, cheaper than tracking truncation state per button.

Two refinements from a second round of live reaction, both settled and folded into variant A (not built out as separate branches — small enough to react to directly):

1. **Bold Input label.** The button's own label — its grid number, "Mode", or an arrow glyph, i.e. *which* Input this is — renders bold (`<b>…</b>` via `Gtk.Label.set_markup()`, `GLib.markup_escape_text()` on both the label and the binding-summary line since the summary is user-influenced content that could contain markup-special characters); the binding summary below it — *what it does* — stays regular weight. Distinguishes the two pieces of information the plain-text version conflated.
2. **Mode key sized like every other button.** Not mentioned when the user first enumerated what should grow, so this ticket's own prototype flagged it live with a dashed outline rather than silently deciding — the user's answer: "the mode key should keep it's current roundish appearance, but otherwise behave like the other buttons." Settled as: same 100×100 footprint as the grid/wheel/diamond buttons (not the 52×40 it has today), keeping its existing `.mode-key` CSS class (`border-radius: 999px`) unchanged. Since that CSS only ever renders a true circle when width equals height, giving it a square footprint is what actually produces a real circle — closing, as a side effect, the exact CSS finding ticket 06 flagged but explicitly left unfixed ("a stadium/oval, not a circle").

**Settled sizing** (all as a genuine cap, not today's floor):
- Grid buttons (all 16 numbered grid keys), the three wheel buttons, the four thumbstick-diamond lobes, and the Mode key: **100×100**.
- Key 20 (`grid_r4c5`, the paddle below the diamond — physically wider on the hardware): **150×100**.
- Relative positions of every button to each other are unchanged from today's real `build_main_view` assembly — only the box sizes and the tuned wrap/ellipsize/tooltip behavior are new. The real window's current `win.set_default_size(920, 680)` (`app.py`) will need to grow to comfortably fit the larger device row without the initial window already needing to shrink buttons below their new fixed size — exact numbers to be measured live during the build rather than guessed here.

Prototype: `prototype/87-fixed-size-device-overview-buttons` (`gui/acheron_gui/prototype_87_fixed_size_device_overview_buttons.py`, `gui/prototype_87_fixed_size_device_overview_buttons.py`). Variants B (auto-shrink font) and C (wrap-then-ellipsize hybrid) are kept on the branch as rejected reference points.

None of this is wired into the real `device_overview.py`/`app.py` yet. Spawned [Build the fixed-size Device Overview buttons for real](./88-task-build-fixed-size-device-overview-buttons.md).
