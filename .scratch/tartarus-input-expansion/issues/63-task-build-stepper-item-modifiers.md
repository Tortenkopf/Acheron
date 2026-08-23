Type: task
Blocked by: None — can start immediately
Status: resolved

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

## Answer

Built exactly as scoped, AFK (no hardware needed — config/dispatch/GUI-widget
work only, per this ticket's own precedent).

**Daemon**: `StepperItem::Key` gained `#[serde(default)] modifiers:
Modifiers` (`daemon/src/config.rs`). `resolve_step`
(`daemon/src/dispatch.rs`) now destructures `{ key, modifiers }` and calls
`executor::keypress_steps(modifiers, key)` instead of hardcoding a bare
KeyDown/KeyUp pair — required promoting `keypress_steps` from private to
`pub(crate)` in `executor.rs`. New direct unit test
`resolve_step_with_modifiers_compiles_the_canned_mods_down_key_up_sequence`
asserts the full mods-down/key/mods-up sequence, mirroring
`compile_keypress_is_a_canned_modifier_key_sequence`'s shape; two new
`config.rs` round-trip tests cover an omitted `modifiers` key (still
defaults) and a present one (`{ ctrl = true }`) parsing correctly.

**D-Bus wire** (not called out explicitly in this ticket's plan, but load-
bearing — found while building): `stepper_item_to_dict`/
`stepper_item_from_dict` in `daemon/src/dbus/wire.rs` only marshaled `key`,
with no path for `modifiers` at all. Mirrored `action_to_dict`/
`action_from_dict`'s existing Keypress convention exactly (reusing the
already-shared `modifiers_to_vec`/`modifiers_from_slice` helpers): an empty
modifier list is omitted from the wire dict entirely, a present one round-
trips as `Vec<String>`. The Python-side counterpart
(`gui/acheron_gui/wire.py`'s `stepper_item_to_variant`) had the identical
gap and got the identical fix, mirroring `action_to_variant`. Without this,
the GUI's new checkboxes would have updated local state that silently never
reached the Daemon.

**GUI** (`gui/acheron_gui/library_view.py`): the Stepper editor's "New item"
row gained the same Ctrl/Shift/Alt/Super `mod_box` checkbox block
`binding_editor.py` renders for Keypress, ported not re-derived.
`new_item_value` gained a `"modifiers": []` key; `on_add`'s constructed item
dict now includes `"modifiers": sorted([...])`. `describe_stepper_item`
prefixes the modifier combo using `action_summary`'s exact format
(`"+".join(m.capitalize() for m in modifiers)`). `warn_predicate=False`
(line ~653) and the key_picker mount were left untouched, confirming ticket
62's Answer that the reasoning doesn't change.

**Tests**: `daemon/src/config.rs` (+2 round-trip tests),
`daemon/src/dispatch.rs` (+1 direct `resolve_step` unit test),
`gui/tests/test_wire.py` (+2, mirroring the Keypress modifiers-omitted/
present tests), `gui/tests/test_library_view.py` (+3: on_add round-trips a
checked modifier through to the persisted item and the next render's list-
row label; `describe_stepper_item` tested directly for both the modifier-
prefixed and bare-key-label cases; the pre-existing
`test_adding_an_item_calls_set_stepper_items_and_appends` updated for the
new always-present `"modifiers": []` key). CONTEXT.md's Stepper glossary
entry updated. 261 Rust + 240 Python tests pass; `cargo fmt --check` and
`cargo clippy --all-targets` both clean.
