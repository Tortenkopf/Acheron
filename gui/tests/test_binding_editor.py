# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

import pytest
from gi.repository import Gtk

from acheron_gui.binding_editor import (
    _ANALOG_REPEAT_HINT,
    DepthTrack,
    action_summary,
    build_binding_editor,
    build_chord_binding_dialog,
)
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


def test_clicking_an_unbound_key_opens_editor_defaulted_to_hold_to_repeat_keypress():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))

    assert "empty" in btn.get_css_classes()
    popover = editor_content(btn)
    heading = find_one(popover, lambda w: "heading" in w.get_css_classes())
    assert heading.get_label() == "Default / base / 1"

    # Ticket 89: a freshly-created Binding starts on Hold-to-repeat, not Fire-once.
    trigger_dd = _dropdown_labeled(popover, "Trigger mode")
    assert TRIGGER_OPTIONS[trigger_dd.get_selected()][0] == "hold_to_repeat"


def test_clicking_an_unbound_scroll_wheel_direction_defaults_to_fire_once():
    # Ticket 89: the scroll wheel's two directions are the carve-out — the
    # wheel fires once per physical detent, so Hold-to-repeat would machine-gun.
    stub = DaemonStub()

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "wheel_scroll_up", lambda: None)
    popover = editor_content(btn)

    trigger_dd = _dropdown_labeled(popover, "Trigger mode")
    assert TRIGGER_OPTIONS[trigger_dd.get_selected()][0] == "fire_once"


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
            # Ticket 89: new-binding default is Hold-to-repeat.
            {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_F1", "modifiers": []},
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

    button_labeled(popover, "Clear Binding").emit("clicked")

    assert stub.calls[-1] == ("clear_binding", "grid_r1c1", "base")
    assert changed == [1]


def test_clearing_an_already_passthrough_input_is_a_noop_but_still_closes():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    button_labeled(popover, "Clear Binding").emit("clicked")

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
            {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_F1", "modifiers": []},
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
            # Ticket 78: Fire-once is excluded once the Action-kind becomes
            # Controller Button, so switching to it from the default
            # fire_once Keypress falls back to Hold-to-repeat rather than
            # keeping an option no longer offered.
            {"trigger": "hold_to_repeat", "type": "controller_button", "button": "BTN_EAST"},
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
        "grid_r1c1", "base", {"trigger": "hold_to_repeat", "type": "controller_button", "button": "BTN_START"}
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
            {"trigger": "hold_to_repeat", "type": "keypress", "key": "BTN_LEFT", "modifiers": []},
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


def test_macro_binding_editor_does_not_carry_the_macro_editor_disclaimer():
    # Output-safety spec §2 / §3 / tickets 02 & 03: the standing "use macros
    # with caution" line and the "About macro safety" expander belong where a
    # Macro is *written* (the library editor), not where it is *assigned* —
    # neither must leak into binding_editor.py.
    stub = DaemonStub()
    stub.create_macro("Screenshot Combo", [{"type": "key_down", "key": "KEY_A"}])
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    _dropdown_labeled(editor, "Action").set_selected([k for k, _ in ACTION_TYPES].index("macro"))

    assert find_all(editor, lambda w: isinstance(w, Gtk.Label) and w.get_label().startswith("⚠️")) == []
    assert find_all(editor, lambda w: isinstance(w, Gtk.Expander)) == []


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
        # Ticket 89: fresh editor → Hold-to-repeat default.
        {"trigger": "hold_to_repeat", "type": "macro", "macro_id": macro_id},
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
        # Ticket 89: fresh editor → Hold-to-repeat default.
        {"trigger": "hold_to_repeat", "type": "macro", "macro_id": macro_id},
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
    button_labeled(editor, "Clear Binding").emit("clicked")
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
        # Ticket 89: fresh editor → Hold-to-repeat default.
        {"trigger": "hold_to_repeat", "type": "step", "stepper_id": stepper_id, "direction": "forward"},
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
        # Ticket 89: fresh editor → Hold-to-repeat default.
        {"trigger": "hold_to_repeat", "type": "step", "stepper_id": stepper_id, "direction": "backward"},
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
        # Ticket 89: fresh editor → Hold-to-repeat default.
        {"trigger": "hold_to_repeat", "type": "step", "stepper_id": stepper_id, "direction": "forward"},
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
    assert "Switch Profile" not in labels  # ticket 89: display label


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


def test_analog_repeat_is_offered_only_for_grid_inputs():
    # Ticket 20/39: mirrors `test_axis_is_offered_only_for_grid_inputs`
    # above, but on the Trigger-mode dropdown rather than Action — only a
    # Grid Input has Depth for the rate curve to read.
    stub = DaemonStub()

    grid_btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    grid_popover = editor_content(grid_btn)
    trigger_dd = _dropdown_labeled(grid_popover, "Trigger mode")
    assert "Analog-repeat" in [
        trigger_dd.get_model().get_string(i) for i in range(trigger_dd.get_model().get_n_items())
    ]

    non_grid_btn = make_input_button(stub, stub.get_config(), "Default", "base", "mode_key", lambda: None)
    non_grid_popover = editor_content(non_grid_btn)
    non_grid_trigger_dd = _dropdown_labeled(non_grid_popover, "Trigger mode")
    assert "Analog-repeat" not in [
        non_grid_trigger_dd.get_model().get_string(i) for i in range(non_grid_trigger_dd.get_model().get_n_items())
    ]


def test_fire_once_is_offered_only_when_action_kind_is_not_controller_button():
    # Ticket 78: Fire-once is locked out for Controller Button (Hold-to-
    # repeat's sustained-hold behavior already covers a quick tap) — unlike
    # Analog-repeat's exclusion above (fixed for an Input's whole grid-ness),
    # this is kind-gated and must react live as the Action dropdown changes.
    stub = DaemonStub()

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    popover = editor_content(btn)
    trigger_dd = _dropdown_labeled(popover, "Trigger mode")
    assert "Fire-once" in [
        trigger_dd.get_model().get_string(i) for i in range(trigger_dd.get_model().get_n_items())
    ]

    action_dd = _dropdown_labeled(popover, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("controller_button"))
    assert "Fire-once" not in [
        trigger_dd.get_model().get_string(i) for i in range(trigger_dd.get_model().get_n_items())
    ]

    # Switching back away from Controller Button restores it.
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("keypress"))
    assert "Fire-once" in [
        trigger_dd.get_model().get_string(i) for i in range(trigger_dd.get_model().get_n_items())
    ]


def test_chord_binding_dialog_does_not_offer_analog_repeat_as_a_trigger():
    # A Chord fires on a discrete member-set completion, not a single grid
    # key's continuous Depth (`SetChordBinding` always rejects it — see
    # `ConfigError::InvalidChordAnalogRepeat`), same reasoning as the
    # Profile Switch Action exclusion above.
    stub = DaemonStub()
    dialog = build_chord_binding_dialog(
        stub, stub.get_config(), "Default", "base", ["grid_r1c1", "grid_r1c2"], None, lambda: None, None
    )

    trigger_dd = _dropdown_labeled(dialog, "Trigger mode")
    labels = [trigger_dd.get_model().get_string(i) for i in range(trigger_dd.get_model().get_n_items())]
    assert "Analog-repeat" not in labels


def test_saving_an_analog_repeat_binding_on_a_grid_input_calls_set_binding():
    stub = DaemonStub()

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    popover = editor_content(btn)
    _pick_key(popover, "Key", "F1")
    trigger_dd = _dropdown_labeled(popover, "Trigger mode")
    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index("analog_repeat"))

    button_labeled(popover, "Save").emit("clicked")

    assert stub.calls == [
        (
            "set_binding",
            "grid_r1c1",
            "base",
            {"trigger": "analog_repeat", "type": "keypress", "key": "KEY_F1", "modifiers": []},
        )
    ]


# --- Analog-repeat selection hint (output-safety spec §4 / ticket 04) ---


def _analog_hint_labels(root):
    return find_all(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == _ANALOG_REPEAT_HINT)


def _select_trigger(root, key):
    _dropdown_labeled(root, "Trigger mode").set_selected([k for k, _ in TRIGGER_OPTIONS].index(key))


def test_analog_repeat_hint_copy_leads_with_the_warning_glyph_and_matches_the_spec():
    # Spec §4: exactly the three sentences, one wrapped line, leading with ⚠️.
    assert _ANALOG_REPEAT_HINT.startswith("⚠️ ")
    assert "\n" not in _ANALOG_REPEAT_HINT
    assert "far faster than a hand could" in _ANALOG_REPEAT_HINT
    assert "single-player or otherwise known-safe games" in _ANALOG_REPEAT_HINT
    assert "never in competitive multiplayer" in _ANALOG_REPEAT_HINT
    assert "flagged as automation" in _ANALOG_REPEAT_HINT


def test_analog_repeat_selection_reveals_the_hint_and_changing_away_removes_it():
    stub = DaemonStub()
    editor = build_binding_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None, capture_mode="analog"
    )

    assert _analog_hint_labels(editor) == []

    _select_trigger(editor, "analog_repeat")
    hints = _analog_hint_labels(editor)
    assert len(hints) == 1
    assert "dim" in hints[0].get_css_classes()
    assert hints[0].get_wrap()

    _select_trigger(editor, "hold_to_repeat")
    assert _analog_hint_labels(editor) == []


def test_analog_repeat_hint_sits_directly_below_the_trigger_mode_row():
    stub = DaemonStub()
    editor = build_binding_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None, capture_mode="analog"
    )
    _select_trigger(editor, "analog_repeat")

    trigger_row = find_one(
        editor, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == "Trigger mode"
    )
    assert trigger_row.get_next_sibling() is _analog_hint_labels(editor)[0]


def test_analog_repeat_hint_shows_for_an_existing_analog_repeat_binding_on_open():
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1", "base",
        {"trigger": "analog_repeat", "type": "keypress", "key": "KEY_A", "modifiers": []},
    )
    editor = build_binding_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None, capture_mode="analog"
    )

    assert len(_analog_hint_labels(editor)) == 1


def test_analog_repeat_hint_never_appears_for_a_non_grid_input():
    stub = DaemonStub()
    btn = make_input_button(stub, stub.get_config(), "Default", "base", "mode_key", lambda: None)
    popover = editor_content(btn)

    trigger_dd = _dropdown_labeled(popover, "Trigger mode")
    labels = [trigger_dd.get_model().get_string(i) for i in range(trigger_dd.get_model().get_n_items())]
    assert "Analog-repeat" not in labels
    assert _analog_hint_labels(popover) == []


def test_analog_repeat_hint_tracks_the_selection_across_action_kind_changes():
    stub = DaemonStub()
    editor = build_binding_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None, capture_mode="analog"
    )
    _select_trigger(editor, "analog_repeat")
    assert len(_analog_hint_labels(editor)) == 1

    action_dd = _dropdown_labeled(editor, "Action")
    # Ticket 09: Macro drops analog_repeat from its Trigger-mode matrix — the
    # selection falls back to hold_to_repeat and the hint must clear.
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("macro"))
    assert _analog_hint_labels(editor) == []

    # Back to keypress, reselect analog_repeat so the next switch has a hint to clear.
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("keypress"))
    _select_trigger(editor, "analog_repeat")
    assert len(_analog_hint_labels(editor)) == 1

    # Profile Switch is locked to Fire-once — the hint must clear.
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("profile_switch"))
    assert _analog_hint_labels(editor) == []

    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("keypress"))
    assert _analog_hint_labels(editor) == []


def test_analog_repeat_hint_does_not_accumulate_across_repeated_action_kind_switching():
    # The show/hide reaction rides a single persistent trigger_dd listener,
    # not one reconnected per render_action_editor() rebuild — cycling kinds
    # must not pile up stale handlers that leave duplicate hint labels.
    stub = DaemonStub()
    editor = build_binding_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None, capture_mode="analog"
    )
    action_dd = _dropdown_labeled(editor, "Action")
    for _ in range(4):
        action_dd.set_selected([k for k, _ in ACTION_TYPES].index("macro"))
        action_dd.set_selected([k for k, _ in ACTION_TYPES].index("keypress"))

    _select_trigger(editor, "analog_repeat")
    assert len(_analog_hint_labels(editor)) == 1

    _select_trigger(editor, "toggle")
    assert _analog_hint_labels(editor) == []


def test_analog_repeat_hint_works_in_the_deep_stage_editor():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)  # lands on the Deep stage

    assert _analog_hint_labels(editor) == []
    _select_trigger(editor, "analog_repeat")
    assert len(_analog_hint_labels(editor)) == 1

    _select_trigger(editor, "hold_to_repeat")
    assert _analog_hint_labels(editor) == []


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

    button_labeled(popover, "Clear Binding").emit("clicked")

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


# --- Dual-stage grid keys (tartarus-dual-stage-keys ticket 07) ---


def _dual_stage_editor(stub, *, layer="base", capture_mode="analog", key="KEY_A"):
    """A `build_binding_editor` for a Grid key that already carries a primary
    Binding — the state that swaps the plain editor for the dual-stage swap
    panel. Clears `stub.calls` so a test only sees what the panel itself
    sends."""
    stub.set_binding(
        "grid_r1c1", layer, {"trigger": "hold_to_repeat", "type": "keypress", "key": key, "modifiers": []}
    )
    stub.calls.clear()
    return build_binding_editor(
        stub, stub.get_config(), "Default", layer, "grid_r1c1", lambda: None, capture_mode=capture_mode
    )


def _toggles_startswith(root, prefix):
    return find_all(
        root, lambda w: isinstance(w, Gtk.ToggleButton) and (w.get_label() or "").startswith(prefix)
    )


def _picker_panels(root):
    return find_all(root, lambda w: "picker-panel" in w.get_css_classes())


def _staging_rows(root):
    return find_all(root, lambda w: isinstance(w, Gtk.Box) and "staging-mode-row" in w.get_css_classes())


def _markers(root, *css):
    wanted = set(css) if css else {
        "marker-actuation", "marker-release", "marker-deep-actuation", "marker-deep-release"
    }
    return find_all(root, lambda w: bool(wanted & set(w.get_css_classes())))


def _add_deep_stage(editor):
    button_labeled(editor, "+ Add deep stage").emit("clicked")


def test_unbound_grid_key_shows_the_unified_panel_with_a_synthetic_primary_stage():
    # Ticket 09: an unbound grid key opens the same swap-toggle panel a bound
    # one does — a synthetic primary stage (Keypress on the Input's default
    # Trigger mode), the `Primary — …` toggle showing the passthrough-default
    # label, `+ Add deep stage` and `Clear Binding` both disabled, and no
    # "Bind a primary Action first" line anywhere.
    stub = DaemonStub()

    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    assert find_all(editor, lambda w: isinstance(w, Gtk.Label) and "Bind a primary Action first" in (w.get_label() or "")) == []

    primary_toggles = _toggles_startswith(editor, "Primary")
    assert len(primary_toggles) == 1
    # The passthrough-default label for grid_r1c1 ("1"), not the synthetic KEY_A.
    assert primary_toggles[0].get_label() == "Primary — 1"
    assert _toggles_startswith(editor, "Deep") == []

    add_btn = find_one(editor, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "+ Add deep stage")
    assert not add_btn.get_sensitive()
    assert add_btn.get_tooltip_text() == "Save or Apply a primary Action first"
    assert not button_labeled(editor, "Clear Binding").get_sensitive()

    # One picker mounted (the synthetic primary's), not also a plain editor.
    assert len(_picker_panels(editor)) == 1
    assert len(_markers(editor)) == 2

    # The synthetic primary stage is a Keypress on the Input's default
    # Trigger mode (Hold-to-repeat for a grid key).
    assert _dropdown_labeled(editor, "Action").get_model().get_string(
        _dropdown_labeled(editor, "Action").get_selected()
    ) == "Keypress"
    trigger_dd = _dropdown_labeled(editor, "Trigger mode")
    assert TRIGGER_OPTIONS[trigger_dd.get_selected()][0] == "hold_to_repeat"


def test_unbound_grid_key_add_deep_stage_is_inert_while_disabled():
    # The disabled `+ Add deep stage` must not reach the Daemon even if its
    # "clicked" is emitted directly (a deep stage structurally requires a
    # primary Binding).
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    button_labeled(editor, "+ Add deep stage").emit("clicked")

    assert stub.calls == []
    assert _toggles_startswith(editor, "Deep") == []


def test_unbound_grid_key_save_with_no_edits_still_creates_the_placeholder_binding():
    # The synthetic primary is committed unconditionally on Save (matching
    # the old plain editor's "Save always calls set_binding"): opening an
    # unbound key and hitting Save binds it to the placeholder — which seeds
    # from the Input's own passthrough default (grid_r1c1 → KEY_1), not a
    # fixed KEY_A.
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = editor_content(btn)

    button_labeled(popover, "Save").emit("clicked")

    assert stub.calls == [
        (
            "set_binding",
            "grid_r1c1",
            "base",
            {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_1", "modifiers": []},
        )
    ]
    assert changed == [1]


def test_unbound_editor_seeds_the_key_picker_from_the_inputs_own_default():
    # Ticket 11 follow-up: opening the editor on an as-yet-unbound Input
    # highlights the key that Input already passes through, not a fixed "A".
    stub = DaemonStub()

    grid = editor_content(
        make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c2", lambda: None)
    )
    grid_summary = find_one(grid, lambda w: "key-picker-summary" in w.get_css_classes())
    assert grid_summary.get_label() == "Selected: 2"  # grid_r1c2 passes through KEY_2

    # Non-grid plain editor takes the same default (Mode key → Left Alt).
    mode = build_binding_editor(stub, stub.get_config(), "Default", "base", "mode_key", lambda: None)
    mode_summary = find_one(mode, lambda w: "key-picker-summary" in w.get_css_classes())
    assert mode_summary.get_label() == "Selected: Left Alt"


def test_a_bound_grid_key_gets_the_swap_panel_with_a_single_editor_slot_and_no_deep_stage():
    stub = DaemonStub()

    editor = _dual_stage_editor(stub)

    assert find_one(editor, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "+ Add deep stage")
    assert len(_toggles_startswith(editor, "Primary")) == 1
    assert _toggles_startswith(editor, "Deep") == []
    assert _staging_rows(editor) == []
    # "never two pickers on screen at once" — only the primary's fields are
    # mounted, not also a plain top-level editor.
    assert len(_picker_panels(editor)) == 1
    assert len(_markers(editor)) == 2


def test_adding_a_deep_stage_wires_the_daemon_and_reveals_the_deep_row_and_staging_row():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)

    _add_deep_stage(editor)

    assert [c[0] for c in stub.calls] == ["set_deep_actuation", "set_deep_stage"]
    assert len(_toggles_startswith(editor, "Deep")) == 1
    assert len(_staging_rows(editor)) == 1
    assert find_all(editor, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "+ Add deep stage") == []
    assert len(find_all(editor, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "✕")) == 1
    # the shared bar gains the two deep markers, still one editor slot.
    assert len(_markers(editor, "marker-deep-actuation", "marker-deep-release")) == 2
    assert len(_markers(editor)) == 4
    assert len(_picker_panels(editor)) == 1


def test_removing_the_deep_stage_calls_clear_deep_stage_and_restores_the_add_button():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)
    stub.calls.clear()

    button_labeled(editor, "✕").emit("clicked")

    assert stub.calls == [("clear_deep_stage", "grid_r1c1", "base")]
    assert find_one(editor, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "+ Add deep stage")
    assert _toggles_startswith(editor, "Deep") == []
    assert _staging_rows(editor) == []
    assert len(_markers(editor)) == 2


def test_only_one_stage_picker_is_ever_mounted_across_the_swap_toggle():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    assert len(_picker_panels(editor)) == 1

    _add_deep_stage(editor)  # lands on the Deep stage
    assert len(_picker_panels(editor)) == 1

    _toggles_startswith(editor, "Primary")[0].set_active(True)
    assert len(_picker_panels(editor)) == 1

    _toggles_startswith(editor, "Deep")[0].set_active(True)
    assert len(_picker_panels(editor)) == 1


def test_the_deep_stages_picker_carries_the_deep_picker_class_only_while_selected():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    assert find_all(editor, lambda w: "deep-picker" in w.get_css_classes()) == []

    _add_deep_stage(editor)
    assert len(find_all(editor, lambda w: "deep-picker" in w.get_css_classes())) == 1

    _toggles_startswith(editor, "Primary")[0].set_active(True)
    assert find_all(editor, lambda w: "deep-picker" in w.get_css_classes()) == []


def test_picking_a_staging_mode_calls_set_staging_mode():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)
    stub.calls.clear()

    find_one(editor, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() == "No-Return").set_active(True)

    assert ("set_staging_mode", "grid_r1c1", "no_return") in stub.calls


def test_saving_the_deep_stage_sends_set_deep_stage_with_the_edited_binding():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)  # lands on the Deep stage
    stub.calls.clear()

    _pick_key(editor, "Key", "F1")
    button_labeled(editor, "Save").emit("clicked")

    assert stub.calls[-1] == (
        "set_deep_stage",
        "grid_r1c1",
        "base",
        {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_F1", "modifiers": []},
    )


def test_clearing_the_primary_cascades_the_deep_stage_away():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)
    _toggles_startswith(editor, "Primary")[0].set_active(True)

    button_labeled(editor, "Clear Binding").emit("clicked")

    assert ("clear_binding", "grid_r1c1", "base") in stub.calls
    profile = stub.get_config()["profiles"]["Default"]
    assert profile["base"] == {}
    assert profile["deep_base"] == {}
    # ticket 06's cascade also drops the deep display: a fresh editor lands
    # back on the synthetic-primary state — no deep toggle, `+ Add deep
    # stage` disabled again.
    fresh = build_binding_editor(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)
    assert _toggles_startswith(fresh, "Deep") == []
    assert not find_one(
        fresh, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "+ Add deep stage"
    ).get_sensitive()


def test_digital_mode_greys_the_bar_and_the_staging_row():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub, capture_mode="digital")

    assert find_all(editor, lambda w: "depth-track-dim" in w.get_css_classes())
    bar_note = find_one(
        editor, lambda w: "digital-note-overlay" in w.get_css_classes() and "No depth" in w.get_label()
    )
    assert bar_note.get_visible()

    _add_deep_stage(editor)

    assert find_one(
        editor, lambda w: "digital-note-overlay" in w.get_css_classes() and "Requires analog" in w.get_label()
    )
    for mode_btn in find_all(editor, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() == "Handoff"):
        assert not mode_btn.get_sensitive()


def test_adding_a_deep_stage_to_a_chord_member_surfaces_the_daemon_rejection():
    stub = DaemonStub()
    stub.set_chord_binding(
        ["grid_r1c1", "grid_r1c2"], "base",
        {"trigger": "fire_once", "type": "keypress", "key": "KEY_A", "modifiers": []},
    )
    editor = _dual_stage_editor(stub)

    _add_deep_stage(editor)

    err = find_one(editor, lambda w: "error" in w.get_css_classes() and w.get_visible())
    assert "Chord member" in err.get_label()
    assert stub.get_config()["profiles"]["Default"]["deep_base"] == {}


def test_adding_a_deep_stage_to_an_analog_repeat_primary_surfaces_the_daemon_rejection():
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1", "base",
        {"trigger": "analog_repeat", "type": "keypress", "key": "KEY_A", "modifiers": []},
    )
    stub.calls.clear()
    editor = build_binding_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None, capture_mode="analog"
    )

    _add_deep_stage(editor)

    err = find_one(editor, lambda w: "error" in w.get_css_classes() and w.get_visible())
    assert "analog_repeat" in err.get_label()


def test_building_the_dual_stage_panel_does_not_start_a_depth_stream_at_construction_time():
    stub = DaemonStub()

    _dual_stage_editor(stub)

    assert stub._depth_target is None


def test_swap_panel_keeps_the_primary_actuation_profile_default_controls():
    stub = DaemonStub()
    stub.set_actuation_point("grid_r1c1", 200, 180)
    editor = _dual_stage_editor(stub)

    button_labeled(editor, "Reset to Profile default").emit("clicked")
    assert ("clear_actuation_point", "grid_r1c1") in stub.calls

    button_labeled(editor, "Set as Profile default").emit("clicked")
    assert any(c[0] == "set_default_actuation" for c in stub.calls)


def test_reset_to_profile_default_also_returns_the_deep_band():
    stub = DaemonStub()
    stub.set_default_deep_actuation(210, 190)  # the Profile's remembered deep band
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)

    # drag the deep band away from the remembered default
    track = find_one(editor, lambda w: isinstance(w, DepthTrack))
    d_act_i = next(i for i, m in enumerate(track.markers) if "marker-deep-actuation" in m["css"])
    d_rel_i = next(i for i, m in enumerate(track.markers) if "marker-deep-release" in m["css"])
    track.markers[d_act_i]["value"] = 250
    track.on_drag_end(d_act_i, 250)
    track.markers[d_rel_i]["value"] = 230
    track.on_drag_end(d_rel_i, 230)
    stub.calls.clear()

    button_labeled(editor, "Reset to Profile default").emit("clicked")

    # primary override cleared *and* the deep band returned to the remembered
    # default (recomputed disjoint from the reset primary).
    assert ("clear_actuation_point", "grid_r1c1") in stub.calls
    assert ("set_deep_actuation", "grid_r1c1", 210, 190) in stub.calls
    assert stub.get_config()["profiles"]["Default"]["deep_stages"]["grid_r1c1"]["actuation"] == {
        "actuation": 210,
        "release": 190,
    }
    # the rebuilt bar shows the reset band, not the dragged one
    fresh_track = find_one(editor, lambda w: isinstance(w, DepthTrack))
    by_kind = {
        ("d_act" if "marker-deep-actuation" in m["css"] else "d_rel"): m["value"]
        for m in fresh_track.markers
        if "deep" in m["css"]
    }
    assert by_kind == {"d_act": 210, "d_rel": 190}


def test_reset_to_profile_default_with_no_deep_stage_touches_only_the_primary():
    stub = DaemonStub()
    stub.set_actuation_point("grid_r1c1", 200, 180)
    editor = _dual_stage_editor(stub)
    stub.calls.clear()

    button_labeled(editor, "Reset to Profile default").emit("clicked")

    assert [c for c in stub.calls if c[0] == "set_deep_actuation"] == []
    assert ("clear_actuation_point", "grid_r1c1") in stub.calls


def test_swap_panel_persists_a_deep_marker_drag_across_a_stage_toggle():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)
    stub.calls.clear()

    track = find_one(editor, lambda w: isinstance(w, DepthTrack))
    d_act_index = next(i for i, m in enumerate(track.markers) if "marker-deep-actuation" in m["css"])
    track.markers[d_act_index]["value"] = 240
    track.on_drag_end(d_act_index, 240)
    assert any(c[0] == "set_deep_actuation" for c in stub.calls)

    # toggling to Primary and back rebuilds the bar from the snapshot — the
    # dragged value must survive rather than snapping back.
    _toggles_startswith(editor, "Primary")[0].set_active(True)
    _toggles_startswith(editor, "Deep")[0].set_active(True)
    track = find_one(editor, lambda w: isinstance(w, DepthTrack))
    d_act = next(m["value"] for m in track.markers if "marker-deep-actuation" in m["css"])
    assert d_act == 240


def test_deep_stage_action_menu_offers_the_full_binding_menu_minus_axis():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)  # lands on the Deep stage

    action_dd = _dropdown_labeled(editor, "Action")
    labels = [action_dd.get_model().get_string(i) for i in range(action_dd.get_model().get_n_items())]
    assert "Keypress" in labels
    assert "Macro" in labels
    assert "Stepper" in labels
    assert "Switch Profile" in labels
    assert "Axis" not in labels


def test_dragging_the_primary_actuation_marker_is_clamped_below_the_deep_band():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)

    track = find_one(editor, lambda w: isinstance(w, DepthTrack))
    p_act_i = next(i for i, m in enumerate(track.markers) if "marker-actuation" in m["css"])
    d_rel_i = next(i for i, m in enumerate(track.markers) if "marker-deep-release" in m["css"])
    track.on_marker_moved(p_act_i, 255)

    assert track.markers[p_act_i]["value"] == track.markers[d_rel_i]["value"] - 1


def test_a_pre_dual_stage_daemon_config_falls_back_to_the_plain_editor():
    # Version skew: a Daemon built before the dual-stage feature has no
    # deep_base/deep_held/deep_stages keys in GetConfig(). The editor must
    # fall back to the plain layout rather than KeyError on the panel.
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1", "base", {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_A", "modifiers": []}
    )
    config = stub.get_config()
    for key in ("deep_base", "deep_held", "deep_stages"):
        config["profiles"]["Default"].pop(key, None)

    editor = build_binding_editor(stub, config, "Default", "base", "grid_r1c1", lambda: None)

    assert _toggles_startswith(editor, "Primary") == []
    assert find_all(editor, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "+ Add deep stage") == []
    # the plain Trigger/Action editor + actuation section are still there
    assert _dropdown_labeled(editor, "Trigger mode")
    assert find_one(editor, lambda w: "sub-heading" in w.get_css_classes() and w.get_label() == "Actuation & release")


def test_save_commits_both_stages_regardless_of_which_one_is_on_screen():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)  # primary = KEY_A
    _add_deep_stage(editor)  # lands on the Deep stage (default KEY_A)

    _pick_key(editor, "Key", "F1")  # edit the deep binding
    _toggles_startswith(editor, "Primary")[0].set_active(True)
    _pick_key(editor, "Key", "F2")  # edit the primary binding
    stub.calls.clear()

    button_labeled(editor, "Save").emit("clicked")

    kinds = {c[0] for c in stub.calls}
    assert "set_binding" in kinds and "set_deep_stage" in kinds
    profile = stub.get_config()["profiles"]["Default"]
    assert profile["base"]["grid_r1c1"]["key"] == "KEY_F2"
    assert profile["deep_base"]["grid_r1c1"]["key"] == "KEY_F1"


def test_editing_only_the_primary_keeps_the_deep_stage_and_pushes_only_set_binding():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)
    _toggles_startswith(editor, "Primary")[0].set_active(True)
    _pick_key(editor, "Key", "F2")
    stub.calls.clear()

    button_labeled(editor, "Save").emit("clicked")

    assert [c[0] for c in stub.calls] == ["set_binding"]
    profile = stub.get_config()["profiles"]["Default"]
    assert profile["base"]["grid_r1c1"]["key"] == "KEY_F2"
    assert "grid_r1c1" in profile["deep_base"]


def test_save_with_no_edits_pushes_nothing():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)
    _toggles_startswith(editor, "Primary")[0].set_active(True)
    _toggles_startswith(editor, "Deep")[0].set_active(True)
    stub.calls.clear()

    button_labeled(editor, "Save").emit("clicked")

    assert stub.calls == []


def test_deep_actuation_marker_drag_persists_across_a_full_editor_rebuild():
    # Regression: a deep-marker drag must survive the editor being torn down
    # and rebuilt from a fresh GetConfig (an app rebuild), exactly the way a
    # primary-marker drag does.
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)
    stub.calls.clear()

    track = find_one(editor, lambda w: isinstance(w, DepthTrack))
    d_act_i = next(i for i, m in enumerate(track.markers) if "marker-deep-actuation" in m["css"])
    d_rel_i = next(i for i, m in enumerate(track.markers) if "marker-deep-release" in m["css"])
    track.markers[d_act_i]["value"] = 240
    track.on_drag_end(d_act_i, 240)
    track.markers[d_rel_i]["value"] = 205
    track.on_drag_end(d_rel_i, 205)

    assert ("set_deep_actuation", "grid_r1c1", 240, 205) in stub.calls
    assert stub.get_config()["profiles"]["Default"]["deep_stages"]["grid_r1c1"]["actuation"] == {
        "actuation": 240,
        "release": 205,
    }

    # A brand-new editor built from the daemon's current config (what an app
    # rebuild does) shows the dragged deep band, not the seeded default.
    fresh = build_binding_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None, capture_mode="analog"
    )
    fresh_track = find_one(fresh, lambda w: isinstance(w, DepthTrack))
    by_kind = {
        ("d_act" if "marker-deep-actuation" in m["css"] else "d_rel"): m["value"]
        for m in fresh_track.markers
        if "deep" in m["css"]
    }
    assert by_kind == {"d_act": 240, "d_rel": 205}


def test_set_as_profile_default_records_the_current_deep_band():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)
    _add_deep_stage(editor)

    # move the deep band, then "Set as Profile default"
    track = find_one(editor, lambda w: isinstance(w, DepthTrack))
    d_act_i = next(i for i, m in enumerate(track.markers) if "marker-deep-actuation" in m["css"])
    d_rel_i = next(i for i, m in enumerate(track.markers) if "marker-deep-release" in m["css"])
    track.markers[d_act_i]["value"] = 244
    track.on_drag_end(d_act_i, 244)
    track.markers[d_rel_i]["value"] = 208
    track.on_drag_end(d_rel_i, 208)
    stub.calls.clear()

    button_labeled(editor, "Set as Profile default").emit("clicked")

    kinds = [c[0] for c in stub.calls]
    assert "set_default_actuation" in kinds
    assert ("set_default_deep_actuation", 244, 208) in stub.calls
    assert stub.get_config()["profiles"]["Default"]["default_deep_actuation"] == {
        "actuation": 244,
        "release": 208,
    }


def test_add_deep_stage_seeds_from_the_profile_default_deep_band():
    stub = DaemonStub()
    stub.set_default_deep_actuation(244, 208)
    editor = _dual_stage_editor(stub)

    _add_deep_stage(editor)

    # the seeded band is the remembered one, not the +20/+35 offset off the
    # primary default (128 -> 148 / 183).
    assert ("set_deep_actuation", "grid_r1c1", 244, 208) in stub.calls
    track = find_one(editor, lambda w: isinstance(w, DepthTrack))
    by_kind = {
        ("d_act" if "marker-deep-actuation" in m["css"] else "d_rel"): m["value"]
        for m in track.markers
        if "deep" in m["css"]
    }
    assert by_kind == {"d_act": 244, "d_rel": 208}


def test_add_deep_stage_clamps_a_remembered_band_that_would_overlap_this_keys_primary():
    stub = DaemonStub()
    # a per-key primary override sitting above the remembered deep release
    stub.set_actuation_point("grid_r1c1", 220, 200)
    stub.set_default_deep_actuation(210, 190)  # release 190 < this key's primary actuation 220
    editor = _dual_stage_editor(stub, key="KEY_A")

    _add_deep_stage(editor)

    # the seed is clamped so release > 220 (disjoint from this key's primary).
    call = next(c for c in stub.calls if c[0] == "set_deep_actuation")
    _, _, d_act, d_rel = call
    assert d_rel > 220 and d_act > d_rel


def test_add_deep_stage_uses_the_offset_when_no_profile_default_is_set():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)  # primary default 128/112, no remembered band

    _add_deep_stage(editor)

    assert ("set_deep_actuation", "grid_r1c1", 183, 148) in stub.calls


# --- "Apply": commit the binding without closing the editor
#     (tartarus-dual-stage-keys ticket 10) ---


def _grid_editor_button(stub, *, layer="base", capture_mode="analog"):
    """A `make_input_button` for grid_r1c1 (so the real window + close-request
    handler are in play) plus a `changed` list counting `on_change()` calls.
    Clears `stub.calls` first so a test only sees what it drives."""
    stub.calls.clear()
    changed = []
    btn = make_input_button(
        stub, stub.get_config(), "Default", layer, "grid_r1c1",
        lambda: changed.append(1), capture_mode=capture_mode,
    )
    return btn, changed


def _dismiss(btn):
    # `Gtk.Window.close()` only emits `close-request` for a realized window,
    # which a headless test never has — emit it directly, standing in for the
    # WM close button / Escape / Save's own `window.close()`.
    btn.binding_editor_window.emit("close-request")


def test_apply_button_is_plain_while_save_keeps_the_accent():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)

    apply_btn = button_labeled(editor, "Apply")
    assert "suggested-action" not in apply_btn.get_css_classes()
    assert "suggested-action" in button_labeled(editor, "Save").get_css_classes()


# --- consistent right-aligned button rows across the three editors
#     (tartarus-dual-stage-keys ticket 11) ---


def _button_row(root, *, save_label="Save"):
    """The action-button row — the Gtk.Box holding the editor's Save/Apply/
    Clear (or Cancel/Save Chord) buttons as direct children."""
    return button_labeled(root, save_label).get_parent()


def _row_button_labels(row):
    labels = []
    child = row.get_first_child()
    while child is not None:
        if isinstance(child, Gtk.Button):
            labels.append(child.get_label())
        child = child.get_next_sibling()
    return labels


def _has_ancestor_of_type(widget, typ):
    parent = widget.get_parent()
    while parent is not None:
        if isinstance(parent, typ):
            return True
        parent = parent.get_parent()
    return False


def test_grid_editor_button_row_is_clear_apply_save_right_aligned():
    stub = DaemonStub()
    editor = _dual_stage_editor(stub)

    row = _button_row(editor)
    assert _row_button_labels(row) == ["Clear Binding", "Apply", "Save"]
    assert row.get_halign() == Gtk.Align.END
    # Outside the panel's scroll container — visible however tall the panel grows.
    assert not _has_ancestor_of_type(row, Gtk.ScrolledWindow)
    assert find_all(editor, lambda w: isinstance(w, Gtk.ScrolledWindow)) != []


def test_non_grid_editor_button_row_is_clear_save_right_aligned():
    stub = DaemonStub()
    editor = build_binding_editor(stub, stub.get_config(), "Default", "base", "mode_key", lambda: None)

    row = _button_row(editor)
    assert _row_button_labels(row) == ["Clear Binding", "Save"]
    assert row.get_halign() == Gtk.Align.END


def test_chord_dialog_button_row_is_cancel_save_chord_right_aligned():
    stub = DaemonStub()
    dialog = build_chord_binding_dialog(
        stub, stub.get_config(), "Default", "base", ["grid_r1c1", "grid_r1c2"], None, lambda: None, None
    )

    row = _button_row(dialog, save_label="Save Chord")
    assert _row_button_labels(row) == ["Cancel", "Save Chord"]
    assert row.get_halign() == Gtk.Align.END


def test_non_grid_and_chord_editors_have_no_apply_button():
    stub = DaemonStub()

    non_grid = build_binding_editor(stub, stub.get_config(), "Default", "base", "mode_key", lambda: None)
    assert find_all(non_grid, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Apply") == []

    chord = build_chord_binding_dialog(
        stub, stub.get_config(), "Default", "base", ["grid_r1c1", "grid_r1c2"], None, lambda: None, None
    )
    assert find_all(chord, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Apply") == []


def test_apply_on_an_unbound_key_creates_the_binding_and_keeps_the_window_open():
    stub = DaemonStub()
    btn, changed = _grid_editor_button(stub)
    editor = editor_content(btn)

    _pick_key(editor, "Key", "F1")
    button_labeled(editor, "Apply").emit("clicked")

    assert stub.calls == [
        (
            "set_binding",
            "grid_r1c1",
            "base",
            {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_F1", "modifiers": []},
        )
    ]
    # No full-app rebuild yet — the window stays open.
    assert changed == []

    # The panel rebuilt in place into the bound layout.
    editor = editor_content(btn)
    assert button_labeled(editor, "+ Add deep stage").get_sensitive()
    assert button_labeled(editor, "Clear Binding").get_sensitive()
    primary_toggle = _toggles_startswith(editor, "Primary")[0]
    assert "F1" in primary_toggle.get_label()
    assert primary_toggle.get_label() != "Primary — 1"


def test_apply_then_add_deep_stage_works_in_one_window_session():
    stub = DaemonStub()
    btn, changed = _grid_editor_button(stub)

    button_labeled(editor_content(btn), "Apply").emit("clicked")  # binds the grid_r1c1 default (KEY_1) placeholder
    button_labeled(editor_content(btn), "+ Add deep stage").emit("clicked")

    assert [c[0] for c in stub.calls] == ["set_binding", "set_deep_actuation", "set_deep_stage"]
    assert len(_toggles_startswith(editor_content(btn), "Deep")) == 1
    # Still no full-app rebuild while the window is open.
    assert changed == []


def test_close_after_apply_drives_exactly_one_on_change():
    stub = DaemonStub()
    btn, changed = _grid_editor_button(stub)

    button_labeled(editor_content(btn), "Apply").emit("clicked")
    button_labeled(editor_content(btn), "Apply").emit("clicked")  # redundant no-op push
    assert changed == []

    _dismiss(btn)
    assert changed == [1]

    _dismiss(btn)  # a second dismissal must not re-fire
    assert changed == [1]


def test_open_and_close_with_no_commit_drives_no_on_change():
    stub = DaemonStub()
    btn, changed = _grid_editor_button(stub)

    _dismiss(btn)

    assert changed == []
    assert stub.calls == []


def test_save_after_apply_still_commits_and_closes_once():
    stub = DaemonStub()
    btn, changed = _grid_editor_button(stub)

    _pick_key(editor_content(btn), "Key", "F1")
    button_labeled(editor_content(btn), "Apply").emit("clicked")
    assert changed == []

    # A second edit, then Save: it commits the delta and drives the one rebuild.
    _pick_key(editor_content(btn), "Key", "F2")
    button_labeled(editor_content(btn), "Save").emit("clicked")

    assert stub.calls[-1] == (
        "set_binding",
        "grid_r1c1",
        "base",
        {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_F2", "modifiers": []},
    )
    assert changed == [1]
    _dismiss(btn)  # nothing left to flush
    assert changed == [1]


def test_apply_is_insensitive_under_the_same_conditions_as_save():
    stub = DaemonStub()  # empty Macro library
    editor = _dual_stage_editor(stub)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("macro"))

    assert not button_labeled(editor, "Save").get_sensitive()
    assert not button_labeled(editor, "Apply").get_sensitive()

    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("keypress"))
    assert button_labeled(editor, "Save").get_sensitive()
    assert button_labeled(editor, "Apply").get_sensitive()


def test_apply_that_pushes_nothing_does_not_arm_the_deferred_rebuild():
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1", "base",
        {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_A", "modifiers": []},
    )
    btn, changed = _grid_editor_button(stub)

    button_labeled(editor_content(btn), "Apply").emit("clicked")  # nothing edited

    assert stub.calls == []
    _dismiss(btn)
    assert changed == []


def test_apply_with_an_axis_primary_falls_back_to_close_and_reopen():
    # An Axis assignment has no representation in the swap panel — Apply
    # commits it (like Save) and then closes rather than stranding the editor
    # on the synthetic-primary layout with Clear disabled.
    stub = DaemonStub()
    btn, changed = _grid_editor_button(stub)
    editor = editor_content(btn)

    action_dd = _dropdown_labeled(editor, "Action")
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index("axis"))
    _click_axis_target(editor_content(btn), "Left Trigger")

    button_labeled(editor_content(btn), "Apply").emit("clicked")

    assert any(c[0] == "set_axis_assignment" for c in stub.calls)
    assert changed == [1]  # the close-and-reopen fallback drove the rebuild


def test_add_deep_stage_then_dismiss_drives_exactly_one_on_change():
    # A structural edit that commits real Daemon state also arms the deferred
    # rebuild, so closing the window afterwards refreshes the other cached
    # editors even though Save/Apply/Clear were never clicked.
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1", "base",
        {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_A", "modifiers": []},
    )
    btn, changed = _grid_editor_button(stub)

    button_labeled(editor_content(btn), "+ Add deep stage").emit("clicked")
    assert changed == []  # still open, no full rebuild yet

    _dismiss(btn)
    assert changed == [1]
    _dismiss(btn)
    assert changed == [1]
