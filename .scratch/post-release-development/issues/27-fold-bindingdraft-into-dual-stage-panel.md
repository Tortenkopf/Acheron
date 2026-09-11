<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 27 — Fold `BindingDraft` into the dual-stage grid-key panel

**What to build:** `build_dual_stage_panel`'s `drafts` dict, `capture_draft`,
and `commit_stages` swapped to hold and diff `BindingDraft` instances (one
per stage — primary and deep) instead of the raw dict/closure pair
`build_action_and_trigger_fields` currently hands back. Full behavior parity
for the dual-stage grid-key editor: unbound-key synthetic primary, `+ Add
deep stage` / `✕ Remove`, Staging-mode row, Save/Apply/Clear, "Reset to
Profile default" / "Set as Profile default", all unchanged.

**Blocked by:** 26 — needs `gui/acheron_gui/binding_draft.py` and the
`BindingDraft`-returning `build_action_and_trigger_fields` it lands.

## Why

Ticket 26 carves `BindingDraft` out of the shared Trigger/Action field
editor and wires it into the two simpler callers (non-grid editor, Chord
dialog). `build_dual_stage_panel` is the third and fourth caller — it mounts
`build_action_and_trigger_fields` twice (once per stage, `binding_editor.py`
lines ~1591–1600) and already receives a `get_binding` closure back from
each call; today that closure is stashed in `slot["get_binding"]` and read by
`capture_draft()`/`commit_stages()` as the source of "what does the
on-screen stage currently say". Once ticket 26 lands, that closure is a
`BindingDraft` for free at the call site — this ticket is the harder half:
threading it through the panel's own state (`drafts`, `ui["stage"]`,
`slot`), not the field editor itself.

Deliberately **not** touched, per the grilling that settled both tickets'
scope: the structural operations that push to the Daemon immediately
(`on_add_deep`, `on_remove_deep`, `on_pick_mode`, the actuation-marker drag
handlers) stay exactly as they are — they were never part of what
`get_binding()`/`BindingDraft` models.

## Shape

`drafts: dict = {"primary": None, "deep": None}` becomes
`drafts: dict[str, BindingDraft | None] = {"primary": None, "deep": None}` —
`None` still means "not edited, read the snapshot", unchanged.

`capture_draft()` folds the on-screen stage's `BindingDraft` into `drafts`
before the widgets are torn down — same guard (`save_btn.get_sensitive()`),
just storing the draft object instead of calling `get_binding()` to freeze a
dict:

```python
def capture_draft() -> None:
    if slot["draft"] is not None and save_btn.get_sensitive():
        drafts[slot["stage"]] = slot["draft"]
```

`stage_starting(stage)` still returns a wire dict, not a `BindingDraft` — the
field editor calls `BindingDraft.from_wire` on whatever it returns (ticket
26's territory), so a still-live draft has to be flattened back to a dict
first:

```python
def stage_starting(stage: str) -> dict | None:
    return drafts[stage].to_wire() if drafts[stage] is not None else stage_snapshot(stage)
```

`commit_stages()` diffs `to_wire()` output against the snapshot exactly as it
diffs `get_binding()`'s dict today — no behavior change, just the source:

```python
def commit_stages() -> bool:
    capture_draft()
    primary_target = drafts["primary"].to_wire() if drafts["primary"] is not None else primary_binding()
    deep_target = drafts["deep"].to_wire() if drafts["deep"] is not None else deep_binding()
    ...  # unchanged from here down
```

No `BindingDraft.__eq__` needed anywhere in this ticket either — every
comparison here is `to_wire()` dict vs. snapshot dict, matching ticket 26's
Q12 answer.

## Acceptance criteria

- [ ] `build_dual_stage_panel`'s `drafts`, `capture_draft`, `stage_starting`, and `commit_stages` operate on `BindingDraft` instances per the shape above; no raw `get_binding` closure remains in the panel.
- [ ] Full dual-stage editor behavior is unchanged: unbound-key synthetic primary, add/remove deep stage, Staging-mode picking, Save/Apply/Clear, cross-stage draft survival on a Primary/Deep toggle swap, "Reset to Profile default" (including its deep-band re-seed), "Set as Profile default".
- [ ] `gui/tests/test_binding_editor.py`'s dual-stage-panel tests are trimmed the same way ticket 26 trimmed the shared-editor tests: drop coverage of per-kind field logic now owned by `test_binding_draft.py`, keep the GTK-wiring assertions (toggle-swap draft survival, Apply-without-close, disabled-button gating, the deep-picker CSS class).
- [ ] Daemon untouched; GUI test suite green; linting/formatting clean.

## Comments

**2026-09-11** — Filed alongside ticket 26, from the same 2026-09-11
`/improve-codebase-architecture` review and grilling. Kept as a separate
ticket per the grilling's Q4 answer: the dual-stage panel's `drafts`/
`capture_draft`/`commit_stages` interplay is the riskier half of this
deepening, worth landing and re-verifying against the simpler two call
sites first.
