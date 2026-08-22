from acheron_gui.systemd_client import DAEMON_UNIT, DBusSystemdClient


class _FakeProxy:
    """Records every `call_sync` — the seam ticket 36's `stop_daemon`/
    `start_daemon` are tested against, same idea as `daemon_stub.DaemonStub`
    recording calls for the real Daemon's own D-Bus client."""

    def __init__(self):
        self.calls: list[tuple] = []

    def call_sync(self, method, parameters, _flags, _timeout, _cancellable):
        self.calls.append((method, parameters.unpack() if parameters is not None else None))


def test_stop_daemon_calls_stop_unit_with_replace_mode():
    proxy = _FakeProxy()
    client = DBusSystemdClient(proxy)

    client.stop_daemon()

    assert proxy.calls == [("StopUnit", (DAEMON_UNIT, "replace"))]


def test_start_daemon_calls_start_unit_with_replace_mode():
    proxy = _FakeProxy()
    client = DBusSystemdClient(proxy)

    client.start_daemon()

    assert proxy.calls == [("StartUnit", (DAEMON_UNIT, "replace"))]
