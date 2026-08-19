Type: prototype

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
