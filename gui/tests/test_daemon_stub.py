import pytest

from acheron_gui.daemon_client import NotFoundError
from acheron_gui.daemon_stub import DaemonStub


def test_fresh_stub_matches_the_seed_configs_shape():
    stub = DaemonStub()

    config = stub.get_config()

    assert config == {
        "schema_version": 1,
        "active_profile": "Default",
        "profiles": {"Default": {"base": {}}},
    }
    assert stub.get_state() == ("Default", "base", [], True)


def test_set_binding_then_get_config_reflects_it():
    stub = DaemonStub()
    binding = {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []}

    stub.set_binding("grid_r1c1", binding)

    assert stub.get_config()["profiles"]["Default"]["base"]["grid_r1c1"] == binding
    assert stub.calls == [("set_binding", "grid_r1c1", binding)]


def test_clear_binding_removes_it():
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []})

    stub.clear_binding("grid_r1c1")

    assert "grid_r1c1" not in stub.get_config()["profiles"]["Default"]["base"]


def test_clear_binding_on_an_unbound_input_raises_not_found():
    stub = DaemonStub()

    with pytest.raises(NotFoundError):
        stub.clear_binding("grid_r1c1")
