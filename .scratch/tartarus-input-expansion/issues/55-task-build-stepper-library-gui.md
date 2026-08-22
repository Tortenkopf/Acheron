Type: task
Blocked by: 54, 52

## Question

Build the real Stepper library GUI against [ticket 54](./54-task-land-stepper-library-daemon.md)'s landed Daemon shape and [ticket 31](./31-prototype-stepper-library-ux.md)'s settled variant B — split off [ticket 41](./41-task-build-stepper-macro-library-ux.md). Blocked on [ticket 52](./52-task-build-macro-library-gui.md) because it fills in the Steppers tab within the tab-switched shell that ticket built (the shell itself is not rebuilt here). No open design questions remain.

Scope:

- **Steppers panel**: fills in the Steppers tab of the shell [ticket 52](./52-task-build-macro-library-gui.md) built. List chrome (name / rename "✎" / delete "×" / "+ New" — no delete gate, unlike Macro: ticket 03 never specified one, since reassignment already silently moves a list off its pair rather than something being "in use" to protect against), an item editor with ↑/↓/× (add via a Key/Mouse-button kind selector + the real picker below).
- **Assignment row**: Forward/Backward Input dropdowns beneath the item list. Assigning a pair already claimed by another list silently steals it and surfaces a toast ("Moved off '<name>' (it no longer has an assigned pair)"), per ticket 31's settled specifics.
- **Autosave note**: the pane states upfront that edits save automatically, matching the Macros panel's pattern from ticket 52.
- **Item entry**: reuses the real key/mouse-button picker (`key_picker.py`, ticket 42/44) — not a redesign, same reuse as ticket 52's Macro step editor.
- **`binding_editor.py`**: the Action dropdown gains "Stepper" as a third option alongside Keypress/Macro, assigning a library entry rather than authoring one inline.

Live-hardware verification is deliberately deferred to [Verify the Stepper library on hardware](./56-task-verify-stepper-library-on-hardware.md), not done in this ticket.
