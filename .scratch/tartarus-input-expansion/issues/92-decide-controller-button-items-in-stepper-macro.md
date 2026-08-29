Type: decide

## Question

The user wants the controller-button picker (`gui/acheron_gui/controller_picker.py`, ticket
38/43 — the gamepad-diagram picker) available when editing **Stepper items** and **Macro
steps** in the library, alongside the existing keyboard/mouse key picker
(`key_picker.build_inline_key_picker`). This means a switcher — "two buttons to switch
between them, like the Grid/Library switcher" (the user's words) — in each editor, plus the
daemon-side representation for a controller-button Stepper item and a controller-button Macro
step.

Settle at least the following before building (ticket 93). Default to `/grilling` +
`/domain-modeling`; a `/prototype` for the switcher UX is likely warranted (the "how should
it look/behave" test) — decide that in this session.

### 1. Macro-step shape for a controller button

Macro steps today are `MacroStep::KeyDown { key }` / `KeyUp { key }` / `Delay { ms }`
(`daemon/src/executor.rs` / `config.rs`), and the injector already routes any `KeyCode` to
the keyboard vs. gamepad `uinput` device by `input::is_gamepad_button` (ticket 43). Two
candidate shapes:

- **(a) A matching down/up pair** — `ControllerButtonDown { button }` / `ControllerButtonUp { button }`,
  mirroring KeyDown/KeyUp. Max flexibility (hold a gamepad button across other steps, chords
  of held buttons), consistent with how the Macro editor already works, but adds two step
  variants and the ticket-33 unbalanced-step stuck-key class now also applies to gamepad
  buttons (probably fine — ticket 33's force-release-on-physical-Up already covers it).
- **(b) A single atomic press step** — `ControllerButton { button }` that compiles to a
  down+up pair (with the ticket-76 `CONTROLLER_BUTTON_*_PULSE_HOLD` dwell between them, since
  a same-tick gamepad down+up can be missed by polled game input — ticket 74/75). Simpler,
  can't be held across steps.
- Is the existing `KeyDown`/`KeyUp` route via a hand-typed `BTN_SOUTH` etc. already reachable
  today (the key field takes any `KeyCode`)? If so, does that change the calculus (the picker
  is then a discoverability affordance over an existing capability, and (a) is the natural
  fit)?

Also: does the allowlist validation (`input::gamepad_button_codes()`'s 57-entry curated set,
enforced for `Action::ControllerButton` in `SetBinding`/`parse`) extend to the Macro-step
form, and where is it enforced (`CreateMacro`/`SetMacroSteps`/`parse`)?

### 2. Stepper-item shape

Less ambiguous — `StepperItem` is `#[serde(tag = "type")]`-tagged and CONTEXT.md's Stepper
entry already says it's "designed to extend to joystick/controller buttons later". Confirm:

- A new `StepperItem::ControllerButton { button }` variant alongside today's
  `Key { key, modifiers }`.
- `resolve_step` (dispatch.rs) compiles it — a bare down/up pair routed to the gamepad device.
  Does it get the same 35ms dwell as `Action::ControllerButton`'s Fire-once path (a Stepper
  item is always fire-once-ish), given the polled-input concern?
- Modifiers: a gamepad button takes no modifier combination — confirm the `modifiers` field
  simply doesn't exist on this variant (not "exists but ignored").
- Toggle stays disallowed for Stepper bindings regardless (unchanged).
- Allowlist validation location (`CreateStepper`/`SetStepperItems`/`parse`).
- The wire layer (`wire.rs` `stepper_item_to_dict`/`from_dict` + Python
  `stepper_item_to_variant` in `daemon_stub.py`/`daemon_client.py`) — ticket 63 already had to
  retrofit `modifiers` marshalling here; make sure this variant's `button` is planned in from
  the start.

### 3. The picker switcher UX

- "Like the Grid/Library switcher" — that's `device_overview.build_destination_switch`, a
  plain-text two-button segmented control. Does the same shape work inside the cramped column-3
  editor area, or does it need to be more compact (a `Gtk.DropDown`, a toggle, tabs)?
- Where does it sit relative to the "Key" row, the modifier checkboxes (key mode only), and —
  for Macro — the step-kind dropdown (KeyDown/KeyUp/Delay)? Is "controller button" a **third
  option in the existing Macro step-kind dropdown** instead of a separate switcher? (That
  might be cleaner for Macro and leave only the Stepper editor needing a real switcher.)
- What's the default mode (keyboard, presumably) and is the choice remembered per session
  like `ui_state["dest"]`?
- When the user switches key→controller, what happens to the in-progress value / the modifier
  checkboxes?
- This lands in the same column-3 area ticket 91 is homogenizing — coordinate: ticket 93 is
  `Blocked by: 91, 92` so the switcher is added on top of the settled layout. **Ticket 91
  resolved:** column 3's body now has exactly one `labeled_row` between the "Changes save
  automatically" hint and the "Key"/"Delay (ms)" picker row (Macro: the step-kind dropdown;
  Stepper: `labeled_row("Modifiers", …)`), sized via `_dropdown_row_height()` so both tabs'
  pickers align. The switcher should slot in as one more `labeled_row`-shaped control in
  that same region, added to **both** editors (even if visually inert on one) so the
  lockstep structure — and the cross-tab alignment — holds. If 92 instead makes "controller
  button" a third Macro step-kind (§1 option), only the Stepper editor gains a real
  switcher and the Macro side stays its plain step-kind dropdown, which also fits.
- `binding_editor.py`'s grid-view Action editor already offers "Controller Button" as its own
  Action kind (ticket 43) — this ticket does **not** change that surface; it's specifically
  about the library's Stepper-item / Macro-step editors.

### 4. Prototype or not?

Decide in this session whether the switcher UX needs a `/prototype` branch (ticket 38's
precedent for the picker itself) or whether it's a direct build on top of an already-proven
component. If prototype: spawn it as a child ticket blocking 93.

## Answer
