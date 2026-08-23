Type: grilling
Status: resolved

## Question

Decide how **depth** (the analog data model settled in [Decide the analog data model](./17-decide-analog-data-model.md)) composes with the three feature-set decisions that landed independently of it: **Chord** ([Design Chord Bindings](./01-decide-chord-bindings.md)), **Stepper** ([Design the Stepper list-stepping construct](./03-decide-stepper-list-stepping.md)), and **Macro** ([Design reusable Macro entities](./15-decide-reusable-macro-entities.md)). This is the analog-composition fog left on the map after the analog charting pass, held back until both sides had settled their own models — they now have, including the capture-path rework and trigger-point UX ([ticket 18](./18-rework-capture-path-for-analog.md), [ticket 26](./26-task-build-trigger-point-depth-ux.md)) being fully built and live-hardware-verified.

Settle at least:

- **Chord + actuation points**: a Chord made of grid-key members — does it fire based on each member key's own individually-configured actuation point, or a single shared threshold for the Chord as a whole? Chords are Base/Held-scoped `Binding`s keyed on a member `BTreeSet<Input>` (ticket 01); actuation points are per-Input-per-Profile (ticket 17). Decide whether the Chord's completion condition needs any new plumbing beyond "each member's `PhysicalEvent` already crosses its own actuation point," or whether it's already correct as-is.
- **Stepper driven by depth**: can a Stepper's Forward/Backward pair be driven by depth (e.g. step once per some depth threshold crossing, or scrub continuously with depth) rather than only a discrete Fire-once/Hold-to-repeat press? Ticket 03 scoped Stepper's Trigger mode to Fire-once/Hold-to-repeat only (Toggle disallowed); decide whether a depth-driven stepping mode is worth adding now, and if so what it would look like architecturally (closer to Analog-repeat's per-Input background task shape, per [ticket 20](./20-decide-analog-repeat-trigger-mode.md), or something else).
- **Macro reading depth**: does a Macro step ever want to *read* depth rather than just fire a discrete KeyDown/KeyUp? If there's no articulated use case, it's fine to rule this out of scope rather than design speculative plumbing.

If any sub-question resolves as "no interaction needed, already correct as designed" (as several analog/Chord and analog/Profile-Switch questions already have, per tickets 01/03/05's own answers), say so plainly and move on — this ticket doesn't need to invent work where none is needed.

## Answer

All three sub-questions resolve to **no interaction needed — already correct as designed.** No code changes, no new plumbing, no CONTEXT.md updates.

- **Chord + actuation points**: already correct as-is. A Chord's members are ordinary Inputs firing ordinary `PhysicalEvent`s; each member's own per-Input-per-Profile actuation point (ticket 17) is already applied before that event ever reaches the Chord's simultaneity-window logic in `dispatch.rs`. The Chord layer only ever sees "this Input went Down/Up," already thresholded — it has no reason to know or care whether that Down/Up came from a fixed evdev press or a depth crossing. A shared Chord-wide threshold was never actually a live option: there's nothing at the Chord's level to threshold, since depth never reaches that far.
- **Stepper driven by depth**: already correct as-is — no depth-driven stepping mode is being added. Stepper's Fire-once/Hold-to-repeat-only scope (ticket 03) stays as designed; no articulated use case justifies a depth-driven stepping mode, and adding one speculatively would mean inventing a second per-Input background-task mechanism (Analog-repeat's shape) for a construct that already works fine as a discrete action.
- **Macro reading depth**: already correct as-is — ruled out of scope, no use case. A Macro step stays a discrete `KeyDown`/`KeyUp` pair; nothing in the Macro model needs to observe depth, and no plumbing exists or is being added for it to do so.

The analog-composition fog is fully closed by this ticket — nothing further graduates from it.
