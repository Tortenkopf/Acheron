Type: prototype

## Question

Design the look and feel of **one shared library-picker GUI covering both Stepper and Macro** — settled as a joint scope by [Design reusable Macro entities](./15-decide-reusable-macro-entities.md)'s resolution, since both are "named entity, defined once in a global library, reassignable to a Binding" patterns with structurally identical picker chrome (list existing entries, create/edit/delete, assign to a Binding). No existing GUI pattern to copy (`binding_editor.py` edits one Input's Action inline today; there is no library picker/manager anywhere in the codebase).

Building on both settled models:
- **Stepper** ([ticket 03](./03-decide-stepper-list-stepping.md)): a named, ordered list of fire-once keyboard-key/mouse-button items, reassignable at any time to a different forward/backward Input pair — **exclusive** (only one pair may reference a given list at once; assigning it elsewhere silently moves it, no confirmation dialog decided).
- **Macro** (ticket 15): `MacroDef { name, steps: Vec<MacroStepDto> }` keyed by a frozen slug `MacroId`, reassignable to any number of Bindings across any Profile — **many-to-one**, no exclusivity, no "silently moves" behavior (deleting instead refuses while still referenced, `CommandError::InvalidRequest`, mirroring `DeleteProfile`).

Settle at least:

- **Shared chrome**: one library picker (browse/create/rename/delete) parameterized over "ordered list of fire-once items" (Stepper) vs. "sequence of Keypresses with delays" (Macro), vs. two visually-adjacent-but-separate pickers. Note the two constructs' deletion UX genuinely differs (Stepper's reassign-silently-moves vs. Macro's refuse-while-referenced) — the picker needs to surface that difference honestly, not paper over it with identical copy.
- **List/step authoring**: adding/reordering/removing a Stepper list's items (drag-to-reorder, up/down buttons, …) vs. Macro's existing step-sequence editor (already built in `binding_editor.py`, per issue 06 — likely relocates here rather than being redesigned).
- **Assignment**: how a user assigns a library Stepper list to a forward/backward Input *pair*, vs. a library Macro to a single Binding's Action slot — and how the GUI surfaces Stepper's silent reassignment (e.g. a "moved from X" toast) where Macro needs no equivalent.
- **Item/step entry**: reuses whatever key/mouse-button picker ticket 02/32 lands on, not a new capture mechanism.

Use the `/prototype` skill. Once resolved, spawn the real build ticket, matching the decide → prototype → build pattern used for Chord (tickets 01 → 30 → build) and the trigger-point UX (tickets 17 → 19 → 26 → 27).
