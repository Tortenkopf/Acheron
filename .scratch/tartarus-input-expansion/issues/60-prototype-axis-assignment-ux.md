Type: prototype
Status: open

## Question

Prototype the **axis-assignment GUI** — the concrete look/interaction [Design Controller/Joystick axis output](./59-decide-controller-axis-output.md) deliberately left undesigned.

Ticket 59 settled the underlying shape: axis assignment is a new, parallel per-(Layer, Input) concept, mutually exclusive with that Layer's Binding/Chord-membership for the same grid key, offering one of 17 axis targets (5 unsigned single-key axes + 6 signed axes split into independently-assignable +/- halves). It reuses the key's existing Actuation/Release-point UX (tickets 19/26) as the axis's start/end thresholds — no new deadzone control needed.

Open UX questions to prototype against, per ticket 59's Answer:

- **The Axis/Digital fork**: the per-key Binding editor currently opens straight into the Action-kind dropdown (Keypress/Controller Button/Macro/Stepper/Profile Switch). Where does "this key is an axis instead" sit relative to that — a toggle above the existing dropdown, a 6th dropdown entry that swaps the whole editor body, something else? Only grid keys are eligible; the editor for every other Input (Mode key, thumbstick, wheel) doesn't need this fork at all.
- **Picking one of 17 targets**: how is the catalog presented — flat categorized list (Triggers / Sticks / Pedals), a diagram similar to ticket 38's winning Gamepad Diagram variant (now needs axis regions, not just buttons), something else? Signed axes need the two independently-assignable halves (+/-) to read clearly as two separate picks, not one.
- **Cross-key awareness**: when picking a target already claimed by another key (same signed half, or its opposite half), does the picker surface that (a toast, an inline note, nothing — since ticket 59 settled this is *allowed*, not rejected)? Mirrors the precedent in [ticket 55](./55-task-build-stepper-library-gui.md)'s steal-toast for a similar "not rejected, but worth surfacing" case.
- **Chord/exclusion visibility**: Device Overview's grid-click Chord-recording flow (ticket 30/40) needs Axis-assigned Inputs on the active Layer to be excluded from selection. How does the grid communicate "this key can't join a Chord right now" at a glance?

Per ticket 59's own precedent (14 → 38 → 43 → 45), a winning variant here spawns a build ticket, then a hardware-verification ticket.
