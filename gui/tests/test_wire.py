from acheron_gui import wire


def test_keypress_binding_round_trips_through_a_variant():
    binding = {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": ["ctrl", "shift"]}

    variant_dict = wire.binding_to_variant(binding)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": ["ctrl", "shift"]}


def test_keypress_with_no_modifiers_omits_the_modifiers_field():
    binding = {"trigger": "fire_once", "type": "keypress", "key": "KEY_A", "modifiers": []}

    variant_dict = wire.binding_to_variant(binding)

    assert "modifiers" not in variant_dict


def test_profile_switch_binding_round_trips_through_a_variant():
    binding = {"trigger": "fire_once", "type": "profile_switch", "target": "Gaming"}

    variant_dict = wire.binding_to_variant(binding)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == binding


def test_controller_button_binding_round_trips_through_a_variant():
    binding = {"trigger": "fire_once", "type": "controller_button", "button": "BTN_SOUTH"}

    variant_dict = wire.binding_to_variant(binding)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == binding


def test_macro_binding_round_trips_through_a_variant():
    binding = {"trigger": "toggle", "type": "macro", "macro_id": "screenshot-combo"}

    variant_dict = wire.binding_to_variant(binding)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == binding


def test_macro_step_to_variant_round_trips_every_step_kind():
    steps = [
        {"type": "key_down", "key": "KEY_A"},
        {"type": "delay_ms", "ms": 50},
        {"type": "key_up", "key": "KEY_A"},
    ]

    for step in steps:
        variant_dict = wire.macro_step_to_variant(step)
        unpacked = {k: v.unpack() for k, v in variant_dict.items()}
        assert unpacked == step


def test_step_binding_round_trips_through_a_variant():
    binding = {"trigger": "fire_once", "type": "step", "stepper_id": "weapon-wheel", "direction": "forward"}

    variant_dict = wire.binding_to_variant(binding)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == binding


def test_stepper_item_to_variant_round_trips():
    item = {"type": "key", "key": "KEY_A"}

    variant_dict = wire.stepper_item_to_variant(item)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == item


def test_stepper_item_with_modifiers_round_trips_through_a_variant():
    item = {"type": "key", "key": "KEY_3", "modifiers": ["ctrl", "shift"]}

    variant_dict = wire.stepper_item_to_variant(item)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == item


def test_stepper_item_with_no_modifiers_omits_the_modifiers_field():
    item = {"type": "key", "key": "KEY_A", "modifiers": []}

    variant_dict = wire.stepper_item_to_variant(item)

    assert "modifiers" not in variant_dict


def test_controller_button_stepper_item_round_trips_through_a_variant():
    item = {"type": "controller_button", "button": "BTN_SOUTH"}

    variant_dict = wire.stepper_item_to_variant(item)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == item
    assert "modifiers" not in variant_dict
