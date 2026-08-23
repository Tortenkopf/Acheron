Type: grilling
Status: resolved

## Question

The regular Keypress key-picker (`binding_editor.py`) pairs the key field with
Ctrl/Shift/Alt/Super checkboxes, building `Action::Keypress`'s existing
`modifiers: Vec<String>` field (a "modifier combination," e.g. Ctrl+Shift+T).
The Stepper library's item picker (`library_view.py`'s `build_stepper_editor`,
via `key_picker.build_inline_key_picker`) has no such checkboxes — and
`StepperItem::Key { key }` has no modifiers field at all in the wire format.

Ticket 03 ([Design the Stepper list-stepping construct](./03-decide-stepper-list-stepping.md))
deliberately scoped a Stepper list item to "a single fire-once keyboard key or
mouse-button" (settled in CONTEXT.md's Stepper glossary entry) — atomic, no
sequence, unlike Macro (whose KeyDown/KeyUp steps already cover a modifier
combination as separate steps, so Macro has no equivalent gap).

Should `StepperItem::Key` gain an optional modifier-combination field, matching
`Action::Keypress`'s model? If yes:

- Data model: `StepperItem::Key { key, modifiers: Vec<String> }` (schema bump,
  `#[serde(default)]`)?
- Compile semantics: `dispatch::resolve_step` (ticket 54) currently compiles a
  step straight to a bare KeyDown/KeyUp pair — does firing modifiers reuse
  `Action::Keypress`'s existing modifier-compile path (mods down → key down →
  key up → mods up), atomically, given Stepper is Fire-once/Hold-to-repeat
  only (Toggle disallowed)?
- GUI: reuse `binding_editor.py`'s `mod_box` checkbox block next to the
  Stepper item's `key_picker` in `library_view.py`?
- `library_view.py:653`'s `warn_predicate=lambda: False` reasoning (the
  modifier warning is "unreachable" because a Stepper item can't hold a
  modifier) needs revisiting if this ships.

If no: is the limitation worth documenting anywhere (README, CONTEXT.md), or
is "use a Macro instead" close enough that it doesn't need a note?

## Answer

**Yes** — `StepperItem::Key` gains an optional modifier combination, matching
`Action::Keypress`. Motivated by concrete use cases, not just parity: MMORPG
hotkey-page cycling and RTS unit-group hotkeys are both commonly modifier-held
(e.g. Ctrl+1..0), which a bare-key-only Stepper item can't represent today.

Traced the actual compile path before deciding, which de-risks this
considerably from the original "architectural rework" framing:

- `Action::Keypress`'s modifiers already compile through a small, reusable,
  always-balanced function — `executor::keypress_steps(modifiers, key)`
  (daemon/src/executor.rs:56): mods-down (fixed ctrl/shift/alt/super order) →
  key-down → key-up → mods-up (reverse order).
- `resolve_step` (daemon/src/dispatch.rs:321), which fires a Stepper item,
  currently hardcodes `vec![KeyDown(key), KeyUp(key)]` at lines 344-348 — a
  direct swap to call `keypress_steps` instead once the item carries a
  `Modifiers`.
- `Modifiers` (daemon/src/config.rs:284) is already a plain
  `#[serde(default)]`-friendly struct — `StepperItem::Key { key: KeyCode,
  #[serde(default)] modifiers: Modifiers }` is purely additive to
  `config.toml`, the same pattern every other ticket on this map has used.
- Because the fire sequence is always balanced (mods-down/key/mods-up as one
  atomic unit, same as Keypress), this does **not** reopen
  [ticket 33](./33-fix-fire-once-hold-to-repeat-stuck-key.md)'s stuck-key
  class — that was specifically about *unbalanced* Macro steps, and a
  Stepper item firing through `keypress_steps` can never produce one.

Settled by precedent, not asked as open questions:

- **GUI**: the "New item" row in `library_view.py`'s Stepper editor (items
  aren't edited in place today — only added/reordered/removed) gets the same
  Ctrl/Shift/Alt/Super `mod_box` checkbox block `binding_editor.py` already
  renders for Keypress (lines 553-568).
- **List label**: `describe_stepper_item` needs to prefix the modifier combo
  the same way `binding_editor.py`'s `action_summary` already does
  (`"+".join(m.capitalize() for m in modifiers)`, e.g. `"Ctrl+3"`), so
  existing rows read correctly once an item carries modifiers.
- **`warn_predicate=False` at library_view.py:653 stays correct as-is** — that
  warning fires when the *main key itself* is a bare modifier (an unheld
  instant pulse); it's orthogonal to the new combo checkboxes and the
  reasoning doesn't change.
- **CONTEXT.md**: the Stepper glossary entry's "restricted to a single
  fire-once keyboard key or mouse-button" line needs a modifier-combination
  mention.
- A main key that duplicates one of the checked modifiers (e.g. key=
  `KEY_LEFTCTRL` + Ctrl checked) is already a known-harmless edge case per
  [ticket 02](./02-decide-mouse-button-output-and-picker.md)'s precedent
  (kernel `EV_KEY` state-dedup collapses the double down/up to one clean
  pair) — inherited for free, not a new case to handle.

Spawned [Build Stepper item modifier-combination support](./63-task-build-stepper-item-modifiers.md).
