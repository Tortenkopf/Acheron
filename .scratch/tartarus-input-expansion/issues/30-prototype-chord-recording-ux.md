Type: prototype
Status: resolved

## Question

Design the look and feel of the Chord-recording flow in the GUI, building on the settled model from [Design Chord Bindings](./01-decide-chord-bindings.md): the user presses N physical Inputs on the device and the GUI records that set as a Chord's membership (membership only — not timing). No existing GUI pattern to copy (`binding_editor.py` always edits exactly one Input today; there is no multi-select/record-combo affordance anywhere in the codebase). Settle at least:

- **Entry point**: where/how a user starts defining a Chord — a dedicated button on the Device Overview, a new option in the existing per-Input binding popover ("add this key to a Chord"), or something else?
- **Live capture feedback**: what the GUI shows while "listening" for the chord's member presses — which Inputs are already down, whether/how the user confirms the set is complete (an explicit "done" action vs. detecting once all pressed-then-released), and how to cancel.
- **Editing an existing Chord**: adding/removing a member from an already-defined Chord without re-recording from scratch, and where the Chord's own Binding (trigger mode + action) gets edited relative to the membership-recording step.
- **Overlap rejection UX**: how the GUI explains a rejected save when the attempted Chord's Input set intersects an existing Chord's set (per ticket 01's overlap rule).
- **Discoverability of the thumbstick-diagonal worked example**: confirm the recording flow makes "press two adjacent thumbstick directions together" an obvious, discoverable way to define a diagonal, not just a theoretical possibility.
- **Debug-only window slider**: per ticket 01's Q8/Q10, this prototype should carry a live-adjustable slider for the ~50ms chord-detection window, used only to converge empirically on the right constant for `dispatch.rs` — the slider itself never ships in the real GUI.

Use the `/prototype` skill. Once resolved, spawn the real build ticket (GUI + any new `SetChordBinding`/`ClearChordBinding` D-Bus surface per ticket 01's implied Command additions), matching the decide → prototype → build pattern used for the trigger-point UX (tickets 01 → 19 → 26 → 27).

## Answer

Prototyped three structurally different variants in a throwaway GTK4 app (`prototype/30-chord-recording-ux`, not `main`): (A) a toolbar entry point with the recording step folded directly into Device Overview's own grid; (B) entry via each Input's own popover, with a non-modal banner that auto-finishes ~1.2s after the last change; (C) a persistent "Chords" sidebar mirroring the Action Table's own pattern, with structural overlap prevention. **Variant A won**, refined over three rounds of live reaction:

- **Entry point**: no dedicated button at all in the final form — clicking any Input directly on Device Overview's own grid toggles it into an in-progress selection (round 1 instead opened a "+ New Chord" button into a modal wizard with its *own* copy of the device grid; round 2 dropped that duplicate grid in favor of the real one).
- **Live capture feedback**: a status line above the grid names the current selection (`Selected: Grid 3 + Grid 4`); a "Binding →" button below the grid stays disabled until ≥2 Inputs are selected (and while the selection conflicts with an existing Chord — see below); "Clear selection" backs out without saving. No explicit "recording" mode to enter/exit — selection just *is* the live state.
- **Editing an existing Chord**: each Chord's row in the "Chords" list has an "Edit" button that pre-loads its members into the same selection state (now addable/removable via the same grid clicks) and its existing Trigger/Action into the same "Binding →" dialog. Clicking a row *without* the Edit button previews that Chord's members as a distinct amber highlight on the grid, without entering edit mode — added per live feedback once the list existed.
- **Where the Binding gets edited**: a single small modal dialog, opened only by "Binding →", containing just the Trigger/Action editor (ticket 01/02's already-settled UI, reused as-is) — membership is fully decided before this dialog ever opens.
- **Overlap rejection UX — and a correction to ticket 01 along the way**: building the thumbstick-diagonal worked example exposed that ticket 01's original "an Input belongs to at most one Chord" rule breaks that exact example (Up needs to sit in both Up-Left and Up-Right). Corrected via a short live grilling exchange (full reasoning in [Design Chord Bindings](./01-decide-chord-bindings.md)'s own Correction section): an Input may now belong to any number of Chords; only a *subset/superset* relationship between two Chords' member sets is rejected (checked against the finished selection, not per-click, since it can only be known once the candidate set stops changing). When blocked, the status line names the conflicting Chord and an "Edit conflicting Chord" button jumps straight to it.
- **Discoverability of the thumbstick diagonals**: confirmed live — all four diagonals (Up+Right, Up+Left, Down+Left, Down+Right) can be defined one after another with no conflict, each cardinal direction correctly reused across two diagonals. A static tip line names the interaction ("click two adjacent thumbstick directions together").
- **Debug-only window slider**: an `Expander` labeled "Debug" below the grid, collapsed by default — present in every round, placement is the only thing that moved (started inside the round-1 modal wizard).

Variants B and C are captured alongside A on the same throwaway branch for reference but reflect the pre-correction single-membership rule; they were not updated for round 3, since they were not the direction ultimately chosen.

**New D-Bus/Command surface reminder** (per ticket 01, unchanged by the correction): `SetChordBinding { inputs: BTreeSet<Input>, layer, binding, reply }` / `ClearChordBinding { inputs, layer, reply }`, with the (now narrower) conflict check enforced wherever it actually lands — GUI-side against a `GetConfig()` snapshot, or Daemon-side inside the handler.

Spawned [Build Chord recording UX and D-Bus surface](./40-task-build-chord-recording-ux.md).
