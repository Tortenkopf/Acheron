# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

from gi.repository import Gtk

from acheron_gui.key_picker import (
    LABEL_BY_CODE,
    MODIFIER_CODES,
    build_inline_key_picker,
    key_css_class,
)

from .widget_tree import button_labeled, find_all, find_one


def _summary(widget) -> Gtk.Label:
    return find_one(widget, lambda w: "key-picker-summary" in w.get_css_classes())


def test_shows_the_current_key_s_nice_label():
    widget, _refresh = build_inline_key_picker("KEY_F1", lambda code: None)

    assert _summary(widget).get_label() == "Selected: F1"


def test_the_full_keyboard_grid_is_always_shown_inline():
    # Ticket 44: no collapse/expand toggle — the grid is always inline.
    # Live-verified on real hardware that both a grow-in-place resize of the
    # outer Binding-editor Popover, and a nested Gtk.Popover positioned off
    # a Gtk.MenuButton, are broken on this GTK4/Wayland stack; always
    # rendering inline avoids both failure modes since the outer popover's
    # size is fixed at its own first render.
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None)

    assert button_labeled(widget, "F1") is not None
    assert button_labeled(widget, "Space") is not None
    assert button_labeled(widget, "Left") is not None  # mouse strip


def test_picking_a_key_updates_the_summary_and_calls_on_change():
    picked = []
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: picked.append(code))

    button_labeled(widget, "F1").emit("clicked")

    assert picked == ["KEY_F1"]
    assert _summary(widget).get_label() == "Selected: F1"


def test_picking_a_mouse_button_reports_its_btn_code():
    picked = []
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: picked.append(code))

    button_labeled(widget, "Left").emit("clicked")

    assert picked == ["BTN_LEFT"]


def test_every_qwerty_home_and_bottom_row_letter_reports_an_uppercase_code():
    # Regression test: the physical-keyboard-layout rows (_QWERTY_ROW,
    # _HOME_ROW, _BOTTOM_ROW) build their code from the row's own lowercase
    # loop variable directly (`f"KEY_{c}"`) while only the *label* uppercases
    # it — live-verified on real hardware (ticket 44) that this produced
    # invalid codes like "KEY_h" the real Daemon correctly rejected
    # (evdev::KeyCode only recognizes uppercase KEY_* names), while the
    # *catalog*-driven letters (`_LETTERS`, used elsewhere) were unaffected.
    for row_letters in ("qwertyuiop", "asdfghjkl", "zxcvbnm"):
        for letter in row_letters:
            picked = []
            widget, _refresh = build_inline_key_picker("KEY_A", lambda code: picked.append(code))
            button_labeled(widget, letter.upper()).emit("clicked")
            assert picked == [f"KEY_{letter.upper()}"]


def test_f13_through_f24_are_shown_inline_with_no_toggle():
    # Ticket 89: F13-F24 render unconditionally, directly under F1-F12 — the
    # old "Show F13-F24 ▸" collapse is gone (it never saved vertical space).
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None)

    assert button_labeled(widget, "F13") is not None
    assert button_labeled(widget, "F24") is not None
    assert find_all(widget, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Show F13-F24 ▸") == []


def test_numpad_keys_are_hidden_behind_a_show_toggle():
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None)

    assert find_all(widget, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Num 7") == []

    button_labeled(widget, "Show Numpad ▸").emit("clicked")

    assert button_labeled(widget, "Num 7") is not None
    assert button_labeled(widget, "Num Enter") is not None
    assert button_labeled(widget, "Num +") is not None

    button_labeled(widget, "Hide Numpad ▾").emit("clicked")

    assert find_all(widget, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Num 7") == []


def test_numpad_toggle_leaves_the_always_visible_f13_f24_row_alone():
    # Ticket 89: F13-F24 are always inline now; the numpad keeps its own
    # independent collapse.
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None)

    assert button_labeled(widget, "F13") is not None
    assert find_all(widget, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Num 7") == []
    assert button_labeled(widget, "Show Numpad ▸") is not None

    button_labeled(widget, "Show Numpad ▸").emit("clicked")
    assert button_labeled(widget, "F13") is not None
    assert button_labeled(widget, "Num 7") is not None


def test_picking_a_numpad_key_reports_its_kp_code():
    picked = []
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: picked.append(code))

    button_labeled(widget, "Show Numpad ▸").emit("clicked")
    button_labeled(widget, "Num Enter").emit("clicked")

    assert picked == ["KEY_KPENTER"]
    assert _summary(widget).get_label() == "Selected: Num Enter"


def _click_a_modifier(widget) -> None:
    # "Ctrl"/"Shift"/"Alt"/"Super" each appear twice (Left/Right) — any
    # modifier keycap exercises the same warning path.
    find_all(widget, lambda w: isinstance(w, Gtk.Button) and "keycap-mod" in w.get_css_classes())[0].emit("clicked")


def test_selecting_a_modifier_shows_the_warning_by_default():
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None)

    _click_a_modifier(widget)

    assert find_all(widget, lambda w: "warning" in w.get_css_classes()) != []


def test_warn_predicate_suppresses_the_modifier_warning():
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None, warn_predicate=lambda: False)

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


def test_picking_a_key_refreshes_the_highlighted_current_keycap():
    widget, _refresh = build_inline_key_picker("KEY_A", lambda code: None)

    button_labeled(widget, "F1").emit("clicked")

    f1_button = button_labeled(widget, "F1")
    assert "suggested-action" in f1_button.get_css_classes()


def test_key_css_class_classifies_modifiers_mouse_and_multimedia_distinctly():
    assert key_css_class("KEY_LEFTCTRL") == "keycap-mod"
    assert key_css_class("BTN_LEFT") == "keycap-mouse"
    assert key_css_class("KEY_PLAYPAUSE") == "keycap-mm"
    assert key_css_class("KEY_A") is None


def test_label_by_code_covers_modifiers_and_mouse_buttons():
    assert LABEL_BY_CODE["KEY_LEFTCTRL"] == "Left Ctrl"
    assert LABEL_BY_CODE["BTN_LEFT"] == "Mouse Left"


def test_label_by_code_covers_the_core_numpad_keys():
    assert LABEL_BY_CODE["KEY_KP7"] == "Num 7"
    assert LABEL_BY_CODE["KEY_KPENTER"] == "Num Enter"
    assert LABEL_BY_CODE["KEY_KPPLUS"] == "Num +"
    assert LABEL_BY_CODE["KEY_KPDOT"] == "Num ."
    assert "KEY_KPEQUAL" not in LABEL_BY_CODE


def test_modifier_codes_cover_all_eight_evdev_modifier_keys():
    assert MODIFIER_CODES == {
        "KEY_LEFTCTRL", "KEY_RIGHTCTRL",
        "KEY_LEFTSHIFT", "KEY_RIGHTSHIFT",
        "KEY_LEFTALT", "KEY_RIGHTALT",
        "KEY_LEFTMETA", "KEY_RIGHTMETA",
    }
