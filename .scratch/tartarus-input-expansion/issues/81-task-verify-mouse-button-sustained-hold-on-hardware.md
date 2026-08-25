Type: task
Blocked by: 80
Status: resolved

## Question

Verify [ticket 80](./80-task-build-mouse-button-sustained-hold.md)'s build against the real Tartarus Pro **and a real drag-capable use case** — not just `evtest` at the device level.

Checklist:
- A Hold-to-repeat mouse-button Keypress Binding (e.g. `BTN_LEFT`), held down, reads as a real sustained click-and-drag in an actual application (e.g. drag-select in a file manager, or drag a window/canvas item) — not a rapid-fire mash of clicks.
- A Fire-once mouse-button Keypress Binding still registers a single click normally (unchanged).
- A Chord whose Action is a mouse-button Keypress under Hold-to-repeat gets the same sustained-hold treatment.
- A hand-edited `config.toml` using a mouse code outside the picker's 5 (e.g. `BTN_FORWARD` or `BTN_TASK`) also gets sustained-hold treatment under Hold-to-repeat, confirming `is_mouse_button`'s wider range actually reaches live dispatch, not just the unit tests.
- Existing keyboard-key Keypress and `Action::ControllerButton` Hold-to-repeat behavior is confirmed unchanged (a quick regression spot-check, not exhaustive — ticket 80's unit tests cover the rest).

## Answer

Rebuilt (`cargo build --release`) and reinstalled [ticket 80](./80-task-build-mouse-button-sustained-hold.md)'s
binary, then wired five temporary test bindings into the already-active `Testing` profile
(`config.toml`, backed up first and restored byte-identical afterward): `grid_r1c5`
(Hold-to-repeat, `BTN_LEFT`), `grid_r2c1` (Fire-once, `BTN_LEFT`), `grid_r4c1` (Hold-to-repeat,
`BTN_TASK` — a hand-edited code outside the picker's 5), `grid_r2c5` (Hold-to-repeat, `KEY_A`,
also a Chord member), a `grid_r2c5+grid_r3c1` Chord (Hold-to-repeat, `BTN_RIGHT`), and a
temporary flip of the existing `grid_r4c3` (`ControllerButton BTN_MODE`) from Fire-once to
Hold-to-repeat for the gamepad regression check.

All five checklist items confirmed live against the real Tartarus Pro, first pass, no bugs found:

- **Drag test**: holding `grid_r1c5` produced a real, clean single click-and-drag in an
  actual application — not mash-clicking. Confirmed by the user directly (this is the one
  item the ticket calls out as needing more than device-level events).
- **Fire-once unchanged**: `grid_r2c1` registered a single normal click.
- **Chord + mouse-button**: captured raw events on the virtual output device
  (`/dev/input/event24`, "Acheron Virtual Tartarus Pro") via `evtest` while the user chorded
  `grid_r2c5+grid_r3c1`. Across four press/hold/release attempts, every cycle showed exactly
  one `BTN_RIGHT` Down, a clean 1–4s hold with zero re-fires, then exactly one Up.
- **Wide-range code reaches live dispatch**: holding `grid_r4c1` (`BTN_TASK`, unreachable from
  the picker) produced one `BTN_TASK` Down, a ~3.9s hold with no re-fires, one Up — confirming
  `is_mouse_button`'s `BTN_LEFT..=BTN_TASK` range is live, not just unit-tested.
- **Regression, keyboard**: holding `grid_r2c5` alone (unchorded) produced repeated `KEY_A`
  Down/Up pairs roughly every 32ms for the duration of the hold — the pre-existing mash-click
  behavior, confirmed unaffected by the mouse-button-scoped carve-out.
- **Regression, ControllerButton**: holding the temporarily-flipped `grid_r4c3` (`BTN_MODE`)
  on the virtual gamepad device (`/dev/input/event25`, "Acheron Virtual Controller") produced
  one clean Down/Up pair over ~4.5s — [ticket 76](./76-task-build-controller-button-pulse-fix.md)'s
  sustained-hold fix is untouched by this ticket's change, as designed (the two carve-outs are
  guarded on disjoint predicates — Action variant vs. `is_mouse_button`).

No code changes needed. `config.toml` restored to its pre-session state (diffed byte-identical
against the backup) and the daemon restarted clean; the installed binary keeps ticket 80's fix.
Closes the map's mouse-button sustained-hold strand (decide → build → verify, tickets 79–81),
mirroring the Controller-button pulse-fix strand's tickets 75–77.

