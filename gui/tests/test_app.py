from gi.repository import GLib

from acheron_gui.app import _wire_focus_tracking, _wire_status_tracking
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
    assert status == {"daemon_running": False, "device_connected": False}


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
    assert status == {"daemon_running": False, "device_connected": False}

    stub.simulate_daemon_started()
    _pump_idle_callbacks()
    assert status["daemon_running"] is True


def test_status_tracking_reflects_device_connection_changes_while_running():
    stub = DaemonStub()
    status = _wire_status_tracking(stub, lambda: None)
    _pump_idle_callbacks()

    stub.simulate_device_disconnected()
    _pump_idle_callbacks()
    assert status == {"daemon_running": True, "device_connected": False}

    stub.simulate_device_connected()
    _pump_idle_callbacks()
    assert status == {"daemon_running": True, "device_connected": True}


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
    assert status == {"daemon_running": True, "device_connected": True}

    stub.simulate_daemon_stopped()
    _pump_idle_callbacks()

    assert status == {"daemon_running": False, "device_connected": False}


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
