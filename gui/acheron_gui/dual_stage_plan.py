# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""The dual-stage swap panel's draft/commit-ordering policy, carved out of
`binding_editor.build_dual_stage_panel` (post-release ticket 30) the same way
ticket 26 carved `BindingDraft` out of `build_action_and_trigger_fields`.

`DualStagePlan` holds the primary and deep stage's in-progress `BindingDraft`s
— `None` for a stage means "not edited since the last commit, read the
snapshot" — and answers the two questions a caller needs to drive Save/Apply:
what should this stage's field editor seed from right now (`stage_starting`),
and what, in what order, actually needs pushing to the Daemon (`diff`).

It never touches the Daemon or a `daemon_client`, and never holds a `config`/
`profile_dict` reference — every method takes exactly what it needs as an
argument, the same discipline `BindingDraft` itself keeps. The caller (the
panel) still owns: deciding *when* to capture (skipped while Save is
disabled), the axis-vs-Binding dispatch on a `diff()` step's wire dict
(`wire["type"] == "axis"` means `set_axis_assignment`, not `set_binding`),
and mutating its own config snapshot on a successful push.
"""

from __future__ import annotations

from .binding_draft import BindingDraft


class DualStagePlan:
    def __init__(self) -> None:
        self.drafts: dict[str, BindingDraft | None] = {"primary": None, "deep": None}

    def capture(self, stage: str, draft: BindingDraft) -> None:
        """Fold the currently-mounted stage's live draft in, replacing
        whatever was captured for it before."""
        self.drafts[stage] = draft

    def stage_starting(self, stage: str, snapshot: dict | None) -> dict | None:
        """The dict a stage's field editor should seed from: its live
        draft's wire form if one exists, else the passed-in snapshot
        (already resolved to the synthetic default for an unbound primary —
        that resolution is the caller's job, not this type's)."""
        draft = self.drafts[stage]
        return draft.to_wire() if draft is not None else snapshot

    def diff(self, primary_snapshot: dict | None, deep_snapshot: dict | None) -> list[tuple[str, dict]]:
        """The ordered `(stage, wire)` pushes actually needed, primary before
        deep. `primary_snapshot`/`deep_snapshot` are the *raw* snapshot
        values (`None` when that stage has no Binding at all yet) — not the
        synthetic-default-resolved form `stage_starting` takes.

        Primary is pushed whenever its draft differs from `primary_snapshot`
        — including unconditionally when `primary_snapshot is None`, since an
        as-yet-unbound key's first Save always creates the Binding, matching
        the old plain editor's "Save always calls set_binding". A primary
        with no captured draft is never pushed (nothing to push) — in
        practice this never arises when primary is the mounted stage, since
        the caller captures it before diffing, and a primary can't go
        unmounted while unbound (a deep stage requires a primary to exist).

        Deep is pushed only when `deep_snapshot` is not `None` (a deep stage
        that doesn't exist yet is never created by this diff — `+ Add deep
        stage` is a separate structural edit) and its draft differs from it.
        """
        steps: list[tuple[str, dict]] = []
        primary_draft = self.drafts["primary"]
        if primary_draft is not None:
            primary_target = primary_draft.to_wire()
            if primary_snapshot is None or primary_target != primary_snapshot:
                steps.append(("primary", primary_target))
        deep_draft = self.drafts["deep"]
        if deep_snapshot is not None and deep_draft is not None:
            deep_target = deep_draft.to_wire()
            if deep_target != deep_snapshot:
                steps.append(("deep", deep_target))
        return steps

    def mark_committed(self, stage: str) -> None:
        """Clear a stage's draft — after a successful push in `diff()`'s
        order, or after a structural edit (`+ Add deep stage`, `Remove deep
        stage`) makes any prior draft for that stage stale."""
        self.drafts[stage] = None
