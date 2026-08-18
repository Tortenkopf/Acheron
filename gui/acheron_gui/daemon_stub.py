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
real Daemon. `simulate_toggle_started` is the equivalent seam for a running
Toggle, used to exercise `switch_profile`'s force-stop-on-switch effect
(ticket 19) at the GUI level.

`create_profile`/`delete_profile`/`rename_profile`/`switch_profile` (ticket
19) mirror the real Daemon's validation: `AlreadyExistsError` on a taken
name, `NotFoundError` on an unknown one, `InvalidBindingError` on deleting
the active Profile (the real Daemon's `active_profile` must always name a
real Profile, so it can never be deleted out from under itself).

`simulate_daemon_stopped`/`_started` and `simulate_device_disconnected`/
`_connected` (ticket 20) are the equivalent seam for the Daemon-presence
`NameOwnerChanged` watch and the `DeviceConnectionChanged` signal — there's
no real session bus in GUI tests either.
"""

from __future__ import annotations

import copy
from typing import Callable

from .daemon_client import AlreadyExistsError, InvalidBindingError, NotFoundError


class DaemonStub:
    def __init__(self, active_profile: str = "Default"):
        self._schema_version = 1
        self._active_profile = active_profile
        self._profiles: dict[str, dict] = {
            active_profile: {"base": {}, "held": {}, "mode_key_role": "layer_switch"}
        }
        self._layer = "base"
        self._active_toggles: list[str] = []
        # Hardcoded, mirroring the real Daemon's ticket 21 stand-in — there
        # is no analog CaptureSource yet for either side to report on.
        self._capture_mode = "digital"
        self._daemon_running = True
        self._device_connected = True
        self._layer_changed_callbacks: list[Callable[[str], None]] = []
        self._profile_changed_callbacks: list[Callable[[str], None]] = []
        self._running_changed_callbacks: list[Callable[[bool], None]] = []
        self._device_connection_changed_callbacks: list[Callable[[bool], None]] = []
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

    def get_state(self) -> dict:
        return {
            "profile": self._active_profile,
            "layer": self._layer,
            "active_toggles": list(self._active_toggles),
            "device_connected": self._device_connected,
            "capture_mode": self._capture_mode,
        }

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

    def create_profile(self, name: str) -> None:
        if name in self._profiles:
            raise AlreadyExistsError(f"a Profile named {name!r} already exists")
        self._profiles[name] = {"base": {}, "held": {}, "mode_key_role": "layer_switch"}
        self.calls.append(("create_profile", name))

    def delete_profile(self, name: str) -> None:
        if name == self._active_profile:
            raise InvalidBindingError("cannot delete the active Profile")
        if name not in self._profiles:
            raise NotFoundError(f"no Profile named {name!r}")
        del self._profiles[name]
        self.calls.append(("delete_profile", name))

    def rename_profile(self, old_name: str, new_name: str) -> None:
        if old_name not in self._profiles:
            raise NotFoundError(f"no Profile named {old_name!r}")
        if new_name != old_name and new_name in self._profiles:
            raise AlreadyExistsError(f"a Profile named {new_name!r} already exists")
        self.calls.append(("rename_profile", old_name, new_name))
        if new_name == old_name:
            return
        self._profiles[new_name] = self._profiles.pop(old_name)
        if self._active_profile == old_name:
            self._active_profile = new_name

    def switch_profile(self, name: str) -> None:
        if name not in self._profiles:
            raise NotFoundError(f"no Profile named {name!r}")
        self._active_profile = name
        # Every active Toggle is force-stopped as part of the switch (ticket
        # 19) — no exact-key-release tracking in this GUI-level fake (that's
        # the Daemon's job, covered by its own tests), just the observable
        # "gone from GetState()" effect the GUI reacts to.
        self._active_toggles = []
        self.calls.append(("switch_profile", name))
        for callback in self._profile_changed_callbacks:
            callback(name)

    def set_output_suppressed(self, suppressed: bool) -> None:
        # Ticket 24's flag is Config-free and never reflected back through
        # GetState()/GetConfig() on the real Daemon either — this stub only
        # needs to record what the GUI sent, for tests to assert against.
        self.calls.append(("set_output_suppressed", suppressed))

    def stop_all_toggles(self) -> None:
        # Ticket 25's GUI-side guard against a Toggle left running once the
        # GUI's own window gains focus — same observable effect on
        # active_toggles as switch_profile's force-stop, minus the Profile
        # change.
        self._active_toggles = []
        self.calls.append(("stop_all_toggles",))

    def subscribe_layer_changed(self, callback: Callable[[str], None]) -> None:
        self._layer_changed_callbacks.append(callback)

    def subscribe_profile_changed(self, callback: Callable[[str], None]) -> None:
        self._profile_changed_callbacks.append(callback)

    def subscribe_daemon_running_changed(self, callback: Callable[[bool], None]) -> None:
        # Mirrors the real `DBusDaemonClient`'s `Gio.bus_watch_name`: it
        # reports the currently-known state right away (covers "already
        # running when the GUI launched"), not just future transitions —
        # unlike subscribe_device_connection_changed below, which really is
        # signal-only on the real Daemon (its initial value instead reaches
        # the GUI through GetState(), once daemon_running is known true).
        self._running_changed_callbacks.append(callback)
        callback(self._daemon_running)

    def subscribe_device_connection_changed(self, callback: Callable[[bool], None]) -> None:
        self._device_connection_changed_callbacks.append(callback)

    def simulate_daemon_stopped(self) -> None:
        """Stands in for `com.acheron.Daemon`'s bus name vanishing (ticket
        20) — there's no real session bus in GUI tests, so this is the seam
        a test uses to drive the same live-update path
        `subscribe_daemon_running_changed` wires up against the real
        `NameOwnerChanged` watch."""
        if not self._daemon_running:
            return
        self._daemon_running = False
        for callback in self._running_changed_callbacks:
            callback(False)

    def simulate_daemon_started(self) -> None:
        if self._daemon_running:
            return
        self._daemon_running = True
        for callback in self._running_changed_callbacks:
            callback(True)

    def simulate_device_disconnected(self) -> None:
        """Stands in for a real `DeviceConnectionChanged(False)` signal
        (ticket 20)."""
        if not self._device_connected:
            return
        self._device_connected = False
        for callback in self._device_connection_changed_callbacks:
            callback(False)

    def simulate_device_connected(self) -> None:
        if self._device_connected:
            return
        self._device_connected = True
        for callback in self._device_connection_changed_callbacks:
            callback(True)

    def simulate_mode_key_press(self) -> None:
        self._set_layer("held")

    def simulate_mode_key_release(self) -> None:
        self._set_layer("base")

    def simulate_toggle_started(self, input_str: str) -> None:
        """Stands in for a real physical press starting a Toggle — there's
        no capture layer in GUI tests, so this is the seam a test uses to
        put `GetState()`'s `active_toggles` in the state `SwitchProfile`
        must clear."""
        if input_str not in self._active_toggles:
            self._active_toggles.append(input_str)

    def _set_layer(self, layer: str) -> None:
        if layer == self._layer:
            return
        self._layer = layer
        for callback in self._layer_changed_callbacks:
            callback(layer)
