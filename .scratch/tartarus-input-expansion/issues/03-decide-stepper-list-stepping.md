Type: grilling

## Question

Design the **Stepper** construct — a generic pairing of two physical Inputs ("forward"/"backward") that steps a cursor through a user-defined ordered list of items, firing the newly-selected item on every step, in one motion (confirmed reading during charting — each physical activation both moves the cursor *and* fires, not a separate select-then-confirm). Primary intended use is the scroll wheel's `ScrollUp`/`ScrollDown`, but the construct itself should stay generic — bindable to any two Inputs, per the domain model's existing treatment of wheel up/down as two independent Inputs. Settle at least:

- **Item type**: items are restricted to fire-once Keypress or mouse-button actions (settled by the user directly — no Macro items, no per-item Hold-to-repeat/Toggle). Confirm this constraint is actually enforceable/enforced in whatever shape you land on, don't re-litigate it.
- **Data model**: is a Stepper a distinct top-level entity (e.g. a named list referenced by two Bindings) or an inline structure attached to a Binding pair? How does it interact with the existing sparse `HashMap<Input, Binding>` per Layer — does binding an Input to "Stepper forward" preclude that Input also carrying an ordinary Binding?
- **Scoping**: is a Stepper (and its current cursor position) Profile-scoped, Layer-scoped, or independent of both?
- **Boundary behavior**: does stepping past either end of the list wrap around or clamp?
- **Cursor persistence**: does the current position survive a Layer switch, a Profile switch, and a Daemon restart, or reset in some/all of those cases?
- **GUI**: building/reordering a Stepper's item list is likely its own "how should it look/behave" question — consider `/prototype`.

Once resolved, add the full **Stepper** entry to CONTEXT.md (currently just a reserved name — see the map's Notes). The map's "Not yet specified" section flags unresolved composition questions between Stepper, Chord, and mouse-button output — revisit whether any of that fog is now specifiable once this ticket lands.
