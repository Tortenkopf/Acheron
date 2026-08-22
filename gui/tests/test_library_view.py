from gi.repository import Gtk

from acheron_gui.daemon_client import DaemonError
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.library_view import build_library_view, macro_used_by_count

from .widget_tree import button_labeled, find_all, find_one


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


def _build(stub, ui_state):
    return build_library_view(stub, stub.get_config(), ui_state, lambda: None)


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


def test_steppers_tab_shows_the_ticket_55_stub_and_no_macro_chrome():
    stub = DaemonStub()
    stub.create_macro("Test macro", [])

    root = _build(stub, {"library_tab": "steppers"})

    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and "ticket 55" in w.get_label().lower())
    assert find_all(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Test macro") == []


def test_clicking_the_steppers_tab_records_the_pick_and_calls_on_change():
    stub = DaemonStub()
    ui_state = {}
    changed = []
    root = build_library_view(stub, stub.get_config(), ui_state, lambda: changed.append(1))

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
    root = build_library_view(stub, stub.get_config(), {}, lambda: changed.append(1))

    # Scoped through the step's own describe_step label to reach the step
    # editor's remove button specifically — the Macro-row delete button
    # carries the same "×" label but lives in a different row.
    step_row = find_one(root, lambda w: isinstance(w, Gtk.Label) and "KeyDown KEY_A" in w.get_label()).get_parent()
    button_labeled(step_row, "×").emit("clicked")

    assert changed == []
    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "boom")
