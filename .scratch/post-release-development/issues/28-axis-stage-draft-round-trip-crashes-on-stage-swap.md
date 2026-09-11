<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 28 — A stage's unsaved Axis draft crashes the dual-stage panel on the next rebuild

**What to build:** Either `build_action_and_trigger_fields` (via
`BindingDraft.from_wire`) tolerates a `starting` dict with no `"trigger"`
key, or `build_dual_stage_panel` stops handing an Axis-kind captured draft
back in as `starting` for a stage whose Trigger-mode dropdown expects one
present. Whichever side owns the fix, it needs a regression test — none
currently exercises this path.

**Blocked by:** None.

## Why

`build_dual_stage_panel`'s `available_action_types` (the primary stage's own
Action menu) includes `"axis"` for a Grid Input — nothing filters it out the
way `_deep_action_types` filters it out of the *deep* stage's menu. If a user
picks Axis on the primary stage and assigns a target without hitting
Save/Apply, `capture_draft()` stores `slot["get_binding"]()`'s result —
`{"type": "axis", "target": ...}`, no `"trigger"` key at all (Axis assignment
isn't a `Binding`, ticket 59 §2) — into `drafts["primary"]`. Toggling to the
other stage and back (or any other structural edit that calls `rebuild()`,
e.g. `+ Add deep stage`) feeds that dict straight back in as `starting` via
`stage_starting("primary")`, and `build_action_and_trigger_fields` crashes
with `KeyError: 'trigger'` — both at its own `trigger_dd.set_selected(
trigger_keys.index(starting["trigger"]))` and, after ticket 26, inside
`BindingDraft.from_wire`'s `trigger=starting["trigger"]`.

Pre-existing: the old dict-based `get_binding()`'s axis branch returned this
exact same shape, so the crash predates ticket 26's `BindingDraft` carve —
carried over rather than introduced, and not something ticket 26's per-stage
scope covers (see that ticket's "Immediate structural pushes stay out of
scope" note). Found by ticket 26's `/code-review`, reproduced end-to-end
against the real widget tree.

## Acceptance criteria

- [ ] Picking Axis on an unbound-elsewhere Grid key's primary stage, assigning
      a target, then toggling to the deep-stage swap and back no longer
      crashes.
- [ ] A `dispatch`/`binding_editor` test reproduces the crash on `dev` before
      the fix and passes after.
- [ ] Decide and document which side owns the invariant: either `starting`
      dicts are never assumed to carry `"trigger"` when `type == "axis"`, or
      the dual-stage panel's own draft capture normalizes/rejects an
      Axis-kind primary draft before it can round-trip back in as `starting`.
- [ ] GUI suite green.

## Comments

**2026-09-11** — Filed from ticket 26's `/code-review` (BindingDraft carve).
