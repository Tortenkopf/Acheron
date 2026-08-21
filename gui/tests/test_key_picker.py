from gi.repository import Gtk

from acheron_gui.key_picker import (
    LABEL_BY_CODE,
    MODIFIER_CODES,
    build_inline_key_picker,
    key_css_class,
)

from .widget_tree import button_labeled, find_all, find_one


def _toggle(widget) -> Gtk.Button:
    return find_one(widget, lambda w: isinstance(w, Gtk.Button) and "key-picker-toggle" in w.get_css_classes())


def test_starts_collapsed_showing_the_current_key_s_nice_label():
    widget, _refresh = build_inline_key_picker("KEY_F1", lambda code: None)

    toggle = _toggle(widget)
    assert toggle.get_label() == "F1  ▸ Change"
    assert find_all(widget, lambda w: "keycap" in w.get_css_classes()) == []


def test_clicking_toggle_expands_the_full_keyboard_grid():
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None)

    _toggle(widget).emit("clicked")

    assert button_labeled(widget, "F1") is not None
    assert button_labeled(widget, "Space") is not None
    assert button_labeled(widget, "Left") is not None  # mouse strip


def test_picking_a_key_collapses_the_panel_and_calls_on_change():
    picked = []
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: picked.append(code))

    _toggle(widget).emit("clicked")
    button_labeled(widget, "F1").emit("clicked")

    assert picked == ["KEY_F1"]
    assert _toggle(widget).get_label() == "F1  ▸ Change"
    assert find_all(widget, lambda w: "keycap" in w.get_css_classes()) == []


def test_picking_a_mouse_button_reports_its_btn_code():
    picked = []
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: picked.append(code))

    _toggle(widget).emit("clicked")
    button_labeled(widget, "Left").emit("clicked")

    assert picked == ["BTN_LEFT"]


def test_f13_through_f24_are_hidden_behind_a_show_toggle():
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None)
    _toggle(widget).emit("clicked")

    assert find_all(widget, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "F13") == []

    button_labeled(widget, "Show F13-F24 ▸").emit("clicked")

    assert button_labeled(widget, "F13") is not None
    assert button_labeled(widget, "F24") is not None


def _click_a_modifier(widget) -> None:
    # "Ctrl"/"Shift"/"Alt"/"Super" each appear twice (Left/Right) — any
    # modifier keycap exercises the same warning path.
    find_all(widget, lambda w: isinstance(w, Gtk.Button) and "keycap-mod" in w.get_css_classes())[0].emit("clicked")


def test_selecting_a_modifier_shows_the_warning_by_default():
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None)
    _toggle(widget).emit("clicked")

    _click_a_modifier(widget)

    assert find_all(widget, lambda w: "warning" in w.get_css_classes()) != []


def test_warn_predicate_suppresses_the_modifier_warning():
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None, warn_predicate=lambda: False)
    _toggle(widget).emit("clicked")

    _click_a_modifier(widget)

    assert find_all(widget, lambda w: "warning" in w.get_css_classes()) == []


def test_refresh_warning_reevaluates_the_predicate_without_a_new_pick():
    should_warn = {"value": False}
    widget, refresh = build_inline_key_picker(
        "KEY_LEFTCTRL", lambda code: None, warn_predicate=lambda: should_warn["value"]
    )

    assert find_all(widget, lambda w: "warning" in w.get_css_classes()) == []

    should_warn["value"] = True
    refresh()

    assert find_all(widget, lambda w: "warning" in w.get_css_classes()) != []


def test_key_css_class_classifies_modifiers_mouse_and_multimedia_distinctly():
    assert key_css_class("KEY_LEFTCTRL") == "keycap-mod"
    assert key_css_class("BTN_LEFT") == "keycap-mouse"
    assert key_css_class("KEY_PLAYPAUSE") == "keycap-mm"
    assert key_css_class("KEY_A") is None


def test_label_by_code_covers_modifiers_and_mouse_buttons():
    assert LABEL_BY_CODE["KEY_LEFTCTRL"] == "Left Ctrl"
    assert LABEL_BY_CODE["BTN_LEFT"] == "Mouse Left"


def test_modifier_codes_cover_all_eight_evdev_modifier_keys():
    assert MODIFIER_CODES == {
        "KEY_LEFTCTRL", "KEY_RIGHTCTRL",
        "KEY_LEFTSHIFT", "KEY_RIGHTSHIFT",
        "KEY_LEFTALT", "KEY_RIGHTALT",
        "KEY_LEFTMETA", "KEY_RIGHTMETA",
    }
