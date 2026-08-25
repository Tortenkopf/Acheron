Type: task
Blocked by: 82
Status: resolved

## Question

Build [ticket 82](./82-decide-mouse-button-sustained-hold-toggle.md)'s settled design, AFK
(no hardware needed for the code itself — hardware verification is the follow-up task):

- Add `ActiveToggle::spawn_held(injector: Injector, key: KeyCode) -> Self` to
  `daemon/src/executor.rs`, alongside the existing `spawn`: fires a single `KeyDown(key)` via
  `execute_step`, awaits the cancellation token, then releases via the existing `force_release`
  helper. Same `{cancel, handle}` struct shape as the loop variant — `stop()` needs no changes.
- `dispatch::fire()`'s `(TriggerMode::Toggle, EventState::Down)` arm and `dispatch::
  fire_chord()`'s equivalent Toggle arm each gain a new match arm ahead of the general one,
  guarded on `matches!(binding.action, Action::Keypress { key, .. } if
  crate::input::is_mouse_button(key))` — structurally identical to the existing `HoldToRepeat`
  mouse-button carve-out already in both functions. Extract `key`, call
  `ActiveToggle::spawn_held(injector.clone(), key)`, insert into `toggles`/`chord_toggles` same
  as today.
- No change to keyboard-key Toggle, `Action::ControllerButton` Toggle, Macro, Stepper, or any
  other Trigger mode. No change to `ActiveToggle::stop()`, `StopAllToggles`, profile-switch
  force-stop, or the Mode-key/second-`Down` stop paths — all call `stop()` generically and work
  unchanged for both `ActiveToggle` variants.
- Add unit tests covering: `spawn_held` fires exactly one `KeyDown` and, on `stop()`, exactly
  one `KeyUp`, with nothing in between even after a delay (no loop); a mouse-button Toggle
  Binding via `fire()` behaves the same at the dispatch level; a Chord whose Action is a
  mouse-button Keypress under Toggle gets the same treatment via `fire_chord()`; existing
  keyboard-key Toggle and `ControllerButton` Toggle behavior is unchanged (regression coverage,
  mirroring ticket 80's own negative tests).

Spawn [Verify the mouse-button sustained-hold Toggle fix on hardware](./84-task-verify-mouse-button-sustained-hold-toggle-on-hardware.md)
(already created, currently blocked on this ticket).

## Answer

Landed both pieces exactly as scoped, AFK, no hardware needed:

- **`ActiveToggle::spawn_held`** (`daemon/src/executor.rs`): a new constructor alongside
  `spawn`, backed by a new `run_toggle_held` task — fires a single `KeyDown` via
  `execute_step`, awaits the cancellation token, then releases via the existing
  `force_release` helper. Same `{cancel, handle}` struct shape as the loop variant, so
  `stop()` and every existing caller of it needed zero changes.
- **`dispatch::fire()`/`fire_chord()`**: each gained a new `(TriggerMode::Toggle,
  EventState::Down)` match arm ahead of the general one, guarded on
  `matches!(binding.action, Action::Keypress { key, .. } if is_mouse_button(key))` —
  structurally identical to the existing `HoldToRepeat` mouse-button carve-out in both
  functions. Extracts the `KeyCode`, calls `ActiveToggle::spawn_held`, inserts into
  `toggles`/`chord_toggles` exactly as the general arm already did.
- **Blast radius confirmed as scoped**: only a `Keypress` whose key is a mouse-button code
  hits either new arm; keyboard-key Toggle, `ControllerButton` Toggle, Macro, and Stepper
  output take the pre-existing loop path unchanged. `ActiveToggle::stop()` itself, and every
  caller of it (`StopAllToggles`, profile switch, the Mode key, a Toggle Chord's "full member
  set again" stop, a plain Input's own second `Down`), needed no changes — they operate on
  `ActiveToggle` generically, oblivious to which variant is inside.
- **Tests** (`daemon/src/executor.rs`/`daemon/src/dispatch.rs`, 351 Rust tests green, net +4,
  `cargo clippy --all-targets`/`cargo fmt --check` clean):
  `spawn_held_holds_a_single_keydown_until_stopped` (executor-level: one KeyDown, silence
  through several ordinary-Toggle-lap-widths of simulated time, one KeyUp on stop),
  `toggle_mouse_button_holds_a_single_keydown_and_the_same_key_stops_it` (dispatch-level,
  plain Input), `toggle_chord_mouse_button_holds_a_single_keydown_and_full_completion_stops_it`
  (Chord mirror), and `toggle_keyboard_key_still_loops_at_dispatch_level` (regression:
  confirms the carve-out doesn't bleed onto an ordinary keyboard-key Toggle, which must still
  produce more than one KeyDown/KeyUp pair over the same simulated window). Existing
  `ControllerButton`/keyboard-key Toggle-via-`Macro` suites (`toggle_starts_on_down_...`,
  `toggle_chord_survives_releasing_one_member_...`) pass unchanged, confirming no regression
  there either.
- Not done here, deliberately: hardware verification and click-and-drag feel — that's
  [ticket 84](./84-task-verify-mouse-button-sustained-hold-toggle-on-hardware.md), now
  unblocked.
