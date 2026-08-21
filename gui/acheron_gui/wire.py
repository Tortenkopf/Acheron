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


def action_to_variant(action: dict) -> dict[str, GLib.Variant]:
    """`action` carries `"type"` plus either Keypress's `"key"`/`"modifiers"`
    or Macro's `"steps"`. Mirrors `action_to_dict` in wire.rs: an empty
    `modifiers` list is omitted entirely rather than sent as `[]`."""
    kind = action["type"]
    if kind == "keypress":
        result = {"type": GLib.Variant("s", "keypress"), "key": GLib.Variant("s", action["key"])}
        modifiers = action.get("modifiers") or []
        if modifiers:
            result["modifiers"] = GLib.Variant("as", modifiers)
        return result
    if kind == "macro":
        steps = [macro_step_to_variant(step) for step in action["steps"]]
        return {"type": GLib.Variant("s", "macro"), "steps": GLib.Variant("aa{sv}", steps)}
    if kind == "profile_switch":
        return {"type": GLib.Variant("s", "profile_switch"), "target": GLib.Variant("s", action["target"])}
    raise ValueError(f"{kind!r} is not a valid Action type")


def binding_to_variant(binding: dict) -> dict[str, GLib.Variant]:
    """Bundles `binding`'s `"trigger"` and Action fields into one flat
    `a{sv}`, matching `binding_to_dict` in wire.rs — `SetBinding`'s single
    self-contained payload rather than parallel trigger/action arguments."""
    result = action_to_variant(binding)
    result["trigger"] = GLib.Variant("s", binding["trigger"])
    return result
