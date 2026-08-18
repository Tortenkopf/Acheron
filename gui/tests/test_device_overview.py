from gi.repository import Gtk

from acheron_gui.daemon_client import AlreadyExistsError, DaemonError, NotFoundError
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.device_overview import build_main_view, build_status_wrapped_view, compute_status
from acheron_gui.inputs import ALL_INPUTS

from .widget_tree import find_all, find_one


def _build(stub, ui_state):
    config = stub.get_config()
    state = stub.get_state()
    profile, layer = state["profile"], state["layer"]
    return build_main_view(stub, config, profile, layer, lambda: None, ui_state)


def _build_status(stub, status: str, ui_state=None):
    config = stub.get_config()
    state = stub.get_state()
    profile, layer = state["profile"], state["layer"]
    return build_status_wrapped_view(
        stub, config, profile, layer, status, lambda: None, ui_state or {"table_open": False}
    )


def _device_overview_root(outer: Gtk.Widget) -> Gtk.Widget:
    """Locates `build_main_view`'s own returned root inside
    `build_status_wrapped_view`'s wrapper: directly the last child of
    `outer` when healthy (`[badge, root]`), or the `Gtk.Overlay`'s main
    child when not (`[badge, Gtk.Overlay(root, dim-overlay)]`)."""
    last = outer.get_last_child()
    if isinstance(last, Gtk.Overlay):
        return last.get_child()
    return last


def _action_table_toggle(root):
    return find_one(
        root, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() in ("Action Table ▸", "Action Table ◂")
    )


def test_device_overview_renders_one_button_per_input():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})

    # Filtered to "bound"/"empty"-classed buttons — the tray mock's own
    # "Quick switch" control is also a Gtk.MenuButton but isn't an Input.
    input_buttons = find_all(
        root,
        lambda w: isinstance(w, Gtk.MenuButton) and ("bound" in w.get_css_classes() or "empty" in w.get_css_classes()),
    )
    assert len(input_buttons) == len(ALL_INPUTS)


def test_action_table_sidebar_closed_by_default():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})

    revealer = find_all(root, lambda w: isinstance(w, Gtk.Revealer))[0]
    assert revealer.get_reveal_child() is False


def test_action_table_open_state_survives_a_rebuild():
    stub = DaemonStub()
    ui_state = {"table_open": False}
    root = _build(stub, ui_state)
    toggle = _action_table_toggle(root)

    toggle.set_active(True)
    assert ui_state["table_open"] is True

    # Simulate a rebuild (a fresh widget tree from a fresh Gtk.Revealer,
    # which defaults closed) — the *state dict* carrying table_open across
    # it is what should keep the sidebar open, per ticket 09.
    rebuilt_root = _build(stub, ui_state)
    rebuilt_revealer = find_all(rebuilt_root, lambda w: isinstance(w, Gtk.Revealer))[0]
    assert rebuilt_revealer.get_reveal_child() is True


def _profile_sidebar(root: Gtk.Widget) -> Gtk.Widget:
    """Scopes a search to the left-hand Profile sidebar specifically — a
    Profile's name button is also duplicated in the tray mock's "Quick
    switch" popover, so an unscoped label search would match both."""
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

    root = _build(stub, {"table_open": False})
    switch_btn = find_one(_profile_sidebar(root), lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Gaming")

    switch_btn.emit("clicked")

    assert stub.get_state()["profile"] == "Gaming"
    assert ("switch_profile", "Gaming") in stub.calls


def test_clicking_the_already_active_profiles_sidebar_button_is_a_no_op():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})
    switch_btn = find_one(_profile_sidebar(root), lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Default")

    switch_btn.emit("clicked")

    # No superfluous SwitchProfile call for the Profile already active — a
    # real SwitchProfile would force-stop every running Toggle even when
    # switching "to" the Profile that's already live.
    assert stub.calls == []


def test_delete_is_disabled_for_the_active_profile_and_enabled_for_others():
    stub = DaemonStub()
    stub.create_profile("Gaming")

    root = _build(stub, {"table_open": False})
    delete_btns = find_all(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×")

    sensitivities = {b.get_sensitive() for b in delete_btns}
    assert sensitivities == {True, False}, "exactly one active (disabled) and one non-active (enabled) Profile"


def test_clicking_delete_on_a_non_active_profile_removes_it():
    stub = DaemonStub()
    stub.create_profile("Gaming")

    root = _build(stub, {"table_open": False})
    delete_btn = find_one(
        root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "×" and w.get_sensitive()
    )

    delete_btn.emit("clicked")

    assert "Gaming" not in stub.get_config()["profiles"]


def test_creating_a_profile_via_the_new_profile_popover_calls_create_profile():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})
    new_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New Profile")

    _fill_and_submit_name_prompt(new_btn, "Gaming", "Create")

    assert "Gaming" in stub.get_config()["profiles"]
    assert ("create_profile", "Gaming") in stub.calls


def test_renaming_the_active_profile_via_its_popover_calls_rename_profile():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})
    rename_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "✎")

    _fill_and_submit_name_prompt(rename_btn, "Renamed", "Rename")

    assert stub.get_config()["active_profile"] == "Renamed"
    assert ("rename_profile", "Default", "Renamed") in stub.calls


def test_tray_quick_switch_calls_switch_profile():
    stub = DaemonStub()
    stub.create_profile("Gaming")

    root = _build(stub, {"table_open": False})
    quick_switch = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "Quick switch ▾")
    popover = _popover_of(quick_switch)
    gaming_btn = find_one(popover, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Gaming")

    gaming_btn.emit("clicked")

    assert stub.get_state()["profile"] == "Gaming"


def test_a_failed_tray_quick_switch_shows_an_error_and_keeps_the_popover_open():
    stub = _SwitchProfileFailsDaemonStub()
    stub.create_profile("Gaming")

    root = _build(stub, {"table_open": False})
    quick_switch = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "Quick switch ▾")
    popover = _popover_of(quick_switch)
    gaming_btn = find_one(popover, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Gaming")

    gaming_btn.emit("clicked")

    error_label = find_one(popover, lambda w: "error" in w.get_css_classes())
    assert error_label.get_visible()
    assert stub.get_state()["profile"] == "Default"


def test_switching_profile_via_the_sidebar_clears_active_toggles():
    # The GUI-level half of ticket 19's live demo: a Toggle left running in
    # the first Profile must be gone from GetState() the instant the switch
    # (from either the sidebar or the tray) lands — exact-key-release is the
    # Daemon's own tested responsibility, not something this GUI-level test
    # can observe.
    stub = DaemonStub()
    stub.create_profile("Gaming")
    stub.simulate_toggle_started("grid_r1c1")
    assert stub.get_state()["active_toggles"] == ["grid_r1c1"]

    root = _build(stub, {"table_open": False})
    switch_btn = find_one(_profile_sidebar(root), lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Gaming")
    switch_btn.emit("clicked")

    assert stub.get_state()["active_toggles"] == []


class _SwitchProfileFailsDaemonStub(DaemonStub):
    def switch_profile(self, name):
        raise NotFoundError(f"no Profile named {name!r}")


def test_a_failed_switch_shows_an_error_in_the_sidebar_instead_of_swallowing_it():
    stub = _SwitchProfileFailsDaemonStub()
    stub.create_profile("Gaming")

    root = _build(stub, {"table_open": False})
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

    root = _build(stub, {"table_open": False})
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

    root = _build(stub, {"table_open": False})
    new_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New Profile")

    _fill_and_submit_name_prompt(new_btn, "Gaming", "Create")

    error_label = find_one(_popover_of(new_btn), lambda w: "error" in w.get_css_classes())
    assert error_label.get_visible()
    assert error_label.get_label()


def test_clicking_the_held_tab_switches_which_layer_is_shown_and_edited():
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", "held", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []})
    ui_state = {"table_open": False, "expanded_rows": set()}

    root = _build(stub, ui_state)
    held_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Held")
    assert held_btn.get_sensitive(), "the Held tab itself must always be clickable"

    held_btn.emit("clicked")
    assert ui_state["selected_layer"] == "held"

    rebuilt = _build(stub, ui_state)
    grid_r1c1_btn = find_one(rebuilt, lambda w: "bound" in w.get_css_classes() if isinstance(w, Gtk.MenuButton) else False)
    heading = find_one(grid_r1c1_btn.get_popover(), lambda w: "heading" in w.get_css_classes())
    assert heading.get_label() == "Default / held / 1"


def test_mode_key_button_is_disabled_under_the_default_layer_switch_role():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})

    mode_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and "mode-key" in w.get_css_classes())
    assert not mode_btn.get_sensitive()


def test_toggling_mode_key_role_to_bound_enables_its_binding_editor():
    stub = DaemonStub()
    ui_state = {"table_open": False}

    root = _build(stub, ui_state)
    role_btn = find_one(root, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() == "Mode key: Layer-shift")
    role_btn.set_active(True)

    assert stub.get_config()["profiles"]["Default"]["mode_key_role"] == "bound"

    rebuilt = _build(stub, ui_state)
    mode_btn = find_one(rebuilt, lambda w: isinstance(w, Gtk.MenuButton) and "mode-key" in w.get_css_classes())
    assert mode_btn.get_sensitive()


class _RoleFailsDaemonStub(DaemonStub):
    def set_mode_key_role(self, role):
        raise DaemonError("dispatch task is not responding")


def test_a_failed_mode_key_role_change_reverts_the_toggle_button():
    stub = _RoleFailsDaemonStub()

    root = _build(stub, {"table_open": False})
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
    assert find_all(outer, lambda w: isinstance(w, Gtk.Overlay)) == []


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


def test_tray_mock_gets_a_status_line_inserted_right_after_its_heading():
    stub = DaemonStub()

    outer = _build_status(stub, "running_disconnected")

    tray_box = find_one(outer, lambda w: "tray-mock" in w.get_css_classes())
    heading = tray_box.get_first_child()
    status_line = heading.get_next_sibling()
    assert "Daemon running — device disconnected" in status_line.get_label()
