Type: task
Status: resolved

Blocked by: 91, 92

## Question

Build [ticket 92's settled design](./92-decide-controller-button-items-in-stepper-macro.md)
for real: the controller-button picker available for **Stepper item** and **Macro step**
editing in the library, with the switcher between it and the keyboard/mouse key picker. Daemon
+ GUI. Follow ticket 92's Answer for every shape decision it settles; this ticket is the
implementation and its scope tracks whatever 92 lands on.

Expected surface (refine against 92's Answer):

### Daemon (`daemon/src/`)

- New `StepperItem::ControllerButton { button }` variant (`config.rs`), compiled in
  `dispatch::resolve_step`, routed to the gamepad `uinput` device via the existing
  `input::is_gamepad_button` injector routing.
- The Macro-step form 92 chose (a down/up pair, an atomic step, or a third step-kind) in
  `executor.rs` / `config.rs`.
- Allowlist validation (`input::gamepad_button_codes()`) at the config-parse and D-Bus paths
  92 identifies (`CreateStepper`/`SetStepperItems`/`CreateMacro`/`SetMacroSteps`/`parse`),
  mirroring `Action::ControllerButton`'s existing two-place enforcement (ticket 43).
- Wire layer: `wire.rs` `stepper_item_to_dict`/`from_dict` and the equivalent Macro-step
  marshalling, plus the Python mirrors in `daemon_stub.py` / `daemon_client.py` — plan the
  `button` field in from the start (ticket 63 had to retrofit `modifiers` here; don't repeat
  that).

### GUI (`gui/acheron_gui/`)

- The switcher (92's chosen shape) in `library_view.py`'s Stepper item editor and Macro step
  editor, built **on top of** ticket 91's homogenized column-3 layout.
- Mount `controller_picker`'s inline picker in the "controller" mode; keep
  `key_picker.build_inline_key_picker` in the "keyboard" mode. The modifier checkboxes and
  (for Macro) the step-kind dropdown behave per 92's Answer when controller mode is active.
- `describe_stepper_item` / `describe_step` render a readable label for a controller-button
  item/step (reuse `controller_picker`'s catalog labels, e.g. "Btn: A / South").
- If 92 spawned a prototype child ticket, this ticket is also `Blocked by` it — add that edge.

### Tests + housekeeping

- Rust: parse/validation/compile coverage for both new forms; clippy/fmt clean.
- Python: switcher behavior, label rendering, round-trip through the stub.
- CONTEXT.md: update the Stepper entry ("designed to extend to ... later" → now done) and the
  Macro entry if the step model changed.
- `/code-review` pass before landing (map discipline).

### Verification

Build here; **hardware verification is split into [ticket 94](./94-task-verify-library-controller-button-picker-on-hardware.md)**
(the largest item in this cluster, daemon-touching — the explicit decide→build→verify split,
per this map's precedent for Controller-button work, tickets 43→45 / 75→77).

## Answer

Built to ticket 92's Answer, daemon + GUI + tests. Live-hardware verification stays
[ticket 94](./94-task-verify-library-controller-button-picker-on-hardware.md) (unblocked
now); this session built and screenshot-verified against the running GUI.

### Daemon (`daemon/src/`)

- **`config.rs`** — new `StepperItem::ControllerButton { button: KeyCode }` variant alongside
  `Key { key, modifiers }`. **No `modifiers` field** (92 §2). With the enum's existing
  `#[serde(tag = "type", rename_all = "snake_case")]` the wire/TOML tag is
  `controller_button` automatically. New `ConfigError::InvalidControllerButtonStepperItem`
  (parallels `InvalidControllerButton`, 92 §2), raised by a new `parse()` scan over
  `config.steppers.values()` → items against `input::is_gamepad_button` — so a hand-edited
  `config.toml` with a `controller_button` item naming `KEY_A` refuses to start.
- **`executor.rs`** — extracted `controller_button_steps(button) -> Vec<MacroStep>` (the
  `KeyDown / Delay(CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD=35ms) / KeyUp` triple);
  `compile`'s `Action::ControllerButton` arm now delegates to it, so `dispatch` doesn't
  hand-inline the triple.
- **`dispatch.rs`** — `resolve_step` switched from the irrefutable `let StepperItem::Key`
  to a `match` on `def.items[next]` (`StepperItem` is `Copy`): `Key` →
  `executor::keypress_steps` (unchanged), `ControllerButton` →
  `executor::controller_button_steps`. New `validate_stepper_items()` guard (mirrors
  `validate_binding`'s `Action::ControllerButton` allowlist check, same error string) wired
  into `Command::CreateStepper` and `Command::SetStepperItems`.
- **`dbus/wire.rs`** — `stepper_item_to_dict` / `stepper_item_from_dict` marshal the
  `controller_button` arm as `{"type": "controller_button", "button": "BTN_SOUTH"}` —
  `button` planned in from the start, no `modifiers` key (the ticket-63 retrofit lesson).
- **Macro steps: no new daemon shape** (92 §1 route (c)) — a gamepad `KeyDown`/`KeyUp` step
  is an ordinary `MacroStepDto::KeyDown(BTN_*)`; the injector already routes it by
  `is_gamepad_button`. No new macro-step validation (a gamepad allowlist gate there would
  contradict the "any code" design and the injector can't crash on any code).

### GUI (`gui/acheron_gui/`)

- **`wire.py`** — `stepper_item_to_variant` gains the `controller_button` arm.
- **`binding_editor.py`** — `describe_step` renders a gamepad `KeyDown`/`KeyUp` step as
  `↓ Btn: A / South` / `↑ Btn: …` via `controller_picker.LABEL_BY_CODE`; a mouse `BTN_*`
  code (not in that catalog) keeps the plain `KeyDown BTN_LEFT` form.
- **`library_view.py`**:
  - `describe_stepper_item` renders a `controller_button` item as `Btn: A / South`.
  - `build_library_picker_switch` — the "Keyboard / mouse" | "Controller" segmented control
    (92 §3), same shape as `device_overview.build_destination_switch`, each button floored
    at `_dropdown_row_height()` (the user's "a little shorter" request).
  - `_mount_picker_mode_switch` — **shared** orchestration for both editors (`current_mode`
    / `set_mode` / re-render), so the two can't drift on the `ui_state["library_picker_mode"]`
    contract (a `/code-review` finding — the two editors had begun to diverge). The switcher
    row sits directly below the "Changes save automatically." hint and above the existing
    single `labeled_row`, on **both** editors (extends ticket 91's lockstep by one row).
  - **Macro editor**: switcher greyed (`sensitive=lambda: step_kind_dd != Delay`), orthogonal
    to the KeyDown/KeyUp/Delay step-kind dropdown. Controller mode mounts
    `build_inline_controller_picker` + the polled-input dwell hint (92's "Editor hint text").
    Now takes `ui_state` (threaded through `build_library_content`).
  - **Stepper editor**: controller mode hides the Modifiers row (not greyed — 92 §3) and
    mounts the controller picker.
  - Each mode keeps its own draft — `new_step_value`/`new_item_value` grow a `controller_key`
    (default `BTN_SOUTH`) beside the keyboard `key`; the mode switch re-renders in place (no
    full rebuild) so neither draft is clobbered flipping back and forth.
- **`daemon_stub.py`** — mirrors the daemon: `_validate_stepper_items` (gamepad allowlist on
  `controller_button` items) on `create_stepper`/`set_stepper_items`, and closed the
  pre-existing gap where `_validate_binding_action` didn't apply the allowlist to
  `controller_button` *bindings* (a `/code-review` finding).
- **`tools/shot_library.py`** — extended with controller-mode shots for both editors; also
  stubbed `TrayIcon` so it runs where no `org.kde.StatusNotifierWatcher` is on the bus.

### No prototype, no new child ticket

92 §4 settled this — the switcher is a shipped shape dropped into a settled layout toggling
two already-live-verified pickers. Ticket 93 stayed `Blocked by: 91, 92`.

### CONTEXT.md

- **Stepper entry**: dropped "designed to extend to joystick/controller buttons later"; a
  list item is now "either a keyboard key/mouse-button … or a single controller button
  (`StepperItem::ControllerButton`, … same down/dwell/up triple …, no modifier combination)".
- **Macro entry**: added that a KeyDown/KeyUp step may target a controller button (routed to
  the gamepad device) with the ~35 ms polled-input dwell caveat.

### `/code-review` (high)

Ran; findings triaged. Acted on the in-scope ones: extracted the shared
`_mount_picker_mode_switch` (the two editors' switcher closures had diverged), closed the
stub's `controller_button`-binding allowlist gap, fixed stale `_HEADER_MIDDLE_H` comment
refs. **Not acted on** (out of scope / settled design): the `packaging/acheron-gui` `-P`
flag and the Device Overview tooltip/`max_width_chars` findings belong to tickets 90/88, not
this diff; `Binding.trigger`'s `#[serde(default)]` predates this ticket; the
"`StepperItem::Key` accepts `BTN_SOUTH` with no dwell" and "two incompatible models" points
relitigate 92 §1's deliberately-settled route (c); the three-copy allowlist guard and the
fourth segmented-switch copy match the ticket's explicit "mirror the existing two-place
enforcement" / "same shape as `build_destination_switch`" instructions and the file's
lockstep-duplication convention. One accepted deviation: flipping keyboard↔controller
in-place shifts column-3 width (the two pickers differ in width) and, across tabs in
controller mode, the body y (Macro keeps its dropdown + hint, Stepper hides Modifiers) —
92 §3 explicitly accepts this ("the row that must stay lockstep across tabs is the switcher
row, which is on both").

### Tests

- Rust: **365 pass** (+8: config parse ×2, executor helper parity, dispatch resolve +
  reject ×3, wire round-trip, real-D-Bus round-trip). clippy/fmt clean.
- Python: **313 pass** (+12: `describe_step`/`describe_stepper_item` labels, switcher on
  both editors, mode recorded in shared `ui_state`, controller item/step persistence,
  Modifiers hidden in controller mode, switcher greyed on Delay, delay round-trip with
  switcher present, draft preservation, stub allowlist, wire round-trip; 1 updated for the
  new row structure).

### Screenshot verification (this session)

`tools/shot_library.py` against the running GUI: switcher row aligns pixel-lockstep across
Macros↔Steppers in keyboard mode (hint y=264, Picker row y=295, next row y=335, picker
y=367 on both); greys correctly on a Delay step; hides the Stepper Modifiers row in
controller mode; the Macro polled-input hint renders below the gamepad diagram.
