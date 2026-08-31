# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

from gi.repository import Gtk

from acheron_gui.controller_picker import (
    GAMEPAD_CATEGORIES,
    LABEL_BY_CODE,
    build_inline_controller_picker,
    button_css_class,
)
from acheron_gui.rules import GAMEPAD_BUTTONS

from .widget_tree import button_labeled, find_all, find_one


def _summary(widget) -> Gtk.Label:
    return find_one(widget, lambda w: "controller-picker-summary" in w.get_css_classes())


def test_shows_the_current_button_s_nice_label():
    widget = build_inline_controller_picker("BTN_SOUTH", lambda code: None)

    assert _summary(widget).get_label() == "Selected: A / South"


def test_the_diagram_is_always_shown_inline():
    # Ticket 44's precedent (the sibling key/mouse-button picker mounted in
    # this exact same per-key modal Gtk.Window) found a collapsed-summary
    # shape broken on this GTK4/Wayland stack; this picker never reproduces
    # it — the diagram's named buttons must be reachable with no toggle
    # click first.
    widget = build_inline_controller_picker("BTN_SOUTH", lambda code: None)

    assert button_labeled(widget, "A") is not None
    assert button_labeled(widget, "Start") is not None
    assert button_labeled(widget, "↑") is not None


def test_picking_a_named_button_updates_the_summary_and_calls_on_change():
    picked = []
    widget = build_inline_controller_picker("BTN_SOUTH", lambda code: picked.append(code))

    button_labeled(widget, "A").emit("clicked")

    assert picked == ["BTN_SOUTH"]
    assert _summary(widget).get_label() == "Selected: A / South"


def test_extra_buttons_are_hidden_behind_a_show_toggle():
    widget = build_inline_controller_picker("BTN_SOUTH", lambda code: None)

    assert find_all(widget, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "1") == []

    button_labeled(widget, "Extra buttons (Trigger-Happy 1-40) ▸").emit("clicked")

    extra_buttons = find_all(
        widget, lambda w: isinstance(w, Gtk.Button) and w.get_tooltip_text() == "Extra 1"
    )
    assert len(extra_buttons) == 1
    extra_buttons[0].emit("clicked")


def test_picking_an_extra_button_reports_its_code():
    picked = []
    widget = build_inline_controller_picker("BTN_SOUTH", lambda code: picked.append(code))

    button_labeled(widget, "Extra buttons (Trigger-Happy 1-40) ▸").emit("clicked")
    find_one(widget, lambda w: isinstance(w, Gtk.Button) and w.get_tooltip_text() == "Extra 40").emit("clicked")

    assert picked == ["BTN_TRIGGER_HAPPY40"]


def test_picking_a_button_refreshes_the_highlighted_current_button():
    widget = build_inline_controller_picker("BTN_SOUTH", lambda code: None)

    button_labeled(widget, "B").emit("clicked")

    assert "suggested-action" in button_labeled(widget, "B").get_css_classes()


def test_button_css_class_classifies_face_shoulder_stick_and_dpad_distinctly():
    assert button_css_class("BTN_SOUTH") == "padbtn-face"
    assert button_css_class("BTN_TL") == "padbtn-shoulder"
    assert button_css_class("BTN_THUMBL") == "padbtn-stick"
    assert button_css_class("BTN_DPAD_UP") == "padbtn-dpad"
    assert button_css_class("BTN_START") is None


def test_catalog_contents_match_the_daemon_allowlist():
    # Contents, not just length — a renamed or swapped entry passes a count
    # check but not this. `rules.GAMEPAD_BUTTONS` is itself contract-tested
    # against the Daemon's `gamepad_button_codes()` (test_rules_contract.py).
    assert set(LABEL_BY_CODE) == GAMEPAD_BUTTONS
    assert {code for entries in GAMEPAD_CATEGORIES.values() for code, _ in entries} == GAMEPAD_BUTTONS
