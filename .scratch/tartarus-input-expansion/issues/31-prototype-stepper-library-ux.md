Type: prototype
Status: resolved

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

## Answer

**Variant B — Two Adjacent Panels** won: a tab switch ("Steppers" / "Macros", the same widget shape as Device Overview's own Base/Held layer tabs) flips between two structurally separate full-width panels, each with its own list chrome and its own editor below it. Never merged into one list — a deliberate answer to the "shared chrome" question: keeping the two constructs visually un-mixed was judged worth never showing both at once, over Variant A's single unified list (type-badged rows, one adaptive editor pane) and Variant C's inline-first, no-browse-page approach (assign at the point of use, manage from a corner menu).

Refined over two rounds of live reaction:
- **Round 1**: three variants as scoped above, reacted to via `python3 gui/prototype_31_stepper_macro_library_ux.py`.
- **Round 2** (this ticket's actual settled shape): a rename ("✎") button added to both the Steppers and Macros panels' rows, ported from Variant A's chrome (the closest existing precedent is `device_overview.build_profile_sidebar`'s own name/rename/delete row). Macro step editing gained ↑/↓ reorder buttons (previously Stepper-item-only) alongside its existing "×" remove, since step order is semantically load-bearing for a Macro too. Both editor panes now state upfront that edits **save automatically** — every mutation here (add/remove/reorder/rename/delete/assign) applies immediately with no Save button, mirroring the Profile sidebar's own convention — called out explicitly because it differs from the Binding editor's explicit Save/Clear pattern and could otherwise read as a bug ("did my edit not take?") to a user used to that pattern.

**Settled specifics carried into the build**:
- **List/step authoring**: Stepper items get ↑/↓/× (add via a Key/Mouse-button kind dropdown + entry); Macro steps get the same ↑/↓/× shape now, relocated near-verbatim from `binding_editor.py`'s existing inline editor (issue 06) rather than redesigned.
- **Assignment**: a Stepper's own editor pane carries "Forward"/"Backward" Input dropdowns beneath its item list; assigning a pair already claimed by another list silently steals it and surfaces a toast ("Moved off '<name>' (it no longer has an assigned pair)") — Macro needs no equivalent since it has no exclusive pair to steal.
- **Deletion honesty**: a Macro's delete ("×") is disabled with a tooltip ("Used by N Binding(s) — can't delete") while `used_by` is non-empty; a Stepper's delete has no such gate (ticket 03 never specified one — reassignment already silently moves a list off its pair, so an assigned list has nothing "in use" to protect against).
- **Item/step entry**: a plain `Gtk.Entry` (plus a Key/Mouse-button kind `Gtk.DropDown`) stands in for whichever picker ticket 32 lands on — not a redesign of that control, confirmed as out of this ticket's scope.

Prototype: `prototype/31-stepper-macro-library-ux` (both variants, all three, kept as the primary source — not folded into `main`). Neither Stepper nor the Macro library exist in the real Daemon/config yet (tickets 03/15 only designed the shapes; `Action::Macro` is still inline-only in code, per the Notes' grounding facts). Spawned [Build the Stepper/Macro library UX for real](./41-task-build-stepper-macro-library-ux.md).
