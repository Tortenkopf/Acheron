Type: task
Status: resolved

## Question

Live-verify [Build numpad support in the key picker](./65-task-build-numpad-key-picker.md) against the real, connected Tartarus Pro — the build session had no physical access to the device, mirroring the ticket 42→44/43→45/48→49/52→53/55→56 build/verify split this map has used throughout.

Checklist (same bar ticket 44 used for the rest of the catalog):

- Open the real GUI's Binding editor (or a Macro step / Stepper item), click "Show Numpad ▸", and confirm the 4x4 grid renders as designed — `Num +` and `Num Enter` visibly spanning two rows, `Num 0` spanning two columns, no layout glitches.
- Bind a grid key to `KEY_KP7` (or similar digit), one operator (`KEY_KPPLUS`), and `KEY_KPENTER`; confirm each persists correctly in `config.toml` and round-trips through D-Bus back into the GUI on reopen.
- Press the physical grid key for each of the three bindings above and confirm the simulated output is the *numpad* keycode specifically (distinguishable from its main-block twin in a tool like `evtest`/`xev` if needed), not the wrong code.
- Confirm "Hide Numpad ▾" and the independent F13-F24 toggle don't interfere with each other in the live GUI.

No new design questions expected — if the real device surfaces something ticket 64/65 didn't anticipate, record it here rather than re-opening 64.

## Answer

- Numpad displays normally in the key picker for both macros/steppers and for keybindings.
- Numpad key can be bound and behave as described.
- Pressing the key produces the correct keycodes.
- Both toggles in the keypicker do not interefere.
