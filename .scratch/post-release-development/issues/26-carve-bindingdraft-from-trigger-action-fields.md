<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 26 — Carve a pure `BindingDraft` out of the Trigger/Action field editor

**What to build:** A new GTK-free `gui/acheron_gui/binding_draft.py` module
holding `BindingDraft` — the draft state `build_action_and_trigger_fields`
currently keeps as a bare `draft` dict of closures — with a matching pure
test file. `build_action_and_trigger_fields`'s `draft` dict + `get_binding()`
ladder is replaced by it. This is the shared Trigger/Action editor used
verbatim by the non-grid Binding editor and the Chord binding dialog, so both
get the seam for free; the dual-stage grid-key panel is deliberately **not**
touched here (see ticket 27).

**Blocked by:** None — can start immediately.

## Why

`build_action_and_trigger_fields` (`gui/acheron_gui/binding_editor.py`) is
shallow where it matters most: a Binding's draft state is a `dict` closed
over by ~15 nested GTK callbacks (`on_key_changed`, `on_mod`,
`on_target_changed`, `on_button_changed`, `on_axis_changed`,
`on_stepper_changed`, `on_direction_changed`, `on_macro_changed`, …), and
`get_binding()` is a second `if kind == …` ladder reproducing the same six
Action kinds the seed dict (lines ~594–612) already enumerates a third time.
None of that draft-composition logic is reachable except by building the
whole widget tree and clicking through it — `gui/tests/test_binding_editor.py`
(2212 lines, the largest test file in the repo) drives every case that way.

Filed from a 2026-09-11 architecture review (run via `/improve-codebase-
architecture`, scoped to `binding_editor.py`/`test_binding_editor.py` now
that ticket 25 left the daemon-side runtime-orphan cluster with no open
wound — rendered as a temporary report, not checked in) and the grilling
that followed it. Thirteen rounds settled the shape; the load-bearing
decisions:

- **Scope is per-stage**, not the whole dual-stage panel. `BindingDraft`
  models exactly what `build_action_and_trigger_fields` already models — one
  Binding's trigger + action-kind + kind-specific fields — because that's the
  actual duplication (three per-kind ladders), and it's already reused at
  four call sites (non-grid editor, Chord dialog, and the dual-stage panel's
  two slots — the last of those is ticket 27's job).
- **Immediate structural pushes stay out of scope.** The dual-stage panel's
  `on_add_deep` / `on_remove_deep` / `on_pick_mode` / actuation-marker-drag
  each push to the Daemon the instant the user acts, independent of Save.
  None of that is "what does this Binding do" — `BindingDraft` doesn't touch
  it, in this ticket or the next.
- **`rules.py` stays where it is.** The panel keeps calling
  `rules.valid_triggers` / `valid_action_kinds` to decide which options a
  `Gtk.DropDown` *offers* — an illegal value can therefore never reach the
  draft. `BindingDraft.is_valid()` is a narrower completeness check (does the
  selected kind's required reference field have a value), matching exactly
  what `save_btn.set_sensitive(...)` checks today.

## Shape

```python
# gui/acheron_gui/binding_draft.py — no GTK imports.
class BindingDraft:
    """One Binding's draft state: all six Action kinds' fields held
    simultaneously (so flipping the Action dropdown and back doesn't lose an
    edit), tagged with which kind is currently active. Mirrors the flat wire
    shape `wire.py`/`daemon_client` already consume — `to_wire()` returns
    exactly what `get_binding()` returns today."""

    @classmethod
    def from_wire(cls, starting: dict, *, inp: str | None, profile: str) -> "BindingDraft":
        # Seeds each kind's sub-state from `starting` if it matches, else a
        # default — `default_key_code_for(inp)` / `default_trigger_for(inp)`
        # (both already pure, already handle `inp=None` for the Chord case)
        # called directly, matching today's seed dict at binding_editor.py
        # lines ~594-612.
        ...

    def to_wire(self) -> dict:
        # Matches get_binding()'s per-kind ladder byte-for-byte, including
        # the axis special case (no "trigger" key).
        ...

    def is_valid(self) -> bool:
        # Narrow completeness only: macro_id/stepper_id/axis target is not
        # None. Never re-derives Trigger/Action legality — the panel's
        # dropdowns structurally can't offer an illegal option.
        ...

    # Named setters, one per field — no generic set_field(kind, field, value):
    def set_kind(self, kind: str) -> None: ...
    def set_trigger(self, trigger: str) -> None: ...
    def set_keypress_key(self, code: str) -> None: ...
    def set_keypress_modifiers(self, modifiers: list[str]) -> None: ...
    def set_macro_id(self, macro_id: str | None) -> None: ...
    def set_stepper(self, stepper_id: str | None, direction: str) -> None: ...
    def set_profile_switch_target(self, target: str) -> None: ...
    def set_controller_button(self, button: str) -> None: ...
    def set_axis_target(self, target: str | None) -> None: ...
```

No `__eq__` — a caller that needs to diff a draft against a snapshot compares
`to_wire()` output as a plain dict (as `commit_stages` already does).

**Write-accumulator, not render-from-draft.** Each GTK callback keeps its own
live widget state and, on change, calls `draft.set_x(...)` immediately
followed by `save_btn.set_sensitive(draft.is_valid())` — two lines where
there's one ad hoc line today. `render_action_editor` still only rebuilds on
an Action-*kind* change, exactly as now; no observer/subscribe machinery, no
full re-render on every keystroke. Transient UI logic that reads "what's
currently selected" (`key_warn_predicate`, `sync_analog_hint`) keeps reading
the live GTK widget directly rather than bouncing through the draft.

**Unknown wire `type`** (`unsupported_kind`): `build_action_and_trigger_fields`
never calls `BindingDraft.from_wire` on it — the panel keeps building a fresh
default-kind draft and disabling Save exactly as today. `BindingDraft` gets
no "unsupported" state; nothing today round-trips an opaque Binding, it's
discarded the moment a real kind is picked.

## Acceptance criteria

- [x] `gui/acheron_gui/binding_draft.py` exists, is importable with no `gi`/`Gtk` dependency, and `BindingDraft` covers all six Action kinds per the shape above.
- [x] `gui/tests/test_binding_draft.py` is a pure test file (no `Gtk` import) covering construction from `from_wire` for every kind (including the `inp=None` Chord case and default-seeding an unbound Input), every setter, `is_valid()` transitions, and `to_wire()` output per kind including the axis special case.
- [x] `build_action_and_trigger_fields`'s `draft` dict and `get_binding()` are gone, replaced by a `BindingDraft` instance; behavior is unchanged for the non-grid Binding editor and the Chord binding dialog.
- [x] `gui/tests/test_binding_editor.py` sheds the widget-tree tests that were only exercising per-kind field logic now covered directly by `test_binding_draft.py`, and keeps the ones asserting real GTK wiring (which picker mounts per kind, Save-button sensitivity, dropdown rebuild on kind change, the Analog-repeat hint). — audited every test in the non-grid/Chord range; none qualified (see Comments).
- [x] `build_dual_stage_panel` is untouched (still uses its own `draft` dict via `build_action_and_trigger_fields` internally) — ticket 27's job.
- [x] Daemon and GUI test suites both green; `gui` linting/formatting clean (no lint/format tooling is configured in this repo to run).

## Comments

**2026-09-11** — Filed. Top recommendation of the 2026-09-11
`/improve-codebase-architecture` review, run once ticket 25 (2026-09-10)
left the daemon-side runtime-orphan cluster with no open wound and the
GUI binding editor had never had a deepening pass. Split into two tickets
(26/27) at the grilling's Q4 boundary: land the shared Trigger/Action
editor first (covers two of four call sites, lower risk), then fold the
harder dual-stage-panel composition in separately.

**2026-09-11** — Implemented on `dev`. `binding_draft.py`'s `BindingDraft`
matches the Shape section exactly: `kind`/`trigger` plus one dict per Action
kind, `from_wire`/`to_wire`/`is_valid()` and the nine named setters.
`build_action_and_trigger_fields` now does `draft.set_kind(kind)` at the top
of `render_action_editor` and `draft.set_<field>(...)` from each GTK
callback instead of mutating a bare dict; the per-branch manual
`save_btn.set_sensitive(...)` calls collapsed into one
`save_btn.set_sensitive(draft.is_valid())` after the kind `if`/`elif` chain
(axis's live `on_axis_changed` keeps its own call, for the interactive
re-arm). `get_binding()` is now two lines: sync the live Trigger-mode
selection onto the draft, then `return draft.to_wire()`. 31 new pure tests in
`test_binding_draft.py`; the existing 120 `test_binding_editor.py` tests and
the rest of the GUI suite (535 total) pass unmodified — TDD'd the pure module
first, then refactored the GTK call sites against the already-green pure
suite, then confirmed the full suite was still green with no test changes
needed there.

On the "sheds widget-tree tests" bullet: read every test in the non-grid/
Chord range (roughly line 57–1350) and classified each one against
`test_binding_draft.py`'s new coverage. None qualified for removal — every
test in that range carries at least one assertion `test_binding_draft.py`
structurally cannot express (a dropdown's live model/selection, a picker's
own click-driven callback wiring, `+ New Macro`/`+ New Stepper` popover
flows, `on_axis_changed` re-arming Save on a live click, Chord dialog's
kind-exclusion list, `on_saved`/`on_clear` callback wiring, Daemon-rejection
error rendering). The file was already lean — one Save-through-the-real-
widget-tree test per kind plus a set of GTK-specific behavior tests, not a
pile of per-kind field-value permutations — so the bullet's intent (don't
keep duplicate coverage) was already satisfied without removing anything.

`/code-review` (Standards + Spec) returned four findings. Two actioned: (1)
`set_keypress_modifiers` was missing its parameter type annotation, unlike
every other setter — added `Iterable[str]`. (2) the "sheds tests" bullet
above, addressed by the audit rather than a code change. One dismissed:
`set_stepper(stepper_id, direction)` taking both fields together (forcing
call sites that change one to read back the other) is exactly the signature
this ticket's own Shape section specifies — not a defect to fix under this
ticket. One filed separately rather than fixed here, since it's a real bug
but predates this ticket and sits inside `build_dual_stage_panel`, which is
explicitly out of scope (ticket 27's job): an Axis-kind stage's captured
draft has no `"trigger"` key and crashes `BindingDraft.from_wire` (and,
identically, the old dict code before it) if it round-trips back in as
`starting` on the next panel rebuild — **ticket 28**.
