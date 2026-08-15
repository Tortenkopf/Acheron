import pytest

from acheron_gui.daemon_client import NotFoundError
from acheron_gui.daemon_stub import DaemonStub


def test_fresh_stub_matches_the_seed_configs_shape():
    stub = DaemonStub()

    config = stub.get_config()

    assert config == {
        "schema_version": 1,
        "active_profile": "Default",
        "profiles": {"Default": {"base": {}, "held": {}, "mode_key_role": "layer_switch"}},
    }
    assert stub.get_state() == ("Default", "base", [], True)


def test_set_binding_then_get_config_reflects_it():
    stub = DaemonStub()
    binding = {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []}

    stub.set_binding("grid_r1c1", "base", binding)

    assert stub.get_config()["profiles"]["Default"]["base"]["grid_r1c1"] == binding
    assert stub.calls == [("set_binding", "grid_r1c1", "base", binding)]


def test_set_binding_targets_the_held_layer_independently_of_base():
    stub = DaemonStub()
    binding = {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []}

    stub.set_binding("grid_r1c1", "held", binding)

    assert stub.get_config()["profiles"]["Default"]["held"]["grid_r1c1"] == binding
    assert "grid_r1c1" not in stub.get_config()["profiles"]["Default"]["base"]


def test_clear_binding_removes_it():
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1", "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []}
    )

    stub.clear_binding("grid_r1c1", "base")

    assert "grid_r1c1" not in stub.get_config()["profiles"]["Default"]["base"]


def test_clear_binding_on_an_unbound_input_raises_not_found():
    stub = DaemonStub()

    with pytest.raises(NotFoundError):
        stub.clear_binding("grid_r1c1", "base")


def test_set_mode_key_role_updates_the_active_profile():
    stub = DaemonStub()

    stub.set_mode_key_role("bound")

    assert stub.get_config()["profiles"]["Default"]["mode_key_role"] == "bound"
    assert stub.calls == [("set_mode_key_role", "bound")]


def test_simulate_mode_key_press_and_release_drives_subscribed_callbacks():
    stub = DaemonStub()
    seen = []
    stub.subscribe_layer_changed(seen.append)

    stub.simulate_mode_key_press()
    stub.simulate_mode_key_release()

    assert seen == ["held", "base"]
    assert stub.get_state()[1] == "base"
