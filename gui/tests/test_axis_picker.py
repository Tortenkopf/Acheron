# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

from gi.repository import Gtk

from acheron_gui.axis_picker import AXIS_LABEL_BY_TARGET, build_inline_axis_picker
from acheron_gui.rules import AXIS_TARGETS

from .widget_tree import button_labeled, find_all, find_one


def _summary(widget) -> Gtk.Label:
    return find_one(widget, lambda w: "controller-picker-summary" in w.get_css_classes())


def _button_with_tooltip(widget, tooltip: str) -> Gtk.Button:
    return find_one(widget, lambda w: isinstance(w, Gtk.Button) and w.get_tooltip_text() == tooltip)


def test_catalog_contents_match_the_daemon_target_list():
    # Contents, not just length — `rules.AXIS_TARGETS` is itself contract-
    # tested against the Daemon's `AxisTarget::ALL` (test_rules_contract.py).
    assert set(AXIS_LABEL_BY_TARGET) == AXIS_TARGETS


def test_shows_the_current_targets_nice_label():
    widget = build_inline_axis_picker("left_trigger", lambda target: None)

    assert _summary(widget).get_label() == "Selected: Left Trigger"


def test_shows_none_when_no_target_is_selected_yet():
    widget = build_inline_axis_picker(None, lambda target: None)

    assert _summary(widget).get_label() == "Selected: None"


def test_the_diagram_is_always_shown_inline():
    widget = build_inline_axis_picker("left_trigger", lambda target: None)

    assert _button_with_tooltip(widget, "Left Trigger") is not None
    assert _button_with_tooltip(widget, "Right Trigger") is not None
    assert _button_with_tooltip(widget, "Left Stick X+") is not None
    assert button_labeled(widget, "Gas") is not None
    assert button_labeled(widget, "Throttle") is not None


def test_picking_a_signed_half_reports_its_own_wire_string():
    picked = []
    widget = build_inline_axis_picker(None, lambda target: picked.append(target))

    _button_with_tooltip(widget, "Left Stick X+").emit("clicked")

    assert picked == ["left_stick_x_pos"]
    assert _summary(widget).get_label() == "Selected: Left Stick X+"


def test_picking_the_opposite_half_is_a_separate_click_not_the_same_control():
    picked = []
    widget = build_inline_axis_picker(None, lambda target: picked.append(target))

    _button_with_tooltip(widget, "Left Stick X+").emit("clicked")
    _button_with_tooltip(widget, "Left Stick X−").emit("clicked")

    assert picked == ["left_stick_x_pos", "left_stick_x_neg"]


def test_picking_an_unsigned_target_from_the_driving_row():
    picked = []
    widget = build_inline_axis_picker(None, lambda target: picked.append(target))

    button_labeled(widget, "Gas").emit("clicked")

    assert picked == ["gas"]


def test_picking_a_target_refreshes_the_current_highlight():
    widget = build_inline_axis_picker("left_trigger", lambda target: None)

    _button_with_tooltip(widget, "Right Trigger").emit("clicked")

    assert "axis-target-current" in _button_with_tooltip(widget, "Right Trigger").get_css_classes()
    assert "axis-target-current" not in _button_with_tooltip(widget, "Left Trigger").get_css_classes()


def test_no_toast_when_the_target_is_unclaimed():
    widget = build_inline_axis_picker(None, lambda target: None)

    button_labeled(widget, "Gas").emit("clicked")

    assert find_all(widget, lambda w: "toast" in w.get_css_classes()) == []


def test_picking_a_target_already_claimed_by_another_key_shows_a_toast():
    widget = build_inline_axis_picker(None, lambda target: None, claimed_by={"gas": "3"})

    button_labeled(widget, "Gas").emit("clicked")

    toasts = find_all(widget, lambda w: "toast" in w.get_css_classes())
    assert len(toasts) == 1
    assert toasts[0].get_label() == "Also assigned to 3 — allowed, both keys will drive this axis."
