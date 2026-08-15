"""A fake `com.acheron.Daemon` backend, in-memory and synchronous, for GUI
tests — the swappable-backend seam ticket 16 asks for, replacing the ticket
09/12 prototypes' `DaemonStub` (which stood in for the not-yet-real D-Bus
surface) with one shaped exactly like the now-real `daemon_client.DaemonClient`
interface, so widget code never branches on which backend it's holding.

Seeded like a fresh install (issue 11): one `Default` Profile, empty Base
and Held Layers (all passthrough), `mode_key_role` defaulting to
`"layer_switch"` — matching the real Daemon's ticket 18 `GetConfig()` shape.

`simulate_mode_key_press`/`_release` stand in for a real physical Mode-key
event reaching the Daemon and it pushing `ActiveLayerChanged` back out: there
is no capture layer in GUI tests, so this is the seam a test uses to drive
the same live-update path `subscribe_layer_changed` wires up against the
real Daemon.
"""

from __future__ import annotations

import copy
from typing import Callable

from .daemon_client import NotFoundError


class DaemonStub:
    def __init__(self, active_profile: str = "Default"):
        self._schema_version = 1
        self._active_profile = active_profile
        self._profiles: dict[str, dict] = {
            active_profile: {"base": {}, "held": {}, "mode_key_role": "layer_switch"}
        }
        self._layer = "base"
        self._layer_changed_callbacks: list[Callable[[str], None]] = []
        # Recorded for tests that want to assert what the GUI actually sent,
        # not just the resulting state.
        self.calls: list[tuple] = []

    def get_config(self) -> dict:
        # Deep-copied: the real DBusDaemonClient's get_config() always
        # returns fresh objects from GLib.Variant.unpack(), fully decoupled
        # from the Daemon's own state — a caller mutating a returned
        # Binding dict in place must not silently corrupt this stub too.
        return {
            "schema_version": self._schema_version,
            "active_profile": self._active_profile,
            "profiles": {name: copy.deepcopy(profile) for name, profile in self._profiles.items()},
        }

    def get_state(self) -> tuple[str, str, list[str], bool]:
        return (self._active_profile, self._layer, [], True)

    def set_binding(self, input_str: str, layer: str, binding: dict) -> None:
        # Deep-copied for the same reason: SetBinding's real wire encoding
        # (wire.py) copies every field into a GLib.Variant, decoupled from
        # the caller's dict, so mutating `binding` afterward must not reach
        # back into this stub's stored state.
        stored = copy.deepcopy(binding)
        self._profiles[self._active_profile][layer][input_str] = stored
        self.calls.append(("set_binding", input_str, layer, copy.deepcopy(stored)))

    def clear_binding(self, input_str: str, layer: str) -> None:
        bindings = self._profiles[self._active_profile][layer]
        if input_str not in bindings:
            raise NotFoundError(f"no Binding is set for {input_str!r}")
        del bindings[input_str]
        self.calls.append(("clear_binding", input_str, layer))

    def set_mode_key_role(self, role: str) -> None:
        self._profiles[self._active_profile]["mode_key_role"] = role
        self.calls.append(("set_mode_key_role", role))

    def subscribe_layer_changed(self, callback: Callable[[str], None]) -> None:
        self._layer_changed_callbacks.append(callback)

    def simulate_mode_key_press(self) -> None:
        self._set_layer("held")

    def simulate_mode_key_release(self) -> None:
        self._set_layer("base")

    def _set_layer(self, layer: str) -> None:
        if layer == self._layer:
            return
        self._layer = layer
        for callback in self._layer_changed_callbacks:
            callback(layer)
