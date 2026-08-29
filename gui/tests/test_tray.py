import os

from acheron_gui.daemon_client import NotFoundError
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.tray import (
    BUNDLED_ICON_DIR,
    ICON_NAMES,
    ITEM_OBJECT_PATH,
    MENU_OBJECT_PATH,
    WATCHER_BUS_NAME,
    WATCHER_OBJECT_PATH,
    TrayIcon,
)


class _FakeWatcherProxy:
    def __init__(self):
        self.calls: list[tuple] = []

    def RegisterStatusNotifierItem(self, service):
        self.calls.append(("RegisterStatusNotifierItem", service))


class _FakeBus:
    """Stands in for `dasbus.connection.SessionMessageBus` — the seam
    `TrayIcon`'s own `bus` parameter exists for, so tests never register a
    throwaway icon on the developer's real desktop tray (mirrors
    `daemon_stub.DaemonStub`'s role for the real session bus `com.acheron.Daemon`
    talks over)."""

    def __init__(self):
        self.published: dict[str, object] = {}
        self.watcher_proxy = _FakeWatcherProxy()

    def publish_object(self, path, obj):
        self.published[path] = obj

    def get_proxy(self, service_name, object_path):
        assert service_name == WATCHER_BUS_NAME
        assert object_path == WATCHER_OBJECT_PATH
        return self.watcher_proxy


class _FakeSystemdClient:
    def __init__(self):
        self.calls: list[str] = []

    def ensure_daemon_started(self) -> None:
        self.calls.append("ensure_daemon_started")

    def stop_daemon(self) -> None:
        self.calls.append("stop_daemon")

    def start_daemon(self) -> None:
        self.calls.append("start_daemon")


def _make_tray(client=None, systemd_client=None, on_show_window=None, on_quit=None):
    bus = _FakeBus()
    show_calls = []
    quit_calls = []
    tray = TrayIcon(
        client or DaemonStub(),
        systemd_client or _FakeSystemdClient(),
        on_show_window or (lambda: show_calls.append(None)),
        on_quit or (lambda: quit_calls.append(None)),
        bus=bus,
    )
    return tray, bus, show_calls, quit_calls


def _menu_layout(bus):
    menu = bus.published[MENU_OBJECT_PATH]
    _revision, layout = menu.GetLayout(0, -1, [])
    return menu, layout


def _child_by_label(layout_children, label):
    for entry in layout_children:
        item_id, properties, children = entry
        if properties.get("label") == label:
            return item_id, properties, children
    raise AssertionError(f"no menu item labeled {label!r} among {layout_children!r}")


def test_construction_publishes_both_objects_and_registers_with_the_watcher():
    _tray, bus, _show, _quit = _make_tray()

    assert ITEM_OBJECT_PATH in bus.published
    assert MENU_OBJECT_PATH in bus.published
    assert bus.watcher_proxy.calls == [("RegisterStatusNotifierItem", ITEM_OBJECT_PATH)]


def test_icon_name_reflects_the_three_status_states():
    tray, bus, _show, _quit = _make_tray()
    config = {"profiles": {"Default": {}}}

    tray.update(config, "Default", "running_connected")
    assert tray.icon_name == "acheron-running-connected"

    tray.update(config, "Default", "running_disconnected")
    assert tray.icon_name == "acheron-running-disconnected"

    tray.update(config, "Default", "not_running")
    assert tray.icon_name == "acheron-not-running"


def _bundled_bytes(icon_name):
    with open(os.path.join(BUNDLED_ICON_DIR, f"{icon_name}.svg"), "rb") as handle:
        return handle.read()


def test_icon_theme_path_is_the_configured_dir_and_never_the_package():
    target = os.environ["ACHERON_TRAY_ICON_DIR"]
    tray, bus, _show, _quit = _make_tray()

    assert tray.icon_theme_path == target
    assert bus.published[ITEM_OBJECT_PATH].IconThemePath == target
    # The whole point of ticket 97: it must not resolve into the (possibly
    # git-checkout) package dir.
    assert os.path.abspath(tray.icon_theme_path) != os.path.abspath(BUNDLED_ICON_DIR)


def test_construction_syncs_the_three_bundled_status_icons():
    tray, _bus, _show, _quit = _make_tray()

    for icon_name in ICON_NAMES.values():
        synced = os.path.join(tray.icon_theme_path, f"{icon_name}.svg")
        assert os.path.isfile(synced)
        with open(synced, "rb") as handle:
            assert handle.read() == _bundled_bytes(icon_name)


def test_a_stale_synced_icon_is_refreshed_on_construction():
    target_dir = os.environ["ACHERON_TRAY_ICON_DIR"]
    os.makedirs(target_dir, exist_ok=True)
    stale = os.path.join(target_dir, "acheron-not-running.svg")
    with open(stale, "wb") as handle:
        handle.write(b"<svg>stale</svg>")

    _tray, _bus, _show, _quit = _make_tray()

    with open(stale, "rb") as handle:
        assert handle.read() == _bundled_bytes("acheron-not-running")


def test_an_unwritable_icon_dir_does_not_raise(tmp_path, monkeypatch, capsys):
    blocker = tmp_path / "blocker"
    blocker.write_text("not a directory")
    monkeypatch.setenv("ACHERON_TRAY_ICON_DIR", str(blocker / "tray-icons"))

    tray, _bus, _show, _quit = _make_tray()  # must not raise

    assert "could not sync tray icons" in capsys.readouterr().err
    # The path is still reported (the host just renders no icon).
    assert tray.icon_theme_path == str(blocker / "tray-icons")


def test_item_properties_are_spec_correct():
    tray, bus, _show, _quit = _make_tray()
    tray.update({"profiles": {"Default": {}}}, "Default", "running_connected")

    item = bus.published[ITEM_OBJECT_PATH]
    assert item.Menu == MENU_OBJECT_PATH
    assert item.ItemIsMenu is True
    assert item.Status == "Active"
    assert item.IconName == "acheron-running-connected"


def test_menu_lists_status_show_window_switch_profile_pause_quit_in_order():
    tray, bus, _show, _quit = _make_tray()
    tray.update({"profiles": {"Default": {}, "Gaming": {}}}, "Default", "running_disconnected")

    _menu, layout = _menu_layout(bus)
    _root_id, _props, children = layout
    labels = [props["label"] for _id, props, _children in children]

    assert labels == [
        "Daemon running — device disconnected",
        "Show Window",
        "Switch Profile",
        "Pause Daemon",
        "Quit",
    ]


def test_clicking_show_window_calls_the_callback():
    tray, bus, show_calls, _quit = _make_tray()
    tray.update({"profiles": {"Default": {}}}, "Default", "running_connected")

    menu, layout = _menu_layout(bus)
    _root_id, _props, children = layout
    show_window_id, _p, _c = _child_by_label(children, "Show Window")

    menu.Event(show_window_id, "clicked", None, 0)

    assert show_calls == [None]


def test_clicking_quit_calls_the_callback():
    tray, bus, _show, quit_calls = _make_tray()
    tray.update({"profiles": {"Default": {}}}, "Default", "running_connected")

    menu, layout = _menu_layout(bus)
    _root_id, _props, children = layout
    quit_id, _p, _c = _child_by_label(children, "Quit")

    menu.Event(quit_id, "clicked", None, 0)

    assert quit_calls == [None]


def test_clicking_a_profile_in_the_submenu_switches_to_it():
    stub = DaemonStub()
    stub.create_profile("Gaming")
    tray, bus, _show, _quit = _make_tray(client=stub)
    tray.update(stub.get_config(), "Default", "running_connected")

    menu, layout = _menu_layout(bus)
    _root_id, _props, children = layout
    _switch_id, _switch_props, switch_children = _child_by_label(children, "Switch Profile")
    gaming_id, _p, _c = _child_by_label(switch_children, "Gaming")

    menu.Event(gaming_id, "clicked", None, 0)

    assert stub.get_state()["profile"] == "Gaming"


def test_a_failed_profile_switch_from_the_tray_does_not_raise(capsys):
    class _FailingClient(DaemonStub):
        def switch_profile(self, name):
            raise NotFoundError(f"no Profile named {name!r}")

    stub = _FailingClient()
    stub.create_profile("Gaming")
    tray, bus, _show, _quit = _make_tray(client=stub)
    tray.update(stub.get_config(), "Default", "running_connected")

    menu, layout = _menu_layout(bus)
    _root_id, _props, children = layout
    _switch_id, _switch_props, switch_children = _child_by_label(children, "Switch Profile")
    gaming_id, _p, _c = _child_by_label(switch_children, "Gaming")

    menu.Event(gaming_id, "clicked", None, 0)  # must not raise

    assert "Switch Profile failed" in capsys.readouterr().err


def test_clicking_pause_daemon_stops_the_unit_over_systemd():
    systemd = _FakeSystemdClient()
    tray, bus, _show, _quit = _make_tray(systemd_client=systemd)
    tray.update({"profiles": {"Default": {}}}, "Default", "running_connected")

    menu, layout = _menu_layout(bus)
    _root_id, _props, children = layout
    pause_id, _p, _c = _child_by_label(children, "Pause Daemon")

    menu.Event(pause_id, "clicked", None, 0)

    assert systemd.calls == ["stop_daemon"]


def test_clicking_resume_daemon_starts_the_unit_over_systemd():
    systemd = _FakeSystemdClient()
    tray, bus, _show, _quit = _make_tray(systemd_client=systemd)
    tray.update({"profiles": {"Default": {}}}, "Default", "not_running")

    menu, layout = _menu_layout(bus)
    _root_id, _props, children = layout
    resume_id, _p, _c = _child_by_label(children, "Resume Daemon")

    menu.Event(resume_id, "clicked", None, 0)

    assert systemd.calls == ["start_daemon"]


def test_update_bumps_the_menu_revision_and_signals_layout_updated():
    tray, bus, _show, _quit = _make_tray()
    menu = bus.published[MENU_OBJECT_PATH]
    received = []
    menu.LayoutUpdated.connect(lambda revision, parent: received.append((revision, parent)))

    tray.update({"profiles": {"Default": {}}}, "Default", "running_connected")
    tray.update({"profiles": {"Default": {}}}, "Default", "not_running")

    assert received == [(1, 0), (2, 0)]


def _unwrap(props):
    return {name: value.unpack() for name, value in props.items()}


def test_update_emits_items_properties_updated_for_a_changed_label():
    tray, bus, _show, _quit = _make_tray()
    menu = bus.published[MENU_OBJECT_PATH]
    received = []
    menu.ItemsPropertiesUpdated.connect(
        lambda changed, removed: received.append((changed, removed))
    )

    config = {"profiles": {"Default": {}}}
    tray.update(config, "Default", "running_connected")
    # First build populates every item fresh — nothing to patch yet.
    assert received == []

    tray.update(config, "Default", "not_running")

    assert len(received) == 1
    changed, removed = received[0]
    assert removed == []
    patched = {item_id: _unwrap(props) for item_id, props in changed}
    # The Pause/Resume row flipped its label; the status line changed too.
    assert "Resume Daemon" in [p["label"] for p in patched.values()]


def test_update_emits_items_properties_updated_for_the_active_profile_greying():
    stub = DaemonStub()
    stub.create_profile("Gaming")
    tray, bus, _show, _quit = _make_tray(client=stub)
    menu = bus.published[MENU_OBJECT_PATH]
    received = []
    menu.ItemsPropertiesUpdated.connect(
        lambda changed, removed: received.append((changed, removed))
    )

    tray.update(stub.get_config(), "Default", "running_connected")
    tray.update(stub.get_config(), "Gaming", "running_connected")

    changed, _removed = received[-1]
    patched = {_unwrap(props)["label"]: _unwrap(props) for _id, props in changed}
    assert patched["Default"]["enabled"] is True
    assert patched["Gaming"]["enabled"] is False


def test_update_does_not_emit_items_properties_updated_when_nothing_changed():
    tray, bus, _show, _quit = _make_tray()
    menu = bus.published[MENU_OBJECT_PATH]
    received = []
    menu.ItemsPropertiesUpdated.connect(lambda *a: received.append(a))

    config = {"profiles": {"Default": {}}}
    tray.update(config, "Default", "running_connected")
    tray.update(config, "Default", "running_connected")

    assert received == []


def test_update_emits_new_icon_new_title_and_new_status():
    tray, bus, _show, _quit = _make_tray()
    item = bus.published[ITEM_OBJECT_PATH]
    icon_calls = []
    title_calls = []
    status_calls = []
    item.NewIcon.connect(lambda: icon_calls.append(None))
    item.NewTitle.connect(lambda: title_calls.append(None))
    item.NewStatus.connect(lambda status: status_calls.append(status))

    tray.update({"profiles": {"Default": {}}}, "Default", "running_connected")

    assert icon_calls == [None]
    assert title_calls == [None]
    assert status_calls == ["Active"]
