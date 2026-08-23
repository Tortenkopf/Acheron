from gi.repository import Gtk

from acheron_gui.daemon_client import DaemonError
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.inputs import ALL_INPUTS
from acheron_gui.library_view import (
    build_library_view,
    describe_stepper_item,
    macro_used_by_count,
    stepper_used_by_count,
)

from .widget_tree import button_labeled, find_all, find_one

_PROFILE = "Default"
_LAYER = "base"


def _row_label_text(box):
    child = box.get_first_child()
    return child.get_label() if isinstance(child, Gtk.Label) else None


def _row_labeled(root, label_text):
    return find_one(root, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == label_text)


def _reorder_buttons(root, glyph):
    # Excludes the inline key picker's own arrow keycaps ("keycap" css
    # class), which carry the same "↑"/"↓" glyphs as the step reorder
    # buttons — see key_picker._ARROW_BLOCK.
    return find_all(
        root,
        lambda w: isinstance(w, Gtk.Button) and w.get_label() == glyph and "keycap" not in w.get_css_classes(),
    )


def _build(stub, ui_state, on_change=lambda: None):
    return build_library_view(stub, stub.get_config(), _PROFILE, _LAYER, ui_state, on_change)


def _dropdown_labeled(root, label_text):
    row = _row_labeled(root, label_text)
    return find_one(row, lambda w: isinstance(w, Gtk.DropDown))


def _popover_of(menu_button: Gtk.MenuButton) -> Gtk.Widget:
    popover = menu_button.get_popover()
    assert popover is not None
    return popover


def _fill_and_submit_name_prompt(menu_button: Gtk.MenuButton, name: str, submit_label: str) -> None:
    popover = _popover_of(menu_button)
    find_one(popover, lambda w: isinstance(w, Gtk.Entry)).set_text(name)
    find_one(popover, lambda w: isinstance(w, Gtk.Button) and w.get_label() == submit_label).emit("clicked")


def test_macros_tab_is_selected_by_default():
    stub = DaemonStub()

    root = _build(stub, {})

    macros_tab = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Macros")
    assert "suggested-action" in macros_tab.get_css_classes()
    steppers_tab = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Steppers")
    assert "suggested-action" not in steppers_tab.get_css_classes()


def test_steppers_tab_shows_the_real_panel_and_no_macro_chrome():
    stub = DaemonStub()
    stub.create_macro("Test macro", [])

    root = _build(stub, {"library_tab": "steppers"})

    assert find_one(
        root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Steppers" and "heading" in w.get_css_classes()
    )
    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and "No Steppers yet" in w.get_label())
    assert find_all(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Test macro") == []


def test_clicking_the_steppers_tab_records_the_pick_and_calls_on_change():
    stub = DaemonStub()
    ui_state = {}
    changed = []
    root = _build(stub, ui_state, lambda: changed.append(1))

    find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Steppers").emit("clicked")

    assert ui_state["library_tab"] == "steppers"
    assert changed == [1]


def test_empty_macro_library_shows_a_create_prompt_and_no_editor():
    stub = DaemonStub()

    root = _build(stub, {})

    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and "No Macros yet" in w.get_label())


def test_macro_list_shows_every_macro_sorted_by_name():
    stub = DaemonStub()
    stub.create_macro("Zeta", [])
    stub.create_macro("Alpha", [])

    root = _build(stub, {})

    rows = find_all(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() in ("Zeta", "Alpha"))
    assert [r.get_label() for r in rows] == ["Alpha", "Zeta"]


def test_first_macro_is_selected_by_default_and_shows_its_editor():
    stub = DaemonStub()
    stub.create_macro("Alpha", [{"type": "key_down", "key": "KEY_A"}])

    root = _build(stub, {})

    alpha_row_btn = button_labeled(root, "Alpha")
    assert "suggested-action" in alpha_row_btn.get_css_classes()
    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and "KeyDown KEY_A" in w.get_label())


def test_clicking_a_macro_row_selects_it_for_editing():
    stub = DaemonStub()
    stub.create_macro("Alpha", [])
    stub.create_macro("Zeta", [{"type": "key_down", "key": "KEY_B"}])
    ui_state = {}

    root = _build(stub, ui_state)
    button_labeled(root, "Zeta").emit("clicked")
    assert ui_state["library_selected_macro"] is not None

    rebuilt = _build(stub, ui_state)
    assert "suggested-action" in button_labeled(rebuilt, "Zeta").get_css_classes()
    assert find_one(rebuilt, lambda w: isinstance(w, Gtk.Label) and "KeyDown KEY_B" in w.get_label())


def test_creating_a_macro_via_new_calls_create_macro_and_selects_it():
    stub = DaemonStub()
    ui_state = {}

    root = _build(stub, ui_state)
    new_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New")
    _fill_and_submit_name_prompt(new_btn, "Fresh Macro", "Create")

    assert ("create_macro", "Fresh Macro", []) in stub.calls
    (macro_id,) = [mid for mid, m in stub.get_config()["macros"].items() if m["name"] == "Fresh Macro"]
    assert ui_state["library_selected_macro"] == macro_id


def test_renaming_a_macro_via_its_popover_calls_rename_macro():
    stub = DaemonStub()
    macro_id = stub.create_macro("Old Name", [])

    root = _build(stub, {})
    rename_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "✎")
    _fill_and_submit_name_prompt(rename_btn, "New Name", "Rename")

    assert stub.get_config()["macros"][macro_id]["name"] == "New Name"
    assert ("rename_macro", macro_id, "New Name") in stub.calls


def test_delete_is_disabled_with_a_used_by_tooltip_while_referenced():
    stub = DaemonStub()
    macro_id = stub.create_macro("Test macro", [])
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "macro", "macro_id": macro_id})

    root = _build(stub, {})

    delete_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×")
    assert not delete_btn.get_sensitive()
    assert "Used by 1 Binding(s)" in delete_btn.get_tooltip_text()


def test_delete_is_enabled_and_works_once_unreferenced():
    stub = DaemonStub()
    macro_id = stub.create_macro("Test macro", [])
    ui_state = {}

    root = _build(stub, ui_state)
    delete_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×")
    assert delete_btn.get_sensitive()
    delete_btn.emit("clicked")

    assert macro_id not in stub.get_config()["macros"]
    assert ui_state["library_selected_macro"] is None


def test_macro_used_by_count_scans_base_and_held_across_profiles():
    stub = DaemonStub()
    stub.create_profile("Gaming")
    macro_id = stub.create_macro("Test macro", [])
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "macro", "macro_id": macro_id})
    stub.switch_profile("Gaming")
    stub.set_binding("grid_r1c2", "held", {"trigger": "fire_once", "type": "macro", "macro_id": macro_id})

    assert macro_used_by_count(stub.get_config(), macro_id) == 2
    assert macro_used_by_count(stub.get_config(), "nonexistent") == 0


def test_adding_a_step_calls_set_macro_steps_and_appends():
    stub = DaemonStub()
    macro_id = stub.create_macro("Test macro", [])

    root = _build(stub, {})
    dd = find_one(root, lambda w: isinstance(w, Gtk.DropDown))
    dd.set_selected(2)  # "Delay (ms)"
    ms_entry = find_one(_row_labeled(root, "Value"), lambda w: isinstance(w, Gtk.Entry))
    ms_entry.set_text("40")
    button_labeled(root, "+ Add step").emit("clicked")

    assert stub.get_config()["macros"][macro_id]["steps"] == [{"type": "delay_ms", "ms": 40}]


def test_reordering_and_removing_steps_calls_set_macro_steps():
    stub = DaemonStub()
    macro_id = stub.create_macro(
        "Test macro",
        [
            {"type": "key_down", "key": "KEY_A"},
            {"type": "key_up", "key": "KEY_A"},
        ],
    )

    root = _build(stub, {})
    _reorder_buttons(root, "↓")[0].emit("clicked")
    assert stub.get_config()["macros"][macro_id]["steps"] == [
        {"type": "key_up", "key": "KEY_A"},
        {"type": "key_down", "key": "KEY_A"},
    ]

    rebuilt = _build(stub, {})
    first_step_row = find_one(
        rebuilt, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "KeyUp KEY_A"
    ).get_parent()
    button_labeled(first_step_row, "×").emit("clicked")

    assert stub.get_config()["macros"][macro_id]["steps"] == [{"type": "key_down", "key": "KEY_A"}]


def test_first_step_up_and_last_step_down_are_disabled():
    stub = DaemonStub()
    stub.create_macro(
        "Test macro",
        [
            {"type": "key_down", "key": "KEY_A"},
            {"type": "key_up", "key": "KEY_A"},
        ],
    )

    root = _build(stub, {})
    up_buttons = _reorder_buttons(root, "↑")
    down_buttons = _reorder_buttons(root, "↓")
    assert [b.get_sensitive() for b in up_buttons] == [False, True]
    assert [b.get_sensitive() for b in down_buttons] == [True, False]


def test_set_macro_steps_failure_shows_the_error_and_does_not_call_on_change():
    class FailingStub(DaemonStub):
        def set_macro_steps(self, macro_id, steps):
            raise DaemonError("boom")

    stub = FailingStub()
    stub.create_macro("Test macro", [{"type": "key_down", "key": "KEY_A"}])
    changed = []
    root = _build(stub, {}, lambda: changed.append(1))

    # Scoped through the step's own describe_step label to reach the step
    # editor's remove button specifically — the Macro-row delete button
    # carries the same "×" label but lives in a different row.
    step_row = find_one(root, lambda w: isinstance(w, Gtk.Label) and "KeyDown KEY_A" in w.get_label()).get_parent()
    button_labeled(step_row, "×").emit("clicked")

    assert changed == []
    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "boom")


# --- Steppers (ticket 55) ---


def _build_steppers(stub, ui_state=None, on_change=lambda: None):
    ui_state = {"library_tab": "steppers", **(ui_state or {})}
    return _build(stub, ui_state, on_change)


def test_empty_stepper_library_shows_a_create_prompt_and_no_editor():
    stub = DaemonStub()

    root = _build_steppers(stub)

    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and "No Steppers yet" in w.get_label())


def test_stepper_list_shows_every_stepper_sorted_by_name():
    stub = DaemonStub()
    stub.create_stepper("Zeta", [])
    stub.create_stepper("Alpha", [])

    root = _build_steppers(stub)

    rows = find_all(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() in ("Zeta", "Alpha"))
    assert [r.get_label() for r in rows] == ["Alpha", "Zeta"]


def test_first_stepper_is_selected_by_default_and_shows_its_editor():
    stub = DaemonStub()
    stub.create_stepper("Alpha", [{"type": "key", "key": "KEY_A"}])

    root = _build_steppers(stub)

    alpha_row_btn = button_labeled(root, "Alpha")
    assert "suggested-action" in alpha_row_btn.get_css_classes()
    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "A" and w.get_hexpand())


def test_clicking_a_stepper_row_selects_it_for_editing():
    stub = DaemonStub()
    stub.create_stepper("Alpha", [])
    stub.create_stepper("Zeta", [{"type": "key", "key": "KEY_B"}])
    ui_state = {"library_tab": "steppers"}

    root = _build(stub, ui_state)
    button_labeled(root, "Zeta").emit("clicked")
    assert ui_state["library_selected_stepper"] is not None

    rebuilt = _build(stub, ui_state)
    assert "suggested-action" in button_labeled(rebuilt, "Zeta").get_css_classes()
    assert find_one(rebuilt, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "B" and w.get_hexpand())


def test_creating_a_stepper_via_new_calls_create_stepper_and_selects_it():
    stub = DaemonStub()
    ui_state = {"library_tab": "steppers"}

    root = _build(stub, ui_state)
    new_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New")
    _fill_and_submit_name_prompt(new_btn, "Fresh Wheel", "Create")

    assert ("create_stepper", "Fresh Wheel", []) in stub.calls
    (stepper_id,) = [sid for sid, s in stub.get_config()["steppers"].items() if s["name"] == "Fresh Wheel"]
    assert ui_state["library_selected_stepper"] == stepper_id


def test_renaming_a_stepper_via_its_popover_calls_rename_stepper():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Old Name", [])

    root = _build_steppers(stub)
    rename_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "✎")
    _fill_and_submit_name_prompt(rename_btn, "New Name", "Rename")

    assert stub.get_config()["steppers"][stepper_id]["name"] == "New Name"
    assert ("rename_stepper", stepper_id, "New Name") in stub.calls


def test_stepper_delete_is_disabled_with_a_used_by_tooltip_while_referenced():
    # Parity with Macro's delete gate, by direct instruction: Stepper delete
    # behaves exactly like Macro's — pre-emptively disabled with a "Used by
    # N Binding(s)" tooltip while referenced, not just relying on the
    # Daemon's own refusal to surface as a post-click error.
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"})

    root = _build_steppers(stub)

    delete_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×")
    assert not delete_btn.get_sensitive()
    assert "Used by 1 Binding(s)" in delete_btn.get_tooltip_text()


def test_stepper_used_by_count_scans_base_and_held_across_profiles():
    stub = DaemonStub()
    stub.create_profile("Gaming")
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"})
    stub.switch_profile("Gaming")
    stub.set_binding("grid_r1c2", "held", {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "backward"})

    assert stepper_used_by_count(stub.get_config(), stepper_id) == 2
    assert stepper_used_by_count(stub.get_config(), "nonexistent") == 0


def test_stepper_delete_is_enabled_and_works_once_unreferenced():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    ui_state = {"library_tab": "steppers"}

    root = _build(stub, ui_state)
    delete_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×")
    assert delete_btn.get_sensitive()
    delete_btn.emit("clicked")

    assert stepper_id not in stub.get_config()["steppers"]
    assert ui_state["library_selected_stepper"] is None


def test_adding_an_item_calls_set_stepper_items_and_appends():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])

    root = _build_steppers(stub)
    picker_row = _row_labeled(root, "New item")
    button_labeled(picker_row, "F1").emit("clicked")
    button_labeled(root, "+ Add item").emit("clicked")

    assert stub.get_config()["steppers"][stepper_id]["items"] == [
        {"type": "key", "key": "KEY_F1", "modifiers": []}
    ]


def test_adding_an_item_with_a_modifier_checked_round_trips_through_on_add_and_the_list_row():
    # Ticket 62/63: the "New item" row gains the same Ctrl/Shift/Alt/Super
    # mod_box binding_editor.py renders for Keypress — checking one must
    # reach on_add's persisted item and then describe_stepper_item's label
    # on the next render.
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])

    root = _build_steppers(stub)
    picker_row = _row_labeled(root, "New item")
    button_labeled(picker_row, "F1").emit("clicked")
    find_one(root, lambda w: isinstance(w, Gtk.CheckButton) and w.get_label() == "ctrl").set_active(True)
    button_labeled(root, "+ Add item").emit("clicked")

    assert stub.get_config()["steppers"][stepper_id]["items"] == [
        {"type": "key", "key": "KEY_F1", "modifiers": ["ctrl"]}
    ]

    rebuilt = _build_steppers(stub)
    assert find_one(rebuilt, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Ctrl+F1")


def test_describe_stepper_item_prefixes_the_modifier_combo():
    assert describe_stepper_item({"key": "KEY_3", "modifiers": ["ctrl", "shift"]}) == "Ctrl+Shift+3"


def test_describe_stepper_item_with_no_modifiers_shows_a_bare_key_label():
    assert describe_stepper_item({"key": "KEY_3", "modifiers": []}) == "3"
    assert describe_stepper_item({"key": "KEY_3"}) == "3"


def test_picking_a_bare_modifier_for_a_new_item_shows_no_modifier_warning():
    # A Stepper item always fires as a bare KeyDown/KeyUp pair (never a
    # Macro step) and Toggle is disallowed outright for a Stepper Binding,
    # so the picker's usual "use Toggle with a KeyDown-only Macro step"
    # warning would point at a workflow this construct can't support —
    # suppressed the same way the Macro editor already suppresses it for
    # its own KeyDown-only steps, just for a different reason.
    stub = DaemonStub()
    stub.create_stepper("Weapon Wheel", [])

    root = _build_steppers(stub)
    picker_row = _row_labeled(root, "New item")
    # "Ctrl" appears twice in the grid (Left/Right) — click whichever comes
    # first, same approach test_binding_editor.py's own `_pick_first_modifier`
    # uses for the identical ambiguity.
    find_all(picker_row, lambda w: isinstance(w, Gtk.Button) and "keycap-mod" in w.get_css_classes())[0].emit(
        "clicked"
    )

    assert find_all(root, lambda w: "warning" in w.get_css_classes()) == []


def test_reordering_and_removing_items_calls_set_stepper_items():
    stub = DaemonStub()
    stepper_id = stub.create_stepper(
        "Weapon Wheel",
        [
            {"type": "key", "key": "KEY_1"},
            {"type": "key", "key": "KEY_2"},
        ],
    )

    root = _build_steppers(stub)
    _reorder_buttons(root, "↓")[0].emit("clicked")
    assert stub.get_config()["steppers"][stepper_id]["items"] == [
        {"type": "key", "key": "KEY_2"},
        {"type": "key", "key": "KEY_1"},
    ]

    rebuilt = _build_steppers(stub)
    first_item_row = find_one(
        rebuilt, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "2" and w.get_hexpand()
    ).get_parent()
    button_labeled(first_item_row, "×").emit("clicked")

    assert stub.get_config()["steppers"][stepper_id]["items"] == [{"type": "key", "key": "KEY_1"}]


def test_first_item_up_and_last_item_down_are_disabled():
    stub = DaemonStub()
    stub.create_stepper(
        "Weapon Wheel",
        [
            {"type": "key", "key": "KEY_1"},
            {"type": "key", "key": "KEY_2"},
        ],
    )

    root = _build_steppers(stub)
    up_buttons = _reorder_buttons(root, "↑")
    down_buttons = _reorder_buttons(root, "↓")
    assert [b.get_sensitive() for b in up_buttons] == [False, True]
    assert [b.get_sensitive() for b in down_buttons] == [True, False]


def test_set_stepper_items_failure_shows_the_error_and_does_not_call_on_change():
    class FailingStub(DaemonStub):
        def set_stepper_items(self, stepper_id, items):
            raise DaemonError("boom")

    stub = FailingStub()
    stub.create_stepper("Weapon Wheel", [{"type": "key", "key": "KEY_1"}])
    changed = []
    root = _build_steppers(stub, on_change=lambda: changed.append(1))

    item_row = find_one(
        root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "1" and w.get_hexpand()
    ).get_parent()
    button_labeled(item_row, "×").emit("clicked")

    assert changed == []
    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "boom")


def test_assignment_row_defaults_to_unassigned_when_no_binding_exists():
    stub = DaemonStub()
    stub.create_stepper("Weapon Wheel", [])

    root = _build_steppers(stub)

    forward_dd = _dropdown_labeled(root, "Forward")
    assert forward_dd.get_model().get_string(forward_dd.get_selected()) == "— Unassigned —"
    backward_dd = _dropdown_labeled(root, "Backward")
    assert backward_dd.get_model().get_string(backward_dd.get_selected()) == "— Unassigned —"


def test_assignment_row_preselects_the_currently_bound_input():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    stub.set_binding(
        "wheel_scroll_up", "base", {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"}
    )
    stub.set_binding(
        "wheel_scroll_down", "base", {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "backward"}
    )

    root = _build_steppers(stub)

    forward_dd = _dropdown_labeled(root, "Forward")
    assert forward_dd.get_model().get_string(forward_dd.get_selected()) == "Wheel ▲"
    backward_dd = _dropdown_labeled(root, "Backward")
    assert backward_dd.get_model().get_string(backward_dd.get_selected()) == "Wheel ▼"


def test_assigning_forward_input_calls_set_binding():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])

    root = _build_steppers(stub)
    forward_dd = _dropdown_labeled(root, "Forward")
    forward_dd.set_selected(ALL_INPUTS.index("wheel_scroll_up") + 1)

    assert ("set_binding", "wheel_scroll_up", "base", {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"}) in stub.calls


def test_reassigning_the_same_stepper_moves_it_off_its_old_pair_with_no_toast():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    stub.set_binding(
        "wheel_scroll_up", "base", {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"}
    )
    ui_state = {"library_tab": "steppers"}

    root = _build(stub, ui_state)
    forward_dd = _dropdown_labeled(root, "Forward")
    forward_dd.set_selected(ALL_INPUTS.index("mode_key") + 1)

    # The Daemon's own SetBinding handling (mirrored by the stub) already
    # removes the stepper's old (forward) Binding wherever it was — no
    # explicit clear_binding call needed from this module.
    config = stub.get_config()
    assert "wheel_scroll_up" not in config["profiles"]["Default"]["base"]
    assert config["profiles"]["Default"]["base"]["mode_key"]["stepper_id"] == stepper_id
    assert "stepper_toast" not in ui_state


def test_assigning_an_input_already_used_by_another_stepper_steals_it_and_shows_toast():
    stub = DaemonStub()
    other_id = stub.create_stepper("Other Wheel", [])
    stub.set_binding(
        "wheel_scroll_up", "base", {"trigger": "fire_once", "type": "step", "stepper_id": other_id, "direction": "forward"}
    )
    stub.create_stepper("Weapon Wheel", [])
    ui_state = {"library_tab": "steppers", "library_selected_stepper": None}
    # Select "Weapon Wheel" specifically (not "Other Wheel", the alphabetically-
    # first/default-selected entry) — need to know its id first.
    (weapon_id,) = [sid for sid, s in stub.get_config()["steppers"].items() if s["name"] == "Weapon Wheel"]
    ui_state["library_selected_stepper"] = weapon_id

    root = _build(stub, ui_state)
    forward_dd = _dropdown_labeled(root, "Forward")
    forward_dd.set_selected(ALL_INPUTS.index("wheel_scroll_up") + 1)

    assert ui_state["stepper_toast"] == "Moved off 'Other Wheel' (it no longer has an assigned pair)"
    config = stub.get_config()
    assert config["profiles"]["Default"]["base"]["wheel_scroll_up"]["stepper_id"] == weapon_id

    rebuilt = _build(stub, ui_state)
    assert find_one(rebuilt, lambda w: isinstance(w, Gtk.Label) and "Moved off 'Other Wheel'" in w.get_label())
    # One-shot: popped on render, gone from ui_state and from a further rebuild.
    assert "stepper_toast" not in ui_state
    rebuilt_again = _build(stub, ui_state)
    assert find_all(rebuilt_again, lambda w: isinstance(w, Gtk.Label) and "Moved off" in w.get_label()) == []


def test_assigning_this_same_steppers_other_direction_input_clears_it_with_a_toast():
    # Forward=A, Backward=B already (a valid pair). Reassigning Forward onto
    # B (the *same* list's own Backward Input) plainly overwrites B's
    # Binding — `take_stepper_direction_elsewhere` only protects the *same*
    # direction living on two Inputs, not this case — so Backward silently
    # disappears unless this module surfaces its own toast for it.
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    stub.set_binding(
        "wheel_scroll_up", "base", {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"}
    )
    stub.set_binding(
        "wheel_scroll_down", "base", {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "backward"}
    )
    ui_state = {"library_tab": "steppers", "library_selected_stepper": stepper_id}

    root = _build(stub, ui_state)
    forward_dd = _dropdown_labeled(root, "Forward")
    forward_dd.set_selected(ALL_INPUTS.index("wheel_scroll_down") + 1)

    assert ui_state["stepper_toast"] == "Also cleared this list's own Backward assignment (it was on the same Input)"
    config = stub.get_config()
    assert config["profiles"]["Default"]["base"]["wheel_scroll_down"] == {
        "trigger": "fire_once",
        "type": "step",
        "stepper_id": stepper_id,
        "direction": "forward",
    }


def test_unassigning_an_input_clears_the_binding():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    stub.set_binding(
        "wheel_scroll_up", "base", {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"}
    )

    root = _build_steppers(stub)
    forward_dd = _dropdown_labeled(root, "Forward")
    forward_dd.set_selected(0)  # "— Unassigned —"

    assert ("clear_binding", "wheel_scroll_up", "base") in stub.calls
    assert "wheel_scroll_up" not in stub.get_config()["profiles"]["Default"]["base"]
