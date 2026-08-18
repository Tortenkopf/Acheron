import pytest

from acheron_gui.daemon_client import AlreadyExistsError, InvalidBindingError, NotFoundError
from acheron_gui.daemon_stub import DaemonStub


def test_fresh_stub_matches_the_seed_configs_shape():
    stub = DaemonStub()

    config = stub.get_config()

    assert config == {
        "schema_version": 1,
        "active_profile": "Default",
        "profiles": {"Default": {"base": {}, "held": {}, "mode_key_role": "layer_switch"}},
    }
    assert stub.get_state() == {
        "profile": "Default",
        "layer": "base",
        "active_toggles": [],
        "device_connected": True,
        "capture_mode": "digital",
    }


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
    assert stub.get_state()["layer"] == "base"


def test_create_profile_adds_an_empty_profile():
    stub = DaemonStub()

    stub.create_profile("Gaming")

    assert stub.get_config()["profiles"]["Gaming"] == {
        "base": {},
        "held": {},
        "mode_key_role": "layer_switch",
    }
    assert stub.calls == [("create_profile", "Gaming")]


def test_create_profile_with_a_duplicate_name_raises_already_exists():
    stub = DaemonStub()

    with pytest.raises(AlreadyExistsError):
        stub.create_profile("Default")


def test_delete_profile_removes_a_non_active_profile():
    stub = DaemonStub()
    stub.create_profile("Gaming")

    stub.delete_profile("Gaming")

    assert "Gaming" not in stub.get_config()["profiles"]


def test_delete_profile_on_the_active_profile_raises_invalid_binding():
    stub = DaemonStub()

    with pytest.raises(InvalidBindingError):
        stub.delete_profile("Default")


def test_delete_profile_on_an_unknown_name_raises_not_found():
    stub = DaemonStub()

    with pytest.raises(NotFoundError):
        stub.delete_profile("Nonexistent")


def test_rename_profile_renames_and_updates_active_profile():
    stub = DaemonStub()

    stub.rename_profile("Default", "Renamed")

    config = stub.get_config()
    assert "Default" not in config["profiles"]
    assert "Renamed" in config["profiles"]
    assert config["active_profile"] == "Renamed"
    assert stub.get_state()["profile"] == "Renamed"


def test_rename_profile_with_a_duplicate_new_name_raises_already_exists():
    stub = DaemonStub()
    stub.create_profile("Gaming")

    with pytest.raises(AlreadyExistsError):
        stub.rename_profile("Gaming", "Default")


def test_rename_profile_on_an_unknown_old_name_raises_not_found():
    stub = DaemonStub()

    with pytest.raises(NotFoundError):
        stub.rename_profile("Nonexistent", "Whatever")


def test_switch_profile_changes_active_profile_and_notifies_subscribers():
    stub = DaemonStub()
    stub.create_profile("Gaming")
    seen = []
    stub.subscribe_profile_changed(seen.append)

    stub.switch_profile("Gaming")

    assert stub.get_state()["profile"] == "Gaming"
    assert seen == ["Gaming"]
    assert stub.calls == [("create_profile", "Gaming"), ("switch_profile", "Gaming")]


def test_switch_profile_on_an_unknown_name_raises_not_found():
    stub = DaemonStub()

    with pytest.raises(NotFoundError):
        stub.switch_profile("Nonexistent")


def test_switch_profile_clears_active_toggles():
    stub = DaemonStub()
    stub.create_profile("Gaming")
    stub.simulate_toggle_started("grid_r1c1")
    assert stub.get_state()["active_toggles"] == ["grid_r1c1"]

    stub.switch_profile("Gaming")

    assert stub.get_state()["active_toggles"] == []
