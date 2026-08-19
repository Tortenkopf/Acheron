Type: prototype
Blocked by: 15

## Question

Design the look and feel of the Stepper library GUI, building on the settled model from [Design the Stepper list-stepping construct](./03-decide-stepper-list-stepping.md): Stepper lists live in one global, named library — created once, holding an ordered sequence of fire-once keyboard-key/mouse-button items, reassignable at any time to a different forward/backward Input pair (only one pair may reference a given list at once; assigning it elsewhere silently moves it). No existing GUI pattern to copy (`binding_editor.py` edits one Input's Action inline today; there is no library picker/manager anywhere in the codebase).

**Blocked on [Design reusable Macro entities](./15-decide-reusable-macro-entities.md) on purpose**: both are "named entity, defined once in a library, reassignable to a Binding" patterns — a library picker/manager (list existing entries, create/edit/delete, assign to a Binding), identity/rename/delete-while-referenced handling, and the global-vs-per-Profile scoping question are all structurally the same shape Macro's ticket is solving. Resolve ticket 15 first and revisit scope here — this may turn out to *be* the same prototype (one shared library-picker UX parameterized over "sequence of Keypresses with delays" vs "ordered list of fire-once items"), or it may still warrant its own session for what's genuinely different: Stepper's item-reordering UI, and assigning a list to a *pair* of Inputs (forward/backward) rather than Macro's single Binding.

Settle at least:

- **Shared vs. separate**: does this ticket produce its own prototype, or fold into whatever ticket 15 spawns? Decide before doing any prototyping work.
- **List authoring**: adding/reordering/removing items in a Stepper list — drag-to-reorder, up/down buttons, something else.
- **Library picker**: where a user browses existing Stepper lists, creates a new one, renames/deletes one (and what happens to a Binding pair referencing a list that gets deleted).
- **Pair assignment**: how a user assigns a library list to a forward/backward Input pair, and how the GUI surfaces a reassignment silently moving the list off its previous pair (per ticket 03 — no confirmation dialog was decided, but this may be worth a "moved from X" toast/notice).
- **Item entry**: how a single fire-once keyboard-key/mouse-button item gets added to the list — likely reuses whatever key/mouse-button picker ticket 02 lands on, not a new capture mechanism.

Use the `/prototype` skill. Once resolved, spawn the real build ticket, matching the decide → prototype → build pattern used for Chord (tickets 01 → 30 → build) and the trigger-point UX (tickets 17 → 19 → 26 → 27).
