from gi.repository import Gtk

from acheron_gui.daemon_client import AlreadyExistsError, DaemonError, NotFoundError
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.device_overview import build_main_view, build_status_wrapped_view, compute_status
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


def test_grid_destination_has_a_reserved_chords_slot_naming_ticket_40():
    stub = DaemonStub()

    root = _build(stub, {})

    find_one(root, lambda w: isinstance(w, Gtk.Label) and w.get_label() == "Chords")
    placeholder = find_one(
        root, lambda w: isinstance(w, Gtk.Label) and "ticket 40" in w.get_label().lower()
    )
    assert placeholder.get_label()


def test_steppers_tab_names_ticket_55_as_a_stub():
    stub = DaemonStub()
    ui_state = {"dest": "library", "library_tab": "steppers"}

    root = _build(stub, ui_state)

    placeholder = find_one(
        root, lambda w: isinstance(w, Gtk.Label) and "ticket 55" in w.get_label().lower()
    )
    assert placeholder.get_label()


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
    library_root = _build(stub, {"dest": "library"})

    grid_sidebar = _profile_sidebar(grid_root)
    library_sidebar = _profile_sidebar(library_root)
    assert grid_sidebar.get_hexpand() is False
    assert library_sidebar.get_hexpand() is False


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
