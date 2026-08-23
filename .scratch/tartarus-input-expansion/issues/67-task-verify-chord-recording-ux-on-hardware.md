Type: task

## Question

Live-verify [Build Chord recording UX and D-Bus surface](./40-task-build-chord-recording-ux.md)'s build against the real, connected Tartarus Pro and GUI — the joint HITL session ticket 40 itself skipped (no GUI-automation/screenshot tooling in that session to click through the running app, confirmed directly — `grim`/`slurp`/`gnome-screenshot`/`import`/`scrot` are all absent — and defining a Chord needs a human physically pressing device keys; per this map's ticket 26/42/47/48 precedent, an unattended session doesn't touch a user's live input/GUI state).

Checklist:

- Install the new Daemon/GUI and open Device Overview on the Grid destination — confirm the Chords section renders beside the grid with a "Select Chord members" toggle (off by default) and an empty Chords list.
- With the toggle off, click a grid key and confirm the ordinary per-Input Binding editor still opens exactly as before this ticket — the click-routing change must not have broken anything for non-Chord use.
- Turn the toggle on, click two grid keys, confirm the status line and "Binding →" button behave as designed (disabled below 2 selected, enabled at 2+), open the dialog, save a Keypress Chord, and confirm pressing both physical keys together fires the Chord's own output — not either key's individual passthrough.
- Define the thumbstick's four diagonals (Up+Right, Up+Left, Down+Left, Down+Right) one after another — confirm all four coexist with no conflict rejection (an Input may belong to any number of Chords per ticket 01's amended Answer) and each fires correctly and independently when its two physical directions are pressed together.
- Trigger the overlap-rejection UX: select a superset or subset of an existing Chord's members, confirm "Binding →" stays disabled, the status line names the conflicting Chord, and "Edit conflicting Chord" jumps straight to editing it.
- Edit an existing Chord's membership (add or remove a member) and save — confirm it persists as the *new* member set with the *old* one actually gone from `config.toml` (this exact case was a code-review-caught bug fixed pre-verification; needs a real check it holds against the real Daemon, not just the stub).
- Exercise Trigger-mode behavior on a physical Chord: Fire-once (single output per press), Hold-to-repeat (re-fires while both keys stay down, matching the device's own kernel autorepeat pacing), and Toggle (starts on completion, and releasing *any one* member — not necessarily a fresh press of the full set again — stops it).
- Physically release just one member mid-window (before the rest complete) and confirm that member's own individual Binding fires retroactively rather than nothing happening.
- Click a Chord's list row (not Edit) and confirm it previews that Chord's members on the grid with a distinct highlight, without entering selection mode.
- Delete a Chord via its "×" and confirm the D-Bus round-trip and disk persistence.

## Answer
