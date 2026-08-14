"""A fake `com.acheron.Daemon` backend, in-memory and synchronous, for GUI
tests — the swappable-backend seam ticket 16 asks for, replacing the ticket
09/12 prototypes' `DaemonStub` (which stood in for the not-yet-real D-Bus
surface) with one shaped exactly like the now-real `daemon_client.DaemonClient`
interface, so widget code never branches on which backend it's holding.

Seeded like a fresh install (issue 11): one `Default` Profile, empty Base
Layer (all passthrough) — there is no Held Layer or second Profile here
because the real Daemon's `GetConfig()` doesn't have one either at this
ticket's scope (Profile only carries `base`; Held/multi-Profile are tickets
18/19).
"""

from __future__ import annotations

import copy

from .daemon_client import NotFoundError


class DaemonStub:
    def __init__(self, active_profile: str = "Default"):
        self._schema_version = 1
        self._active_profile = active_profile
        self._profiles: dict[str, dict[str, dict]] = {active_profile: {"base": {}}}
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
            "profiles": {
                name: {"base": copy.deepcopy(profile["base"])} for name, profile in self._profiles.items()
            },
        }

    def get_state(self) -> tuple[str, str, list[str], bool]:
        return (self._active_profile, "base", [], True)

    def set_binding(self, input_str: str, binding: dict) -> None:
        # Deep-copied for the same reason: SetBinding's real wire encoding
        # (wire.py) copies every field into a GLib.Variant, decoupled from
        # the caller's dict, so mutating `binding` afterward must not reach
        # back into this stub's stored state.
        stored = copy.deepcopy(binding)
        self._profiles[self._active_profile]["base"][input_str] = stored
        self.calls.append(("set_binding", input_str, copy.deepcopy(stored)))

    def clear_binding(self, input_str: str) -> None:
        base = self._profiles[self._active_profile]["base"]
        if input_str not in base:
            raise NotFoundError(f"no Binding is set for {input_str!r}")
        del base[input_str]
        self.calls.append(("clear_binding", input_str))
