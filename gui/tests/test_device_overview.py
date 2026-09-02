# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

from gi.repository import Gtk, Pango

from acheron_gui.daemon_client import AlreadyExistsError, DaemonError, NotFoundError
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.device_overview import (
    PLACEHOLDER_CONFIG,
    build_main_view,
    build_status_wrapped_view,
    compute_status,
)
from acheron_gui.library_view import build_library_sidebar
from acheron_gui.inputs import ALL_INPUTS

from .widget_tree import editor_content, find_all, find_one


def _build(stub, ui_state):
    config = stub.get_config()
    state = stub.get_state()
    profile, layer = state["profile"], state["layer"]
    return build_main_view(stub, config, profile, layer, lambda: None, ui_state)


def _build_status(stub, status: str, ui_state=None):
    config = stub.get_config()
    state = stub.get_state()
    profile, layer = state["profile"], state["layer"]
    return build_status_wrapped_view(stub, config, profile, layer, status, lambda: None, ui_state or {})


def _device_overview_root(outer: Gtk.Widget) -> Gtk.Widget:
    """Locates `build_main_view`'s own returned root inside
    `build_status_wrapped_view`'s wrapper: directly the last child of
    `outer` when healthy (`[badge, root]`), or the `Gtk.Overlay`'s main
    child when not (`[badge, Gtk.Overlay(root, dim-overlay)]`)."""
    last = outer.get_last_child()
    if isinstance(last, Gtk.Overlay):
        return last.get_child()
    return last


def _dest_switch_button(root, label):
    return find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == label)


def test_device_overview_renders_one_button_per_input():
    stub = DaemonStub()

    root = _build(stub, {})

    # Filtered to "bound"/"empty"-classed buttons — only make_input_button's
    # grid buttons ever carry either class.
    input_buttons = find_all(
        root,
        lambda w: isinstance(w, Gtk.Button) and ("bound" in w.get_css_classes() or "empty" in w.get_css_classes()),
    )
    assert len(input_buttons) == len(ALL_INPUTS)


def test_grid_destination_is_selected_by_default():
    stub = DaemonStub()

    root = _build(stub, {})

    assert "suggested-action" in _dest_switch_button(root, "Grid").get_css_classes()
    assert "suggested-action" not in _dest_switch_button(root, "Library").get_css_classes()
    # Action Table is cut outright (ticket 48), not relocated.
    assert find_all(root, lambda w: isinstance(w, Gtk.Revealer)) == []


def test_library_destination_fully_replaces_the_content_area():
    stub = DaemonStub()

    root = _build(stub, {})
    library_btn = _dest_switch_button(root, "Library")
    library_btn.emit("clicked")

    # A real rebuild, mirroring how app.py's on_change triggers one — the
    # switch button itself only records the pick into ui_state and calls
    # on_change(), same pattern as build_layer_bar's own Base/Held tabs.
    ui_state = {}
    root = _build(stub, ui_state)
    library_btn = _dest_switch_button(root, "Library")
    library_btn.emit("clicked")
    assert ui_state["dest"] == "library"

    rebuilt = _build(stub, ui_state)
    assert "suggested-action" in _dest_switch_button(rebuilt, "Library").get_css_classes()
    # No grid buttons, no layer bar, no Chords slot while Library is shown.
    assert find_all(rebuilt, lambda w: isinstance(w, Gtk.Button) and "bound" in w.get_css_classes()) == []
    assert find_all(rebuilt, lambda w: isinstance(w, Gtk.Button) and "empty" in w.get_css_classes()) == []
    # The real Steppers/Macros tab-switched panel pair (ticket 52) replaces
    # the old placeholder — the Macros tab is selected by default.
    assert find_one(rebuilt, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Steppers")
    macros_tab = find_one(rebuilt, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Macros")
    assert "suggested-action" in macros_tab.get_css_classes()


def test_grid_destination_selection_survives_a_rebuild():
    stub = DaemonStub()
    ui_state = {"dest": "library"}

    rebuilt = _build(stub, ui_state)
    grid_btn = _dest_switch_button(rebuilt, "Grid")
    grid_btn.emit("clicked")
    assert ui_state["dest"] == "grid"

    rebuilt_again = _build(stub, ui_state)
    assert "suggested-action" in _dest_switch_button(rebuilt_again, "Grid").get_css_classes()
    input_buttons = find_all(
        rebuilt_again,
        lambda w: isinstance(w, Gtk.Button) and ("bound" in w.get_css_classes() or "empty" in w.get_css_classes()),
    )
    assert len(input_buttons) == len(ALL_INPUTS)


def _grid_button(root, label, profile="Default", layer="base"):
    """Locates one specific device button by the title its
    `binding_editor_window` was built with — every device button carries
    this attribute (`make_input_button`), unique per `(profile, layer,
    input_label)`."""
    title = f"{profile} / {layer} / {label}"
    return find_one(
        root,
        lambda w: isinstance(w, Gtk.Button)
        and getattr(w, "binding_editor_window", None) is not None
        and w.binding_editor_window.get_title() == title,
    )


def test_device_buttons_are_a_fixed_size_cap_not_a_growing_floor():
    # Ticket 87/88: every device button is capped at a fixed size regardless
    # of label content — a 4-modifier chord is exactly the pathological case
    # the old "floor, not cap" sizing let grow the button and its grid column.
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1",
        "base",
        {
            "trigger": "fire_once",
            "type": "keypress",
            "key": "KEY_F12",
            "modifiers": ["ctrl", "shift", "alt", "super"],
        },
    )
    root = _build(stub, {})

    grid_btn = _grid_button(root, "1")
    assert grid_btn.get_size_request() == (100, 100)
    label = grid_btn.get_child()
    assert label.get_lines() == 3
    assert label.get_ellipsize() == Pango.EllipsizeMode.END
    # width_chars == max_width_chars: a fixed width request, no float.
    assert label.get_max_width_chars() == 8
    assert label.get_width_chars() == 8

    # Key 20 (the wider hardware paddle) and the Mode key.
    key20 = _grid_button(root, "20")
    assert key20.get_size_request() == (150, 100)
    assert key20.get_child().get_max_width_chars() == 14

    mode = _grid_button(root, "Mode")
    assert mode.get_size_request() == (100, 100)
    assert "mode-key" in mode.get_css_classes()


def test_device_button_label_is_bold_input_line_with_full_text_tooltip():
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "keypress", "key": "KEY_A", "modifiers": ["ctrl"]},
    )
    root = _build(stub, {})

    btn = _grid_button(root, "1")
    markup = btn.get_child().get_label()
    assert markup.startswith("<b>1</b>\n")
    assert "Ctrl+A" in markup
    # The tooltip is the full untruncated text, newline flattened to two
    # spaces — set unconditionally, not gated on whether the label truncated.
    assert btn.get_tooltip_text() == "1  Ctrl+A  [1x]"


def test_device_button_summary_line_is_markup_escaped():
    stub = DaemonStub()
    stub.create_profile("A & <b>B</b>")
    stub.set_binding(
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "profile_switch", "target": "A & <b>B</b>"},
    )
    root = _build(stub, {})

    btn = _grid_button(root, "1")
    markup = btn.get_child().get_label()
    assert "&amp;" in markup
    assert "&lt;b&gt;" in markup
    # The tooltip is plain text, so it carries the raw unescaped summary.
    assert btn.get_tooltip_text() == "1  → A & <b>B</b>"


def test_chord_member_with_its_own_binding_shows_both_in_the_tooltip():
    # Ticket 96: a grid key that is both a Chord member *and* carries its own
    # individual Binding used to get only the Chord membership tooltip — its
    # own binding summary appeared nowhere once the face ellipsized.
    stub = DaemonStub()
    stub.set_chord_binding(
        ["grid_r1c1", "grid_r1c2"], "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_C"}
    )
    stub.set_binding(
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "keypress", "key": "KEY_F9", "modifiers": ["ctrl", "shift", "alt"]},
    )
    root = _build(stub, {})

    btn = _grid_button(root, "1")
    assert btn.get_tooltip_text() == "1  Ctrl+Shift+Alt+F9  [1x]\n\nPart of Chord:\n1 + 2 → C  [1x]"

    # A Chord-only member (no individual Binding) is unchanged — just the
    # membership tooltip.
    other = _grid_button(root, "2")
    assert other.get_tooltip_text() == "Part of Chord:\n1 + 2 → C  [1x]"


def test_grid_destination_shows_the_real_chords_section_with_a_selecting_toggle():
    stub = DaemonStub()

    root = _build(stub, {})

    find_one(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Chords")
    selecting_btn = find_one(
        root, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() == "Select Chord members"
    )
    assert not selecting_btn.get_active()
    find_one(root, lambda w: isinstance(w, Gtk.Label) and "no chords" in w.get_label().lower())


def test_enabling_chord_selecting_reroutes_grid_clicks_to_the_selection_instead_of_the_editor():
    stub = DaemonStub()
    ui_state = {}

    root = _build(stub, ui_state)
    selecting_btn = find_one(
        root, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() == "Select Chord members"
    )
    selecting_btn.set_active(True)
    assert ui_state["chord"]["selecting"] is True

    rebuilt = _build(stub, ui_state)
    grid_r1c1 = _grid_button(rebuilt, "1")
    grid_r1c1.emit("clicked")

    # The click was consumed by Chord selection, not the Binding editor —
    # ordinary editing is unaffected while selecting is off.
    assert ui_state["chord"]["recorded"] == ["grid_r1c1"]

    # Clicking it again removes it from the selection.
    rebuilt_again = _build(stub, ui_state)
    _grid_button(rebuilt_again, "1").emit("clicked")
    assert ui_state["chord"]["recorded"] == []


def test_axis_assigned_grid_key_always_carries_the_stripe_even_outside_chord_selecting():
    stub = DaemonStub()
    stub.set_axis_assignment("grid_r1c1", "base", "left_trigger")

    root = _build(stub, {})

    grid_r1c1 = _grid_button(root, "1")
    assert "axis-stripe" in grid_r1c1.get_css_classes()


def test_clicking_an_axis_assigned_key_while_selecting_chord_members_shows_an_inline_error():
    stub = DaemonStub()
    stub.set_axis_assignment("grid_r1c1", "base", "left_trigger")
    ui_state = {"chord": {"selecting": True, "recorded": [], "edit_key": None, "preview": None}}

    root = _build(stub, ui_state)
    _grid_button(root, "1").emit("clicked")

    assert ui_state["chord"]["recorded"] == []
    assert ui_state["chord"]["axis_error"] == "1 is Axis-assigned — can't join a Chord"

    rebuilt = _build(stub, ui_state)
    find_one(
        rebuilt,
        lambda w: isinstance(w, Gtk.Label)
        and "error" in w.get_css_classes()
        and w.get_label() == "1 is Axis-assigned — can't join a Chord",
    )


def test_clicking_an_ordinary_key_after_an_axis_error_clears_it():
    stub = DaemonStub()
    stub.set_axis_assignment("grid_r1c1", "base", "left_trigger")
    ui_state = {
        "chord": {
            "selecting": True,
            "recorded": [],
            "edit_key": None,
            "preview": None,
            "axis_error": "1 is Axis-assigned — can't join a Chord",
        }
    }

    root = _build(stub, ui_state)
    _grid_button(root, "2").emit("clicked")

    assert ui_state["chord"]["recorded"] == ["grid_r1c2"]
    assert ui_state["chord"]["axis_error"] is None


def test_binding_button_is_disabled_until_two_inputs_are_selected_and_enables_a_chord_save():
    stub = DaemonStub()
    ui_state = {"chord": {"selecting": True, "recorded": ["grid_r1c1"], "edit_key": None, "preview": None}}

    root = _build(stub, ui_state)
    binding_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Binding →")
    assert not binding_btn.get_sensitive()

    ui_state["chord"]["recorded"].append("grid_r1c2")
    rebuilt = _build(stub, ui_state)
    binding_btn = find_one(rebuilt, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Binding →")
    assert binding_btn.get_sensitive()

    binding_btn.emit("clicked")
    dialog = binding_btn.last_chord_dialog
    assert dialog.get_title() == "Chord binding"
    save_btn = find_one(dialog, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Save Chord")
    save_btn.emit("clicked")

    assert stub.get_config()["profiles"]["Default"]["chords_base"]["grid_r1c1+grid_r1c2"]["type"] == "keypress"
    # Saving clears the in-progress selection (fresh ui_state read below is
    # only meaningful once app.py's own rebuild runs; here we assert the
    # in-place mutation directly).
    assert ui_state["chord"]["recorded"] == []


def test_a_subset_superset_conflict_disables_binding_and_offers_edit_conflicting_chord():
    stub = DaemonStub()
    stub.set_chord_binding(
        ["grid_r1c1", "grid_r1c2"], "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_C"}
    )
    ui_state = {
        "chord": {
            "selecting": True,
            "recorded": ["grid_r1c1", "grid_r1c2", "mode_key"],
            "edit_key": None,
            "preview": None,
        }
    }

    root = _build(stub, ui_state)

    binding_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Binding →")
    assert not binding_btn.get_sensitive()
    conflict_btn = find_one(
        root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Edit conflicting Chord"
    )

    conflict_btn.emit("clicked")
    assert sorted(ui_state["chord"]["recorded"]) == ["grid_r1c1", "grid_r1c2"]
    assert ui_state["chord"]["edit_key"] == "grid_r1c1+grid_r1c2"


def test_editing_a_conflicting_chord_clears_a_stale_axis_error():
    # Code-review finding: every other reset site (on_selecting_toggled,
    # on_saved, on_clear, on_edit) already clears `axis_error` — this one
    # didn't, so a stale "<key> is Axis-assigned" message could survive
    # switching to editing the real conflicting Chord.
    stub = DaemonStub()
    stub.set_chord_binding(
        ["grid_r1c1", "grid_r1c2"], "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_C"}
    )
    ui_state = {
        "chord": {
            "selecting": True,
            "recorded": ["grid_r1c1", "grid_r1c2", "mode_key"],
            "edit_key": None,
            "preview": None,
            "axis_error": "3 is Axis-assigned — can't join a Chord",
        }
    }

    root = _build(stub, ui_state)
    conflict_btn = find_one(
        root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Edit conflicting Chord"
    )
    conflict_btn.emit("clicked")

    assert ui_state["chord"]["axis_error"] is None


def test_clicking_edit_on_an_existing_chord_reenters_selection_mode_preloaded():
    stub = DaemonStub()
    stub.set_chord_binding(
        ["grid_r1c1", "grid_r1c2"], "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_C"}
    )
    ui_state = {}

    root = _build(stub, ui_state)
    edit_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Edit")
    edit_btn.emit("clicked")

    assert ui_state["chord"]["selecting"] is True
    assert sorted(ui_state["chord"]["recorded"]) == ["grid_r1c1", "grid_r1c2"]
    assert ui_state["chord"]["edit_key"] == "grid_r1c1+grid_r1c2"


def test_removing_a_chord_calls_clear_chord_binding():
    stub = DaemonStub()
    stub.set_chord_binding(
        ["grid_r1c1", "grid_r1c2"], "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_C"}
    )
    ui_state = {}

    root = _build(stub, ui_state)
    # Disambiguated from the Profile sidebar's own "×" delete button by its
    # tooltip — both share the same glyph label.
    remove_btn = find_one(
        root,
        lambda w: isinstance(w, Gtk.Button)
        and w.get_label() == "×"
        and (w.get_tooltip_text() or "").startswith("Delete the"),
    )
    remove_btn.emit("clicked")

    assert stub.get_config()["profiles"]["Default"]["chords_base"] == {}


def test_steppers_tab_shows_the_real_panel_through_device_overview():
    # Ticket 55: the Steppers panel is real now, mounted with the same
    # currently-selected Profile/Layer `make_input_button`'s own editor
    # popovers use (build_main_view's `profile`/`selected_layer`) — this
    # exercises that threading end-to-end rather than only unit-testing
    # library_view.build_library_content directly (test_library_view.py's job).
    stub = DaemonStub()
    ui_state = {"dest": "library", "library_tab": "steppers"}

    root = _build(stub, ui_state)

    steppers_tab = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Steppers")
    assert "suggested-action" in steppers_tab.get_css_classes()
    assert find_one(root, lambda w: isinstance(w, Gtk.Label) and "No Steppers yet" in w.get_label())


def test_switching_to_the_steppers_tab_hides_the_macros_panel():
    stub = DaemonStub()
    stub.create_macro("Test macro", [])
    ui_state = {"dest": "library"}

    root = _build(stub, ui_state)
    steppers_tab = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Steppers")
    steppers_tab.emit("clicked")
    assert ui_state["library_tab"] == "steppers"

    rebuilt = _build(stub, ui_state)
    assert find_all(rebuilt, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Test macro") == []


def test_profile_sidebar_does_not_widen_between_destinations():
    # Ticket 47's round-2 GTK4 bug, caught and fixed live: a plain
    # Gtk.Box's expand-propagation let the sidebar compete for whatever
    # horizontal slack the *other* destination's content left unclaimed.
    # `build_profile_sidebar`'s explicit `set_hexpand(False)` should pin it
    # to the same width regardless of which destination is showing.
    stub = DaemonStub()

    grid_root = _build(stub, {"dest": "grid"})
    grid_sidebar = _profile_sidebar(grid_root)
    assert grid_sidebar.get_hexpand() is False

    # Ticket 69/70: column 1 for the Library destination is a different
    # widget (build_library_sidebar, no "Profiles" chrome) — built directly
    # here rather than located via _profile_sidebar's "Profiles"-label
    # anchor, which no longer exists while Library is showing. It must
    # still share the Grid sidebar's exact pinned width so nothing visibly
    # resizes when flipping destinations.
    library_sidebar = build_library_sidebar(stub, stub.get_config(), {}, lambda: None)
    assert library_sidebar.get_hexpand() is False
    assert library_sidebar.get_size_request().width == grid_sidebar.get_size_request().width == 220


def _profile_sidebar(root: Gtk.Widget) -> Gtk.Widget:
    heading = find_one(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Profiles")
    return heading.get_parent()


def _popover_of(menu_button: Gtk.MenuButton) -> Gtk.Widget:
    popover = menu_button.get_popover()
    assert popover is not None
    return popover


def _fill_and_submit_name_prompt(menu_button: Gtk.MenuButton, name: str, submit_label: str) -> None:
    popover = _popover_of(menu_button)
    find_one(popover, lambda w: isinstance(w, Gtk.Entry)).set_text(name)
    find_one(popover, lambda w: isinstance(w, Gtk.Button) and w.get_label() == submit_label).emit("clicked")


def test_clicking_a_sidebar_profile_button_switches_to_it():
    stub = DaemonStub()
    stub.create_profile("Gaming")

    root = _build(stub, {})
    switch_btn = find_one(_profile_sidebar(root), lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Gaming")

    switch_btn.emit("clicked")

    assert stub.get_state()["profile"] == "Gaming"
    assert ("switch_profile", "Gaming") in stub.calls


def test_clicking_the_already_active_profiles_sidebar_button_is_a_no_op():
    stub = DaemonStub()

    root = _build(stub, {})
    switch_btn = find_one(_profile_sidebar(root), lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Default")

    switch_btn.emit("clicked")

    # No superfluous SwitchProfile call for the Profile already active — a
    # real SwitchProfile would force-stop every running Toggle even when
    # switching "to" the Profile that's already live.
    assert stub.calls == []


def test_delete_is_disabled_for_the_active_profile_and_enabled_for_others():
    stub = DaemonStub()
    stub.create_profile("Gaming")

    root = _build(stub, {})
    delete_btns = find_all(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×")

    sensitivities = {b.get_sensitive() for b in delete_btns}
    assert sensitivities == {True, False}, "exactly one active (disabled) and one non-active (enabled) Profile"


def test_clicking_delete_on_a_non_active_profile_removes_it():
    stub = DaemonStub()
    stub.create_profile("Gaming")

    root = _build(stub, {})
    delete_btn = find_one(
        root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×" and w.get_sensitive()
    )

    delete_btn.emit("clicked")

    assert "Gaming" not in stub.get_config()["profiles"]


def test_creating_a_profile_via_the_new_profile_popover_calls_create_profile():
    stub = DaemonStub()

    root = _build(stub, {})
    new_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New Profile")

    _fill_and_submit_name_prompt(new_btn, "Gaming", "Create")

    assert "Gaming" in stub.get_config()["profiles"]
    assert ("create_profile", "Gaming") in stub.calls


def test_renaming_the_active_profile_via_its_popover_calls_rename_profile():
    stub = DaemonStub()

    root = _build(stub, {})
    rename_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "✎")

    _fill_and_submit_name_prompt(rename_btn, "Renamed", "Rename")

    assert stub.get_config()["active_profile"] == "Renamed"
    assert ("rename_profile", "Default", "Renamed") in stub.calls


def test_switching_profile_via_the_sidebar_clears_active_toggles():
    # The GUI-level half of ticket 19's live demo: a Toggle left running in
    # the first Profile must be gone from GetState() the instant the switch
    # lands — exact-key-release is the Daemon's own tested responsibility,
    # not something this GUI-level test can observe.
    stub = DaemonStub()
    stub.create_profile("Gaming")
    stub.simulate_toggle_started("grid_r1c1")
    assert stub.get_state()["active_toggles"] == ["grid_r1c1"]

    root = _build(stub, {})
    switch_btn = find_one(_profile_sidebar(root), lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Gaming")
    switch_btn.emit("clicked")

    assert stub.get_state()["active_toggles"] == []


class _SwitchProfileFailsDaemonStub(DaemonStub):
    def switch_profile(self, name):
        raise NotFoundError(f"no Profile named {name!r}")


def test_a_failed_switch_shows_an_error_in_the_sidebar_instead_of_swallowing_it():
    stub = _SwitchProfileFailsDaemonStub()
    stub.create_profile("Gaming")

    root = _build(stub, {})
    sidebar = _profile_sidebar(root)
    switch_btn = find_one(sidebar, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Gaming")

    switch_btn.emit("clicked")

    error_label = find_one(sidebar, lambda w: "sidebar-error" in w.get_css_classes())
    assert error_label.get_visible()
    assert error_label.get_label()
    # The failed switch must not be misreported as having happened.
    assert stub.get_state()["profile"] == "Default"


class _DeleteProfileFailsDaemonStub(DaemonStub):
    def delete_profile(self, name):
        raise NotFoundError(f"no Profile named {name!r}")


def test_a_failed_delete_shows_an_error_in_the_sidebar_instead_of_swallowing_it():
    stub = _DeleteProfileFailsDaemonStub()
    stub.create_profile("Gaming")

    root = _build(stub, {})
    sidebar = _profile_sidebar(root)
    delete_btn = find_one(
        sidebar, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×" and w.get_sensitive()
    )

    delete_btn.emit("clicked")

    error_label = find_one(sidebar, lambda w: "sidebar-error" in w.get_css_classes())
    assert error_label.get_visible()
    assert error_label.get_label()
    assert "Gaming" in stub.get_config()["profiles"]


class _CreateProfileFailsDaemonStub(DaemonStub):
    def create_profile(self, name):
        raise AlreadyExistsError(f"a Profile named {name!r} already exists")


def test_a_failed_create_profile_shows_an_error_instead_of_closing_the_popover():
    stub = _CreateProfileFailsDaemonStub()

    root = _build(stub, {})
    new_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New Profile")

    _fill_and_submit_name_prompt(new_btn, "Gaming", "Create")

    error_label = find_one(_popover_of(new_btn), lambda w: "error" in w.get_css_classes())
    assert error_label.get_visible()
    assert error_label.get_label()


def test_clicking_the_held_tab_switches_which_layer_is_shown_and_edited():
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", "held", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []})
    ui_state = {}

    root = _build(stub, ui_state)
    held_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Held")
    assert held_btn.get_sensitive(), "the Held tab itself must always be clickable"

    held_btn.emit("clicked")
    assert ui_state["selected_layer"] == "held"

    rebuilt = _build(stub, ui_state)
    grid_r1c1_btn = find_one(rebuilt, lambda w: "bound" in w.get_css_classes() if isinstance(w, Gtk.Button) else False)
    heading = find_one(editor_content(grid_r1c1_btn), lambda w: "heading" in w.get_css_classes())
    assert heading.get_label() == "Default / held / 1"


def test_mode_key_button_is_disabled_under_the_default_layer_switch_role():
    stub = DaemonStub()

    root = _build(stub, {})

    mode_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and "mode-key" in w.get_css_classes())
    assert not mode_btn.get_sensitive()


def test_toggling_mode_key_role_to_bound_enables_its_binding_editor():
    stub = DaemonStub()
    ui_state = {}

    root = _build(stub, ui_state)
    role_btn = find_one(root, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() == "Mode key: Layer-shift")
    role_btn.set_active(True)

    assert stub.get_config()["profiles"]["Default"]["mode_key_role"] == "bound"

    rebuilt = _build(stub, ui_state)
    mode_btn = find_one(rebuilt, lambda w: isinstance(w, Gtk.Button) and "mode-key" in w.get_css_classes())
    assert mode_btn.get_sensitive()


class _RoleFailsDaemonStub(DaemonStub):
    def set_mode_key_role(self, role):
        raise DaemonError("dispatch task is not responding")


def test_a_failed_mode_key_role_change_reverts_the_toggle_button():
    stub = _RoleFailsDaemonStub()

    root = _build(stub, {})
    role_btn = find_one(root, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() == "Mode key: Layer-shift")

    role_btn.set_active(True)

    # The Daemon call failed, so the visible toggle state must not disagree
    # with what the Daemon actually has (still layer_switch) — matching
    # build_binding_editor's Save/Clear error handling.
    assert role_btn.get_active() is False
    assert stub.get_config()["profiles"]["Default"]["mode_key_role"] == "layer_switch"


def test_compute_status_collapses_the_two_booleans_to_the_three_reachable_states():
    # device_connected is meaningless while the Daemon isn't running (ticket
    # 12) — a not-running Daemon reporting device_connected=True must still
    # collapse to "not_running", not a nonexistent fourth state.
    assert compute_status(daemon_running=False, device_connected=False) == "not_running"
    assert compute_status(daemon_running=False, device_connected=True) == "not_running"
    assert compute_status(daemon_running=True, device_connected=False) == "running_disconnected"
    assert compute_status(daemon_running=True, device_connected=True) == "running_connected"


def test_running_connected_status_enables_the_grid_with_no_dim_overlay():
    stub = DaemonStub()

    outer = _build_status(stub, "running_connected")

    root = _device_overview_root(outer)
    assert root.get_sensitive()
    # Scoped to the dim-overlay specifically (its "dim-overlay" CSS class),
    # not `Gtk.Overlay` generally — ticket 26's Actuation & release section
    # legitimately builds its own `Gtk.Overlay`s (the live depth bar, the
    # digital-mode fallback note) inside every Grid key's editor, which a
    # bare `isinstance` check would now always find regardless of status.
    assert find_all(outer, lambda w: "dim-overlay" in w.get_css_classes()) == []


def test_not_running_status_disables_the_grid_under_its_own_message():
    stub = DaemonStub()

    outer = _build_status(stub, "not_running")

    root = _device_overview_root(outer)
    assert not root.get_sensitive()
    msg = find_one(outer, lambda w: "dim-overlay-label" in w.get_css_classes())
    assert msg.get_label() == "Daemon not running — start it to edit Bindings"


def test_running_disconnected_status_disables_the_grid_under_its_own_message():
    stub = DaemonStub()

    outer = _build_status(stub, "running_disconnected")

    root = _device_overview_root(outer)
    assert not root.get_sensitive()
    msg = find_one(outer, lambda w: "dim-overlay-label" in w.get_css_classes())
    assert msg.get_label() == "Device disconnected — plug in the Tartarus Pro to edit Bindings"


def test_status_badge_shows_the_right_label_per_state():
    stub = DaemonStub()
    expected = {
        "running_connected": "Connected",
        "running_disconnected": "Daemon running — device disconnected",
        "not_running": "Daemon not running",
    }

    for status, label_text in expected.items():
        outer = _build_status(stub, status)
        badge = find_one(outer, lambda w: "status-badge" in w.get_css_classes())
        label = find_one(badge, lambda w: isinstance(w, Gtk.Label))
        assert label_text in label.get_label()


# --- Status LEDs lozenge group (tartarus-status-leds ticket 04) ---


def _status_led(root, colour: str) -> Gtk.Button:
    return find_one(
        root,
        lambda w: isinstance(w, Gtk.Button) and f"status-led-{colour}" in w.get_css_classes(),
    )


def _status_leds(root) -> list[Gtk.Button]:
    return find_all(
        root, lambda w: isinstance(w, Gtk.Button) and "status-led" in w.get_css_classes()
    )


def test_status_leds_group_renders_lit_and_unlit_from_the_active_profiles_config():
    stub = DaemonStub()
    stub.set_status_leds(True, False, True)
    stub.calls.clear()

    root = _build(stub, {})

    heading = find_one(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Status LEDs")
    assert "heading" in heading.get_css_classes()

    orange, green, blue = (_status_led(root, c) for c in ("orange", "green", "blue"))
    assert "lit" in orange.get_css_classes()
    assert "lit" not in green.get_css_classes()
    assert "lit" in blue.get_css_classes()

    assert orange.get_tooltip_text() == "Orange status LED — on"
    assert green.get_tooltip_text() == "Green status LED — off"
    assert blue.get_tooltip_text() == "Blue status LED — on"

    # No visible text on the lozenges themselves.
    assert all(led.get_label() is None for led in _status_leds(root))


def test_clicking_a_status_led_calls_set_status_leds_with_the_full_triple():
    stub = DaemonStub()  # seed Profile: all three off
    root = _build(stub, {})

    _status_led(root, "green").emit("clicked")

    assert stub.calls == [("set_status_leds", False, True, False)]


def test_clicking_a_lit_status_led_turns_only_that_channel_off():
    stub = DaemonStub()
    stub.set_status_leds(True, True, True)
    stub.calls.clear()

    root = _build(stub, {})
    _status_led(root, "blue").emit("clicked")

    assert stub.calls == [("set_status_leds", True, True, False)]


def test_status_leds_group_rebuilds_from_config_on_change():
    stub = DaemonStub()
    ui_state = {}
    root = _build(stub, ui_state)
    _status_led(root, "orange").emit("clicked")

    rebuilt = _build(stub, ui_state)
    assert "lit" in _status_led(rebuilt, "orange").get_css_classes()
    assert _status_led(rebuilt, "orange").get_tooltip_text() == "Orange status LED — on"


def test_a_newly_created_profile_shows_all_status_leds_dark():
    stub = DaemonStub()
    stub.set_status_leds(True, True, True)  # dirty the original Profile
    stub.create_profile("Fresh")
    stub.switch_profile("Fresh")

    root = _build(stub, {})

    assert all("lit" not in led.get_css_classes() for led in _status_leds(root))
    assert len(_status_leds(root)) == 3


def test_status_leds_group_renders_identically_on_base_and_held():
    stub = DaemonStub()
    stub.set_status_leds(True, False, False)

    base = _build(stub, {"selected_layer": "base"})
    held = _build(stub, {"selected_layer": "held"})

    for root in (base, held):
        assert "lit" in _status_led(root, "orange").get_css_classes()
        assert "lit" not in _status_led(root, "green").get_css_classes()
        assert "lit" not in _status_led(root, "blue").get_css_classes()


def test_status_leds_group_is_grid_destination_only():
    stub = DaemonStub()

    grid = _build(stub, {"dest": "grid"})
    assert len(_status_leds(grid)) == 3

    library = _build(stub, {"dest": "library"})
    assert _status_leds(library) == []


def test_status_leds_group_still_renders_stored_state_when_device_disconnected():
    stub = DaemonStub()
    stub.set_status_leds(False, True, False)
    stub.simulate_device_disconnected()

    outer = _build_status(stub, "running_disconnected")

    assert "lit" in _status_led(outer, "green").get_css_classes()
    assert "lit" not in _status_led(outer, "orange").get_css_classes()
    assert _status_led(outer, "green").get_tooltip_text() == "Green status LED — on"


def test_placeholder_config_renders_before_the_daemon_ever_answers():
    # Regression: the pre-Daemon launch path (app.py's `last_known["config"]
    # = PLACEHOLDER_CONFIG`) builds the whole Device Overview — grid buttons
    # each eagerly build a Binding editor (reads `default_actuation` /
    # `actuation_overrides`) and the Status LEDs group reads `status_leds`.
    # A key missing from PLACEHOLDER_CONFIG is a launch-time KeyError.
    class _InertClient:
        def __getattr__(self, _name):
            return lambda *a, **k: None

    outer = build_status_wrapped_view(
        _InertClient(), PLACEHOLDER_CONFIG, "Default", "base", "not_running", lambda: None, {}
    )

    root = _device_overview_root(outer)
    assert not root.get_sensitive()
    leds = _status_leds(root)
    assert len(leds) == 3
    assert all("lit" not in led.get_css_classes() for led in leds)
