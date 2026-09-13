# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Post-release-development ticket 29: the axis-vs-Binding push fork shared
by `build_dual_stage_panel.commit_stages` and `build_binding_editor.on_save`
(`binding_editor.py`) — both picked the right Daemon call for a committed
`(stage, wire)` pair by hand-writing the identical `wire["type"] == "axis"`
fork; `push_stage` is that fork, extracted once both call sites proved it.

No GTK import — a pure dispatch function over a `daemon_client`-shaped
`client`, with no snapshot mirroring and no error handling: `commit_stages`
and `on_save` differ on both (see their own call sites), so `push_stage`
stays pure — call the right method, return `None`, let `DaemonError`
propagate.
"""

from __future__ import annotations

from typing import Literal


def push_stage(
    client, stage: Literal["primary", "deep"], inp: str, layer: str, wire: dict
) -> None:
    """Picks the Daemon call for a stage's committed wire dict. Raises
    DaemonError through — callers keep their own error handling and
    snapshot mirroring, which differ between them."""
    if stage == "primary" and wire.get("type") == "axis":
        client.set_axis_assignment(inp, layer, wire["target"])
    elif stage == "primary":
        client.set_binding(inp, layer, wire)
    else:
        client.set_deep_stage(inp, layer, wire)
