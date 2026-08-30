from gi.repository import GLib

from acheron_gui.app import (
    _about_dialog_state,
    _activate_window,
    _build_primary_menu,
    _ensure_daemon_started_on_launch,
    _present_window,
    _wire_focus_tracking,
    _wire_status_tracking,
    _wire_window_close_to_hide,
)
from acheron_gui.daemon_client import DaemonError
from acheron_gui.daemon_stub import DaemonStub


def _pump_idle_callbacks() -> None:
    """`_wire_focus_tracking` defers its actual `set_output_suppressed` call
    via `GLib.idle_add` (to avoid nesting inside an in-flight `call_sync` —
    see `app.py`'s docstring), so tests need to drain the default
    `GMainContext` once to observe the effect, same as a running
    `Gtk.Application` main loop would do on its own."""
    context = GLib.MainContext.default()
    while context.iteration(False):
        pass


class _FakeFocusWindow:
    """Stands in for a real `Gtk.ApplicationWindow`'s `is-active` — a
    GTK-computed, read-only property (`set_property("is-active", ...)`
    raises) that only a real window manager can change, so it can't be
    driven directly in a headless test. Mirrors `DaemonStub`'s own
    `simulate_*` seam, just on the GTK side of ticket 25's wiring.
    """

    def __init__(self, initially_active: bool = False):
        self._active = initially_active
        self._handlers: list = []

    def is_active(self) -> bool:
        return self._active

    def connect(self, signal_name: str, callback) -> None:
        assert signal_name == "notify::is-active"
        self._handlers.append(callback)

    def simulate_focus_change(self, active: bool) -> None:
        self._active = active
        for handler in self._handlers:
            handler(self, None)


def test_initial_focus_state_is_pushed_once_on_connect():
    stub = DaemonStub()
    window = _FakeFocusWindow(initially_active=True)

    _wire_focus_tracking(window, stub)
    _pump_idle_callbacks()

    # Launching already focused also stops any Toggle a fresh connect might
    # find already running — same guard as any other focus-gain.
    assert stub.calls == [
        ("set_output_suppressed", True),
        ("stop_all_toggles",),
    ]


def test_initial_push_reflects_an_unfocused_start_too():
    stub = DaemonStub()
    window = _FakeFocusWindow(initially_active=False)

    _wire_focus_tracking(window, stub)
    _pump_idle_callbacks()

    assert stub.calls == [("set_output_suppressed", False)]


def test_gaining_focus_suppresses_output_and_stops_every_toggle():
    # Ticket 25's live-hardware finding: suppression alone can't undo a key
    # that armed the OS's own autorepeat before the GUI gained focus, so
    # focus-gain also force-stops any running Toggle, suppress-call first.
    stub = DaemonStub()
    window = _FakeFocusWindow(initially_active=False)
    _wire_focus_tracking(window, stub)
    _pump_idle_callbacks()

    window.simulate_focus_change(True)
    _pump_idle_callbacks()

    assert stub.calls == [
        ("set_output_suppressed", False),
        ("set_output_suppressed", True),
        ("stop_all_toggles",),
    ]


def test_losing_focus_resumes_output_without_touching_toggles():
    # No resume-on-unfocus, deliberately: a Toggle stopped by a focus-gain
    # guard must not be silently restarted by the GUI later losing focus.
    stub = DaemonStub()
    window = _FakeFocusWindow(initially_active=True)
    _wire_focus_tracking(window, stub)
    _pump_idle_callbacks()
    stub.calls.clear()

    window.simulate_focus_change(False)
    _pump_idle_callbacks()

    assert stub.calls == [("set_output_suppressed", False)]


def test_daemon_calls_are_not_made_synchronously_from_the_signal_handler():
    # Regression guard for the nested-call_sync reentrancy hazard the
    # module docstring describes: the signal handler must only ever queue
    # the Daemon calls via GLib.idle_add, never call straight through.
    stub = DaemonStub()
    window = _FakeFocusWindow(initially_active=False)
    _wire_focus_tracking(window, stub)
    _pump_idle_callbacks()
    stub.calls.clear()

    window.simulate_focus_change(True)

    assert stub.calls == []


def test_status_tracking_starts_conservative_before_any_signal_is_pumped():
    stub = DaemonStub()

    status = _wire_status_tracking(stub, lambda: None)

    # `subscribe_daemon_running_changed` fires its initial announce
    # synchronously within this call (`DaemonStub` reports the
    # already-known state right away, mirroring the real
    # `Gio.bus_watch_name`), but `_wire_status_tracking` defers actually
    # applying it via `GLib.idle_add` — same reentrancy guard
    # `_wire_focus_tracking` uses — so nothing's visible in `status` until
    # that's pumped, even though `DaemonStub` starts running+connected.
    assert status == {"daemon_running": False, "device_connected": False, "capture_mode": "digital"}


def test_status_tracking_reflects_the_daemon_already_running_on_launch():
    stub = DaemonStub()  # starts running (a fresh install's steady state)

    status = _wire_status_tracking(stub, lambda: None)
    _pump_idle_callbacks()

    assert status["daemon_running"] is True


def test_status_tracking_reflects_the_daemon_stopping_and_restarting():
    stub = DaemonStub()
    status = _wire_status_tracking(stub, lambda: None)
    _pump_idle_callbacks()
    assert status["daemon_running"] is True

    stub.simulate_daemon_stopped()
    _pump_idle_callbacks()
    assert status == {"daemon_running": False, "device_connected": False, "capture_mode": "digital"}

    stub.simulate_daemon_started()
    _pump_idle_callbacks()
    assert status["daemon_running"] is True


def test_status_tracking_reflects_device_connection_changes_while_running():
    stub = DaemonStub()
    status = _wire_status_tracking(stub, lambda: None)
    _pump_idle_callbacks()

    stub.simulate_device_disconnected()
    _pump_idle_callbacks()
    assert status == {"daemon_running": True, "device_connected": False, "capture_mode": "digital"}

    stub.simulate_device_connected()
    _pump_idle_callbacks()
    assert status == {"daemon_running": True, "device_connected": True, "capture_mode": "digital"}


def test_status_tracking_forces_device_connected_false_when_the_daemon_stops():
    # "Not running" already implies "not connected" (device_overview.py's
    # compute_status) — there's no live poll loop left to ask once the
    # Daemon itself is gone, so this must not keep reporting a stale `True`.
    stub = DaemonStub()
    status = _wire_status_tracking(stub, lambda: None)
    # DaemonStub starts already connected, so a disconnect/reconnect pair is
    # needed to get an actual registered transition (subscribe_device_
    # connection_changed has no initial announce, unlike subscribe_daemon_
    # running_changed above).
    stub.simulate_device_disconnected()
    stub.simulate_device_connected()
    _pump_idle_callbacks()
    assert status == {"daemon_running": True, "device_connected": True, "capture_mode": "digital"}

    stub.simulate_daemon_stopped()
    _pump_idle_callbacks()

    assert status == {"daemon_running": False, "device_connected": False, "capture_mode": "digital"}


def test_status_tracking_calls_on_change_for_every_transition_deferred_via_idle_add():
    stub = DaemonStub()
    calls = []
    _wire_status_tracking(stub, lambda: calls.append(None))

    # subscribe_daemon_running_changed's initial announce already queued one
    # idle_add call by this point.
    assert calls == [], "must be deferred via GLib.idle_add, not called inline"
    _pump_idle_callbacks()
    assert len(calls) == 1

    stub.simulate_device_disconnected()
    assert len(calls) == 1, "must be deferred via GLib.idle_add, not called inline"
    _pump_idle_callbacks()
    assert len(calls) == 2


class _FakeSystemdClient:
    """Stands in for `systemd_client.DBusSystemdClient` — no real systemd
    --user session in GUI tests, so this is the seam
    `_ensure_daemon_started_on_launch` is tested against, mirroring
    `DaemonStub`'s role for the Daemon's own D-Bus surface."""

    def __init__(self, raises: Exception | None = None):
        self._raises = raises
        self.calls: list[str] = []

    def ensure_daemon_started(self) -> None:
        self.calls.append("ensure_daemon_started")
        if self._raises is not None:
            raise self._raises


def test_ensure_daemon_started_on_launch_calls_the_systemd_client():
    fake = _FakeSystemdClient()

    _ensure_daemon_started_on_launch(fake)

    assert fake.calls == ["ensure_daemon_started"]


def test_ensure_daemon_started_on_launch_does_not_raise_on_a_glib_error(capsys):
    # Best-effort: a failure here (unit not installed, systemd unreachable,
    # etc.) must never prevent the GUI from opening — but it also must not
    # go completely unreported, per the live-hardware finding that a fully
    # silent swallow made a real bug (wrong systemd method name) far harder
    # to notice.
    fake = _FakeSystemdClient(raises=GLib.Error("systemd unreachable"))

    _ensure_daemon_started_on_launch(fake)  # must not raise

    assert "systemd unreachable" in capsys.readouterr().err


def test_dbus_systemd_client_does_not_connect_at_construction_time():
    # Regression guard: connecting eagerly in __init__ would let a
    # proxy-construction failure crash AcheronApplication.__init__ itself,
    # before do_activate's try/except (_ensure_daemon_started_on_launch) ever
    # gets a chance to swallow it — exactly the "must never block the GUI
    # from opening" case this client exists to avoid.
    from acheron_gui.systemd_client import DBusSystemdClient

    client = DBusSystemdClient()

    assert client._proxy is None


class _FakeCloseableWindow:
    """Stands in for a real `Gtk.ApplicationWindow`'s titlebar-close
    gesture — headless tests have no window manager to actually click a
    close button, so this is the seam `_wire_window_close_to_hide` is
    tested against, mirroring `_FakeFocusWindow`'s own role above."""

    def __init__(self):
        self._handlers: list = []
        self.visible = True

    def connect(self, signal_name: str, callback) -> None:
        assert signal_name == "close-request"
        self._handlers.append(callback)

    def set_visible(self, visible: bool) -> None:
        self.visible = visible

    def simulate_close_request(self):
        return [handler(self) for handler in self._handlers]


def test_close_request_hides_the_window_instead_of_destroying_it():
    window = _FakeCloseableWindow()
    _wire_window_close_to_hide(window)

    window.simulate_close_request()

    assert window.visible is False


def test_close_request_handler_returns_true_to_stop_the_default_close():
    # Ticket 36: `True` here is what stops GTK's own default handler from
    # destroying the window (which would drop it from the Gtk.Application's
    # window list and quit the process) — see the function's own docstring.
    window = _FakeCloseableWindow()
    _wire_window_close_to_hide(window)

    results = window.simulate_close_request()

    assert results == [True]


class _FakePresentableWindow:
    """Stands in for the main `Gtk.ApplicationWindow` on the presentation
    side — a headless test has no window manager to map or raise a real
    one. Mirrors `_FakeFocusWindow`/`_FakeCloseableWindow` above; driving
    the real `do_activate` would additionally need a registered
    `Gtk.Application` and a live session bus for the tray."""

    def __init__(self, visible: bool = True):
        self.visible = visible
        self.present_calls = 0

    def set_visible(self, visible: bool) -> None:
        self.visible = visible

    def present(self) -> None:
        self.present_calls += 1


def test_present_window_shows_a_tray_hidden_window_before_presenting_it():
    # `_wire_window_close_to_hide` hides rather than destroys the window, so
    # re-surfacing it has to make it visible again, not just present() an
    # invisible window.
    win = _FakePresentableWindow(visible=False)

    _present_window(win)

    assert win.visible is True
    assert win.present_calls == 1


def test_first_activation_builds_the_window_and_presents_it():
    win = _FakePresentableWindow()
    builds = []

    result = _activate_window(None, lambda: builds.append(None) or win, _present_window)

    assert builds == [None]
    assert result is win
    assert win.present_calls == 1


def test_second_activation_reuses_the_window_without_rebuilding():
    # Ticket 105: `Gio.Application` re-emits `activate` when a second
    # `acheron-gui` / `gtk-launch acheron.desktop` hands off to the running
    # primary. The one-time setup (window, tray icon, CSS, D-Bus
    # subscriptions) must not run again — re-running it rebuilt the tray's
    # SNI object and crashed on the already-exported path, aborting before
    # the real window was raised and leaking a zombie second window.
    win = _FakePresentableWindow()
    builds = []

    def build():
        builds.append(None)
        return win

    first = _activate_window(None, build, _present_window)
    second = _activate_window(first, build, _present_window)

    assert builds == [None]  # built once, not once per activation
    assert second is win
    assert win.present_calls == 2  # but re-presented every time


def test_reactivation_reshows_the_window_when_it_was_hidden_to_the_tray():
    win = _FakePresentableWindow(visible=False)

    def build():
        raise AssertionError("must not rebuild on a later activation")

    _activate_window(win, build, _present_window)

    assert win.visible is True
    assert win.present_calls == 1


# --- ticket 102: the header-bar primary menu + About dialog wiring --------


def test_primary_menu_has_one_about_item_targeting_the_app_action():
    menu = _build_primary_menu()

    assert menu.get_n_items() == 1
    assert menu.get_item_attribute_value(0, "label", None).get_string() == "About Acheron"
    assert menu.get_item_attribute_value(0, "action", None).get_string() == "app.about"


def test_about_dialog_state_returns_the_live_snapshot_when_the_daemon_is_running():
    stub = DaemonStub()

    state = _about_dialog_state(stub, daemon_running=True)

    assert state["daemon_version"] == "1.0.0"
    assert state["firmware_version"] == "v1.2"


def test_about_dialog_state_is_none_when_the_daemon_is_not_running():
    stub = DaemonStub()

    # Not even attempted — a stopped Daemon can't answer GetState().
    assert _about_dialog_state(stub, daemon_running=False) is None


def test_about_dialog_state_is_none_when_get_state_raises_a_daemon_error():
    class _Boom:
        def get_state(self):
            raise DaemonError("vanished mid-call")

    assert _about_dialog_state(_Boom(), daemon_running=True) is None
