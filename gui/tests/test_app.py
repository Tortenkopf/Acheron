from gi.repository import GLib

from acheron_gui.app import _wire_focus_tracking
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

    assert stub.calls == [("set_output_suppressed", True)]


def test_initial_push_reflects_an_unfocused_start_too():
    stub = DaemonStub()
    window = _FakeFocusWindow(initially_active=False)

    _wire_focus_tracking(window, stub)
    _pump_idle_callbacks()

    assert stub.calls == [("set_output_suppressed", False)]


def test_gaining_focus_suppresses_output():
    stub = DaemonStub()
    window = _FakeFocusWindow(initially_active=False)
    _wire_focus_tracking(window, stub)
    _pump_idle_callbacks()

    window.simulate_focus_change(True)
    _pump_idle_callbacks()

    assert stub.calls == [
        ("set_output_suppressed", False),
        ("set_output_suppressed", True),
    ]


def test_losing_focus_resumes_output():
    stub = DaemonStub()
    window = _FakeFocusWindow(initially_active=True)
    _wire_focus_tracking(window, stub)
    _pump_idle_callbacks()

    window.simulate_focus_change(False)
    _pump_idle_callbacks()

    assert stub.calls == [
        ("set_output_suppressed", True),
        ("set_output_suppressed", False),
    ]


def test_set_output_suppressed_is_not_called_synchronously_from_the_signal_handler():
    # Regression guard for the nested-call_sync reentrancy hazard the
    # module docstring describes: the signal handler must only ever queue
    # the Daemon call via GLib.idle_add, never call straight through.
    stub = DaemonStub()
    window = _FakeFocusWindow(initially_active=False)
    _wire_focus_tracking(window, stub)
    _pump_idle_callbacks()
    stub.calls.clear()

    window.simulate_focus_change(True)

    assert stub.calls == []
