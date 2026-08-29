Type: decide
Status: resolved

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

Settled with the user over two grilling rounds against the real code on both
sides (`library_view.py`, `key_picker.py`, `controller_picker.py`, `config.rs`,
`executor.rs`, `dispatch.rs`, `injector.rs`, `input.rs`).

### Grounding facts found before the questions

- **A gamepad code in a Macro step already works today, unvalidated.**
  `MacroStepDto::KeyDown`/`KeyUp` take any `KeyCode`; `injector::sink_for` routes
  anything `input::is_gamepad_button` to the gamepad `uinput` device.
  `CreateMacro`/`SetMacroSteps` do no per-code validation at all. So a hand-typed
  `BTN_SOUTH` KeyDown step already routes correctly — a picker here is a pure
  discoverability affordance over an existing capability.
- **`is_gamepad_button` *is* the 57-entry allowlist**, and both the injector's
  routing decision (`sink_for`) and the gamepad device's advertised capability
  set (`build_gamepad_device` → `gamepad_button_codes`) are derived from it, so
  they can't drift.
- **The keyboard `uinput` device advertises the entire `0..=0x2ff` range**
  (`build_device` → `all_injectable_key_codes`). Combined with the point above,
  **a `KeyDown`/`KeyUp` step cannot crash the injector regardless of code** — the
  code is always advertised on whichever sink it routes to. Ticket 43's allowlist
  check on `Action::ControllerButton` is a sanity guard (a controller action
  pointing at `KEY_A` is nonsense), not crash-prevention.
- **Stepper items have no gamepad path today.** `StepperItem` is
  `#[serde(tag = "type")]` with one `Key { key, modifiers }` variant;
  `resolve_step` compiles via `executor::keypress_steps` with no dwell.
- **`Action::ControllerButton`** validates in two places (`validate_binding` +
  `config::parse`) and compiles to `KeyDown / Delay(35ms) / KeyUp`
  (`executor::CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`) because a same-tick gamepad
  down+up is dropped by polled game input (ticket 74/75).
- **Ticket 91's column-3 layout**: pinned header (error label, "+ Add",
  Stepper's assignment row or Macro's `_header_middle_reserve()`, separator),
  then a `_vscrollable` body of: "Changes save automatically." hint → exactly
  one `labeled_row` (Macro: the KeyDown/KeyUp/Delay step-kind dropdown; Stepper:
  `labeled_row("Modifiers", …)`) → the "Key"/"Delay (ms)" picker row. Both
  editors built in lockstep, no hardcoded pixel heights (`_dropdown_row_height()`
  measures the running theme).

### 1. Macro-step shape — route (c): no new `MacroStepDto` variant

The Macro step editor keeps its existing `KeyDown / KeyUp / Delay` step-kind
dropdown. A **keyboard↔controller switcher** (§3) chooses which picker fills the
value slot when the step-kind is `KeyDown` or `KeyUp`; a gamepad code selected
there is stored as an ordinary `MacroStepDto::KeyDown(BTN_*)` / `KeyUp(BTN_*)`.
The injector already routes it to the gamepad device.

Rationale for (c) over the ticket's (a)/(b):

- Gamepad codes **already flow through `KeyDown`/`KeyUp`** and route correctly —
  the picker is a discoverability affordance over an existing capability, which
  the ticket itself flagged as making (a)-style flexibility "the natural fit".
- (c) gives that flexibility for free with **zero new daemon variants and zero
  new Macro wire marshalling**: a gamepad button pressed via `KeyDown` stays held
  across intervening `Delay` steps until its matching `KeyUp`, exactly like a
  keyboard key. Chords of held gamepad buttons, hold-across-steps — all work.
- The ticket-33 unbalanced-step stuck-key class already covers gamepad buttons
  via the existing force-release-on-physical-`Up` path (`fire()`'s
  `(FireOnce | HoldToRepeat, Up)` arm force-releases whatever the firing left
  down, gamepad codes included since it routes by the same `sink_for`).
- The same-tick-swallow risk in a Macro is the **user's own authored sequence**
  to manage (they insert `Delay` steps by hand). The editor surfaces a hint (§7)
  when a KeyDown/KeyUp step targets a gamepad code.

**No new validation for Macro steps** — `KeyDown`/`KeyUp` deliberately accept any
`KeyCode` (ticket 14's "target any key"), the injector can't crash on any code,
and the GUI picker only ever emits valid codes. A gamepad allowlist gate on
macro steps would contradict the "any code" design and buy nothing.

`describe_step` renders a gamepad `KeyDown`/`KeyUp` step with its existing
down/up prefix plus the `controller_picker.LABEL_BY_CODE` label, e.g.
`↓ Btn: A / South`.

### 2. Stepper-item shape — new `StepperItem::ControllerButton { button }`

- New variant alongside `Key { key, modifiers }`. **No `modifiers` field** — it
  does not exist on this variant (not "exists but ignored"); a gamepad button
  takes no modifier combination.
- `dispatch::resolve_step` compiles it to `KeyDown(button) /
  Delay(executor::CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD) / KeyUp(button)` — a
  Stepper item is always an atomic one-shot press, so it hits the same
  polled-input swallow risk as `Action::ControllerButton`'s digital path and
  needs the dwell. **Reuse the existing 35ms `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`
  constant** — same job (single-poll coverage), unlike the Analog-repeat dwell,
  which is a separately-tuned knob. `resolve_step` will need to branch on the
  `StepperItem` variant: `Key` → `executor::keypress_steps(modifiers, key)`
  (unchanged); `ControllerButton` → the down/dwell/up triple. Consider a small
  `executor` helper mirroring `compile`'s `Action::ControllerButton` arm so the
  triple isn't hand-inlined in dispatch.
- **Toggle stays disallowed** for Stepper bindings regardless (unchanged — the
  restriction is on the binding's `TriggerMode`, not the item type).
- **Allowlist validation** against `input::gamepad_button_codes()` (the 57-entry
  set) in **three places**: `Command::CreateStepper`, `Command::SetStepperItems`,
  and `config::parse` (the `load_or_seed` path) — mirroring
  `Action::ControllerButton`'s enforcement, as a sanity guard so a hand-edited
  `config.toml` with a `controller_button` item naming `KEY_A` refuses to start
  with a clear error. Add a `ConfigError` variant paralleling
  `InvalidControllerButton`.
- **Wire layer**: `wire.rs` `stepper_item_to_dict` / `stepper_item_from_dict` and
  the Python `stepper_item_to_variant` (in `daemon_stub.py` / `daemon_client.py`)
  marshal the new `button` field — **planned in from the start**, not retrofitted
  (the lesson from ticket 63's `modifiers` retrofit). The dict shape follows the
  tagged-enum convention already in use: `{"type": "controller_button",
  "button": "BTN_SOUTH"}`.
- `describe_stepper_item` renders a controller item as
  `Btn: A / South` (via `controller_picker.LABEL_BY_CODE`).

### 3. The switcher UX

- **Shape**: a plain-text two-button segmented control — "Keyboard / mouse" |
  "Controller" — the same shape as `device_overview.build_destination_switch`
  (the Grid/Library switcher the user named), **sized to `_dropdown_row_height()`**
  (a little shorter than the Grid/Library switcher's own buttons — the user's
  explicit request). Not a `Gtk.DropDown`: two options, and it matches the
  stated reference.
- **Placement**: its **own new row** in column 3's `_vscrollable` body, directly
  below the "Changes save automatically." hint and **above** the existing single
  `labeled_row` (Macro's step-kind dropdown / Stepper's Modifiers row). This
  **extends ticket 91's lockstep row structure by one row**, added to **both**
  editors so the cross-tab alignment and the "build the same widget stack"
  discipline hold. (This deviates from ticket 91 finding (a)'s "same
  single-`labeled_row` slot" suggestion — that slot is already occupied in both
  editors, so sharing it would mean two controls fighting for one row. A
  dedicated switcher row on both sides is cleaner and keeps the lockstep.)
- **Macro step-kind interaction**: the switcher is **orthogonal** to the
  step-kind dropdown (which stays `KeyDown / KeyUp / Delay`). It is **greyed
  (insensitive)** when the step-kind is `Delay` — there's no value picker to
  switch. "Controller button" is *not* a fourth step-kind entry.
- **Default mode**: keyboard/mouse (the existing behavior; controller is the new
  opt-in).
- **Memory**: session-only, in a single **shared** `ui_state` key
  (`ui_state["library_picker_mode"]`) across both editors — so working in
  controller mode stays put when moving between Steppers and Macros. Resets to
  keyboard on GUI restart; not persisted to the daemon (matches `ui_state["dest"]`).
- **In-progress value on switch**: each mode keeps its **own independent draft**
  (`new_step_value` / `new_item_value` grows a `controller_key` alongside the
  existing `key`). Switching back and forth never clobbers either draft; only the
  active mode's value is what "+ Add step" / "+ Add item" commits. Controller
  draft defaults to `BTN_SOUTH`.
- **Stepper Modifiers row in controller mode**: **hidden**, not greyed — a
  gamepad button has no modifier concept, so a greyed row is pure clutter. It
  reappears in keyboard mode. (A within-one-editor vertical shift; the row that
  must stay lockstep across tabs is the switcher row, which is on both.)

### 4. Prototype — none

The switcher is a shipped component shape (`build_destination_switch`) dropped
into ticket 91's settled, lockstep-aligned layout, toggling between two pickers
(`key_picker`, `controller_picker`) that are both already built **and**
live-hardware-verified (tickets 42/44, 43/45). Ticket 38 already prototyped the
gamepad picker itself. Nothing left that a throwaway branch would answer. Ticket
93 stays `Blocked by: 91, 92` — no new child ticket.

### CONTEXT.md follow-through (for ticket 93)

- **Stepper entry**: drop "designed to extend to joystick/controller buttons
  later" — it's done; note `StepperItem` now has a `ControllerButton` variant.
- **Macro entry**: add a sentence that a `KeyDown`/`KeyUp` step may target a
  controller button (routed to the gamepad device), with the polled-input dwell
  caveat.
- Trigger-mode and Action entries unchanged.

### Editor hint text (Macro, when a KeyDown/KeyUp step targets a gamepad code)

> Controller buttons are polled by most games once per frame — add a Delay step
> of at least 35 ms between a button's Down and Up (and before pressing it
> again) or the press may not register.
