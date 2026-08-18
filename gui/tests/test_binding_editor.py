from gi.repository import Gtk

from acheron_gui.binding_editor import DepthTrack, build_binding_editor
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.device_overview import make_input_button

from .widget_tree import button_labeled, find_all, find_one


def _entry_labeled(root, label_text):
    row = find_one(root, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == label_text)
    return find_one(row, lambda w: isinstance(w, Gtk.Entry))


def _row_label_text(box):
    child = box.get_first_child()
    return child.get_label() if isinstance(child, Gtk.Label) else None


def test_clicking_an_unbound_key_opens_editor_defaulted_to_fire_once_keypress():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))

    assert "empty" in btn.get_css_classes()
    popover = btn.get_popover()
    heading = find_one(popover, lambda w: "heading" in w.get_css_classes())
    assert heading.get_label() == "Default / base / 1"


def test_saving_a_keypress_binding_calls_set_binding_and_closes_popover():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = btn.get_popover()

    key_entry = _entry_labeled(popover, "Key")
    key_entry.set_text("KEY_F1")
    key_entry.emit("changed")

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
    popover = btn.get_popover()

    button_labeled(popover, "Clear (passthrough)").emit("clicked")

    assert stub.calls[-1] == ("clear_binding", "grid_r1c1", "base")
    assert changed == [1]


def test_clearing_an_already_passthrough_input_is_a_noop_but_still_closes():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = btn.get_popover()

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


def test_editing_targets_the_held_layer_independently_of_base():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "held", "grid_r1c1", lambda: changed.append(1))
    popover = btn.get_popover()
    heading = find_one(popover, lambda w: "heading" in w.get_css_classes())
    assert heading.get_label() == "Default / held / 1"

    key_entry = _entry_labeled(popover, "Key")
    key_entry.set_text("KEY_F1")
    key_entry.emit("changed")
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


def test_force_digital_checkbox_calls_set_force_digital():
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    check = find_one(
        editor,
        lambda w: isinstance(w, Gtk.CheckButton) and w.get_label() == "Force digital capture (disable analog)",
    )

    check.set_active(True)

    assert ("set_force_digital", True) in stub.calls


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
