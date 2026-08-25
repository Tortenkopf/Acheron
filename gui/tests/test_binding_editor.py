import pytest
from gi.repository import Gtk

from acheron_gui.binding_editor import DepthTrack, action_summary, build_binding_editor, build_chord_binding_dialog
from acheron_gui.daemon_client import InvalidBindingError
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.device_overview import make_input_button
from acheron_gui.inputs import ACTION_TYPES, TRIGGER_OPTIONS

from .widget_tree import button_labeled, editor_content, find_all, find_one


def _dropdown_labeled(root, label_text):
    row = find_one(root, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == label_text)
    return find_one(row, lambda w: isinstance(w, Gtk.DropDown))


def _row_label_text(box):
    child = box.get_first_child()
    return child.get_label() if isinstance(child, Gtk.Label) else None


def _key_picker_row(root, label_text):
    return find_one(root, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == label_text)


def _pick_key(root, label_text, key_label):
    """Clicks the keycap button labeled `key_label` (e.g. "F1", "Left") in
    the picker labeled `label_text` (e.g. "Key"/"Value"). Ticket 44: the
    keyboard grid is always shown inline (no collapse/expand toggle), so
    it's reachable directly."""
    row = _key_picker_row(root, label_text)
    button_labeled(row, key_label).emit("clicked")


def _pick_first_modifier(root, label_text):
    """Every modifier keycap label ("Ctrl"/"Shift"/"Alt"/"Super") appears
    twice (Left/Right) — this clicks whichever comes first, which is enough
    to exercise the modifier-selected path."""
    row = _key_picker_row(root, label_text)
    find_all(row, lambda w: isinstance(w, Gtk.Button) and "keycap-mod" in w.get_css_classes())[0].emit("clicked")


def _has_warning(root) -> bool:
    return find_all(root, lambda w: "warning" in w.get_css_classes()) != []


def test_clicking_an_unbound_key_opens_editor_defaulted_to_fire_once_keypress():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))

    assert "empty" in btn.get_css_classes()
    popover = editor_content(btn)
    heading = find_one(popover, lambda w: "heading" in w.get_css_classes())
    assert heading.get_label() == "Default / base / 1"


def test_saving_a_keypress_binding_calls_set_binding_and_closes_popover():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    _pick_key(popover, "Key", "F1")

    button_labeled(popover, "Save").emit("clicked")

    assert stub.calls == [
        (
            "set_binding",
            "grid_r1c1",
            "base",
            {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []},
        )
    ]
    assert changed == [1]


def test_clearing_an_existing_binding_calls_clear_binding():
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1", "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []}
    )
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    button_labeled(popover, "Clear (passthrough)").emit("clicked")

    assert stub.calls[-1] == ("clear_binding", "grid_r1c1", "base")
    assert changed == [1]


def test_clearing_an_already_passthrough_input_is_a_noop_but_still_closes():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    button_labeled(popover, "Clear (passthrough)").emit("clicked")

    assert stub.calls == []
    assert changed == [1]


def test_bound_input_shows_bound_css_class_and_summary():
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": ["ctrl"]},
    )

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    assert "bound" in btn.get_css_classes()
    label = btn.get_child()
    assert "Ctrl+F1" in label.get_label()


def test_action_summary_shows_a_friendly_label_for_a_mouse_button_key():
    # Ticket 42's picker makes BTN_LEFT one click away — action_summary must
    # not show the raw wire code once it's this reachable.
    assert action_summary(
        {"trigger": "fire_once", "type": "keypress", "key": "BTN_LEFT", "modifiers": []}, "grid_r1c1", {}
    ) == "Mouse Left  [1x]"


def test_editing_targets_the_held_layer_independently_of_base():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "held", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)
    heading = find_one(popover, lambda w: "heading" in w.get_css_classes())
    assert heading.get_label() == "Default / held / 1"

    _pick_key(popover, "Key", "F1")
    button_labeled(popover, "Save").emit("clicked")

    assert stub.calls == [
        (
            "set_binding",
            "grid_r1c1",
            "held",
            {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []},
        )
    ]
    assert "grid_r1c1" not in stub.get_config()["profiles"]["Default"]["base"]


# --- Profile Switch (ticket 34) ---


def test_saving_a_profile_switch_binding_calls_set_binding_with_fire_once_and_the_chosen_target():
    stub = DaemonStub()
    stub.create_profile("Gaming")
    stub.calls.clear()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    action_dd = _dropdown_labeled(popover, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("profile_switch"))

    target_dd = _dropdown_labeled(popover, "Target Profile")
    profile_names = sorted(stub.get_config()["profiles"].keys())
    target_dd.set_selected(profile_names.index("Gaming"))

    button_labeled(popover, "Save").emit("clicked")

    assert stub.calls == [
        (
            "set_binding",
            "grid_r1c1",
            "base",
            {"trigger": "fire_once", "type": "profile_switch", "target": "Gaming"},
        )
    ]
    assert changed == [1]


def test_selecting_profile_switch_disables_and_forces_the_trigger_dropdown_to_fire_once():
    stub = DaemonStub()
    stub.create_profile("Gaming")

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    popover = editor_content(btn)

    trigger_dd = _dropdown_labeled(popover, "Trigger mode")
    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index("toggle"))

    action_dd = _dropdown_labeled(popover, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("profile_switch"))

    assert not trigger_dd.get_sensitive()
    assert TRIGGER_OPTIONS[trigger_dd.get_selected()][0] == "fire_once"


def test_profile_switch_action_summary_shows_the_target_with_no_trigger_suffix():
    assert action_summary(
        {"trigger": "fire_once", "type": "profile_switch", "target": "Gaming"}, "grid_r1c1", {}
    ) == "→ Gaming"


def test_bound_profile_switch_shows_the_target_in_the_grid_button_label():
    stub = DaemonStub()
    stub.create_profile("Gaming")
    stub.set_binding(
        "grid_r1c1", "base", {"trigger": "fire_once", "type": "profile_switch", "target": "Gaming"}
    )

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    assert "bound" in btn.get_css_classes()
    label = btn.get_child()
    assert "→ Gaming" in label.get_label()


# --- Controller Button (ticket 43) ---


def test_saving_a_controller_button_binding_calls_set_binding_with_the_chosen_button():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    action_dd = _dropdown_labeled(popover, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("controller_button"))

    button_row = find_one(popover, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == "Button")
    button_labeled(button_row, "B").emit("clicked")

    button_labeled(popover, "Save").emit("clicked")

    assert stub.calls == [
        (
            "set_binding",
            "grid_r1c1",
            "base",
            {"trigger": "fire_once", "type": "controller_button", "button": "BTN_EAST"},
        )
    ]
    assert changed == [1]


def test_controller_button_keeps_the_trigger_dropdown_selectable():
    stub = DaemonStub()

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    popover = editor_content(btn)

    action_dd = _dropdown_labeled(popover, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("controller_button"))

    trigger_dd = _dropdown_labeled(popover, "Trigger mode")
    assert trigger_dd.get_sensitive()


def test_controller_button_action_summary_shows_the_button_and_trigger():
    assert (
        action_summary(
            {"trigger": "hold_to_repeat", "type": "controller_button", "button": "BTN_SOUTH"},
            "grid_r1c1",
            {},
        )
        == "Btn: A / South  [hold]"
    )


def test_bound_controller_button_shows_the_button_in_the_grid_button_label():
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1", "base", {"trigger": "fire_once", "type": "controller_button", "button": "BTN_START"}
    )

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    assert "bound" in btn.get_css_classes()
    label = btn.get_child()
    assert "Btn: Start" in label.get_label()


# --- Actuation & release (ticket 26) ---


def test_grid_key_editor_has_an_actuation_section_seeded_from_the_profile_default():
    stub = DaemonStub()

    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    heading = find_one(editor, lambda w: "sub-heading" in w.get_css_classes())
    assert heading.get_label() == "Actuation & release"
    value_label = find_one(
        editor, lambda w: isinstance(w, Gtk.Label) and "dim" in w.get_css_classes() and "%" in w.get_label()
    )
    assert value_label.get_label() == "Actuation 50%   Release 44%"


def test_non_grid_key_editor_has_no_actuation_section():
    stub = DaemonStub()

    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "mode_key", lambda: None)

    assert find_all(editor, lambda w: "sub-heading" in w.get_css_classes()) == []


def test_reset_to_profile_default_calls_clear_actuation_point():
    stub = DaemonStub()
    stub.set_actuation_point("grid_r1c1", 200, 180)
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    button_labeled(editor, "Reset to Profile default").emit("clicked")

    assert ("clear_actuation_point", "grid_r1c1") in stub.calls


def test_set_as_profile_default_sends_the_current_markers_values():
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    button_labeled(editor, "Set as Profile default").emit("clicked")

    assert ("set_default_actuation", 128, 112) in stub.calls


def test_reset_all_keys_to_profile_default_calls_reset_actuation_points():
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    button_labeled(editor, "Reset all keys to Profile default").emit("clicked")

    assert ("reset_actuation_points",) in stub.calls


def test_set_as_profile_default_closes_the_popover_and_refreshes_the_cached_config():
    # Ticket 27's live-hardware verification caught this: every Grid key's
    # popover is pre-built once from a single `GetConfig()` snapshot, and
    # there's no Daemon signal for a `default_actuation` change (unlike
    # `capture_mode`) — so without forcing a rebuild here, a new default was
    # invisible in any freshly opened popover until the whole GUI restarted.
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    button_labeled(popover, "Set as Profile default").emit("clicked")

    assert changed == [1]


def test_reset_all_keys_to_profile_default_closes_the_popover_and_refreshes_the_cached_config():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    button_labeled(popover, "Reset all keys to Profile default").emit("clicked")

    assert changed == [1]


def test_reset_to_profile_default_does_not_close_the_popover():
    # Unlike the two above, this only ever affects the current key, whose
    # markers it already updates directly — no other popover's data goes
    # stale, so it must not force a rebuild that would tear down live
    # editing (matching the drag-driven `set_actuation_point` path, which
    # is exercised continuously and would be unusable if every drag closed
    # the popover).
    stub = DaemonStub()
    stub.set_actuation_point("grid_r1c1", 200, 180)
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    button_labeled(popover, "Reset to Profile default").emit("clicked")

    assert changed == []


def test_force_digital_checkbox_calls_set_force_digital():
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    check = find_one(
        editor,
        lambda w: isinstance(w, Gtk.CheckButton) and w.get_label() == "Force digital capture (disable analog)",
    )

    check.set_active(True)

    assert ("set_force_digital", True) in stub.calls


def test_force_digital_checkbox_seeds_from_the_persisted_preference():
    # Ticket 27's live-hardware verification caught this: the checkbox
    # always constructed unchecked regardless of the real Daemon's
    # persisted `force_digital`, because `GetConfig()` never serialized it
    # — so reopening the editor after checking it showed unchecked again.
    stub = DaemonStub()
    stub.set_force_digital(True)

    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    check = find_one(
        editor,
        lambda w: isinstance(w, Gtk.CheckButton) and w.get_label() == "Force digital capture (disable analog)",
    )

    assert check.get_active()
    # Seeding the initial state must not itself re-send an unchanged value.
    assert stub.calls == [("set_force_digital", True)]


def test_badge_reflects_the_capture_mode_passed_in():
    stub = DaemonStub()

    analog_editor = build_binding_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None, capture_mode="analog"
    )
    badge = find_one(analog_editor, lambda w: isinstance(w, Gtk.Label) and "badge" in w.get_css_classes())
    assert badge.get_label() == "analog"
    assert "badge-analog" in badge.get_css_classes()
    note = find_one(analog_editor, lambda w: "digital-note-overlay" in w.get_css_classes())
    assert not note.get_visible()

    digital_editor = build_binding_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None, capture_mode="digital"
    )
    badge = find_one(digital_editor, lambda w: isinstance(w, Gtk.Label) and "badge" in w.get_css_classes())
    assert badge.get_label() == "digital"
    assert "badge-digital" in badge.get_css_classes()
    note = find_one(digital_editor, lambda w: "digital-note-overlay" in w.get_css_classes())
    assert note.get_visible()


def test_building_the_editor_does_not_start_a_depth_stream_at_construction_time():
    """Regression guard for the leak `start_depth_stream`'s docstring
    describes: `build_binding_editor` runs eagerly for every Grid key on
    every app rebuild, so `StartDepthStream` must only fire once the
    popover is actually mapped (opened) — never at construction time, or
    every rebuild would call it for all 20 grid keys instead of just
    whichever one popover a user might have open. The map/unmap-triggered
    calls themselves aren't exercised here: forcing GTK's "map"/"unmap"
    signals on a widget with no real backing surface aborts the process in
    this headless test environment, so that half is covered by live-hardware
    verification instead (this map's standing execution discipline)."""
    stub = DaemonStub()

    build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    assert stub.calls == []


def test_depth_track_set_live_value_updates_the_fill_and_tolerates_none():
    """`DepthTrack.set_live_value` is what `start_depth_stream`'s `on_depth`
    callback drives — exercised directly here, independent of GTK's map
    lifecycle (see the test above for why that can't be forced safely)."""
    track = DepthTrack(
        markers=[{"value": 128, "css": "marker-actuation", "draggable": True}],
        on_marker_moved=lambda i, v: None,
        on_drag_end=lambda i, v: None,
    )

    track.set_live_value(200)
    assert track.live_value == 200
    assert track.fill.get_visible()

    track.set_live_value(None)
    assert track.live_value is None
    assert not track.fill.get_visible()


# --- Key/mouse-button picker (ticket 42) ---


def test_saving_a_mouse_button_binding_round_trips_the_btn_code():
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    _pick_key(editor, "Key", "Left")
    button_labeled(editor, "Save").emit("clicked")

    assert stub.calls == [
        (
            "set_binding",
            "grid_r1c1",
            "base",
            {"trigger": "fire_once", "type": "keypress", "key": "BTN_LEFT", "modifiers": []},
        )
    ]


def test_modifier_warning_shows_for_fire_once_key_and_hides_for_toggle():
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    _pick_first_modifier(editor, "Key")
    assert _has_warning(editor)

    trigger_dd = _dropdown_labeled(editor, "Trigger mode")
    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index("toggle"))
    assert not _has_warning(editor)

    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index("fire_once"))
    assert _has_warning(editor)


def test_trigger_mode_warning_wiring_survives_repeated_action_kind_switching():
    # Ticket 42: the Trigger-mode dropdown outlives every render_action_editor()
    # rebuild, so its "notify::selected" listener is disconnected and
    # reconnected on each rebuild (see build_binding_editor's _trigger_handler)
    # rather than left to accumulate — exercised here by cycling kinds
    # several times before checking the warning still responds correctly.
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    action_dd = _dropdown_labeled(editor, "Action")
    for _ in range(3):
        action_dd.set_selected([k for k, _ in ACTION_TYPES].index("macro"))
        action_dd.set_selected([k for k, _ in ACTION_TYPES].index("keypress"))

    _pick_first_modifier(editor, "Key")
    assert _has_warning(editor)

    trigger_dd = _dropdown_labeled(editor, "Trigger mode")
    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index("toggle"))
    assert not _has_warning(editor)


def test_selecting_macro_with_an_empty_library_shows_no_macros_yet_and_disables_save():
    # Ticket 52's real assignment flow: with no library entries to pick
    # from (and no "+ New Macro" submitted yet), Save must stay disabled
    # rather than send a Macro Binding with no macro_id.
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("macro"))

    assert find_one(
        editor, lambda w: isinstance(w, Gtk.Label) and "No Macros in the library yet" in w.get_label()
    )
    assert not button_labeled(editor, "Save").get_sensitive()


def test_selecting_macro_with_existing_entries_defaults_to_the_first_and_save_resends_it():
    stub = DaemonStub()
    macro_id = stub.create_macro("Screenshot Combo", [{"type": "key_down", "key": "KEY_A"}])
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("macro"))

    macro_dd = _dropdown_labeled(editor, "Macro")
    assert macro_dd.get_model().get_string(macro_dd.get_selected()) == "Screenshot Combo"

    button_labeled(editor, "Save").emit("clicked")

    assert stub.calls[-1] == (
        "set_binding",
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "macro", "macro_id": macro_id},
    )


def test_opening_an_existing_macro_binding_preselects_it_in_the_dropdown():
    stub = DaemonStub()
    stub.create_macro("Other Macro", [])
    macro_id = stub.create_macro("Test macro", [{"type": "key_down", "key": "KEY_A"}])
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "macro", "macro_id": macro_id})

    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    macro_dd = _dropdown_labeled(editor, "Macro")
    assert macro_dd.get_model().get_string(macro_dd.get_selected()) == "Test macro"
    save_btn = button_labeled(editor, "Save")
    assert save_btn.get_sensitive()

    save_btn.emit("clicked")

    assert stub.calls[-1] == (
        "set_binding",
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "macro", "macro_id": macro_id},
    )


def test_creating_a_macro_inline_via_new_macro_assigns_it_and_enables_save():
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("macro"))

    new_btn = find_one(editor, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New Macro")
    popover = new_btn.get_popover()
    find_one(popover, lambda w: isinstance(w, Gtk.Entry)).set_text("Fresh Macro")
    find_one(popover, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Create").emit("clicked")

    assert ("create_macro", "Fresh Macro", []) in stub.calls
    (macro_id,) = [mid for mid, m in stub.get_config()["macros"].items() if m["name"] == "Fresh Macro"]

    macro_dd = _dropdown_labeled(editor, "Macro")
    assert macro_dd.get_model().get_string(macro_dd.get_selected()) == "Fresh Macro"

    save_btn = button_labeled(editor, "Save")
    assert save_btn.get_sensitive()
    save_btn.emit("clicked")

    assert stub.calls[-1] == (
        "set_binding",
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "macro", "macro_id": macro_id},
    )


def test_action_summary_resolves_the_macros_display_name_not_the_raw_macro_id():
    assert (
        action_summary(
            {"trigger": "fire_once", "type": "macro", "macro_id": "screenshot-combo"},
            "grid_r1c1",
            {"screenshot-combo": {"name": "Screenshot Combo", "steps": []}},
        )
        == "Macro: Screenshot Combo  [1x]"
    )


def test_action_summary_shows_the_raw_stepper_id_when_no_steppers_dict_is_given():
    # `steppers` is optional (defaults to None) — a caller that hasn't been
    # updated to thread it in (or a Stepper missing from a stale snapshot)
    # falls back to the raw id, same as Macro's identical fallback stance.
    assert (
        action_summary(
            {"trigger": "fire_once", "type": "step", "stepper_id": "weapon-wheel", "direction": "forward"},
            "grid_r1c1",
            {},
        )
        == "Step ↑ weapon-wheel  [1x]"
    )
    assert (
        action_summary(
            {"trigger": "hold_to_repeat", "type": "step", "stepper_id": "weapon-wheel", "direction": "backward"},
            "grid_r1c1",
            {},
        )
        == "Step ↓ weapon-wheel  [hold]"
    )


def test_action_summary_resolves_the_steppers_display_name_not_the_raw_stepper_id():
    assert (
        action_summary(
            {"trigger": "fire_once", "type": "step", "stepper_id": "weapon-wheel", "direction": "forward"},
            "grid_r1c1",
            {},
            {"weapon-wheel": {"name": "Weapon Wheel", "items": []}},
        )
        == "Step ↑ Weapon Wheel  [1x]"
    )


def test_opening_an_existing_binding_with_an_unknown_action_kind_does_not_crash_and_disables_save():
    # Regression test for the general crash-guard mechanism ticket 54 added
    # (originally exercised via `Action::Step`, which ticket 55 now gives a
    # real editor to — see the Stepper-support tests below). Kept here with
    # a synthetic, never-real kind so the guard itself — opening a popover
    # for a Binding type this editor doesn't (yet) know how to render must
    # never raise `ValueError` from `ACTION_TYPES`'s `.index()` lookup —
    # stays covered against whatever the *next* net-new Action variant is.
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "future_action_kind"})

    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    save_btn = button_labeled(editor, "Save")
    assert not save_btn.get_sensitive()

    # Clear must still work normally, as an escape hatch.
    button_labeled(editor, "Clear (passthrough)").emit("clicked")
    assert stub.calls[-1] == ("clear_binding", "grid_r1c1", "base")


def test_picking_a_real_action_over_an_unsupported_binding_reenables_save():
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "future_action_kind"})

    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("controller_button"))

    save_btn = button_labeled(editor, "Save")
    assert save_btn.get_sensitive()


# --- Stepper (ticket 55) ---


def test_selecting_stepper_with_an_empty_library_shows_no_steppers_yet_and_disables_save():
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("step"))

    assert find_one(
        editor, lambda w: isinstance(w, Gtk.Label) and "No Steppers in the library yet" in w.get_label()
    )
    assert not button_labeled(editor, "Save").get_sensitive()


def test_selecting_stepper_with_existing_entries_defaults_to_the_first_and_forward_and_save_sends_it():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [{"type": "key", "key": "KEY_1"}])
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("step"))

    stepper_dd = _dropdown_labeled(editor, "Stepper")
    assert stepper_dd.get_model().get_string(stepper_dd.get_selected()) == "Weapon Wheel"
    direction_dd = _dropdown_labeled(editor, "Direction")
    assert direction_dd.get_model().get_string(direction_dd.get_selected()) == "Forward"

    button_labeled(editor, "Save").emit("clicked")

    assert stub.calls[-1] == (
        "set_binding",
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"},
    )


def test_changing_direction_for_a_step_binding_updates_the_saved_binding():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("step"))

    direction_dd = _dropdown_labeled(editor, "Direction")
    direction_dd.set_selected(1)  # Backward

    button_labeled(editor, "Save").emit("clicked")

    assert stub.calls[-1] == (
        "set_binding",
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "backward"},
    )


def test_opening_an_existing_step_binding_preselects_the_stepper_and_direction():
    stub = DaemonStub()
    stub.create_stepper("Other Wheel", [])
    stepper_id = stub.create_stepper("Weapon Wheel", [{"type": "key", "key": "KEY_1"}])
    stub.set_binding(
        "grid_r1c1", "base", {"trigger": "hold_to_repeat", "type": "step", "stepper_id": stepper_id, "direction": "backward"}
    )

    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    stepper_dd = _dropdown_labeled(editor, "Stepper")
    assert stepper_dd.get_model().get_string(stepper_dd.get_selected()) == "Weapon Wheel"
    direction_dd = _dropdown_labeled(editor, "Direction")
    assert direction_dd.get_model().get_string(direction_dd.get_selected()) == "Backward"
    save_btn = button_labeled(editor, "Save")
    assert save_btn.get_sensitive()

    save_btn.emit("clicked")

    assert stub.calls[-1] == (
        "set_binding",
        "grid_r1c1",
        "base",
        {"trigger": "hold_to_repeat", "type": "step", "stepper_id": stepper_id, "direction": "backward"},
    )


def test_creating_a_stepper_inline_via_new_stepper_assigns_it_and_enables_save():
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("step"))

    new_btn = find_one(editor, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New Stepper")
    popover = new_btn.get_popover()
    find_one(popover, lambda w: isinstance(w, Gtk.Entry)).set_text("Fresh Wheel")
    find_one(popover, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Create").emit("clicked")

    assert ("create_stepper", "Fresh Wheel", []) in stub.calls
    (stepper_id,) = [sid for sid, s in stub.get_config()["steppers"].items() if s["name"] == "Fresh Wheel"]

    stepper_dd = _dropdown_labeled(editor, "Stepper")
    assert stepper_dd.get_model().get_string(stepper_dd.get_selected()) == "Fresh Wheel"

    save_btn = button_labeled(editor, "Save")
    assert save_btn.get_sensitive()
    save_btn.emit("clicked")

    assert stub.calls[-1] == (
        "set_binding",
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"},
    )


def test_saving_a_step_binding_with_toggle_trigger_surfaces_the_daemons_rejection():
    # Toggle is disallowed for a Stepper Binding (ticket 03/54's Answer) —
    # this editor doesn't pre-emptively lock the Trigger-mode dropdown down
    # the way it does for Profile Switch (Step allows two of the three
    # options, not exactly one), so the Daemon's own rejection is relied on
    # and must surface through the ordinary error path.
    stub = DaemonStub()
    stub.create_stepper("Weapon Wheel", [])
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("step"))
    trigger_dd = _dropdown_labeled(editor, "Trigger mode")
    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index("toggle"))

    button_labeled(editor, "Save").emit("clicked")

    assert find_one(editor, lambda w: "error" in w.get_css_classes() and "Toggle" in w.get_label())


def test_chord_binding_dialog_does_not_offer_profile_switch_as_an_action():
    # A Chord's own Action can never be Profile Switch (`SetChordBinding`
    # always rejects it — see `ConfigError::InvalidChordProfileSwitch`), so
    # offering it here would be a guaranteed-failing round-trip rather than
    # a structurally-prevented one (code-review finding).
    stub = DaemonStub()
    dialog = build_chord_binding_dialog(
        stub, stub.get_config(), "Default", "base", ["grid_r1c1", "grid_r1c2"], None, lambda: None, None
    )

    action_dd = _dropdown_labeled(dialog, "Action")
    labels = [action_dd.get_model().get_string(i) for i in range(action_dd.get_model().get_n_items())]
    assert "Profile Switch" not in labels


def test_saving_an_edited_chords_grown_membership_clears_the_old_key_first():
    # Regression test: setting the new membership before clearing the old
    # one would make the still-present old key spuriously conflict with
    # itself whenever the edit grows/shrinks membership by containment
    # (e.g. {grid_r1c1, grid_r1c2} edited into {grid_r1c1, grid_r1c2,
    # mode_key} — a superset the Daemon's own subset/superset rule would
    # otherwise reject against the not-yet-cleared old Chord).
    stub = DaemonStub()
    stub.set_chord_binding(
        ["grid_r1c1", "grid_r1c2"], "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_C"}
    )
    existing = stub.get_config()["profiles"]["Default"]["chords_base"]["grid_r1c1+grid_r1c2"]

    dialog = build_chord_binding_dialog(
        stub,
        stub.get_config(),
        "Default",
        "base",
        ["grid_r1c1", "grid_r1c2", "mode_key"],
        existing,
        lambda: None,
        None,
        "grid_r1c1+grid_r1c2",
    )
    button_labeled(dialog, "Save Chord").emit("clicked")

    chords = stub.get_config()["profiles"]["Default"]["chords_base"]
    assert "grid_r1c1+grid_r1c2" not in chords
    assert any(set(key.split("+")) == {"grid_r1c1", "grid_r1c2", "mode_key"} for key in chords)


def test_saving_an_edited_chord_with_unchanged_membership_does_not_clear_it():
    stub = DaemonStub()
    stub.set_chord_binding(
        ["grid_r1c1", "grid_r1c2"], "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_C"}
    )
    existing = stub.get_config()["profiles"]["Default"]["chords_base"]["grid_r1c1+grid_r1c2"]

    dialog = build_chord_binding_dialog(
        stub,
        stub.get_config(),
        "Default",
        "base",
        ["grid_r1c2", "grid_r1c1"],  # same members, different order
        existing,
        lambda: None,
        None,
        "grid_r1c1+grid_r1c2",
    )
    button_labeled(dialog, "Save Chord").emit("clicked")

    assert "clear_chord_binding" not in [call[0] for call in stub.calls]
    assert "grid_r1c1+grid_r1c2" in stub.get_config()["profiles"]["Default"]["chords_base"]


# --- Axis assignment (ticket 71) ---


def _click_axis_target(popover, tooltip: str) -> None:
    find_one(popover, lambda w: isinstance(w, Gtk.Button) and w.get_tooltip_text() == tooltip).emit("clicked")


def test_axis_is_offered_only_for_grid_inputs():
    stub = DaemonStub()

    grid_btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    grid_popover = editor_content(grid_btn)
    action_dd = _dropdown_labeled(grid_popover, "Action")
    assert "Axis" in [action_dd.get_model().get_string(i) for i in range(action_dd.get_model().get_n_items())]

    non_grid_btn = make_input_button(stub, stub.get_config(), "Default", "base", "mode_key", lambda: None)
    non_grid_popover = editor_content(non_grid_btn)
    non_grid_action_dd = _dropdown_labeled(non_grid_popover, "Action")
    assert "Axis" not in [
        non_grid_action_dd.get_model().get_string(i) for i in range(non_grid_action_dd.get_model().get_n_items())
    ]


def test_selecting_axis_disables_the_trigger_dropdown():
    stub = DaemonStub()

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    popover = editor_content(btn)

    action_dd = _dropdown_labeled(popover, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("axis"))

    trigger_dd = _dropdown_labeled(popover, "Trigger mode")
    assert not trigger_dd.get_sensitive()


def test_saving_an_axis_assignment_calls_set_axis_assignment_not_set_binding():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    action_dd = _dropdown_labeled(popover, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("axis"))
    _click_axis_target(popover, "Left Trigger")

    button_labeled(popover, "Save").emit("clicked")

    assert stub.calls == [("set_axis_assignment", "grid_r1c1", "base", "left_trigger")]
    assert changed == [1]
    assert "grid_r1c1" not in stub.get_config()["profiles"]["Default"]["base"]


def test_save_stays_disabled_until_an_axis_target_is_picked():
    stub = DaemonStub()

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    popover = editor_content(btn)

    action_dd = _dropdown_labeled(popover, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("axis"))

    assert not button_labeled(popover, "Save").get_sensitive()


def test_save_becomes_enabled_after_picking_an_axis_target():
    # Regression test (ticket 72's live-hardware verification): Save stayed
    # disabled after picking a target in the diagram picker, because only
    # render_action_editor()'s initial pass set save_btn's sensitivity —
    # on_axis_changed updated the draft but never re-armed Save, so a real
    # click on a target button did nothing until the Action dropdown was
    # rebuilt some other way.
    stub = DaemonStub()

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    popover = editor_content(btn)

    action_dd = _dropdown_labeled(popover, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("axis"))
    assert not button_labeled(popover, "Save").get_sensitive()

    _click_axis_target(popover, "Left Trigger")

    assert button_labeled(popover, "Save").get_sensitive()


def test_opening_an_axis_assigned_key_defaults_to_axis_with_the_current_target():
    stub = DaemonStub()
    stub.set_axis_assignment("grid_r1c1", "base", "right_trigger")

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    popover = editor_content(btn)

    action_dd = _dropdown_labeled(popover, "Action")
    assert [k for k, _ in ACTION_TYPES][action_dd.get_selected()] == "axis"
    summary = find_one(popover, lambda w: "controller-picker-summary" in w.get_css_classes())
    assert summary.get_label() == "Selected: Right Trigger"


def test_clearing_an_axis_assigned_key_calls_clear_axis_assignment():
    stub = DaemonStub()
    stub.set_axis_assignment("grid_r1c1", "base", "right_trigger")
    stub.calls.clear()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    button_labeled(popover, "Clear (passthrough)").emit("clicked")

    assert stub.calls == [("clear_axis_assignment", "grid_r1c1", "base")]
    assert changed == [1]


def test_axis_action_summary_has_no_trigger_suffix():
    assert action_summary(None, "grid_r1c1", {}, axis_target="left_trigger") == "Axis: Left Trigger"


def test_axis_assigned_grid_button_shows_axis_summary_in_its_label():
    stub = DaemonStub()
    stub.set_axis_assignment("grid_r1c1", "base", "left_trigger")

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    assert "axis-stripe" in btn.get_css_classes()
    label = btn.get_child()
    assert "Axis: Left Trigger" in label.get_label()


def test_axis_assigned_grid_button_is_not_dimmed_as_empty():
    # Code-review finding: `binding` is always `None` for an Axis-assigned
    # key (mutual exclusion), so the old unconditional
    # `add_css_class("bound" if binding else "empty")` dimmed it to 0.75
    # opacity — contradicting the whole point of the always-visible stripe.
    stub = DaemonStub()
    stub.set_axis_assignment("grid_r1c1", "base", "left_trigger")

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    assert "empty" not in btn.get_css_classes()
    assert "bound" not in btn.get_css_classes()


def test_set_axis_assignment_rejects_an_unknown_target():
    stub = DaemonStub()

    with pytest.raises(InvalidBindingError):
        stub.set_axis_assignment("grid_r1c1", "base", "not_a_real_target")


def test_chord_dialog_does_not_offer_axis_as_an_action_kind():
    stub = DaemonStub()

    dialog = build_chord_binding_dialog(
        stub,
        stub.get_config(),
        "Default",
        "base",
        ["grid_r1c1", "grid_r1c2"],
        None,
        lambda: None,
        None,
    )
    action_dd = _dropdown_labeled(dialog, "Action")
    assert "Axis" not in [action_dd.get_model().get_string(i) for i in range(action_dd.get_model().get_n_items())]
