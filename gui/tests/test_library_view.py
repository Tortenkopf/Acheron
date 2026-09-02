# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

from dataclasses import dataclass

import pytest
from gi.repository import Gtk

from acheron_gui.daemon_client import DaemonError
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.inputs import ALL_INPUTS
from acheron_gui.library_view import (
    MACRO,
    STEPPER,
    LibraryKind,
    build_library_content,
    build_library_sidebar,
    describe_macro_step,
    describe_stepper_item,
    used_by_count,
)

from .widget_tree import button_labeled, find_all, find_one, walk

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
    # Combines column 1 (build_library_sidebar) and columns 2+3
    # (build_library_content) into one tree, mirroring how
    # device_overview.build_main_view mounts them as siblings of `root` —
    # tests search across both without caring which column something is in.
    config = stub.get_config()
    root = Gtk.Box()
    root.append(build_library_sidebar(stub, config, ui_state, on_change))
    root.append(build_library_content(stub, config, _PROFILE, _LAYER, ui_state, on_change))
    return root


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


# --- Kind-agnostic pairs, parameterised over MACRO / STEPPER (ticket 13) ---
#
# Every macro/stepper test that exercises the *shared* code path — the
# browse list, the row (select/rename/delete), the selection default, the
# reference-count guard, the ↑/↓/× item rows, `persist` — is one
# parameterised test now, asserting both kinds run one code path. The
# kind-specific tests below (`describe_*`, the step-kind / modifiers /
# controller draft UI, the Stepper assignment row, the ticket-91 layout
# checks) stay verbatim.


@dataclass(frozen=True)
class _KindCase:
    """Per-kind test data. Everything derivable from the `LibraryKind`
    adapter — the config sub-dict key, the DaemonStub method names — is a
    property here rather than a re-typed constant, so the fixture and the
    adapter can't drift."""

    kind: LibraryKind
    tab: str
    a: dict  # sample item + its list-row label
    a_label: str
    b: dict
    b_label: str
    add_label: str  # "+ Add step" / "+ Add item"
    add_expected: dict  # what `_add_via_ui` appends (default KeyDown / Key, "F1")

    @property
    def _slug(self) -> str:
        return self.kind.noun.lower()  # "macro" / "stepper"

    @property
    def cfg(self) -> str:  # "macros" / "steppers"
        return self.kind.config_key

    @property
    def items(self) -> str:  # "steps" / "items"
        return self.kind.items_key

    @property
    def create(self) -> str:  # DaemonStub method + `stub.calls` tag
        return f"create_{self._slug}"

    @property
    def rename(self) -> str:
        return f"rename_{self._slug}"

    @property
    def set_call(self) -> str:  # "set_macro_steps" / "set_stepper_items"
        return f"set_{self._slug}_{self.items}"


_MACRO_CASE = _KindCase(
    kind=MACRO,
    tab="macros",
    a={"type": "key_down", "key": "KEY_A"},
    a_label="KeyDown KEY_A",
    b={"type": "key_down", "key": "KEY_B"},
    b_label="KeyDown KEY_B",
    add_label="+ Add step",
    add_expected={"type": "key_down", "key": "KEY_F1"},
)
_STEPPER_CASE = _KindCase(
    kind=STEPPER,
    tab="steppers",
    a={"type": "key", "key": "KEY_A"},
    a_label="A",
    b={"type": "key", "key": "KEY_B"},
    b_label="B",
    add_label="+ Add item",
    add_expected={"type": "key", "key": "KEY_F1", "modifiers": []},
)


@pytest.fixture(params=[_MACRO_CASE, _STEPPER_CASE], ids=["macro", "stepper"])
def case(request) -> _KindCase:
    return request.param


def _make(case: _KindCase, stub: DaemonStub, name: str = "Test", items: list | None = None) -> str:
    return getattr(stub, case.create)(name, items or [])


def _ref_binding(case: _KindCase, entry_id: str, direction: str = "forward") -> dict:
    if case.kind.binding_type == "macro":
        return {"trigger": "fire_once", "type": "macro", "macro_id": entry_id}
    return {"trigger": "fire_once", "type": "step", "stepper_id": entry_id, "direction": direction}


def _seed_references(case: _KindCase, stub: DaemonStub, entry_id: str) -> int:
    """Seed references the way each original used-by test did and return the
    count. Macro: Base + a Chord + a second Profile's Held (3). Stepper:
    one direction per keyspace — a `(list, direction)` pair is unique
    across both the per-Input and the Chord keyspace, so at most 2."""
    if case.kind.binding_type == "macro":
        stub.create_profile("Gaming")
        stub.set_binding("grid_r1c1", "base", _ref_binding(case, entry_id))
        stub.set_chord_binding(
            ["grid_r2c1", "grid_r2c2"], "base", _ref_binding(case, entry_id)
        )
        stub.switch_profile("Gaming")
        stub.set_binding("grid_r1c2", "held", _ref_binding(case, entry_id))
        return 3
    stub.set_binding("grid_r1c1", "base", _ref_binding(case, entry_id, "forward"))
    stub.set_chord_binding(
        ["grid_r2c1", "grid_r2c2"], "base", _ref_binding(case, entry_id, "backward")
    )
    return 2


def _add_via_ui(case: _KindCase, root) -> None:
    button_labeled(_row_labeled(root, "Key"), "F1").emit("clicked")
    button_labeled(root, case.add_label).emit("clicked")


def test_empty_library_shows_a_create_prompt_and_no_editor(case):
    stub = DaemonStub()

    root = _build(stub, {"library_tab": case.tab})

    assert find_one(
        root, lambda w: isinstance(w, Gtk.Label) and f"No {case.kind.noun}s yet" in w.get_label()
    )


def test_library_list_shows_every_entry_sorted_by_name(case):
    stub = DaemonStub()
    _make(case, stub, "Zeta")
    _make(case, stub, "Alpha")

    root = _build(stub, {"library_tab": case.tab})

    rows = find_all(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() in ("Zeta", "Alpha"))
    assert [r.get_label() for r in rows] == ["Alpha", "Zeta"]


def test_first_entry_is_selected_by_default_and_shows_its_editor(case):
    stub = DaemonStub()
    _make(case, stub, "Alpha", [case.a])

    root = _build(stub, {"library_tab": case.tab})

    assert "suggested-action" in button_labeled(root, "Alpha").get_css_classes()
    assert find_one(
        root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == case.a_label and w.get_hexpand()
    )


def test_clicking_a_row_selects_it_for_editing(case):
    stub = DaemonStub()
    _make(case, stub, "Alpha")
    _make(case, stub, "Zeta", [case.b])
    ui_state = {"library_tab": case.tab}

    root = _build(stub, ui_state)
    button_labeled(root, "Zeta").emit("clicked")
    assert ui_state[case.kind.selection_key] is not None

    rebuilt = _build(stub, ui_state)
    assert "suggested-action" in button_labeled(rebuilt, "Zeta").get_css_classes()
    assert find_one(
        rebuilt, lambda w: isinstance(w, Gtk.Label) and w.get_label() == case.b_label and w.get_hexpand()
    )


def test_creating_via_new_calls_create_and_selects_it(case):
    stub = DaemonStub()
    ui_state = {"library_tab": case.tab}

    root = _build(stub, ui_state)
    new_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New")
    _fill_and_submit_name_prompt(new_btn, "Fresh", "Create")

    assert (case.create, "Fresh", []) in stub.calls
    (entry_id,) = [eid for eid, e in stub.get_config()[case.cfg].items() if e["name"] == "Fresh"]
    assert ui_state[case.kind.selection_key] == entry_id


def test_renaming_via_its_popover_calls_rename(case):
    stub = DaemonStub()
    entry_id = _make(case, stub, "Old Name")

    root = _build(stub, {"library_tab": case.tab})
    rename_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "✎")
    _fill_and_submit_name_prompt(rename_btn, "New Name", "Rename")

    assert stub.get_config()[case.cfg][entry_id]["name"] == "New Name"
    assert (case.rename, entry_id, "New Name") in stub.calls


def test_delete_is_disabled_with_a_used_by_tooltip_while_referenced(case):
    stub = DaemonStub()
    entry_id = _make(case, stub, "Test")
    stub.set_binding("grid_r1c1", "base", _ref_binding(case, entry_id))

    root = _build(stub, {"library_tab": case.tab})

    delete_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×")
    assert not delete_btn.get_sensitive()
    assert "Used by 1 Binding(s)" in delete_btn.get_tooltip_text()


def test_delete_is_enabled_and_works_once_unreferenced(case):
    stub = DaemonStub()
    entry_id = _make(case, stub, "Test")
    ui_state = {"library_tab": case.tab}

    root = _build(stub, ui_state)
    delete_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×")
    assert delete_btn.get_sensitive()
    delete_btn.emit("clicked")

    assert entry_id not in stub.get_config()[case.cfg]
    assert ui_state[case.kind.selection_key] is None


def test_used_by_count_scans_base_held_and_chords_across_profiles(case):
    # Mirrors `edit.rs::macro_references` / `stepper_references` (via
    # `config::profile_all_bindings`) — Chord Bindings count too, so the
    # delete guard the real Daemon enforces and the tooltip the GUI shows
    # agree.
    stub = DaemonStub()
    entry_id = _make(case, stub, "Test")
    expected = _seed_references(case, stub, entry_id)

    assert used_by_count(stub.get_config(), case.kind, entry_id) == expected
    assert used_by_count(stub.get_config(), case.kind, "nonexistent") == 0


def test_adding_an_item_calls_the_set_call_and_appends(case):
    stub = DaemonStub()
    entry_id = _make(case, stub, "Test")

    root = _build(stub, {"library_tab": case.tab})
    _add_via_ui(case, root)

    assert stub.get_config()[case.cfg][entry_id][case.items] == [case.add_expected]


def test_reordering_and_removing_items_calls_the_set_call(case):
    stub = DaemonStub()
    entry_id = _make(case, stub, "Test", [case.a, case.b])

    root = _build(stub, {"library_tab": case.tab})
    _reorder_buttons(root, "↓")[0].emit("clicked")
    assert stub.get_config()[case.cfg][entry_id][case.items] == [case.b, case.a]

    rebuilt = _build(stub, {"library_tab": case.tab})
    first_row = find_one(
        rebuilt, lambda w: isinstance(w, Gtk.Label) and w.get_label() == case.b_label and w.get_hexpand()
    ).get_parent()
    button_labeled(first_row, "×").emit("clicked")

    assert stub.get_config()[case.cfg][entry_id][case.items] == [case.a]


def test_first_item_up_and_last_item_down_are_disabled(case):
    stub = DaemonStub()
    _make(case, stub, "Test", [case.a, case.b])

    root = _build(stub, {"library_tab": case.tab})
    up_buttons = _reorder_buttons(root, "↑")
    down_buttons = _reorder_buttons(root, "↓")
    assert [b.get_sensitive() for b in up_buttons] == [False, True]
    assert [b.get_sensitive() for b in down_buttons] == [True, False]


def test_set_call_failure_shows_the_error_and_does_not_call_on_change(case):
    class FailingStub(DaemonStub):
        pass

    def _boom(self, *_a, **_kw):
        raise DaemonError("boom")

    setattr(FailingStub, case.set_call, _boom)
    stub = FailingStub()
    entry_id = _make(case, stub, "Test", [case.a])
    changed = []
    root = _build(stub, {"library_tab": case.tab}, lambda: changed.append(1))

    # Scoped through the item's own describe label to reach the editor's
    # remove button specifically — the browse-row delete button carries the
    # same "×" label but lives in a different row.
    item_row = find_one(
        root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == case.a_label and w.get_hexpand()
    ).get_parent()
    button_labeled(item_row, "×").emit("clicked")

    assert changed == []
    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "boom")
    assert entry_id in stub.get_config()[case.cfg]


# --- Tabs (kind-specific: the tab row is the divergence's frame) ---


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

    steppers_tab = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Steppers")
    assert "suggested-action" in steppers_tab.get_css_classes()
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


# --- describe_* (kind-specific: the item shapes genuinely differ) ---


def test_describe_stepper_item_prefixes_the_modifier_combo():
    assert describe_stepper_item({"key": "KEY_3", "modifiers": ["ctrl", "shift"]}) == "Ctrl+Shift+3"


def test_describe_stepper_item_with_no_modifiers_shows_a_bare_key_label():
    assert describe_stepper_item({"key": "KEY_3", "modifiers": []}) == "3"
    assert describe_stepper_item({"key": "KEY_3"}) == "3"


def test_describe_stepper_item_renders_a_controller_button_label():
    assert (
        describe_stepper_item({"type": "controller_button", "button": "BTN_SOUTH"})
        == "Btn: A / South"
    )


def test_describe_macro_step_renders_a_gamepad_keydown_with_the_button_label():
    assert describe_macro_step({"type": "key_down", "key": "BTN_SOUTH"}) == "↓ Btn: A / South"
    assert describe_macro_step({"type": "key_up", "key": "BTN_TL"}) == "↑ Btn: LB (Left bumper)"
    # A mouse button is also `BTN_*` but isn't in the gamepad catalog — it
    # keeps the plain form.
    assert describe_macro_step({"type": "key_down", "key": "BTN_LEFT"}) == "KeyDown BTN_LEFT"
    assert describe_macro_step({"type": "key_down", "key": "KEY_A"}) == "KeyDown KEY_A"


# --- Stepper item modifier row (ticket 62/63, kind-specific) ---


def _build_steppers(stub, ui_state=None, on_change=lambda: None):
    ui_state = {"library_tab": "steppers", **(ui_state or {})}
    return _build(stub, ui_state, on_change)


def test_adding_an_item_with_a_modifier_checked_round_trips_through_on_add_and_the_list_row():
    # Ticket 62/63: the "Key" row gains the same Ctrl/Shift/Alt/Super
    # mod_box binding_editor.py renders for Keypress — checking one must
    # reach on_add's persisted item and then describe_stepper_item's label
    # on the next render.
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])

    root = _build_steppers(stub)
    picker_row = _row_labeled(root, "Key")
    button_labeled(picker_row, "F1").emit("clicked")
    find_one(root, lambda w: isinstance(w, Gtk.CheckButton) and w.get_label() == "ctrl").set_active(True)
    button_labeled(root, "+ Add item").emit("clicked")

    assert stub.get_config()["steppers"][stepper_id]["items"] == [
        {"type": "key", "key": "KEY_F1", "modifiers": ["ctrl"]}
    ]

    rebuilt = _build_steppers(stub)
    assert find_one(rebuilt, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Ctrl+F1")


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
    picker_row = _row_labeled(root, "Key")
    # "Ctrl" appears twice in the grid (Left/Right) — click whichever comes
    # first, same approach test_binding_editor.py's own `_pick_first_modifier`
    # uses for the identical ambiguity.
    find_all(picker_row, lambda w: isinstance(w, Gtk.Button) and "keycap-mod" in w.get_css_classes())[0].emit(
        "clicked"
    )

    assert find_all(root, lambda w: "warning" in w.get_css_classes()) == []


# --- Controller-button items / steps (ticket 92/93, kind-specific) ---


def _picker_switch_buttons(root):
    return find_all(
        root,
        lambda w: isinstance(w, Gtk.Button)
        and w.get_label() in ("Keyboard / mouse", "Controller"),
    )


def test_both_library_editors_carry_the_picker_switcher():
    stub = DaemonStub()
    stub.create_macro("M", [])
    stub.create_stepper("S", [])

    macro_root = _build(stub, {"library_tab": "macros"})
    stepper_root = _build(stub, {"library_tab": "steppers"})

    assert {b.get_label() for b in _picker_switch_buttons(macro_root)} == {
        "Keyboard / mouse",
        "Controller",
    }
    assert {b.get_label() for b in _picker_switch_buttons(stepper_root)} == {
        "Keyboard / mouse",
        "Controller",
    }


def test_clicking_controller_on_the_switcher_records_the_shared_mode():
    stub = DaemonStub()
    stub.create_stepper("Weapon Wheel", [])
    ui_state = {"library_tab": "steppers"}

    root = _build(stub, ui_state)
    button_labeled(root, "Controller").emit("clicked")

    assert ui_state["library_picker_mode"] == "controller"


def test_adding_a_controller_button_stepper_item_persists_the_controller_button_shape():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    ui_state = {"library_tab": "steppers"}

    root = _build(stub, ui_state)
    button_labeled(root, "Controller").emit("clicked")

    rebuilt = _build(stub, ui_state)
    button_row = _row_labeled(rebuilt, "Button")
    button_labeled(button_row, "B").emit("clicked")
    button_labeled(rebuilt, "+ Add item").emit("clicked")

    assert stub.get_config()["steppers"][stepper_id]["items"] == [
        {"type": "controller_button", "button": "BTN_EAST"}
    ]

    final = _build(stub, ui_state)
    assert find_one(final, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Btn: B / East")


def test_stepper_modifiers_row_is_hidden_in_controller_mode():
    stub = DaemonStub()
    stub.create_stepper("Weapon Wheel", [])

    keyboard_root = _build(stub, {"library_tab": "steppers"})
    assert _row_labeled(keyboard_root, "Modifiers").get_visible()

    controller_root = _build(
        stub, {"library_tab": "steppers", "library_picker_mode": "controller"}
    )
    assert not _row_labeled(controller_root, "Modifiers").get_visible()
    assert _row_labeled(controller_root, "Button") is not None
    assert find_all(
        controller_root, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == "Key"
    ) == []


def test_adding_a_controller_keydown_macro_step_stores_the_gamepad_code():
    stub = DaemonStub()
    macro_id = stub.create_macro("Combo", [])
    ui_state = {"library_tab": "macros", "library_picker_mode": "controller"}

    root = _build(stub, ui_state)
    button_row = _row_labeled(root, "Button")
    button_labeled(button_row, "A").emit("clicked")
    button_labeled(root, "+ Add step").emit("clicked")

    assert stub.get_config()["macros"][macro_id]["steps"] == [
        {"type": "key_down", "key": "BTN_SOUTH"}
    ]


def test_macro_switcher_is_insensitive_when_the_step_kind_is_delay():
    stub = DaemonStub()
    stub.create_macro("Combo", [])

    root = _build(stub, {"library_tab": "macros"})
    step_kind_dd = _dropdown_labeled(root, "New step")

    def switch_box():
        row = _row_labeled(root, "Picker")
        child = row.get_first_child().get_next_sibling()  # past the "Picker" label
        return child

    assert switch_box().get_sensitive()

    step_kind_dd.set_selected(2)  # Delay (ms)
    assert not switch_box().get_sensitive()

    step_kind_dd.set_selected(0)  # back to KeyDown
    assert switch_box().get_sensitive()


def test_macro_delay_step_still_round_trips_with_the_switcher_present():
    stub = DaemonStub()
    macro_id = stub.create_macro("Combo", [])

    root = _build(stub, {"library_tab": "macros"})
    _dropdown_labeled(root, "New step").set_selected(2)
    delay_entry = find_one(_row_labeled(root, "Delay (ms)"), lambda w: isinstance(w, Gtk.Entry))
    delay_entry.set_text("120")
    button_labeled(root, "+ Add step").emit("clicked")

    assert stub.get_config()["macros"][macro_id]["steps"] == [{"type": "delay_ms", "ms": 120}]


def test_switching_modes_back_and_forth_preserves_the_keyboard_draft():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    ui_state = {"library_tab": "steppers"}

    root = _build(stub, ui_state)
    button_labeled(_row_labeled(root, "Key"), "F1").emit("clicked")
    # Flip to controller (in place, no rebuild), touch its picker, flip back.
    button_labeled(root, "Controller").emit("clicked")
    button_labeled(_row_labeled(root, "Button"), "B").emit("clicked")
    button_labeled(root, "Keyboard / mouse").emit("clicked")
    button_labeled(root, "+ Add item").emit("clicked")

    assert stub.get_config()["steppers"][stepper_id]["items"] == [
        {"type": "key", "key": "KEY_F1", "modifiers": []}
    ]


def test_switching_modes_back_and_forth_preserves_the_controller_draft():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    ui_state = {"library_tab": "steppers"}

    root = _build(stub, ui_state)
    button_labeled(root, "Controller").emit("clicked")
    button_labeled(_row_labeled(root, "Button"), "B").emit("clicked")
    button_labeled(root, "Keyboard / mouse").emit("clicked")
    button_labeled(_row_labeled(root, "Key"), "F1").emit("clicked")
    button_labeled(root, "Controller").emit("clicked")
    button_labeled(root, "+ Add item").emit("clicked")

    assert stub.get_config()["steppers"][stepper_id]["items"] == [
        {"type": "controller_button", "button": "BTN_EAST"}
    ]


# --- Stepper assignment row (ticket 31 round 2, kind-specific) ---


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
    # direction living on two Inputs — so Backward silently disappears
    # unless this module surfaces its own toast for it.
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


# --- Ticket 91: the Stepper and Macro library editors are built to
# identical measurements, and their key fields all read "Key" ---


def test_macro_keydown_step_value_row_is_labeled_key():
    stub = DaemonStub()
    stub.create_macro("M", [])

    root = _build(stub, {})  # macros tab, default step kind is KeyDown

    assert _row_labeled(root, "Key") is not None
    assert find_all(root, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == "Value") == []


def test_macro_delay_step_value_row_is_labeled_delay_ms():
    stub = DaemonStub()
    stub.create_macro("M", [])

    root = _build(stub, {})
    find_one(root, lambda w: isinstance(w, Gtk.DropDown)).set_selected(2)  # Delay

    assert _row_labeled(root, "Delay (ms)") is not None
    assert find_all(root, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == "Value") == []


def test_stepper_item_picker_row_is_labeled_key():
    stub = DaemonStub()
    stub.create_stepper("Weapon Wheel", [])

    root = _build_steppers(stub)

    assert _row_labeled(root, "Key") is not None
    assert find_all(root, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == "New item") == []


def test_stepper_modifier_checkboxes_render_above_the_key_picker_row():
    # Ticket 91 #3: the key picker owns a tall on-screen grid that used to
    # push the modifier checkboxes past the default window height — they now
    # sit above the "Key" row (with ticket 92's switcher row above them
    # again).
    stub = DaemonStub()
    stub.create_stepper("Weapon Wheel", [])

    root = _build_steppers(stub)
    ordered = list(walk(root))
    mod_row = _row_labeled(root, "Modifiers")
    key_row = _row_labeled(root, "Key")
    assert mod_row is not None and key_row is not None
    assert ordered.index(mod_row) < ordered.index(key_row)
    assert find_one(
        mod_row, lambda w: isinstance(w, Gtk.CheckButton) and w.get_label() == "ctrl"
    ) is not None


def test_editor_columns_mount_with_col3_at_its_natural_width_on_both_tabs():
    # Column 3 (the add-controls + picker) holds the same natural width on
    # both tabs; column 2 (name + list) absorbs the slack, so nothing shifts
    # horizontally when flipping tabs.
    stub = DaemonStub()
    stub.create_macro("M", [{"type": "key_down", "key": "KEY_A"}])
    stub.create_stepper("S", [{"type": "key", "key": "KEY_A"}])

    for tab in ("macros", "steppers"):
        content = build_library_content(stub, stub.get_config(), _PROFILE, _LAYER, {"library_tab": tab}, lambda: None)
        col2, col3 = content.get_first_child(), content.get_first_child().get_next_sibling()
        assert col2.get_hexpand() is True
        assert col3.get_hexpand() is False
    assert "wheel_scroll_up" not in stub.get_config()["profiles"]["Default"]["base"]
