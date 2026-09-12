<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 31 — Carve a per-kind widget adapter out of `render_action_editor`

**What to build:** `render_action_editor` (`gui/acheron_gui/binding_editor.py`,
inside `build_action_and_trigger_fields`) currently holds a 240-line
`if kind == "keypress": … elif …` chain that builds the GTK widgets for
whichever of the six Action kinds (Keypress, Macro, Stepper step, Switch
Profile, Controller button, Axis) is selected. Replace it with a lookup into
a new `_ACTION_EDITORS: dict[str, ActionEditorFn]` table, one small builder
function per kind, co-located in `binding_editor.py` (this table has exactly
one consumer — a new sibling module would be a seam with no second caller).
`BindingDraft` (`binding_draft.py`) is untouched in shape — it stays the
GTK-free home for `from_wire`/`to_wire`/`is_valid`/the named setters — except
for one small addition: an optional `on_change` hook, defaulting to a no-op,
fired at the end of every setter, so `save_btn.set_sensitive(draft.is_valid())`
resyncs generically instead of once after the kind branch (as today) plus a
special-cased inline call from the Axis branch alone. Fold in the Macro and
Stepper "+ New" inline-creation blocks (already near-identical duplicates of
each other) into one shared helper while this seam is open.

**Blocked by:** None — can start immediately.

## Why

Filed from the 2026-09-12 `/improve-codebase-architecture` review (scoped to
the dual-stage/Binding-editor hot spot — tickets 26/27/29/30 already carved
`BindingDraft`, folded it into the dual-stage panel, carved `DualStagePlan`,
and shared the axis-vs-Binding push fork, but `binding_editor.py` is still
the largest and most-touched file in the GUI) and the two-round grilling that
followed it. Top-ranked, "Strong" candidate.

Ticket 26 centralized the *data* shape of an Action kind (seed, wire shape,
completeness) into `BindingDraft`, but the *widget-construction* ladder never
moved — it's the fourth ladder over the same six kinds, still living 400
lines into `build_action_and_trigger_fields`. `library_view.py` already
proves the fix for exactly this shape: `LibraryKind`, a frozen per-kind
adapter used for the Macro/Stepper Library screen. The grilling settled the
load-bearing decisions for applying it here:

- **Two adapters, not one.** Unlike `LibraryKind` (which bundles metadata
  *and* widget-builder callables in one table), `BindingDraft` already exists
  as a pure, headless-tested module (`test_binding_draft.py`, no `Gtk`
  import). Merging the widget-builders into it would force a `Gtk` import
  into `binding_draft.py` for a marginal consistency gain — not worth trading
  away its current testability. The new table is separate and GTK-only.
- **Builder returns one composed `Gtk.Widget`.** Matches
  `LibraryKind.build_middle_slot`'s own `-> Gtk.Widget` convention; the
  caller appends it once to `editor_slot` instead of the current
  multi-append-per-branch shape.
- **Explicit params, not a bundled context object.** `LibraryKind`'s own
  callables already take several explicit positional params (`client,
  config, profile, layer, entry_id, ui_state, on_change, show_error`) rather
  than one context bundle — the new builder signature follows that
  precedent for consistency, even though it runs a little long.
- **Sensitivity resync becomes generic**, via `BindingDraft.on_change`
  (a plain `Callable[[], None] | None`, no `Gtk` dependency). Today, Macro
  and Step happen to never leave `is_valid()` false once their dropdown is
  showing (so they never re-call `save_btn.set_sensitive`), while Axis can
  (no target picked yet) and does call it inline from its own `on_change`
  handler — an implicit, easy-to-violate invariant for any future Action
  kind. The hook is set as a plain attribute after construction (not a
  `from_wire`/constructor parameter), so `BindingDraft`'s other callers — the
  dual-stage panel (ticket 27) and `test_binding_draft.py` — are unaffected;
  they simply never set it.
- **Trigger-mode locking/narrowing stays in the shared harness.** The two
  hardcoded `kind in (...)` conditionals (Profile Switch/Axis lock the
  Trigger dropdown; Controller button/Macro trigger a live model rebuild)
  sit on top of the already-centralized `rules.valid_triggers` table as UI
  affordance (disable vs. show-one-option, avoid an empty dropdown) — a
  different, smaller concern than the seed/to_wire/is_valid/widget
  four-way duplication this ticket targets. Folding it into the new adapter
  would bloat every kind's entry for the two kinds that need it.
- **Fold in the Macro/Stepper "+ New" dedup now.** Independent of the
  six-kind ladder, these two blocks are already near-identical (same seam,
  touched by the same split anyway — a follow-up ticket would just reopen
  this code).
- **New `test_action_editor.py`, not folded into `test_binding_editor.py`**,
  mirroring the `binding_draft.py`/`test_binding_draft.py` naming
  convention. Recalibration from the review, worth recording: this doesn't
  make Action-kind testing GTK-free — `library_view.py`'s own test file
  (`test_library_view.py`) still has 49 GTK-widget-tree-navigation calls
  across 53 tests even with `LibraryKind` in place. The win is a smaller,
  independently named unit under test per kind (call one builder function,
  inspect its returned widget), not the elimination of GTK from testing.
- **No CONTEXT.md entry for the adapter type itself** — matching the
  ADR-0010-lineage precedent (tickets 10/14/15/17/29) of declining a
  glossary entry for internal-plumbing locus refactors. The Action entry
  *does* get a one-line pointer to it, the way the Library entry already
  names `LibraryKind` — so CONTEXT.md stays a consistent map of where a
  kind's per-variant behavior lives, without inventing a new domain term.

## Shape

```python
# binding_draft.py — still no GTK import.
class BindingDraft:
    def __init__(self, *, kind, trigger, keypress, macro, step,
                 profile_switch, controller_button, axis) -> None:
        ...
        self.on_change: Callable[[], None] = lambda: None  # set by a caller that wants it

    def set_axis_target(self, target: str | None) -> None:
        self.axis["target"] = target
        self.on_change()
    # ...same trailing self.on_change() added to every other setter
```

```python
# binding_editor.py
ActionEditorFn = Callable[
    [object, dict, str, str | None, str | None, "BindingDraft", str | None, Callable[[], None]],
    Gtk.Widget,
]
# (client, config, profile, layer, inp, draft, picker_css_class, rerender) -> Widget

_ACTION_EDITORS: dict[str, ActionEditorFn] = {
    "keypress": _build_keypress_editor,
    "profile_switch": _build_profile_switch_editor,
    "controller_button": _build_controller_button_editor,
    "axis": _build_axis_editor,
    "step": _build_step_editor,
    "macro": _build_macro_editor,
}

def render_action_editor():
    ...
    kind = available_action_types[action_dd.get_selected()][0]
    draft.set_kind(kind)
    clear_children(editor_slot)
    editor_slot.append(
        _ACTION_EDITORS[kind](client, config, profile, layer, inp, draft, picker_css_class, render_action_editor)
    )
    save_btn.set_sensitive(draft.is_valid())
    sync_analog_hint()
```

Shared "+ New library entry" helper, used by both `_build_macro_editor` and
`_build_step_editor`:

```python
def _build_new_library_entry_button(
    label: str, prompt_title: str, create: Callable[[object, str], str], on_created: Callable[[str], None],
) -> Gtk.MenuButton:
    btn = Gtk.MenuButton(label=label)
    def on_submitted(name: str) -> None:
        on_created(create(client, name))
    btn.set_popover(build_name_prompt_popover(prompt_title, "", "Create", on_submitted))
    return btn
```

## Acceptance criteria

- [x] `render_action_editor` is a lookup into a new `_ACTION_EDITORS` table (one builder function per Action kind); the 240-line `if/elif` chain is gone.
- [x] `BindingDraft` gains an `on_change` hook (default no-op), invoked at the end of every setter; `build_action_and_trigger_fields` sets it to resync `save_btn.set_sensitive(draft.is_valid())`, and the Axis branch's inline resync call is removed as now-redundant.
- [x] Other `BindingDraft` callers (`build_dual_stage_panel`, `test_binding_draft.py`) are unaffected — verified they never set `on_change` and behave identically.
- [x] The Macro and Stepper "+ New" inline-creation blocks share one helper instead of two near-identical copies.
- [x] The trigger-mode locking/narrowing conditionals are unchanged, still in the shared harness.
- [x] `gui/tests/test_action_editor.py` exists: a pure-construction unit test per Action kind, asserting the returned widget's structure directly (no full popover build).
- [x] `gui/tests/test_binding_editor.py` is audited (per-test, not blindly trimmed — as ticket 26 did) and reduced to genuinely cross-kind behavior: dropdown switching on an Action-kind change, the Analog-repeat hint, trigger-option narrowing, Save-button sensitivity through the real widget tree.
- [x] `CONTEXT.md`'s Action entry gets a one-line mention of the new adapter, mirroring the Library entry's existing `LibraryKind` mention.
- [x] Behavior is unchanged for every existing caller of the Trigger/Action editor: the non-grid Binding editor, the Chord binding dialog, and the dual-stage panel.
- [x] Full GUI test suite green.

## Comments

**2026-09-12** — Filed. Top-ranked ("Strong") candidate of the 2026-09-12
`/improve-codebase-architecture` review, scoped to the dual-stage/Binding-editor
hot spot. Two rounds of grilling settled the shape; see "Why" above for the
load-bearing decisions.

**2026-09-12** — Done. Implemented per the Shape section, with one signature
deviation the ticket's illustrative pseudocode didn't quite account for:
`ActionEditorFn` also takes `trigger_dd`/`trigger_options`/
`set_trigger_listener` (11 params total, not 8) — Keypress's modifier
warning depends on the live Trigger-mode selection and needs to register a
disconnect-tracked listener on `trigger_dd` (ticket 42's no-stale-handlers
invariant), which a `Gtk.Widget`-only return can't carry back out; every
other kind ignores these three, same as `LibraryKind.build_middle_slot`'s
own shared-but-partly-ignored signature. `_build_new_library_entry_button`
also takes `client` explicitly (the Shape's sketch left it as a free
variable, which only works for a nested closure — these are top-level
module functions, one per `_ACTION_EDITORS` entry).

`BindingDraft.on_change` fires at the end of all nine setters;
`build_action_and_trigger_fields` sets `draft.on_change = lambda:
save_btn.set_sensitive(draft.is_valid())` right after construction, and the
Axis builder's `on_axis_changed` no longer touches `save_btn` at all — it
just calls `draft.set_axis_target(target)`. `build_dual_stage_panel` and
`test_binding_draft.py` construct `BindingDraft` directly and never set
`on_change`, confirmed unaffected (full suite green). Added three new
`test_binding_draft.py` tests for the hook itself (default no-op, fires for
every setter, sees the field already updated).

`test_action_editor.py`: 21 tests, one file, calling each `_build_*_editor`
directly with a hand-built `BindingDraft`/`trigger_dd` — no popover, per the
acceptance criterion.

`test_binding_editor.py` audit: removed 11 tests whose only remaining value
was re-proving per-kind widget-construction/field-mapping already covered by
the new `test_action_editor.py` plus `test_binding_draft.py`'s existing
`to_wire()`/`is_valid()` coverage (dropdown-preselect and direction-change
tests for Macro/Stepper, the BTN_LEFT round-trip). `/code-review` (run
after the first trim pass) flagged that this left Profile Switch,
Controller Button, Macro, and Stepper with no *end-to-end* Save round trip
through the real popover — Keypress and Axis were the only kinds still
exercised from `action_dd` selection through to the actual
`client.set_binding` call — a real regression risk for
`render_action_editor`'s kind dispatch / `get_draft().to_wire()` / the
"+ New" `rerender()` ordering. Restored four tests to close that gap: one
slim Save-round-trip test each for Profile Switch and Controller Button, and
the original "+ New Macro"/"+ New Stepper" inline-creation-then-Save tests
(the ordering-sensitive path the reviewer specifically flagged) — net trim
is 7 tests, not 11. Full GUI suite green (569 passed, up from 552 before
this ticket); `/code-review` clean on the second pass.
