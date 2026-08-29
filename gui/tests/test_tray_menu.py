from acheron_gui.tray_menu import ROOT_ID, MenuModel, build_menu_items


def _build(status="running_connected", profile="Default", profiles=None, daemon_running=True, **callbacks):
    calls = {"show_window": [], "switch_profile": [], "toggle_daemon": [], "quit": []}

    def record(key):
        def _cb(*args):
            calls[key].append(args)

        return _cb

    items = build_menu_items(
        status,
        profile,
        profiles if profiles is not None else [profile],
        daemon_running,
        callbacks.get("on_show_window", record("show_window")),
        callbacks.get("on_switch_profile", record("switch_profile")),
        callbacks.get("on_toggle_daemon", record("toggle_daemon")),
        callbacks.get("on_quit", record("quit")),
    )
    return items, calls


def _child(items, item_id, label):
    """Finds a direct child of `item_id` by its label."""
    for child_id in items[item_id].children:
        if items[child_id].properties.get("label") == label:
            return child_id
    raise AssertionError(f"no child of {item_id} labeled {label!r}")


def test_root_children_are_ordered_status_show_switch_pause_quit():
    items, _ = _build(profile="Default", profiles=["Default", "Gaming"])

    labels_or_kind = []
    for child_id in items[ROOT_ID].children:
        props = items[child_id].properties
        labels_or_kind.append(props["label"])

    assert labels_or_kind == [
        "Connected",
        "Show Window",
        "Switch Profile",
        "Pause Daemon",
        "Quit",
    ]


def test_status_line_reuses_status_states_label_and_is_disabled():
    items, _ = _build(status="running_disconnected")

    status_id = _child(items, ROOT_ID, "Daemon running — device disconnected")
    assert items[status_id].properties["enabled"] is False


def test_show_window_item_invokes_its_callback():
    items, calls = _build()
    show_window_id = _child(items, ROOT_ID, "Show Window")

    items[show_window_id].on_activate()

    assert calls["show_window"] == [()]


def test_switch_profile_submenu_lists_profiles_and_flags_children_display():
    items, _ = _build(profile="Default", profiles=["Default", "Gaming"])

    switch_id = _child(items, ROOT_ID, "Switch Profile")
    assert items[switch_id].properties["children-display"] == "submenu"
    child_labels = {items[cid].properties["label"] for cid in items[switch_id].children}
    assert child_labels == {"Default", "Gaming"}


def test_active_profile_entry_is_disabled_others_enabled():
    items, _ = _build(profile="Default", profiles=["Default", "Gaming"])

    switch_id = _child(items, ROOT_ID, "Switch Profile")
    default_id = _child(items, switch_id, "Default")
    gaming_id = _child(items, switch_id, "Gaming")

    assert items[default_id].properties["enabled"] is False
    assert items[gaming_id].properties["enabled"] is True


def test_clicking_a_profile_entry_calls_on_switch_profile_with_its_name():
    items, calls = _build(profile="Default", profiles=["Default", "Gaming"])
    switch_id = _child(items, ROOT_ID, "Switch Profile")
    gaming_id = _child(items, switch_id, "Gaming")

    items[gaming_id].on_activate()

    assert calls["switch_profile"] == [("Gaming",)]


def test_pause_resume_label_flips_with_daemon_running():
    running_items, _ = _build(daemon_running=True)
    stopped_items, _ = _build(daemon_running=False)

    assert _child(running_items, ROOT_ID, "Pause Daemon") is not None
    assert _child(stopped_items, ROOT_ID, "Resume Daemon") is not None


def test_pause_resume_item_invokes_its_callback():
    items, calls = _build(daemon_running=True)
    pause_id = _child(items, ROOT_ID, "Pause Daemon")

    items[pause_id].on_activate()

    assert calls["toggle_daemon"] == [()]


def test_quit_item_invokes_its_callback():
    items, calls = _build()
    quit_id = _child(items, ROOT_ID, "Quit")

    items[quit_id].on_activate()

    assert calls["quit"] == [()]


def test_model_starts_with_an_empty_root_and_revision_zero():
    model = MenuModel()

    assert model.revision == 0
    assert model.items[ROOT_ID].children == []


def test_rebuild_replaces_items_and_bumps_revision_each_time():
    model = MenuModel()
    items, _ = _build()

    model.rebuild(items)
    assert model.revision == 1
    assert model.items is items

    model.rebuild(_build()[0])
    assert model.revision == 2


def test_fixed_rows_keep_stable_ids_regardless_of_profile_count():
    one, _ = _build(profile="Default", profiles=["Default"])
    many, _ = _build(profile="Default", profiles=["Default", "Gaming", "Work"])

    def ids_by_label(items):
        return {
            items[cid].properties["label"]: cid for cid in items[ROOT_ID].children
        }

    assert ids_by_label(one) == ids_by_label(many)


def test_rebuild_delta_reports_the_pause_resume_label_flip():
    model = MenuModel()
    model.rebuild(_build(daemon_running=True)[0])

    changed, removed = model.rebuild(_build(daemon_running=False)[0])

    assert removed == []
    assert dict(changed)  # id -> new props
    pause_id = _child(model.items, ROOT_ID, "Resume Daemon")
    assert (pause_id, {"label": "Resume Daemon"}) in changed


def test_rebuild_delta_reports_the_active_profile_greying_on_a_switch():
    model = MenuModel()
    model.rebuild(_build(profile="Default", profiles=["Default", "Gaming"])[0])

    changed, _removed = model.rebuild(
        _build(profile="Gaming", profiles=["Default", "Gaming"])[0]
    )

    switch_id = _child(model.items, ROOT_ID, "Switch Profile")
    default_id = _child(model.items, switch_id, "Default")
    gaming_id = _child(model.items, switch_id, "Gaming")
    delta = dict(changed)
    assert delta[default_id]["enabled"] is True
    assert delta[gaming_id]["enabled"] is False


def test_rebuild_delta_is_empty_when_nothing_changed():
    model = MenuModel()
    model.rebuild(_build()[0])

    changed, removed = model.rebuild(_build()[0])

    assert (changed, removed) == ([], [])


def test_rebuild_delta_omits_a_newly_added_profile():
    model = MenuModel()
    model.rebuild(_build(profile="Default", profiles=["Default"])[0])

    changed, removed = model.rebuild(
        _build(profile="Default", profiles=["Default", "Gaming"])[0]
    )

    # The new "Gaming" entry is a fresh id — the host fetches its properties
    # itself off the LayoutUpdated re-scan, so it must not be in the delta.
    assert (changed, removed) == ([], [])


def test_get_layout_unlimited_depth_returns_the_whole_tree():
    model = MenuModel()
    items, _ = _build(profile="Default", profiles=["Default", "Gaming"])
    model.rebuild(items)

    revision, layout = model.get_layout(ROOT_ID, -1, [])

    assert revision == 1
    root_id, _props, children = layout
    assert root_id == ROOT_ID
    switch_layout = next(c for c in children if c[1]["label"] == "Switch Profile")
    grandchild_labels = {c[1]["label"] for c in switch_layout[2]}
    assert grandchild_labels == {"Default", "Gaming"}


def test_get_layout_depth_zero_returns_only_the_requested_item():
    model = MenuModel()
    items, _ = _build()
    model.rebuild(items)

    _revision, layout = model.get_layout(ROOT_ID, 0, [])

    _root_id, _props, children = layout
    assert children == []


def test_get_layout_depth_one_stops_before_grandchildren():
    model = MenuModel()
    items, _ = _build(profile="Default", profiles=["Default", "Gaming"])
    model.rebuild(items)

    _revision, layout = model.get_layout(ROOT_ID, 1, [])

    _root_id, _props, children = layout
    switch_layout = next(c for c in children if c[1]["label"] == "Switch Profile")
    assert switch_layout[2] == []


def test_get_layout_filters_properties_to_the_requested_names():
    model = MenuModel()
    items, _ = _build()
    model.rebuild(items)

    _revision, layout = model.get_layout(ROOT_ID, -1, ["label"])

    _root_id, _props, children = layout
    show_window = next(c for c in children if "label" in c[1] and c[1]["label"] == "Show Window")
    assert set(show_window[1]) == {"label"}


def test_get_group_properties_returns_requested_ids_and_skips_unknown():
    model = MenuModel()
    items, _ = _build()
    model.rebuild(items)
    show_window_id = _child(items, ROOT_ID, "Show Window")

    result = model.get_group_properties([show_window_id, 999], [])

    assert [item_id for item_id, _props in result] == [show_window_id]


def test_get_property_falls_back_to_dbusmenu_defaults():
    model = MenuModel()
    items, _ = _build()
    model.rebuild(items)
    show_window_id = _child(items, ROOT_ID, "Show Window")

    # `enabled`/`visible` aren't set explicitly on this item — the
    # com.canonical.dbusmenu spec's own default for both is True.
    assert model.get_property(show_window_id, "enabled") is True
    assert model.get_property(show_window_id, "visible") is True
    assert model.get_property(show_window_id, "label") == "Show Window"


def test_event_ignores_anything_other_than_clicked():
    model = MenuModel()
    items, calls = _build()
    model.rebuild(items)
    show_window_id = _child(items, ROOT_ID, "Show Window")

    model.event(show_window_id, "hovered", None, 0)

    assert calls["show_window"] == []


def test_event_clicked_invokes_the_items_callback():
    model = MenuModel()
    items, calls = _build()
    model.rebuild(items)
    show_window_id = _child(items, ROOT_ID, "Show Window")

    model.event(show_window_id, "clicked", None, 0)

    assert calls["show_window"] == [()]


def test_event_on_an_item_without_a_callback_is_a_safe_no_op():
    model = MenuModel()
    items, _ = _build()
    model.rebuild(items)
    status_id = _child(items, ROOT_ID, "Connected")

    model.event(status_id, "clicked", None, 0)  # must not raise


def test_about_to_show_always_reports_no_update_needed():
    model = MenuModel()

    assert model.about_to_show(ROOT_ID) is False
