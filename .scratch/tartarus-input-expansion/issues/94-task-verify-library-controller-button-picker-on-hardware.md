Type: task

Blocked by: 93

## Question

Live-verify [ticket 93's build](./93-task-build-library-controller-button-picker.md) against
the real Daemon + Tartarus Pro + the virtual gamepad `uinput` device ("Acheron Virtual
Controller", `event*` + `js*` — ticket 43/45), and a real game or `evtest`/`jstest` where the
checklist calls for it.

Checklist (refine against tickets 92/93's Answers):

- The key↔controller switcher renders correctly in **both** the Stepper item editor and the
  Macro step editor, inside ticket 91's homogenized column-3 layout — no clipping, nothing
  shifts when toggling modes (the same inline-picker layout constraint ticket 44 hit for the
  keyboard picker).
- A **Stepper** with a controller-button item: create it, assign the Stepper to a
  Forward/Backward Input pair, and confirm stepping fires the correct gamepad button on the
  virtual controller device (cross-checked with `jstest`/`evtest` on the gamepad node, not
  the keyboard node — confirm no leakage, as ticket 45 did for `Action::ControllerButton`).
- A **Macro** with a controller-button step (in whatever form ticket 92 chose): create it,
  bind it, fire it, confirm correct gamepad output. If 92 chose a down/up pair, verify an
  unbalanced sequence is force-released on physical Up (ticket 33 path) rather than stranding
  a held gamepad button.
- Mixed Macro (keyboard step + controller-button step + delay) fires all parts correctly in
  order.
- Allowlist rejection: a hand-edited `config.toml` with an out-of-allowlist button code in a
  Stepper item / Macro step refuses to start with a specific error (mirrors ticket 45).
- The readable label ("Btn: …") shows on the library list row and anywhere the item/step is
  summarized.
- Regression spot-check: a keyboard-key Stepper item and a keyboard Macro step still work
  unchanged; the grid-view "Controller Button" Action (ticket 43) is untouched.
- If a real game is used: pick buttons the target game actually binds (ticket 77's process
  note).

Full Rust + Python suites green. Revert every throwaway Stepper/Macro/Binding and restore
`config.toml` byte-identical to its pre-session backup (map discipline).

This closes the `/wayfinder` GUI-polish cluster (tickets 89–94).

## Answer
