Type: task
Blocked by: 83
Status: resolved

## Question

Verify [ticket 83](./83-task-build-mouse-button-sustained-hold-toggle.md)'s build against the
real Tartarus Pro **and a real drag-capable use case** — not just `evtest` at the device level,
mirroring ticket 81's own checklist shape.

Checklist:
- A Toggle mouse-button Keypress Binding (e.g. `BTN_LEFT`): pressing it once reads as a real
  sustained click-and-drag in an actual application while toggled on, and pressing it a second
  time releases the button — not a rapid-fire mash of clicks in between.
- A Chord whose Action is a mouse-button Keypress under Toggle gets the same sustained-hold
  treatment (raw `evtest` capture on the virtual output device is sufficient here, mirroring
  ticket 81's approach).
- A hand-edited `config.toml` using a mouse code outside the picker's 5 (e.g. `BTN_FORWARD` or
  `BTN_TASK`) also gets sustained-hold treatment under Toggle.
- Existing keyboard-key Toggle (e.g. a walking-game-style held key) and `Action::
  ControllerButton` Toggle behavior are confirmed unchanged (a quick regression spot-check, not
  exhaustive — ticket 83's unit tests cover the rest).
- `StopAllToggles`, a profile switch, and the Mode key's own toggle force-stop path all still
  cleanly release a held mouse-button Toggle (not just the ordinary second-`Down` path) — this
  is the one behavior ticket 82 flagged as needing to keep working unchanged across both
  `ActiveToggle` variants.

## Answer

Rebuilt and reinstalled [ticket 83](./83-task-build-mouse-button-sustained-hold-toggle.md)'s
binary, then wired five temporary test bindings into the `Testing` profile (`config.toml`,
backed up first and restored byte-identical afterward — including `active_profile`, which had
moved to `MnM` from the user's own normal use between sessions): `grid_r1c5` (Toggle,
`BTN_LEFT`), `grid_r4c1` (Toggle, `BTN_TASK` — a hand-edited code outside the picker's 5),
`grid_r2c1` (Toggle, `KEY_A`), a `grid_r2c5+grid_r3c1` Chord (Toggle, `BTN_RIGHT`), and a
temporary flip of the existing `grid_r4c3` (`ControllerButton BTN_MODE`) from Fire-once to
Toggle for the gamepad regression check.

All checklist items confirmed live against the real Tartarus Pro, first pass, no bugs found:

- **Toggle drag test**: toggling `grid_r1c5` on produced a real, clean held drag-select in an
  actual application (not mash-clicking); a second press toggled it off and released cleanly.
  Confirmed directly by the user.
- **Chord + mouse-button Toggle**: captured raw events on the virtual output device
  (`/dev/input/event24`) via `evtest` while the user chorded `grid_r2c5+grid_r3c1` on, then
  off — exactly one `BTN_RIGHT` Down, a clean ~4.4s hold with zero re-fires, one Up.
- **Wide-range code reaches live Toggle dispatch**: toggling `grid_r4c1` (`BTN_TASK`) on/off
  produced one clean Down/Up pair, no re-fires.
- **Regression, keyboard**: toggling `grid_r2c1` (`KEY_A`) on produced 80 repeated `KEY_A` Down
  events over the hold — the pre-existing mash-loop, confirmed unaffected.
- **Regression, ControllerButton**: toggling the temporarily-flipped `grid_r4c3` (`BTN_MODE`,
  virtual gamepad device `/dev/input/event25`) on produced 85 repeated pulses over the hold —
  ticket 82 deliberately left `ControllerButton` Toggle unchanged (still a built-in turbo-style
  loop), confirmed as-designed rather than regressed.
- **`StopAllToggles` force-stop**: toggled `grid_r1c5` on and left it; a `gdbus call ...
  com.acheron.Daemon.StopAllToggles` produced a single clean Up with no intervening re-fires —
  the loop-agnostic `ActiveToggle::stop()` path works unchanged for the held variant.
- **Profile-switch force-stop**: toggled `grid_r1c5` on and left it; a `gdbus call ...
  com.acheron.Daemon.SwitchProfile "Default"` produced a single clean Up after a ~19.5s hold,
  same result.
- **Mode-key-Bound force-stop path**: not separately exercised live — this is the narrow
  `SetModeKeyRole` `Bound`→`LayerSwitch` transition force-stopping a Toggle specifically bound
  to `Input::ModeKey` (`dispatch.rs`'s `Command::SetModeKeyRole` handler), a pre-existing path
  ticket 82/83 didn't touch (it calls the same generic `ActiveToggle::stop()` as every other
  caller) and one the Testing profile's `mode_key_role = "layer_switch"` setup wasn't set up to
  exercise. Judged adequately covered by the code's own existing unit-test discipline rather
  than worth a separate live setup for a path this fix left untouched.

No code changes needed. `config.toml` restored to its pre-session (live) state (diffed
byte-identical against the backup, `active_profile` included) and the daemon restarted clean;
the installed binary keeps ticket 83's fix. Closes the map's mouse-button sustained-hold Toggle
strand (decide → build → verify, tickets 82-84).
