# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""D-Bus wire encoding for `com.acheron.Daemon`, matching the conventions
hand-written on the Rust side in `daemon/src/dbus/wire.rs`: `Input` is a
plain string (its TOML `Display`/`FromStr` form); `Action`/`MacroStep`
marshal as `a{sv}` dicts with a `"type"` tag key; `Binding` bundles its
`TriggerMode` flat alongside the `Action` fields in one dict, not nested.

Only the *encode* direction lives here — `GLib.Variant.unpack()` already
turns a `GetConfig()`/`GetState()` reply into plain Python dicts/lists/str
recursively (including nested `a{sv}`/`aa{sv}`), so there is nothing to
hand-write for decoding.

The Python-side in-memory shape for a Binding is deliberately the same flat
dict the wire uses (`{"trigger": ..., "type": ..., "key": ..., ...}`), not
the nested `{"trigger": ..., "action": {...}}` shape the ticket 09 prototype
used — mirroring what `GetConfig()` actually hands back avoids a translation
layer between "what the Daemon said" and "what the editor edits".
"""

from __future__ import annotations

import gi

gi.require_version("GLib", "2.0")
from gi.repository import GLib


def macro_step_to_variant(step: dict) -> dict[str, GLib.Variant]:
    """`step` is `{"type": "key_down"|"key_up", "key": "KEY_A"}` or
    `{"type": "delay_ms", "ms": 50}`, matching `MacroStepDto`'s wire tags."""
    kind = step["type"]
    if kind in ("key_down", "key_up"):
        return {"type": GLib.Variant("s", kind), "key": GLib.Variant("s", step["key"])}
    if kind == "delay_ms":
        return {"type": GLib.Variant("s", kind), "ms": GLib.Variant("t", step["ms"])}
    raise ValueError(f"{kind!r} is not a valid MacroStep type")


def stepper_item_to_variant(item: dict) -> dict[str, GLib.Variant]:
    """`item` is `{"type": "key", "key": "KEY_A", "modifiers": [...]}` or
    `{"type": "controller_button", "button": "BTN_SOUTH"}` (ticket 92),
    matching `StepperItem`'s wire tags — mirroring `macro_step_to_variant`'s
    shape. `modifiers` (ticket 62/63's Answer) follows `action_to_variant`'s
    own convention: an empty list is omitted entirely rather than sent as
    `[]`. The `controller_button` variant has no `modifiers` field at all —
    a gamepad button takes no modifier combination."""
    kind = item["type"]
    if kind == "key":
        result = {"type": GLib.Variant("s", "key"), "key": GLib.Variant("s", item["key"])}
        modifiers = item.get("modifiers") or []
        if modifiers:
            result["modifiers"] = GLib.Variant("as", modifiers)
        return result
    if kind == "controller_button":
        return {
            "type": GLib.Variant("s", "controller_button"),
            "button": GLib.Variant("s", item["button"]),
        }
    raise ValueError(f"{kind!r} is not a valid StepperItem type")


def action_to_variant(action: dict) -> dict[str, GLib.Variant]:
    """`action` carries `"type"` plus either Keypress's `"key"`/`"modifiers"`,
    Macro's `"macro_id"` (ticket 51 — a Binding references a library entry
    rather than carrying step content directly), or Step's `"stepper_id"`/
    `"direction"` (ticket 03/54, same reference-not-inline shape). Mirrors
    `action_to_dict` in wire.rs: an empty `modifiers` list is omitted
    entirely rather than sent as `[]`."""
    kind = action["type"]
    if kind == "keypress":
        result = {"type": GLib.Variant("s", "keypress"), "key": GLib.Variant("s", action["key"])}
        modifiers = action.get("modifiers") or []
        if modifiers:
            result["modifiers"] = GLib.Variant("as", modifiers)
        return result
    if kind == "macro":
        return {"type": GLib.Variant("s", "macro"), "macro_id": GLib.Variant("s", action["macro_id"])}
    if kind == "profile_switch":
        return {"type": GLib.Variant("s", "profile_switch"), "target": GLib.Variant("s", action["target"])}
    if kind == "controller_button":
        return {"type": GLib.Variant("s", "controller_button"), "button": GLib.Variant("s", action["button"])}
    if kind == "step":
        return {
            "type": GLib.Variant("s", "step"),
            "stepper_id": GLib.Variant("s", action["stepper_id"]),
            "direction": GLib.Variant("s", action["direction"]),
        }
    raise ValueError(f"{kind!r} is not a valid Action type")


def binding_to_variant(binding: dict) -> dict[str, GLib.Variant]:
    """Bundles `binding`'s `"trigger"` and Action fields into one flat
    `a{sv}`, matching `binding_to_dict` in wire.rs — `SetBinding`'s single
    self-contained payload rather than parallel trigger/action arguments."""
    result = action_to_variant(binding)
    result["trigger"] = GLib.Variant("s", binding["trigger"])
    return result


def _colour_to_variant(colour: dict) -> GLib.Variant:
    """`colour` is `{"r": int, "g": int, "b": int}` — the same shape
    `GetConfig()` hands back (`colour_to_dict`'s `a{sv}` shape, unpacked), so
    reading a Profile's `lighting` off `GetConfig()` and feeding it straight
    back into `set_lighting` (e.g. "copy from Profile X") needs no
    translation. `SetLighting`'s own wire encoding for a Colour is a `(yyy)`
    byte-triple instead — the daemon's deliberate, minimal choice for this
    one decode path (`colour_from_field`'s doc comment in
    `daemon/src/dbus/wire.rs`), so this helper is the one place that
    translation happens, not the Python-side dict shape."""
    return GLib.Variant("(yyy)", (colour["r"], colour["g"], colour["b"]))


def _breath_style_to_variant(style: dict) -> dict[str, GLib.Variant]:
    """`style` carries `"style"` plus `Single`'s `"colour"` or `Dual`'s
    `"first"`/`"second"`, matching `breath_style_to_dict`'s `"style"` tag —
    distinct from `FixedEffect`'s own `"type"` tag, mirroring
    `config::BreathStyle`'s two-tag-keys-at-two-levels shape exactly."""
    kind = style["style"]
    if kind == "random":
        return {"style": GLib.Variant("s", "random")}
    if kind == "single":
        return {
            "style": GLib.Variant("s", "single"),
            "colour": _colour_to_variant(style["colour"]),
        }
    if kind == "dual":
        return {
            "style": GLib.Variant("s", "dual"),
            "first": _colour_to_variant(style["first"]),
            "second": _colour_to_variant(style["second"]),
        }
    raise ValueError(f"{kind!r} is not a valid BreathStyle")


def _fixed_effect_to_variant(effect: dict) -> dict[str, GLib.Variant]:
    """Matches `fixed_effect_to_dict`'s `"type"`-tagged shape."""
    kind = effect["type"]
    if kind == "static":
        return {
            "type": GLib.Variant("s", "static"),
            "colour": _colour_to_variant(effect["colour"]),
        }
    if kind == "spectrum":
        return {"type": GLib.Variant("s", "spectrum")}
    if kind == "reactive":
        return {
            "type": GLib.Variant("s", "reactive"),
            "colour": _colour_to_variant(effect["colour"]),
            "speed": GLib.Variant("y", effect["speed"]),
        }
    if kind == "wave":
        return {
            "type": GLib.Variant("s", "wave"),
            "direction": GLib.Variant("s", effect["direction"]),
        }
    if kind == "breath":
        return {
            "type": GLib.Variant("s", "breath"),
            "style": GLib.Variant("a{sv}", _breath_style_to_variant(effect["style"])),
        }
    if kind == "starlight":
        return {
            "type": GLib.Variant("s", "starlight"),
            "style": GLib.Variant("a{sv}", _breath_style_to_variant(effect["style"])),
            "speed": GLib.Variant("y", effect["speed"]),
        }
    raise ValueError(f"{kind!r} is not a valid FixedEffect type")


def lighting_assignment_to_variant(assignment: dict) -> dict[str, GLib.Variant]:
    """`assignment` is `LightingAssignment`'s `"type"`-tagged dict
    (`Off`/`FixedEffect`/`CustomLayout`), the same shape `GetConfig()` hands
    back for a Profile's `"lighting"` entry — matches
    `lighting_assignment_to_dict`'s `"type"` tag convention for
    `SetLighting`'s request (`tartarus-backlight` ticket 03)."""
    kind = assignment["type"]
    if kind == "off":
        return {"type": GLib.Variant("s", "off")}
    if kind == "fixed_effect":
        return {
            "type": GLib.Variant("s", "fixed_effect"),
            "effect": GLib.Variant("a{sv}", _fixed_effect_to_variant(assignment["effect"])),
        }
    if kind == "custom_layout":
        colours = [(c["r"], c["g"], c["b"]) for c in assignment["colours"]]
        return {
            "type": GLib.Variant("s", "custom_layout"),
            "colours": GLib.Variant("a(yyy)", colours),
        }
    raise ValueError(f"{kind!r} is not a valid LightingAssignment type")
