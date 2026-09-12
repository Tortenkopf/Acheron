<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 29 — Share the axis-vs-Binding push fork behind `push_stage`

**What to build:** A new GTK-free `gui/acheron_gui/stage_push.py` module
holding one function, `push_stage(client, stage, inp, layer, wire) -> None`,
that picks the right Daemon call for a `(stage, wire)` pair:

- `stage == "primary"` and `wire["type"] == "axis"` → `client.set_axis_
  assignment(inp, layer, wire["target"])`
- `stage == "primary"`, otherwise → `client.set_binding(inp, layer, wire)`
- `stage == "deep"` → `client.set_deep_stage(inp, layer, wire)` (never checks
  for `"axis"` — a deep stage can't be one, CONTEXT.md's Axis assignment
  entry)

`build_dual_stage_panel.commit_stages` (`binding_editor.py:1210-1221`) and
`build_binding_editor.on_save` (`binding_editor.py:1725-1735`) both call it
instead of re-deriving the fork inline. Nothing else about either caller
changes: `commit_stages` still mirrors the snapshot and calls
`plan.mark_committed(stage)` itself after a successful push; `on_save` still
does neither. Both keep their own `try`/`except DaemonError`/`show_error` —
`push_stage` raises through, it doesn't catch.

**Out of scope:** `on_clear`'s `clear_axis_assignment` vs. `clear_binding`
fork (`binding_editor.py:1739-1757`) stays where it is. It has exactly one
caller today — the dual-stage panel's `on_clear_stage` never clears an axis,
since an Axis-assigned key never reaches that panel in the first place — so
a shared `clear_stage` would be a hypothetical seam, not a real one. Revisit
if a second clear-caller appears.

**Blocked by:** None — can start immediately.

## Why

Filed from the 2026-09-12 `/improve-codebase-architecture` review (scoped to
the dual-stage binding editor, the area tickets 26-30 were already carving
up) and the grilling that followed it. `commit_stages` and `on_save` each
hand-write the identical `wire["type"] == "axis"` fork against the matching
Daemon call — duplicated, not shared. Deletion test: removing the fork from
either call site just makes it reappear verbatim in the other, so it was
never earning its keep as separate code.

`library_view.py`'s `LibraryKind` (two adapters, `MACRO`/`STEPPER`) already
proves the shape this needs — a small adapter shared by two concrete cases.
Binding/Axis is the same "two adapters, a real seam" situation, just never
built.

The grilling settled the load-bearing decisions:

- **Push only, not push+clear.** `clear_axis_assignment`/`clear_binding` has
  one real caller; building a shared adapter for it now would be speculative.
- **New module, not inside `dual_stage_plan.py`.** `DualStagePlan`'s own
  docstring is explicit that it "never touches the Daemon or a
  `daemon_client`" and that axis-vs-Binding dispatch is deliberately the
  caller's job (ticket 30). Putting `push_stage` there would undo that.
  `binding_draft.py` is an even worse fit — a pure value object with no
  Daemon awareness at all.
- **`stage` is an explicit parameter, not inferred.** A plain Keypress wire
  looks identical whether it's the primary or the deep Binding — nothing in
  `wire` says which.
- **No snapshot mirroring inside the adapter.** `commit_stages` mutates
  `profile_dict` in place (the window stays open for Apply); `on_save` does
  no mirroring at all (the window closes, a later full rebuild picks up
  fresh state). That asymmetry means mirroring is a caller concern, not
  shared behavior — folding it into `push_stage` would force one caller to
  pass a no-op.
- **No error handling inside the adapter**, for the same reason: `commit_
  stages` returns a bool used across a loop of steps; `on_save` just returns
  early. `push_stage` stays pure dispatch — call the right method, return
  `None`, let `DaemonError` propagate.
- **No CONTEXT.md entry.** `push_stage`/`stage_push` is internal plumbing
  with no domain meaning, matching the ADR-0010 (ticket 10/14/15/17)
  precedent for declining a glossary entry for a locus refactor.

## Shape

```python
# gui/acheron_gui/stage_push.py — no GTK imports.
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
```

`commit_stages`' loop body becomes:

```python
for stage, wire in steps:
    push_stage(client, stage, inp, layer, wire)
    if stage == "primary" and wire.get("type") == "axis":
        profile_dict[f"axis_{layer}"][inp] = wire["target"]
        profile_dict[layer].pop(inp, None)
    elif stage == "primary":
        profile_dict[layer][inp] = wire
    else:
        deep_map()[inp] = wire
    plan.mark_committed(stage)
    on_commit()
```

(the mirroring `if`/`elif`/`else` stays inline — it's not shared with
`on_save`, see "Why") and `on_save` becomes:

```python
def on_save(b):
    binding = get_draft().to_wire()
    try:
        push_stage(client, "primary", inp, layer, binding)
    except DaemonError as exc:
        show_error(exc)
        return
    on_saved()
```

## Testing

`gui/tests/test_stage_push.py`, a pure test file (no `Gtk` import) using
`DaemonStub()` directly — the existing in-memory fake `test_binding_editor.py`
already uses for its GTK-integration tests — and asserting on the stub's
resulting config state, not on a call-recording mock:

- `stage="primary"`, non-axis wire → `stub`'s `base`/`held` map gets the
  Binding.
- `stage="primary"`, axis wire → `stub`'s `axis_base`/`axis_held` map gets
  the target, no Binding.
- `stage="deep"`, non-axis wire → `stub`'s `deep_base`/`deep_held` map gets
  the Binding.
- A rejected push (e.g. `analog_repeat` on a non-grid Input) raises
  `DaemonError` and leaves the stub's config unchanged.

## Acceptance criteria

- [x] `gui/acheron_gui/stage_push.py` exists, is importable with no `gi`/`Gtk`
      dependency, and `push_stage` matches the Shape section.
- [x] `gui/tests/test_stage_push.py` covers the four cases above against
      `DaemonStub`.
- [x] `commit_stages` and `on_save` both call `push_stage`; the axis-vs-
      Binding `if`/`elif`/`else` fork is gone from both, replaced by the one
      in `stage_push.py`.
- [x] `on_clear`'s clear-fork is untouched.
- [x] Behavior is unchanged for both callers (no test assertions need to
      change beyond what moves to `test_stage_push.py`, if anything does).
- [x] GUI test suite green.

## Comments

**2026-09-12** — Filed. Second-ranked candidate of the 2026-09-12
`/improve-codebase-architecture` review (top recommendation, ahead of the
broader "collapse the dual-stage panel's push-then-mirror-then-rebuild
idiom" candidate — this ticket's `push_stage` is meant to seed that larger
cleanup rather than duplicate it). Two rounds of grilling settled the shape;
see "Why" above for the load-bearing decisions.

**2026-09-12** — Done. Implemented exactly per the Shape section:
`stage_push.py` holds the one `push_stage` function, no GTK import.
`commit_stages` (`binding_editor.py`) keeps its inline snapshot-mirroring
`if`/`elif`/`else` and `plan.mark_committed`/`on_commit()` calls, now
preceded by a `push_stage(...)` call instead of the inline Daemon-call fork;
`on_save` collapses to a single `push_stage(client, "primary", inp, layer,
binding)` inside its existing `try`/`except DaemonError`. `on_clear`
untouched. `test_stage_push.py` covers all four cases against `DaemonStub`
directly (no call-recording mock — assertions read the stub's resulting
`base`/`axis_base`/`deep_base` maps, and that a rejected push leaves
`get_config()` unchanged). Full GUI suite green (552 passed);
`/code-review` surfaced no findings.
