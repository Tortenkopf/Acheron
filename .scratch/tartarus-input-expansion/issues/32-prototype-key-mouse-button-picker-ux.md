Type: prototype

Status: resolved

## Question

Design the look and feel of the key/mouse-button picker GUI, replacing `binding_editor.py`'s free-text `Gtk.Entry` key field, per [Finalize mouse-button/full-keyboard output and design the picker](./02-decide-mouse-button-output-and-picker.md)'s settled scope: mouse buttons Left/Right/Middle/Back/Forward (`BTN_LEFT`/`BTN_RIGHT`/`BTN_MIDDLE`/`BTN_SIDE`/`BTN_EXTRA` — the last two labeled by their observable function, "Back"/"Forward"), plus the entire standard keyboard range (letters/digits, modifiers, function keys F1-F24, navigation cluster, lock keys, misc, multimedia/consumer-control), no exclusions.

Explore two candidate layouts side by side rather than committing to one up front:

- A graphical, representative keyboard-layout picker (visual keycap grid, grouped sections for function/nav/lock/misc/multimedia, plus a mouse-button strip).
- A category-sorted menu/list alternative (e.g. dropdown-of-dropdowns or a searchable categorized list), in case a full keyboard rendering ends up too large for the binding editor's existing popover/space budget.

Settle at least:

- Which layout wins, or whether they combine (e.g. keyboard visual for the common case, collapsed into categories for the long tail).
- Where the picker lives relative to the existing Modifiers checkboxes (`binding_editor.py`'s `render_action_editor`) — replacing the `Gtk.Entry` inline, or opening as a popover/dialog.
- How the "modifier as a main-key target" case surfaces: per ticket 02, modifiers stay selectable but the UI should carry a note that a bare modifier as a Fire-once/Hold-to-repeat main key is a near-instant pulse, and that Toggle (a single `KeyDown`-only Macro step) is the proven pattern for "hold until pressed again."
- Whether the same picker gets reused for Macro step editing (the `KeyDown`/`KeyUp` step values in the Macro step list) and Stepper library items ([ticket 31](./31-prototype-stepper-library-ux.md)'s "Item entry" already expects to reuse whatever this ticket lands on).

Use the `/prototype` skill. Once resolved, spawn the real build ticket, matching the decide → prototype → build pattern used for Chord (01 → 30 → build) and the trigger-point UX (17 → 19 → 26 → 27).

## Answer

**Variant A — Inline Keyboard Panel — won**, live reaction via `python3 gui/prototype_32_key_mouse_button_picker_ux.py`: a collapsed "`<key label>` ▸ Change" button expands a full graphical keyboard grid in place. Chosen outright over Variant B (popover search + category dropdown) for one stated reason: every available key is visible at a glance, with nothing to scroll or search through for a key the user might not have known was bindable (multimedia/consumer-control keys especially) — Variant B's flat filtered list was judged strictly worse for that discovery case, even though it stays narrower. No combination was pursued; A's win was immediate and unqualified.

**Settled specifics, refined over four rounds of live reaction:**
- **Where it lives**: inline, directly in the Binding editor's existing vertical flow, replacing the `Gtk.Entry` — not a popover/dialog. The panel starts collapsed (a compact summary button) and expands on click, so it costs no space until asked for.
- **Key size**: settled at a 5% shrink from the prototype's initial mock (grid unit 30px → 28.5px; a first attempt at 10% read as too small). Font/padding scaled to match (12px font, 25px min button height, 2px 4px padding) — a real build should treat these as starting points to re-tune against the actual `binding_editor.py` popover, not fixed constants.
- **Mouse-button layout**: physical arrangement, not catalog order — **Left, Middle, Right**, a visual gap, then **Back, Forward** (the two thumb buttons), inside the same keyboard-panel grid as its own strip. Labels stay "Mouse Left/Right/Middle/Back/Forward" for the flat/searchable form ticket 02 already settled; the physical strip drops the "Mouse " prefix since context (a distinct strip below the keyboard rows) already disambiguates them from the navigation cluster's arrow keys.
- **Modifier-as-main-key warning**: surfaces correctly, worded per ticket 02's settled finding ("fires a near-instant pulse... use Toggle with a single KeyDown-only Macro step"), driven off one shared `is_modifier` check so it appears consistently under every mounted instance of the picker.
- **Window sizing — two real, general GTK4 findings, not picker-specific**, surfaced only by testing live against the real windowing stack rather than reading the code: (1) `Gtk.ScrolledWindow` does not propagate its child's natural width by default, so a plain `set_default_size(-1, height)` does *not* make the window track its content's real width — `propagate_natural_width(True)` is required (height deliberately left un-propagated, so the window still scrolls vertically instead of growing to fit all content); (2) a `wrap=True` `Gtk.Label`'s *natural* size request is its full unwrapped line width unless `max_width_chars` is set — two long notes (the modifier warning, a footnote about Stepper reuse) were silently setting the window's real width floor the moment fix (1) landed, since the modifier warning renders immediately whenever the current key is a modifier (the prototype's Macro-step mock defaults to `KEY_LEFTCTRL`). Both worth remembering for any future dialog/popover sizing in this GUI, not just this ticket.
- **Reuse**: demonstrated structurally rather than asserted — the exact same picker component (`build_inline_key_picker`) is mounted twice in the prototype host with zero variant-specific code, once as a Binding's "Key" field and once as a Macro step's `KeyDown` value. Stepper library items ([ticket 31](./31-prototype-stepper-library-ux.md)) reuse it unchanged too, since a Stepper item is the identical "single fire-once key or mouse-button" shape as a Macro `KeyDown`/`KeyUp` step — not separately mocked here, and ticket 31's own item-entry stub (a plain `Gtk.Entry` + kind dropdown) was already waiting on this ticket to land.
- **Catalog**: this prototype hand-lists a representative ~112-entry catalog across 8 categories (letters/digits, modifiers, F1–F24, navigation, lock keys, misc, multimedia, mouse buttons) since the GUI has no binding to the real `evdev::KeyCode` enum — the real build reads the true list. F13–F24 sit behind a collapsed "Show F13-F24" toggle rather than always rendered, since they're rare and the function row would otherwise run to 25 keys wide.

No CONTEXT.md changes — no new domain term, purely a GUI affordance (mirrors ticket 02/31's own "unchanged" precedent).

Prototype: `prototype/32-key-mouse-button-picker-ux` (not `main`). Spawned [Build the key/mouse-button picker UX for real](./42-task-build-key-mouse-button-picker-ux.md).
