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
    """`item` is `{"type": "key", "key": "KEY_A", "modifiers": [...]}`,
    matching `StepperItem`'s wire tag — today's sole `Key` variant, mirroring
    `macro_step_to_variant`'s shape. `modifiers` (ticket 62/63's Answer)
    follows `action_to_variant`'s own convention: an empty list is omitted
    entirely rather than sent as `[]`."""
    kind = item["type"]
    if kind == "key":
        result = {"type": GLib.Variant("s", "key"), "key": GLib.Variant("s", item["key"])}
        modifiers = item.get("modifiers") or []
        if modifiers:
            result["modifiers"] = GLib.Variant("as", modifiers)
        return result
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
