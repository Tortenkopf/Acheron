Type: prototype

## Question

Design the look and feel of the Chord-recording flow in the GUI, building on the settled model from [Design Chord Bindings](./01-decide-chord-bindings.md): the user presses N physical Inputs on the device and the GUI records that set as a Chord's membership (membership only — not timing). No existing GUI pattern to copy (`binding_editor.py` always edits exactly one Input today; there is no multi-select/record-combo affordance anywhere in the codebase). Settle at least:

- **Entry point**: where/how a user starts defining a Chord — a dedicated button on the Device Overview, a new option in the existing per-Input binding popover ("add this key to a Chord"), or something else?
- **Live capture feedback**: what the GUI shows while "listening" for the chord's member presses — which Inputs are already down, whether/how the user confirms the set is complete (an explicit "done" action vs. detecting once all pressed-then-released), and how to cancel.
- **Editing an existing Chord**: adding/removing a member from an already-defined Chord without re-recording from scratch, and where the Chord's own Binding (trigger mode + action) gets edited relative to the membership-recording step.
- **Overlap rejection UX**: how the GUI explains a rejected save when the attempted Chord's Input set intersects an existing Chord's set (per ticket 01's overlap rule).
- **Discoverability of the thumbstick-diagonal worked example**: confirm the recording flow makes "press two adjacent thumbstick directions together" an obvious, discoverable way to define a diagonal, not just a theoretical possibility.
- **Debug-only window slider**: per ticket 01's Q8/Q10, this prototype should carry a live-adjustable slider for the ~50ms chord-detection window, used only to converge empirically on the right constant for `dispatch.rs` — the slider itself never ships in the real GUI.

Use the `/prototype` skill. Once resolved, spawn the real build ticket (GUI + any new `SetChordBinding`/`ClearChordBinding` D-Bus surface per ticket 01's implied Command additions), matching the decide → prototype → build pattern used for the trigger-point UX (tickets 01 → 19 → 26 → 27).
