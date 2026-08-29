Type: task

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
