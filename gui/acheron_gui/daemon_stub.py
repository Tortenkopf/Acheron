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
import re
from typing import Callable

from .axis_picker import AXIS_LABEL_BY_TARGET
from .daemon_client import AlreadyExistsError, InvalidBindingError, NotFoundError
from .inputs import is_grid_input


class DaemonStub:
    _SEED_PROFILE = {
        "base": {},
        "held": {},
        "mode_key_role": "layer_switch",
        # Matches `ActuationPoint::default()` (daemon/src/config.rs) — the
        # same 128/112 placeholder the real Daemon seeds a fresh Profile
        # with (ticket 26).
        "default_actuation": {"actuation": 128, "release": 112},
        "actuation_overrides": {},
        # Ticket 40: a Profile's Chord Bindings, keyed the same way the real
        # Daemon's wire shape does — a "+"-joined, sorted string of member
        # Input strings (mirrors `daemon/src/config.rs::ChordKey`'s Display).
        "chords_base": {},
        "chords_held": {},
        # Ticket 71: a Profile's Axis assignments, keyed by Input like
        # `base`/`held` — mirrors the real Daemon's wire shape (a flat
        # Input -> target-wire-string map, `daemon/src/dbus/wire.rs::
        # axis_map_to_dict`).
        "axis_base": {},
        "axis_held": {},
    }

    def __init__(self, active_profile: str = "Default"):
        self._schema_version = 1
        self._active_profile = active_profile
        self._profiles: dict[str, dict] = {active_profile: copy.deepcopy(self._SEED_PROFILE)}
        # Ticket 51: the global Macro library — macro_id -> {"name", "steps"}.
        self._macros: dict[str, dict] = {}
        # Ticket 03/54: the global Stepper-list library — stepper_id ->
        # {"name", "items"}, plus each one's Daemon-side-only runtime cursor
        # (stepper_id -> current index), mirroring the real Daemon's
        # never-persisted, always-resets-on-restart state.
        self._steppers: dict[str, dict] = {}
        self._stepper_cursors: dict[str, int] = {}
        self._layer = "base"
        self._active_toggles: list[str] = []
        # Hardcoded, mirroring the real Daemon's ticket 21 stand-in — there
        # is no analog CaptureSource yet for either side to report on.
        self._capture_mode = "digital"
        self._force_digital = False
        self._daemon_running = True
        self._device_connected = True
        self._layer_changed_callbacks: list[Callable[[str], None]] = []
        self._profile_changed_callbacks: list[Callable[[str], None]] = []
        self._running_changed_callbacks: list[Callable[[bool], None]] = []
        self._device_connection_changed_callbacks: list[Callable[[bool], None]] = []
        self._capture_mode_changed_callbacks: list[Callable[[str], None]] = []
        # Ticket 26: mirrors `DBusDaemonClient`'s single-current-target
        # depth-stream routing (see `start_depth_stream`'s docstring there)
        # rather than a list of subscribers.
        self._depth_target: str | None = None
        self._depth_callback: Callable[[int], None] | None = None
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
            "force_digital": self._force_digital,
            "macros": {macro_id: copy.deepcopy(m) for macro_id, m in self._macros.items()},
            "steppers": {stepper_id: copy.deepcopy(s) for stepper_id, s in self._steppers.items()},
        }

    def get_state(self) -> dict:
        return {
            "profile": self._active_profile,
            "layer": self._layer,
            "active_toggles": list(self._active_toggles),
            "device_connected": self._device_connected,
            "capture_mode": self._capture_mode,
            # Every library entry gets a reported cursor, defaulting to `0`
            # ("the list's first item") for one never yet stepped — mirrors
            # the real Daemon's `GetState()` shape (ticket 03/54).
            "stepper_cursors": {
                stepper_id: self._stepper_cursors.get(stepper_id, 0) for stepper_id in self._steppers
            },
        }

    def _validate_binding_action(self, binding: dict) -> None:
        # Ticket 51/03/54/40: mirrors the real Daemon's shared
        # `dispatch::validate_binding` — a Macro/Step Action naming an
        # unknown library entry, or a Step paired with Toggle, is rejected
        # outright. Shared by `set_binding` and `set_chord_binding` (a Chord
        # Binding is "just a Binding keyed by a Set<Input>", ticket 01's
        # Answer, held to the exact same rules).
        if binding.get("type") == "macro" and binding.get("macro_id") not in self._macros:
            raise InvalidBindingError(
                f"{binding.get('macro_id')!r} does not name a Macro in the library"
            )
        if binding.get("type") == "step":
            stepper_id = binding.get("stepper_id")
            if stepper_id not in self._steppers:
                raise InvalidBindingError(f"{stepper_id!r} does not name a Stepper in the library")
            if binding.get("trigger") == "toggle":
                raise InvalidBindingError("Toggle is not allowed for a Stepper Binding")
        if binding.get("type") == "controller_button" and binding.get("trigger") == "fire_once":
            # Ticket 78: Fire-once is locked out for Controller Button —
            # Hold-to-repeat's sustained-hold behavior already covers a
            # quick tap, so there's nothing left for Fire-once's decoupled
            # pulse to uniquely serve.
            raise InvalidBindingError("Fire-once is not allowed for a Controller Button Binding")

    def _reject_if_axis_assigned(self, input_str: str, layer: str) -> None:
        # Ticket 59 §2's mutual exclusion: `SetBinding`/`SetChordBinding`
        # both reject a grid key already Axis-assigned on this Layer with a
        # specific error, not a silent overwrite — mirrors the real
        # Daemon's `dispatch::axis_conflict` check.
        if input_str in self._profiles[self._active_profile][f"axis_{layer}"]:
            raise InvalidBindingError(
                f"{input_str!r} already has an Axis assignment on this Layer — clear it first"
            )

    def set_binding(self, input_str: str, layer: str, binding: dict) -> None:
        self._reject_if_axis_assigned(input_str, layer)
        self._validate_binding_action(binding)
        if binding.get("trigger") == "analog_repeat" and not is_grid_input(input_str):
            raise InvalidBindingError("Analog-repeat is only valid on Grid Inputs")
        if binding.get("type") == "step":
            # Ticket 03's Answer: assigning a Stepper list to a new Input
            # silently moves it off its old one — no reject-at-save step,
            # mirroring the real Daemon's `take_stepper_direction_elsewhere`.
            stepper_id = binding.get("stepper_id")
            direction = binding.get("direction")
            for profile in self._profiles.values():
                for layer_bindings in (profile["base"], profile["held"]):
                    for other_input in [
                        other
                        for other, other_binding in layer_bindings.items()
                        if other_binding.get("type") == "step"
                        and other_binding.get("stepper_id") == stepper_id
                        and other_binding.get("direction") == direction
                    ]:
                        del layer_bindings[other_input]
        # Deep-copied for the same reason: SetBinding's real wire encoding
        # (wire.py) copies every field into a GLib.Variant, decoupled from
        # the caller's dict, so mutating `binding` afterward must not reach
        # back into this stub's stored state.
        stored = copy.deepcopy(binding)
        self._profiles[self._active_profile][layer][input_str] = stored
        self.calls.append(("set_binding", input_str, layer, copy.deepcopy(stored)))

    @staticmethod
    def _input_sort_key(inp: str) -> tuple:
        # Mirrors `daemon/src/input.rs::Input`'s *derived* `Ord` exactly —
        # ModeKey < Grid(row, col) < Thumbstick(Direction) < Wheel(WheelEvent),
        # each variant's own fields compared in declaration order (Grid by
        # (row, col); Direction/WheelEvent by their own declared variant
        # order). A plain alphabetical sort disagrees for any Chord mixing
        # Input variant kinds — e.g. {mode_key, grid_r1c1}: the real Daemon's
        # `ChordKey` Display is "mode_key+grid_r1c1" (ModeKey sorts first),
        # not "grid_r1c1+mode_key" (code-review finding).
        if inp == "mode_key":
            return (0,)
        grid_match = re.fullmatch(r"grid_r(\d+)c(\d+)", inp)
        if grid_match:
            return (1, int(grid_match.group(1)), int(grid_match.group(2)))
        direction_order = {
            "thumbstick_up": 0,
            "thumbstick_down": 1,
            "thumbstick_left": 2,
            "thumbstick_right": 3,
        }
        if inp in direction_order:
            return (2, direction_order[inp])
        wheel_order = {"wheel_scroll_up": 0, "wheel_scroll_down": 1, "wheel_middle": 2}
        return (3, wheel_order[inp])

    @classmethod
    def _chord_key(cls, inputs: list[str]) -> str:
        # Mirrors `daemon/src/config.rs::ChordKey`'s Display: a "+"-joined
        # string of member Input strings, ordered by `Input`'s own `Ord`
        # (see `_input_sort_key`), not alphabetically.
        return "+".join(sorted(inputs, key=cls._input_sort_key))

    @staticmethod
    def _chord_conflict(chords: dict, key: str, members: set[str]) -> str | None:
        # Ticket 01's amended Answer: only a subset/superset relationship
        # between two Chords' member sets conflicts — a plain intersection
        # (the thumbstick-diagonal shape) does not.
        for other_key in chords:
            if other_key == key:
                continue
            other_members = set(other_key.split("+"))
            if members <= other_members or other_members <= members:
                return other_key
        return None

    def set_chord_binding(self, inputs: list[str], layer: str, binding: dict) -> None:
        if len(inputs) < 2:
            raise InvalidBindingError("a Chord needs at least two member Inputs")
        if binding.get("type") == "profile_switch":
            raise InvalidBindingError("a Chord's Binding can't be a Profile Switch")
        if binding.get("trigger") == "analog_repeat":
            raise InvalidBindingError("a Chord's Binding can't use Analog-repeat")
        for input_str in inputs:
            self._reject_if_axis_assigned(input_str, layer)
        self._validate_binding_action(binding)
        key = self._chord_key(inputs)
        chords = self._profiles[self._active_profile][f"chords_{layer}"]
        conflicting = self._chord_conflict(chords, key, set(inputs))
        if conflicting is not None:
            raise InvalidBindingError(
                f"conflicts with the existing Chord {conflicting}: one member set fully "
                "contains the other"
            )
        chords[key] = copy.deepcopy(binding)
        self.calls.append(("set_chord_binding", list(inputs), layer, copy.deepcopy(binding)))

    def clear_chord_binding(self, inputs: list[str], layer: str) -> None:
        key = self._chord_key(inputs)
        chords = self._profiles[self._active_profile][f"chords_{layer}"]
        if key not in chords:
            raise NotFoundError(f"no Chord with members {key!r}")
        del chords[key]
        self.calls.append(("clear_chord_binding", list(inputs), layer))

    def set_axis_assignment(self, input_str: str, layer: str, target: str) -> None:
        if not is_grid_input(input_str):
            raise InvalidBindingError(f"{input_str!r} is not a Grid Input")
        if target not in AXIS_LABEL_BY_TARGET:
            # Mirrors the real Daemon's `wire::axis_target_from_str`
            # rejecting an unknown target string outright (code-review
            # finding: without this, a stub-based test can't catch a typo/
            # desync between `axis_picker.py`'s target strings and the
            # Daemon's own 17-entry wire catalog).
            raise InvalidBindingError(f"{target!r} is not a valid Axis target")
        profile = self._profiles[self._active_profile]
        # Ticket 59 §2's mutual exclusion: atomically clears any existing
        # Binding *and* any Chord membership for (layer, input_str)
        # alongside the insert — mirrors the real Daemon's `SetAxisAssignment`
        # handler, the mirror image of `_reject_if_axis_assigned` above.
        profile[layer].pop(input_str, None)
        chords = profile[f"chords_{layer}"]
        for key in [k for k in chords if input_str in k.split("+")]:
            del chords[key]
        profile[f"axis_{layer}"][input_str] = target
        self.calls.append(("set_axis_assignment", input_str, layer, target))

    def clear_axis_assignment(self, input_str: str, layer: str) -> None:
        axis_map = self._profiles[self._active_profile][f"axis_{layer}"]
        if input_str not in axis_map:
            raise NotFoundError(f"no Axis assignment is set for {input_str!r}")
        del axis_map[input_str]
        self.calls.append(("clear_axis_assignment", input_str, layer))

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
        self._profiles[name] = copy.deepcopy(self._SEED_PROFILE)
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

    @staticmethod
    def _slug_base(name: str, fallback: str = "macro") -> str:
        # Mirrors daemon/src/config.rs's `slug_base`: lowercase, runs of
        # non-alphanumeric characters collapsed to one hyphen, trimmed,
        # falling back to `fallback` if nothing alphanumeric survives.
        # Shared by both libraries, like the Rust side (ticket 03/54).
        slug = re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-")
        return slug or fallback

    def _unique_macro_id(self, name: str) -> str:
        base = self._slug_base(name, "macro")
        if base not in self._macros:
            return base
        n = 2
        while f"{base}-{n}" in self._macros:
            n += 1
        return f"{base}-{n}"

    def _unique_stepper_id(self, name: str) -> str:
        base = self._slug_base(name, "stepper")
        if base not in self._steppers:
            return base
        n = 2
        while f"{base}-{n}" in self._steppers:
            n += 1
        return f"{base}-{n}"

    def _stepper_referenced(self, stepper_id: str) -> bool:
        return any(
            binding.get("type") == "step" and binding.get("stepper_id") == stepper_id
            for profile in self._profiles.values()
            for layer in ("base", "held")
            for binding in profile[layer].values()
        )

    def _macro_referenced(self, macro_id: str) -> bool:
        return any(
            binding.get("type") == "macro" and binding.get("macro_id") == macro_id
            for profile in self._profiles.values()
            for layer in ("base", "held")
            for binding in profile[layer].values()
        )

    def create_macro(self, name: str, steps: list[dict]) -> str:
        if not name.strip():
            raise InvalidBindingError("Macro name can't be empty")
        macro_id = self._unique_macro_id(name)
        self._macros[macro_id] = {"name": name, "steps": copy.deepcopy(steps)}
        self.calls.append(("create_macro", name, copy.deepcopy(steps)))
        return macro_id

    def rename_macro(self, macro_id: str, new_name: str) -> None:
        if macro_id not in self._macros:
            raise NotFoundError(f"no Macro with id {macro_id!r}")
        if not new_name.strip():
            raise InvalidBindingError("Macro name can't be empty")
        self._macros[macro_id]["name"] = new_name
        self.calls.append(("rename_macro", macro_id, new_name))

    def delete_macro(self, macro_id: str) -> None:
        if macro_id not in self._macros:
            raise NotFoundError(f"no Macro with id {macro_id!r}")
        if self._macro_referenced(macro_id):
            raise InvalidBindingError(f"Macro {macro_id!r} is still referenced by a Binding")
        del self._macros[macro_id]
        self.calls.append(("delete_macro", macro_id))

    def set_macro_steps(self, macro_id: str, steps: list[dict]) -> None:
        if macro_id not in self._macros:
            raise NotFoundError(f"no Macro with id {macro_id!r}")
        self._macros[macro_id]["steps"] = copy.deepcopy(steps)
        self.calls.append(("set_macro_steps", macro_id, copy.deepcopy(steps)))

    def create_stepper(self, name: str, items: list[dict]) -> str:
        if not name.strip():
            raise InvalidBindingError("Stepper name can't be empty")
        stepper_id = self._unique_stepper_id(name)
        self._steppers[stepper_id] = {"name": name, "items": copy.deepcopy(items)}
        self.calls.append(("create_stepper", name, copy.deepcopy(items)))
        return stepper_id

    def rename_stepper(self, stepper_id: str, new_name: str) -> None:
        if stepper_id not in self._steppers:
            raise NotFoundError(f"no Stepper with id {stepper_id!r}")
        if not new_name.strip():
            raise InvalidBindingError("Stepper name can't be empty")
        self._steppers[stepper_id]["name"] = new_name
        self.calls.append(("rename_stepper", stepper_id, new_name))

    def delete_stepper(self, stepper_id: str) -> None:
        if stepper_id not in self._steppers:
            raise NotFoundError(f"no Stepper with id {stepper_id!r}")
        if self._stepper_referenced(stepper_id):
            raise InvalidBindingError(f"Stepper {stepper_id!r} is still referenced by a Binding")
        del self._steppers[stepper_id]
        self._stepper_cursors.pop(stepper_id, None)
        self.calls.append(("delete_stepper", stepper_id))

    def set_stepper_items(self, stepper_id: str, items: list[dict]) -> None:
        if stepper_id not in self._steppers:
            raise NotFoundError(f"no Stepper with id {stepper_id!r}")
        self._steppers[stepper_id]["items"] = copy.deepcopy(items)
        self.calls.append(("set_stepper_items", stepper_id, copy.deepcopy(items)))

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

    def set_actuation_point(self, input_str: str, actuation: int, release: int) -> None:
        if release > actuation:
            raise InvalidBindingError("release must not exceed actuation")
        overrides = self._profiles[self._active_profile]["actuation_overrides"]
        overrides[input_str] = {"actuation": actuation, "release": release}
        self.calls.append(("set_actuation_point", input_str, actuation, release))

    def clear_actuation_point(self, input_str: str) -> None:
        self._profiles[self._active_profile]["actuation_overrides"].pop(input_str, None)
        self.calls.append(("clear_actuation_point", input_str))

    def set_default_actuation(self, actuation: int, release: int) -> None:
        if release > actuation:
            raise InvalidBindingError("release must not exceed actuation")
        self._profiles[self._active_profile]["default_actuation"] = {
            "actuation": actuation,
            "release": release,
        }
        self.calls.append(("set_default_actuation", actuation, release))

    def reset_actuation_points(self) -> None:
        self._profiles[self._active_profile]["actuation_overrides"] = {}
        self.calls.append(("reset_actuation_points",))

    def set_force_digital(self, force: bool) -> None:
        # Ticket 27: `GetConfig()` now serializes `force_digital` (closing
        # the gap that left the "Force digital capture" checkbox unable to
        # reflect the real Daemon's persisted preference on reopen), so this
        # stub tracks it too rather than only recording the call.
        self._force_digital = force
        self.calls.append(("set_force_digital", force))

    def start_depth_stream(self, input_str: str, on_depth: Callable[[int], None]) -> None:
        self._depth_target = input_str
        self._depth_callback = on_depth
        self.calls.append(("start_depth_stream", input_str))

    def stop_depth_stream(self, input_str: str) -> None:
        self._depth_target = None
        self._depth_callback = None
        self.calls.append(("stop_depth_stream", input_str))

    def simulate_depth(self, input_str: str, depth: int) -> None:
        """Stands in for a real `DepthChanged(input, depth)` signal (ticket
        26) — only delivered if `input_str` matches the currently active
        `start_depth_stream` target, mirroring the real Daemon's single-
        current-stream semantics (`daemon_client.DBusDaemonClient.
        start_depth_stream`'s docstring)."""
        if input_str == self._depth_target and self._depth_callback is not None:
            self._depth_callback(depth)

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

    def subscribe_capture_mode_changed(self, callback: Callable[[str], None]) -> None:
        self._capture_mode_changed_callbacks.append(callback)

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

    def simulate_capture_mode(self, mode: str) -> None:
        """Stands in for a real `CaptureModeChanged` signal (ticket 17/23)
        — there's no analog `CaptureSource`/supervisor in GUI tests either."""
        if mode == self._capture_mode:
            return
        self._capture_mode = mode
        for callback in self._capture_mode_changed_callbacks:
            callback(mode)

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
