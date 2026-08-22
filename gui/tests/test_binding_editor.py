from gi.repository import Gtk

from acheron_gui.binding_editor import DepthTrack, action_summary, build_binding_editor
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
        {"trigger": "fire_once", "type": "keypress", "key": "BTN_LEFT", "modifiers": []}, "grid_r1c1"
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
        {"trigger": "fire_once", "type": "profile_switch", "target": "Gaming"}, "grid_r1c1"
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


def test_selecting_macro_on_an_unbound_key_shows_the_placeholder_and_disables_save():
    # Ticket 51: the Macro Action cutover invalidated the old inline
    # step-editing UI outright — reduced to a read-only macro_id display.
    # With no existing Macro Binding (and no picker yet to assign a fresh
    # macro_id), Save must be disabled rather than send an invalid payload.
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("macro"))

    assert find_one(editor, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Macro: (none)") is not None
    assert not button_labeled(editor, "Save").get_sensitive()


def test_opening_an_existing_macro_binding_shows_its_macro_id_read_only_and_save_resends_it():
    stub = DaemonStub()
    macro_id = stub.create_macro("Test macro", [{"type": "key_down", "key": "KEY_A"}])
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "macro", "macro_id": macro_id})

    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    assert find_one(editor, lambda w: isinstance(w, Gtk.Label) and w.get_label() == f"Macro: {macro_id}") is not None
    save_btn = button_labeled(editor, "Save")
    assert save_btn.get_sensitive()

    save_btn.emit("clicked")

    assert stub.calls[-1] == (
        "set_binding",
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "macro", "macro_id": macro_id},
    )
