Type: task
Blocked by: None — can start immediately

## Question

Build [Decide whether Stepper items should support modifier combinations](./62-decide-stepper-item-modifiers.md)'s
settled shape for real, Daemon + GUI, mirroring the split this map has used
throughout (config/dispatch first, GUI against it) but as one ticket since
the scope here is small.

Daemon (`daemon/src/config.rs`, `daemon/src/dispatch.rs`):

- `StepperItem::Key { key: KeyCode }` gains `#[serde(default)] modifiers:
  Modifiers` (config.rs:400-402) — purely additive to `config.toml`, no
  `SCHEMA_VERSION` bump, same convention as every other additive field on
  this map.
- `resolve_step` (dispatch.rs:321-349) swaps its hardcoded `vec![KeyDown(key),
  KeyUp(key)]` (lines 344-348) for a call into `executor::keypress_steps`
  (executor.rs:56) — check whether that function needs to move from private
  to `pub(crate)` to be reachable from `dispatch.rs`.
- Rust tests: extend `resolve_step`'s existing coverage with a
  modifiers-present case (assert the full mods-down/key/mods-up sequence,
  matching `compile_keypress_is_a_canned_modifier_key_sequence`'s existing
  shape in executor.rs), plus a config round-trip test confirming an omitted
  `modifiers` key in `config.toml` still parses (the `#[serde(default)]`
  path).

GUI (`gui/acheron_gui/library_view.py`):

- The Stepper editor's "New item" add-row (around `new_item_value`,
  currently just `{"key": "KEY_A"}`) gains the same Ctrl/Shift/Alt/Super
  `mod_box` checkbox block `binding_editor.py` renders for Keypress
  (lines 553-568) — port the pattern, don't re-derive it.
- `on_add`'s constructed item dict (`{"type": "key", "key": ...}`) gains a
  `"modifiers": [...]` key, sorted, same shape as a Binding's own
  `modifiers` list.
- `describe_stepper_item` (line 361-363) prefixes the modifier combo onto
  the label using the same format as `binding_editor.py`'s `action_summary`
  (`"+".join(m.capitalize() for m in modifiers)` + the key label, e.g.
  `"Ctrl+3"`), so existing list rows read correctly once an item carries
  modifiers.
- No change needed to `warn_predicate=False` (line 653) or the "New item"
  row's key_picker mount — see ticket 62's Answer for why.
- Python tests: cover an item round-tripping with modifiers through
  `on_add`/`describe_stepper_item`, and the empty-modifiers case still
  rendering a bare key label (no stray `"+"`).

Docs:

- `CONTEXT.md`'s Stepper glossary entry: update "restricted to a single
  fire-once keyboard key or mouse-button" to note the optional modifier
  combination.

No live hardware required to build (config/dispatch/GUI-widget work only,
mirrors ticket 51/54/57's own AFK precedent) — spawn a hardware-verification
follow-up ticket only if this session doesn't have live Daemon/Tartarus Pro
access to confirm a modifier-combined Stepper item actually fires correctly
end-to-end (the standing quality bar every other ticket on this map has met).
