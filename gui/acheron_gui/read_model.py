# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Small read-only scans over a whole `GetConfig()` payload — the client-side
mirror of the Daemon's `edit.rs` reference-count guards.

Deliberately *not* a general read-model layer (ADR 0005): it holds exactly
the one Binding-reference scan that both `library_view` (the "Used by N"
delete gate) and `daemon_stub` (the delete-refusal it mirrors from
`edit.rs::macro_references` / `stepper_references`) need, so the predicate
lives in one place instead of three.
"""

from __future__ import annotations


def _profile_all_bindings(profile: dict):
    """Mirror of `config::profile_all_bindings` — every Binding across both
    per-Input Layers *and* both Chord Layers (ticket 40). The Daemon's
    reference-count guards scan all four, so a client-side mirror must too."""
    for layer_key in ("base", "held", "chords_base", "chords_held"):
        yield from profile[layer_key].values()


def reference_count(
    profiles: dict, *, binding_type: str, id_field: str, id_value: str
) -> int:
    """How many Bindings, across every Profile's Base/Held *and* Chord
    Layers, carry `binding[type] == binding_type` and
    `binding[id_field] == id_value` — `macro`/`macro_id` for a Macro,
    `step`/`stepper_id` for a Stepper list. Counted rather than boolean so
    the GUI's delete tooltip can name N."""
    return sum(
        1
        for profile in profiles.values()
        for binding in _profile_all_bindings(profile)
        if binding.get("type") == binding_type and binding.get(id_field) == id_value
    )
