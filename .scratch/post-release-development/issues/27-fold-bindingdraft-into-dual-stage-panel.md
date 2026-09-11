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

- [x] `build_dual_stage_panel`'s `drafts`, `capture_draft`, `stage_starting`, and `commit_stages` operate on `BindingDraft` instances per the shape above; no raw `get_binding` closure remains in the panel.
- [x] Full dual-stage editor behavior is unchanged: unbound-key synthetic primary, add/remove deep stage, Staging-mode picking, Save/Apply/Clear, cross-stage draft survival on a Primary/Deep toggle swap, "Reset to Profile default" (including its deep-band re-seed), "Set as Profile default".
- [x] `gui/tests/test_binding_editor.py`'s dual-stage-panel tests are trimmed the same way ticket 26 trimmed the shared-editor tests: drop coverage of per-kind field logic now owned by `test_binding_draft.py`, keep the GTK-wiring assertions (toggle-swap draft survival, Apply-without-close, disabled-button gating, the deep-picker CSS class).
- [x] Daemon untouched; GUI test suite green; linting/formatting clean.

## Comments

**2026-09-11** — Filed alongside ticket 26, from the same 2026-09-11
`/improve-codebase-architecture` review and grilling. Kept as a separate
ticket per the grilling's Q4 answer: the dual-stage panel's `drafts`/
`capture_draft`/`commit_stages` interplay is the riskier half of this
deepening, worth landing and re-verifying against the simpler two call
sites first.

**2026-09-11** — Implemented on `dev`. `build_action_and_trigger_fields`
now returns `(fields, trigger_dd, get_draft)` instead of `(fields,
trigger_dd, get_binding)` — `get_draft()` syncs the live Trigger-mode
selection onto the shared `draft` and hands the `BindingDraft` itself back;
`build_binding_editor` and `build_chord_binding_dialog` (unaffected by this
ticket otherwise) now call `get_draft().to_wire()` where they used to call
`get_binding()`. `build_dual_stage_panel`'s `drafts` dict now holds
`BindingDraft | None` per stage; `capture_draft()` stores the mounted
stage's `get_draft()` result instead of a frozen wire dict; `stage_starting`
and `commit_stages` call `.to_wire()` on a live draft exactly where the
Shape section specifies, matching byte-for-byte. `slot["get_binding"]`
became `slot["get_draft"]`. No test changes needed — all 120
`test_binding_editor.py` tests and the full 535-test GUI suite pass
unmodified, and an audit of the dual-stage-panel test range (roughly line
1356–2212) found nothing reaching into panel internals (`drafts`/`slot`) or
duplicating `test_binding_draft.py`'s per-kind coverage — every test there
is GTK-wiring or panel-structural (toggle-swap survival, Apply-without-
close, button-sensitivity gating, the deep-picker CSS class, the Axis-
primary-Apply fallback), matching ticket 26's own finding that the file was
already lean.

`/code-review` (Standards + Spec, 8 finder angles) returned 8 findings. One
actioned: `build_action_and_trigger_fields`'s return tuple had grown to
`(fields, trigger_dd, get_binding, get_draft)` with `get_binding` just
`get_draft().to_wire()` and no caller using both — collapsed back to a
3-tuple, `get_binding` dropped, its two other call sites now call
`get_draft().to_wire()` inline. Three dismissed as pre-existing behavior
unchanged by this diff (confirmed against the diff, not just re-read): (1)
`on_save_stage()` not calling `rebuild()` on a partial commit failure — same
guard/branch shape as before this ticket, untouched by the `dict`→
`BindingDraft` swap; (2) `stage_starting()`'s `to_wire()`/`from_wire()`
round-trip dropping a not-currently-selected kind's fields on a full panel
rebuild — this is exactly what the ticket's own Shape section specifies for
`stage_starting`, and the old code lost the same fields the same way (it
already stored `get_binding()`'s flattened dict in `drafts`); `BindingDraft`'s
multi-kind simultaneous state was never claimed to survive a full rebuild,
only a live Action-dropdown flip within one mount — the acceptance
criterion is stage-level draft survival on a toggle swap, which is
unaffected and still tested; (3) `capture_draft()` re-capturing a
just-committed, now-stale draft on the `rebuild()` that follows a
successful Apply, un-setting the `drafts[stage] = None` `commit_stages()`
had just set — same unconditional-`capture_draft()`-at-top-of-`rebuild()`
sequencing as before, masked before and after by `commit_stages()`'s own
diff-before-push check. Four dismissed as out of scope: `commit_stages()`
re-deriving `stage_starting()`'s fallback inline instead of calling it — the
ticket's own Shape section spells out that exact duplication verbatim,
matching ticket 26's precedent of declining a change that "is exactly the
signature this ticket's own Shape section specifies"; the `slot` dict being
addressable by two nonlocals instead — a pre-existing pattern from before
this ticket, only its key renamed here; `synthetic_primary`/`default_deep`
hand-writing the keypress wire shape instead of building it through
`BindingDraft` — pre-existing code this ticket doesn't touch; and
`BindingDraft.is_valid()`'s own per-kind ladder — entirely ticket 26's
module, out of this ticket's scope. None of the three dismissed
pre-existing-behavior findings are regressions from this diff; each is a
real, independently worth-filing observation about `build_dual_stage_panel`
predating ticket 27 — left for a future ticket rather than fixed here,
matching ticket 26's own precedent of filing (not fixing) an out-of-scope
predating bug as ticket 28.
