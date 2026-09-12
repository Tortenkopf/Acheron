# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""One Binding's draft state, carved out of
`binding_editor.build_action_and_trigger_fields` (ticket 26) so the six
per-Action-kind field ladders it used to reproduce three times over (the seed
dict, the live GTK callbacks, `get_binding()`) live in one place, reachable
without building a widget tree.

`BindingDraft` holds all six Action kinds' fields simultaneously — flipping
the Action dropdown and back doesn't lose an edit to a kind that isn't
currently selected — tagged with which kind is currently active. `to_wire()`
returns exactly the flat dict `wire.py`/`daemon_client` already consume
(`{"trigger": ..., "type": ..., ...}`), matching `get_binding()`'s old
per-kind ladder byte-for-byte, including the Axis special case (no
`"trigger"` key at all — Axis assignment isn't a `Binding`, ticket 59 §2).

`is_valid()` is a narrow completeness check — does the selected kind's
required reference field have a value — matching exactly what
`save_btn.set_sensitive(...)` checked inline before this carve. It never
re-derives Trigger/Action legality: `rules.valid_triggers`/
`valid_action_kinds` still gate what a `Gtk.DropDown` *offers*, so an illegal
value can never reach a draft in the first place.

No `__eq__` — a caller that needs to diff a draft against a snapshot compares
`to_wire()` output as a plain dict (as `commit_stages` already does).

`on_change` (post-release ticket 31) is a plain `Callable[[], None]`,
defaulting to a no-op, fired at the end of every setter. It carries no `Gtk`
dependency itself — a caller that wants to resync some UI state (e.g.
`build_action_and_trigger_fields`'s `save_btn.set_sensitive(draft.is_valid())`)
assigns it as a plain attribute after construction. Every other caller
(`build_dual_stage_panel`, `test_binding_draft.py`) never sets it, so it stays
inert for them — the same as it was before this hook existed.
"""

from __future__ import annotations

from typing import Callable, Iterable

from .inputs import default_key_code_for, default_trigger_for


class BindingDraft:
    def __init__(
        self,
        *,
        kind: str,
        trigger: str,
        keypress: dict,
        macro: dict,
        step: dict,
        profile_switch: dict,
        controller_button: dict,
        axis: dict,
    ) -> None:
        self.kind = kind
        self.trigger = trigger
        self.keypress = keypress
        self.macro = macro
        self.step = step
        self.profile_switch = profile_switch
        self.controller_button = controller_button
        self.axis = axis
        self.on_change: Callable[[], None] = lambda: None

    @classmethod
    def from_wire(cls, starting: dict | None, *, inp: str | None, profile: str) -> "BindingDraft":
        """Seeds each kind's sub-state from `starting` if it matches
        `starting["type"]`, else a default — `default_key_code_for(inp)`
        (already pure, already handles `inp is None` for the Chord case) for
        the inactive Keypress slot, a fixed default everywhere else. Mirrors
        `build_action_and_trigger_fields`'s old seed dict exactly, including
        for a `starting["type"]` this module doesn't itself know how to
        render (`unsupported_kind`, handled entirely by the caller — every
        sub-state below just falls through to its default, same as today).

        `starting=None` (post-release ticket 29) means "no Binding at all
        yet" — a fresh, unbound Keypress: `kind` defaults to `"keypress"`,
        seeded the same way the *inactive* Keypress slot always has been
        (`default_key_code_for(inp)` / `default_trigger_for(inp)`), not the
        dead-code `"KEY_A"` fallback the *active*-Keypress branch below keeps
        for a real (malformed) `starting` dict missing its `"key"`. This is
        now the single place every "what does an unbound Binding look like"
        call site asks — `BindingDraft.from_wire(None, inp=inp,
        profile=profile).to_wire()` replaces what used to be four separately
        hand-built dict literals (the dual-stage panel's synthetic primary
        stage and fresh deep stage, the plain editor's fresh-binding
        fallback, the Chord dialog's fresh-binding fallback).

        `starting.get("trigger", ...)` rather than `starting["trigger"]`
        (ticket 28): an Axis-kind `starting` — this class's own `to_wire()`
        output, round-tripped back in as `starting` by the dual-stage panel's
        `stage_starting` after an unsaved Axis edit survives a stage swap —
        carries no `"trigger"` key at all (Axis assignment isn't a `Binding`,
        ticket 59 §2). `self.trigger` is inert for Axis either way (locked/
        hidden in the UI, dropped by `to_wire()`), so any in-range default
        does; `default_trigger_for(inp)` matches the fresh-Binding seed
        above."""
        kind = starting["type"] if starting is not None else "keypress"
        return cls(
            kind=kind,
            trigger=(starting or {}).get("trigger", default_trigger_for(inp)),
            keypress=(
                {"key": starting.get("key", "KEY_A"), "modifiers": list(starting.get("modifiers", []))}
                if starting is not None and kind == "keypress"
                else {"key": default_key_code_for(inp), "modifiers": []}
            ),
            macro={"macro_id": starting.get("macro_id")} if kind == "macro" else {"macro_id": None},
            step=(
                {"stepper_id": starting.get("stepper_id"), "direction": starting.get("direction", "forward")}
                if kind == "step"
                else {"stepper_id": None, "direction": "forward"}
            ),
            profile_switch=(
                {"target": starting.get("target", profile)} if kind == "profile_switch" else {"target": profile}
            ),
            controller_button=(
                {"button": starting.get("button", "BTN_SOUTH")}
                if kind == "controller_button"
                else {"button": "BTN_SOUTH"}
            ),
            axis={"target": starting.get("target")} if kind == "axis" else {"target": None},
        )

    def to_wire(self) -> dict:
        if self.kind == "axis":
            return {"type": "axis", "target": self.axis["target"]}
        if self.kind == "keypress":
            return {
                "trigger": self.trigger,
                "type": "keypress",
                "key": self.keypress["key"],
                "modifiers": self.keypress["modifiers"],
            }
        if self.kind == "profile_switch":
            # Always Fire-once, regardless of `self.trigger` — the Daemon
            # rejects anything else anyway (`InvalidProfileSwitchTrigger`),
            # and the Trigger-mode dropdown is disabled/forced to match
            # while Profile Switch is selected.
            return {"trigger": "fire_once", "type": "profile_switch", "target": self.profile_switch["target"]}
        if self.kind == "controller_button":
            return {
                "trigger": self.trigger,
                "type": "controller_button",
                "button": self.controller_button["button"],
            }
        if self.kind == "step":
            return {
                "trigger": self.trigger,
                "type": "step",
                "stepper_id": self.step["stepper_id"],
                "direction": self.step["direction"],
            }
        return {"trigger": self.trigger, "type": "macro", "macro_id": self.macro["macro_id"]}

    def is_valid(self) -> bool:
        if self.kind == "axis":
            return self.axis["target"] is not None
        if self.kind == "macro":
            return self.macro["macro_id"] is not None
        if self.kind == "step":
            return self.step["stepper_id"] is not None
        return True

    def set_kind(self, kind: str) -> None:
        self.kind = kind
        self.on_change()

    def set_trigger(self, trigger: str) -> None:
        self.trigger = trigger
        self.on_change()

    def set_keypress_key(self, code: str) -> None:
        self.keypress["key"] = code
        self.on_change()

    def set_keypress_modifiers(self, modifiers: Iterable[str]) -> None:
        self.keypress["modifiers"] = sorted(modifiers)
        self.on_change()

    def set_macro_id(self, macro_id: str | None) -> None:
        self.macro["macro_id"] = macro_id
        self.on_change()

    def set_stepper(self, stepper_id: str | None, direction: str) -> None:
        self.step["stepper_id"] = stepper_id
        self.step["direction"] = direction
        self.on_change()

    def set_profile_switch_target(self, target: str) -> None:
        self.profile_switch["target"] = target
        self.on_change()

    def set_controller_button(self, button: str) -> None:
        self.controller_button["button"] = button
        self.on_change()

    def set_axis_target(self, target: str | None) -> None:
        self.axis["target"] = target
        self.on_change()
