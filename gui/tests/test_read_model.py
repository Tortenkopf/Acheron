# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.read_model import reference_count


def _bindings(stub):
    return stub.get_config()["profiles"]


def test_reference_count_is_zero_for_an_unreferenced_id():
    stub = DaemonStub()
    macro_id = stub.create_macro("Test macro", [])

    assert reference_count(
        _bindings(stub), binding_type="macro", id_field="macro_id", id_value=macro_id
    ) == 0


def test_reference_count_spans_base_held_and_chord_layers_across_profiles():
    # Mirrors `edit.rs::macro_references` (via `config::profile_all_bindings`)
    # — Base, Held, and both Chord Layers, in every Profile, all count.
    stub = DaemonStub()
    stub.create_profile("Gaming")
    macro_id = stub.create_macro("Test macro", [])
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "macro", "macro_id": macro_id})
    stub.set_chord_binding(
        ["grid_r2c1", "grid_r2c2"], "base", {"trigger": "fire_once", "type": "macro", "macro_id": macro_id}
    )
    stub.switch_profile("Gaming")
    stub.set_binding("grid_r1c2", "held", {"trigger": "fire_once", "type": "macro", "macro_id": macro_id})

    assert reference_count(
        _bindings(stub), binding_type="macro", id_field="macro_id", id_value=macro_id
    ) == 3


def test_reference_count_filters_on_both_type_and_id_field():
    # A Stepper Binding and a Macro Binding at different Inputs — each scan
    # sees only its own kind, keyed on its own id field.
    stub = DaemonStub()
    macro_id = stub.create_macro("M", [])
    stepper_id = stub.create_stepper("S", [])
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "macro", "macro_id": macro_id})
    stub.set_binding(
        "grid_r1c2",
        "base",
        {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"},
    )

    profiles = _bindings(stub)
    assert reference_count(profiles, binding_type="macro", id_field="macro_id", id_value=macro_id) == 1
    assert (
        reference_count(profiles, binding_type="step", id_field="stepper_id", id_value=stepper_id) == 1
    )
    assert reference_count(profiles, binding_type="step", id_field="stepper_id", id_value=macro_id) == 0
