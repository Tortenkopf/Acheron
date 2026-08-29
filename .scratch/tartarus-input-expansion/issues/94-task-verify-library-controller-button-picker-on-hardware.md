Type: task
Status: resolved

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

Every checklist item live-verified against the real Tartarus Pro + the daily-driver
`acheron-daemon` (595a876, the installed binary + GUI both confirmed byte-identical to HEAD
before starting) + the "Acheron Virtual Controller" `uinput` device (`event27` / `js0`).
Ticket 93's build is fully hardware-verified; no `daemon` or `gui` behaviour changes were
needed. One trailing lint from ticket 93's own diff fixed (see below).

Method: drove Stepper/Macro/Binding creation over D-Bus via the GUI's own
`DBusDaemonClient` against the live daemon, in a throwaway `Wf94` profile (actuation lowered
to 26/10 so a normal press registers in Analog Capture mode); the user pressed physical grid
keys 1/2/3 on cue while `evtest` captured **both** the virtual gamepad node (`event27`) and
the virtual keyboard node (`event26`) so every fire could be checked for correct routing and
zero cross-node leakage.

### 1. Switcher renders in both editors — PASS

`gui/tools/shot_library.py` (ticket 91's harness, which already grew `steppers_controller` /
`macros_controller` shots in ticket 93) against the running GUI — had to patch a
non-unique application id into the harness for this run since the user's real GUI holds
`com.acheron.gui` (noted for tickets 95+; a one-liner, not committed).

- The "Keyboard / mouse | Controller" switcher row sits at the **same y on all four states**
  (Stepper keyboard, Stepper controller, Macro keyboard, Macro controller) — the lockstep
  row 92 §3 protects.
- Controller mode: the gamepad diagram + "Extra buttons (Trigger-Happy 1-40) ▶" collapser
  render fully, no clipping, inside the same column-3 area.
- Stepper editor: the Modifiers row is **hidden** (not greyed) in controller mode, back in
  keyboard mode — per 92 §3.
- Macro editor: the KeyDown/KeyUp/Delay step-kind dropdown stays present in controller mode
  and the polled-input dwell hint renders below the diagram.
- Accepted deviations (per 92 §3 / ticket 93): flipping keyboard↔controller shifts
  column-3 width, and across tabs *in controller mode* the body-y differs (Macro keeps its
  dropdown+hint, Stepper hides Modifiers) — the switcher row itself stays lockstep.

### 2. Stepper with a controller-button item — PASS

Stepper "WF94 Step" = `[BTN_SOUTH, BTN_EAST, KEY_A]` (a controller/keyboard mix), assigned
Forward→grid key 1, Backward→grid key 2. Four forward then two backward presses produced
**exactly** the expected sequence, every event on the right node:

| press | fired | node | dwell |
|---|---|---|---|
| fwd 1 | `BTN_EAST` (305) ↓↑ | gamepad | 36 ms |
| fwd 2 | `KEY_A` (30) ↓↑ | keyboard | ~0 (no dwell — keyboard path) |
| fwd 3 | `BTN_SOUTH` (304) ↓↑ | gamepad | 36 ms |
| fwd 4 | `BTN_EAST` ↓↑ | gamepad | 36 ms |
| bwd 1 | `BTN_SOUTH` ↓↑ | gamepad | 36 ms |
| bwd 2 | `KEY_A` ↓↑ | keyboard | ~0 |

Forward advances-then-fires, backward decrements-then-fires, both wrap. The ~35 ms
`CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD` dwell (92 §2) is present on every gamepad-item fire
and correctly absent on the keyboard item. **Zero leakage** — no `BTN_*` on the keyboard
node, no `KEY_A` on the gamepad node.

### 3. Macro with a controller-button step + unbalanced force-release — PASS

Route (c): a gamepad `KeyDown`/`KeyUp` is an ordinary `MacroStepDto` (92 §1).

- **Balanced** (`KeyDown BTN_NORTH` / `Delay 40` / `KeyUp BTN_NORTH`): `BTN_NORTH` (307) ↓→↑
  with a 42 ms gap, gamepad node only.
- **Unbalanced** (`KeyDown BTN_NORTH` only, Fire-once): pressed-and-held grid key 3 →
  `BTN_NORTH` ↓ and stayed down (`evtest --query` = pressed) for the ~11 s it was held;
  **released the instant grid key 3 was physically released** — ticket 33's
  `(FireOnce | HoldToRepeat, Up)` force-release arm covers gamepad codes (routed by the same
  `sink_for`). Not stranded; final state released.

### 4. Mixed Macro (keyboard + controller + delay) — PASS

`KeyDown KEY_H` / `KeyUp KEY_H` / `Delay 40` / `KeyDown BTN_WEST` / `Delay 40` /
`KeyUp BTN_WEST`. Timeline: `KEY_H` ↓↑ on the keyboard node, then +41 ms `BTN_WEST` (308) ↓
on the gamepad node, then +41 ms `BTN_WEST` ↑. All parts, correct order, correct nodes,
delays honoured.

### 5. Allowlist rejection — PASS (Stepper items; Macro steps intentionally unguarded)

- **D-Bus**: `SetStepperItems` and `CreateStepper` with a `controller_button` item naming
  `KEY_A` / `KEY_ENTER` both reject with
  `com.acheron.Daemon.Error.InvalidBinding: "<CODE>" is not a valid gamepad button`; the
  existing items are left unchanged (atomic).
- **Config parse**: with the daemon stopped, a hand-added
  `[[steppers.teststepper.items]] type = "controller_button" / button = "KEY_LEFTCTRL"`
  makes it **refuse to start** —
  `refusing to start: …/config.toml (config.toml contains a Stepper list item whose
  controller button "KEY_LEFTCTRL" is not a valid gamepad button)`, `status=1/FAILURE`.
  Config hard-restored + daemon restarted afterward.
- **Macro steps are deliberately not validated** (92 §1 route (c) — `KeyDown`/`KeyUp` accept
  any `KeyCode`, the injector can't crash on any code); the checklist's "Stepper item /
  Macro step" is satisfied by the Stepper-item half, which is where 93 put the guard.

### 6. Readable "Btn: …" labels — PASS

Screenshotted against the **real** daemon (`GetConfig()` round-trip), so this also confirms
the daemon's `controller_button` stepper-item wire marshalling:

- Stepper item list rows: `Btn: A / South`, `Btn: B / East` (keyboard item shows plain `A`).
- Macro step list rows: `↓ Btn: X / West`, `↑ Btn: X / West` (with the interleaved
  `KeyDown KEY_H` / `Delay 40ms` steps rendering as before).

### 7. Regression — PASS

- Keyboard Stepper item (`KEY_A` in "WF94 Step") and keyboard Macro step (`KEY_H` in the
  mixed macro) both fired exactly as before, on the keyboard node.
- Grid-view `Action::ControllerButton` (ticket 43) — after ticket 93 refactored
  `compile`'s arm to delegate to the shared `executor::controller_button_steps` — still
  works: a Toggle `BTN_TR` binding on grid key 1 pressed twice gave `BTN_TR` (311) ↓ …
  (held 3.3 s) … ↑, gamepad node only.
- Bonus: a `fire_once` grid controller-button binding is still correctly **rejected**
  (`Fire-once is not allowed for a Controller Button Binding` — ticket 78/86 restriction
  intact).

### Suites + lint

- Rust: **365 pass** (`daemon`), `cargo fmt --check` clean.
- `cargo clippy --all-targets` surfaced one `unnecessary_get_then_check` **warning
  introduced by ticket 93's own diff** (`daemon/src/dbus/wire.rs:665`, a test assertion —
  93 claimed clippy-clean; the lint is default-warn on this toolchain, 1.97.1). Fixed in
  this session: `dict.get("modifiers").is_none()` → `!dict.contains_key("modifiers")`.
  clippy now clean.
- Python: **317 pass** (`gui/.venv/bin/pytest gui/tests`).

### Cleanup

Switched back to `MnM`; deleted the `Wf94` profile, `wf94-step` stepper, `wf94-macro` macro;
hard-restored `config.toml` from the pre-session backup with the daemon stopped and
restarted it — **`config.toml` is byte-identical** (`sha256 f0c66d3d…`, `cmp` clean).
Daemon back up on `MnM`, device connected, Analog Capture mode. No stray `evtest` processes.

### Map status

This resolves the last ticket in the `/wayfinder` GUI-polish cluster (89–94). The
controller-button-in-library strand (92 → 93 → 94) is complete and hardware-verified.
