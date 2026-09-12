# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

import pytest

from acheron_gui.daemon_client import DaemonError
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.stage_push import push_stage


def _keypress(key="KEY_F1", trigger="hold_to_repeat"):
    return {"trigger": trigger, "type": "keypress", "key": key, "modifiers": []}


def test_primary_non_axis_wire_lands_in_the_base_map():
    stub = DaemonStub()

    push_stage(stub, "primary", "grid_r1c1", "base", _keypress())

    assert stub.get_config()["profiles"]["Default"]["base"]["grid_r1c1"] == _keypress()
    assert stub.get_config()["profiles"]["Default"]["axis_base"] == {}


def test_primary_axis_wire_lands_in_the_axis_map_not_the_binding_map():
    stub = DaemonStub()

    push_stage(stub, "primary", "grid_r1c1", "base", {"type": "axis", "target": "left_trigger"})

    profile = stub.get_config()["profiles"]["Default"]
    assert profile["axis_base"] == {"grid_r1c1": "left_trigger"}
    assert profile["base"] == {}


def test_deep_wire_lands_in_the_deep_map():
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", "base", _keypress(trigger="fire_once"))
    # A deep stage needs its own Actuation/Staging config before `SetDeepStage`
    # will accept it — mirrors the real Daemon's `DeepStageMissingConfig`.
    stub.set_deep_actuation("grid_r1c1", 200, 150)

    push_stage(stub, "deep", "grid_r1c1", "base", _keypress(key="KEY_F2"))

    assert stub.get_config()["profiles"]["Default"]["deep_base"]["grid_r1c1"] == _keypress(
        key="KEY_F2"
    )


def test_a_rejected_push_raises_daemon_error_and_leaves_the_config_unchanged():
    stub = DaemonStub()
    before = stub.get_config()

    with pytest.raises(DaemonError):
        # analog_repeat is only legal on a Grid Input (`rules.valid_triggers`)
        # — `wheel_scroll_up` isn't one.
        push_stage(
            stub,
            "primary",
            "wheel_scroll_up",
            "base",
            {"trigger": "analog_repeat", "type": "keypress", "key": "KEY_F1", "modifiers": []},
        )

    assert stub.get_config() == before
