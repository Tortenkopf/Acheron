<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 13 — Collapse the parallel Macro and Stepper library views into one `LibraryKind`-parameterised module

**What to build:** The Macro half and the Stepper half of
`gui/acheron_gui/library_view.py` become one set of kind-agnostic builders
plus a per-kind adapter. A frozen `LibraryKind` dataclass — two module
constants, `MACRO` and `STEPPER` — carries everything the generic builders
would otherwise name `"macro"` or `"stepper"` themselves: the display noun,
the `GetConfig` sub-dict key, the `ui_state` selection key, the used-by scan
predicate, the four `client` calls, the item label, the "add item" controls,
and the editor's middle slot. Everything framing the divergence — the browse
list, the row (select / rename / delete), the selection default, the
reference-count guard, the column-2/3 skeleton, the ↑/↓/× item rows, and
`persist` — is written once.

**No behaviour change, no visual change.** Every widget the user sees, every
`client` call, every `ui_state` key, and the ticket-91 cross-tab pixel
alignment are all preserved exactly. GUI-only — no daemon, wire, or
`config.toml` change; every `client` method already exists.

CONTEXT.md gains a **Library** entry (already added alongside this ticket).

## The friction

`library_view.py` is 1,197 lines and `test_library_view.py` is 934, and the
two halves are a parallel implementation of one concept — CONTEXT.md's new
**Library** entry — kept aligned by hand. The churn is the tell: of the 10
commits to touch the file, tickets 55, 61, 63, 70, 91 and 93 were each
"build/adjust the same thing on both sides." Ticket 91's own follow-through
notes read *"added to **both** editors to keep the lockstep"*, and a
`/code-review` pass on ticket 93 caught the two editors' switcher closures
already drifted apart (`5c42062`).

The near-verbatim pairs:

| Macro | Stepper | Differs only in |
|---|---|---|
| `_sorted_macro_ids` (`:176`) | `_sorted_stepper_ids` (`:555`) | the dict |
| `_selected_macro_id` (`:238`) | `_selected_stepper_id` (`:809`) | `ui_state` key, dict, sort fn |
| `macro_used_by_count` (`:162`) | `stepper_used_by_count` (`:647`) | `binding["type"]` tag, id field |
| `build_macro_row` (`:180`) | `build_stepper_row` (`:661`) | `client.rename_*` / `delete_*`, `ui_state` key, sub-dict |
| `build_macros_browse_list` (`:247`) | `build_steppers_browse_list` (`:818`) | row builder, `client.create_*`, popover text, `ui_state` key |
| `build_macro_editor_columns` (`:351`) | `build_stepper_editor_columns` (`:861`) | see below |
| `describe_step` (`binding_editor.py:105`) | `describe_stepper_item` (`:559`) | genuinely different item shapes |

`describe_step` is already orphaned: nothing in `binding_editor.py` calls it
any more (its `Action::Step` / `Action::Macro` branches only assign a
reference), only `library_view.py` and its test import it.

The two `build_*_editor_columns` bodies (`:351`, `:861`) are ~100 lines each
and identical apart from:

- **the item list rows** — `describe_step(step)` vs `describe_stepper_item(item)`
  in the label; otherwise the same ↑/↓/× reorder-and-persist block, twice.
- **the middle slot** — Stepper renders `build_stepper_assignment_row`
  (`:719`); Macro renders `_header_middle_reserve()` (`:338`), a blank box
  whose *only* job is to occupy the identical height so nothing shifts on a
  tab flip.
- **the "add item" panel** — Macro has a KeyDown/KeyUp/Delay step-kind
  dropdown and greys the picker switcher on Delay; Stepper has a
  Ctrl/Shift/Alt/Super modifiers row and hides it in controller mode. Each
  owns a `new_*_value` draft dict and an `on_add` that shapes the wire item.
- **the toast** — Stepper pops `ui_state["stepper_toast"]`; Macro has none.

The kind-agnostic scaffolding already exists — `_build_editor_col2`,
`_vexpanding_list_scroller`, `_dropdown_row_height`, `_header_middle_reserve`,
`_mount_picker_mode_switch`, `build_library_picker_switch`, `_vscrollable`,
`_mount_editor_columns`, `_empty_library_label`, `build_library_tabs` — and
`build_library_sidebar` (`:1101`) / `build_library_content` (`:1143`)
*already* branch on `ui_state["library_tab"]`. The dispatch point is there;
everything below it is duplicated.

The daemon test stub carries the same used-by scan a third time:
`daemon_stub.py`'s `_macro_referenced` (`:452`) / `_stepper_referenced`
(`:445`) / `_all_bindings` (`:437`) are `any(...)` over the exact predicate
`macro_used_by_count` / `stepper_used_by_count` / `_profile_all_bindings`
(`:152`) `sum`. ADR 0005's closing paragraph names this pair specifically and
says to fold it *"opportunistically if you are already editing those call
sites"* — this ticket is.

## The module shape

```python
# gui/acheron_gui/library_view.py

@dataclass(frozen=True)
class LibraryKind:
    """One library flavour. Everything the kind-agnostic builders would
    otherwise have to name 'macro' or 'stepper' for themselves."""
    noun: str                    # "Macro" / "Stepper" — popover titles, tooltips
    config_key: str              # "macros" / "steppers" — the GetConfig sub-dict
    selection_key: str           # "library_selected_macro" / "...stepper" — ui_state
    binding_type: str            # "macro" / "step" — the used-by scan predicate
    id_field: str                # "macro_id" / "stepper_id" — the used-by scan field

    # client calls — client passed explicitly so `grep client.create_macro`
    # still finds the call site
    create:    Callable[[object, str], str]           # (client, name) -> new id
    rename:    Callable[[object, str, str], None]      # (client, entry_id, new_name)
    delete:    Callable[[object, str], None]           # (client, entry_id)
    set_items: Callable[[object, str, list], None]     # (client, entry_id, items)

    describe_item:      Callable[[dict], str]
    build_add_controls: Callable[..., Gtk.Widget]      # the value slot + kind-specific
                                                       #   row + switcher wiring + on_add
    build_middle_slot:  Callable[..., Gtk.Widget]      # Stepper: assignment row;
                                                       #   Macro: the blank reserve


MACRO = LibraryKind(
    noun="Macro", config_key="macros",
    selection_key="library_selected_macro",
    binding_type="macro", id_field="macro_id",
    create=lambda client, name: client.create_macro(name, []),
    rename=lambda client, mid, new: client.rename_macro(mid, new),
    delete=lambda client, mid: client.delete_macro(mid),
    set_items=lambda client, mid, steps: client.set_macro_steps(mid, steps),
    describe_item=describe_macro_step,
    build_add_controls=_build_macro_add_controls,
    build_middle_slot=_build_macro_middle_reserve,
)
STEPPER = LibraryKind(
    noun="Stepper", config_key="steppers",
    selection_key="library_selected_stepper",
    binding_type="step", id_field="stepper_id",
    create=lambda client, name: client.create_stepper(name, []),
    rename=lambda client, sid, new: client.rename_stepper(sid, new),
    delete=lambda client, sid: client.delete_stepper(sid),
    set_items=lambda client, sid, items: client.set_stepper_items(sid, items),
    describe_item=describe_stepper_item,
    build_add_controls=_build_stepper_add_controls,
    build_middle_slot=_build_stepper_assignment_row,  # today's build_stepper_assignment_row
)

_KINDS = {"steppers": STEPPER, "macros": MACRO}   # insertion order = tab order
```

### The seam

| Generic — takes a `LibraryKind` | Per-kind — a `LibraryKind` callable |
|---|---|
| `build_library_tabs` (iterates `_KINDS`) | `describe_item` — `describe_macro_step` / `describe_stepper_item`, two bodies, both in `library_view.py` |
| `_sorted_ids`, `_selected_id` | `build_add_controls` — value slot, step-kind dropdown **or** modifiers row, switcher `sensitive` / `on_mode_changed`, the `new_*_value` draft, `on_add` → wire shape |
| `used_by_count(config, kind, entry_id)` (folds both counts + `_profile_all_bindings`) | `build_middle_slot` — Stepper: the Forward/Backward assignment row; Macro: the blank `_header_middle_reserve` |
| `build_row`, `build_browse_list` | |
| one `build_editor_columns` — `_build_editor_col2`, the col-3 skeleton (error/toast label, "+ Add", `kind.build_middle_slot(...)`, separator, `_vscrollable` body: hint + shared switcher + `kind.build_add_controls(...)`), the ↑/↓/× item rows, `persist` via `kind.set_items` | |
| `build_library_sidebar` / `build_library_content` — `kind = _KINDS[selected_tab]`, no `if/else` | |

## What moves, what stays

- **Deleted** — `build_macro_row`, `build_stepper_row`, `build_macros_browse_list`,
  `build_steppers_browse_list`, `_sorted_macro_ids`, `_sorted_stepper_ids`,
  `_selected_macro_id`, `_selected_stepper_id`, `macro_used_by_count`,
  `stepper_used_by_count`, `build_macro_editor_columns`,
  `build_stepper_editor_columns`. Replaced by the generic builders above.
- **Relocated** — `describe_step` (`binding_editor.py:105`) →
  `library_view.py` as `describe_macro_step`; its `from .binding_editor
  import describe_step` line goes away, `labeled_row` stays imported from
  `binding_editor`.
- **Renamed but kept public** — `describe_stepper_item` unchanged;
  `describe_macro_step` is the new name for `describe_step`.
- **Unchanged names / signatures** — `build_library_sidebar`,
  `build_library_content` (the only symbols `device_overview.py` imports —
  `device_overview.py:82`, `:799`, `:817`), `build_library_tabs`,
  `build_stepper_assignment_row` (still exists; now reached as
  `STEPPER.build_middle_slot`).
- **`ui_state`** — flat keys unchanged: `library_tab`, `library_picker_mode`,
  `library_selected_macro`, `library_selected_stepper`. The two selection
  keys are now read through `kind.selection_key`.
- **Ticket-91 machinery stays** — `_dropdown_row_height`,
  `_header_middle_reserve`, `_build_editor_col2`, `_vexpanding_list_scroller`,
  `_mount_editor_columns`: flipping tabs still swaps the whole editor, so the
  "reserve the same height" work is still needed. `build_middle_slot` is
  where the Macro side returns `_header_middle_reserve()`.
- **`_mount_picker_mode_switch`** stays; invoked from each
  `build_add_controls` (the `sensitive` / `on_mode_changed` args are
  kind-specific), same as today.

## The used-by fold

`used_by_count(config, kind, entry_id)` scans `config["profiles"].values()`
→ `_profile_all_bindings(profile)` → `binding.get("type") ==
kind.binding_type and binding.get(kind.id_field) == entry_id`, `sum`-counted.

Then a shared `reference_count(profiles, *, binding_type, id_field,
id_value) -> int` — the same scan without the `config` wrapper — is imported
by `daemon_stub.py`, whose `_macro_referenced` / `_stepper_referenced`
become one-liners returning `reference_count(...) > 0` and whose
`_all_bindings` is deleted. `_profile_switch_referenced` (`daemon_stub.py:459`)
is a *different* scan (Base/Held only) and stays as-is. Where
`reference_count` lives: a small `read_model.py`, or `inputs.py` (already the
home of `is_grid_input` and the grid enumeration, imported by both sides) —
decide in the build; do not grow it into a general read-model module (ADR
0005: not a dedicated effort).

## Landing in one pass

Rewrite `library_view.py`, relocate `describe_step`, add `reference_count`
and rewire the stub, parameterise the tests — one PR, ticket 03–12
precedent. A half-migrated file with `build_macro_editor_columns` gone but
`build_stepper_editor_columns` still hand-rolled is harder to read than
either end state.

## Behaviour-preservation protocol

A pure GUI refactor with no input-path risk, but the ticket-91 alignment is
fragile and easy to regress:

- **Diff each generic builder against the two bodies it replaces.**
  Load-bearing: the late-binding-closure `on_add` reading `new_*_value` at
  click time (construction order ≠ visual order — the existing comments say
  so); the `_header_middle_reserve` height matching
  `build_stepper_assignment_row`'s real rendered height; the switcher row
  landing at the same y on both tabs; column 3 holding its natural width on
  both tabs (`_mount_editor_columns`); `ui_state.pop("stepper_toast")`
  firing once and only on the Stepper tab.
- **The existing `test_library_view.py` suite passes** — parameterised
  where called for (below) but never with a case removed — **before** any
  hand-rolled builder is deleted.
- **`gui/tools/shot_library.py` before/after**, both tabs, keyboard and
  controller picker modes — the same self-screenshot check tickets 91 and 92
  used to prove nothing shifts.
- **`/code-review` on both the Standards and Spec axes**, as tickets 05–12
  did.

## Tests: parameterise, don't duplicate

- **Parameterise** the ~13 near-identical macro/stepper pairs in
  `test_library_view.py` with `@pytest.mark.parametrize("kind", [MACRO,
  STEPPER])` (or an id-keyed fixture): empty→create-prompt, sorted list,
  default selection, row-click select, create-via-New, rename-via-popover,
  delete-disabled+tooltip, delete-once-unreferenced, used-by scan,
  add-item-appends, reorder/remove, first-up/last-down disabled,
  set-items-failure-shows-error. Each pair collapses to one test that now
  *asserts both kinds run one code path*.
- **Keep verbatim** — every kind-specific test: `build_stepper_assignment_row`
  ×5 (`:732`–`:871`), the Macro step-kind / Delay switcher
  (`:578`, `:599`), the Stepper modifiers row (`:438`, `:546`, `:648`,
  `:902`), the controller-draft round-trips (`:496`–`:563`, `:612`–`:630`),
  the col-3 natural-width check (`:921`), and the `describe_*` unit tests
  (`:460`–`:485`).
- **Update imports** — `from acheron_gui.binding_editor import describe_step`
  → `from acheron_gui.library_view import describe_macro_step`.
- **`test_daemon_stub.py`** — if it asserts on `_macro_referenced` /
  `_stepper_referenced` / `_all_bindings` by name, point those at the new
  behaviour; the observable stub behaviour (delete refusal, tooltips) is
  unchanged.
- Net: ~55 tests → ~40, coverage identical.

## Decisions from the grilling

- **Scope: the Library screen only.** `binding_editor.py`'s own smaller
  parallel pair (the `Action::Step` / `Action::Macro` "pick an existing
  entry + '+ New X'" branches) is a *different* concern — assigning a
  reference to a Binding, not authoring an entry — and is left alone. Its
  duplication is a separate, unfiled candidate.
- **Rewrite `library_view.py` in place**, no new `library.py`. The file
  already *is* the Library screen; a rename is diff for no navigability gain,
  and there is no second module worth separating here.
- **An honest shallow-pair collapse**, not a new model layer. The win is
  locality (lockstep-drift becomes structurally impossible) and leverage
  (one editor, N kinds — a third kind is a `_KINDS` entry). No `Library`
  object owning behaviour — the domain does not ask for one.
- **Adapter is a frozen `@dataclass`, two constants**, not a `Protocol`/ABC
  (implies behaviour this doesn't have) and not bare closures (lose the
  greppable `MACRO` / `STEPPER` handle). Callables take `client` explicitly.
- **The seam falls at "add a new item".** The generic half owns the frame
  and the list; the adapter owns the add-controls panel, the item label, and
  the middle slot. ~5 data fields + 3 callables.
- **`describe_step` + `describe_stepper_item` → `kind.describe_item`**,
  backed by two module-level bodies (the item shapes really differ). Both
  re-homed in `library_view.py`.
- **Flat `ui_state` keys kept** — `kind.selection_key` carries the name;
  zero change to any test's `ui_state` seeding.
- **`_KINDS` registry drives tabs and dispatch** — removes the last two
  `if selected_tab == "steppers"` branches; default tab stays `"macros"`,
  order stays Steppers-then-Macros.
- **The used-by fold reaches the stub** — one `reference_count` helper,
  `daemon_stub`'s two `_*_referenced` + `_all_bindings` collapse to it. ADR
  0005's blessed opportunistic case; `_profile_switch_referenced` untouched.
- **CONTEXT.md gains a "Library" entry** after "Stepper cursor" in the
  Configuration section (`domain-modeling` run — added with this ticket). No
  ADR: nothing was rejected.

## Facts dug from the code during the grilling (not asked of the user)

- `device_overview.py` imports only `build_library_content` /
  `build_library_sidebar` from `library_view` (`:82`); no other module
  imports it. `binding_editor.py` is imported *by* `library_view`
  (`describe_step`, `labeled_row`), not the reverse.
- `binding_editor.py` no longer calls `describe_step` anywhere — its
  `Action::Step` / `Action::Macro` branches only assign `stepper_id` /
  `macro_id` (`binding_editor.py:706`, `:770`).
- `build_library_sidebar` and `build_library_content` already dispatch on
  `ui_state["library_tab"]` (`:1118`, `:1156`); `_mount_editor_columns` and
  `_empty_library_label` are already kind-agnostic.
- The Daemon refuses `DeleteMacro` and `DeleteStepper` identically
  (`dispatch`/`edit` reference-count guard, tickets 03/54); the GUI's
  used-by gate on both is a UX mirror, not a kind difference.
- `daemon_stub._all_bindings` / `_macro_referenced` / `_stepper_referenced`
  are the exact `any(...)` form of `library_view`'s `sum` scan; ADR 0005
  names this pair.
- Tests run `gui/.venv/bin/pytest gui/tests` (real GTK widgets, no main
  loop); no ruff/black step. `shot_library.py` is the visual harness
  (ticket 91).
- No `config.toml` / wire / `daemon/` change: `create_macro`,
  `set_macro_steps`, `rename_macro`, `delete_macro` and the four Stepper
  equivalents all already exist on `DaemonClient`.

**Blocked by:** None — GUI-only, and the shared scaffolding
(`_build_editor_col2`, `_mount_picker_mode_switch`, `_KINDS`-able dispatch)
is already in place from tickets 70 / 91 / 92.

**Status:** resolved

- [x] `LibraryKind` frozen dataclass + `MACRO` / `STEPPER` constants +
      `_KINDS` registry in `library_view.py`; the 12 `build_macro_*` /
      `build_stepper_*` / `_*_macro_id` / `_*_stepper_id` /
      `*_used_by_count` symbols are gone, replaced by kind-agnostic
      `build_row` / `build_browse_list` / `build_editor_columns` /
      `_sorted_ids` / `_selected_id` / `used_by_count`.
- [x] `build_library_sidebar` / `build_library_content` dispatch via
      `_KINDS[selected_tab]` with no `if/else`; their signatures and the
      names `device_overview.py` imports are unchanged.
- [x] `describe_step` relocated to `library_view.py` as `describe_macro_step`;
      removed from `binding_editor.py`; `describe_stepper_item` unchanged.
      Both reachable as `kind.describe_item`.
- [x] `reference_count(profiles, *, binding_type, id_field, id_value)`
      helper (in a small `gui/acheron_gui/read_model.py`);
      `library_view.used_by_count` and `daemon_stub`'s `_macro_referenced`
      / `_stepper_referenced` both call it; `daemon_stub._all_bindings`
      deleted; `_profile_switch_referenced` untouched.
- [x] Flat `ui_state` keys (`library_tab`, `library_picker_mode`,
      `library_selected_macro`, `library_selected_stepper`) unchanged;
      selection read through `kind.selection_key`.
- [x] `test_library_view.py`: the 13 macro/stepper pairs parameterised over
      `case` (MACRO / STEPPER); every kind-specific test kept verbatim;
      `describe_*` imports updated. `test_daemon_stub.py` needed no change
      (it never named the removed private methods).
- [x] `gui/.venv/bin/pytest gui/tests` green (400 passed); `shot_library.py`
      before/after byte-identical on grid, macros, macros_delay, steppers,
      and both picker modes. `/code-review` (Standards + Spec) clean.
- [x] CONTEXT.md "Library" entry present (added with this ticket).

## Comments

**2026-09-02** — Filed from the `/improve-codebase-architecture` grilling
(candidate 1 of the second review,
`/tmp/architecture-review-20260902-122831.html`; candidates 2–4 — the
contract-tested D-Bus wire surface, migrating the dispatch/D-Bus behaviour
tests onto the ticket 05–12 seams, and the `config.rs` vocabulary/transaction
split — are unfiled). Design tree settled over three rounds; see "Decisions
from the grilling". CONTEXT.md "Library" entry added at filing time; ticket
not yet implemented.

**2026-09-02** — Resolved in one pass. `library_view.py` 1197 → ~1090 lines
(one `build_editor_columns` / `build_row` / `build_browse_list` /
`_sorted_ids` / `_selected_id` / `used_by_count`, plus `MACRO` / `STEPPER` /
`_KINDS` and two `_build_*_add_controls` / one `_build_macro_middle_reserve`);
`test_library_view.py` 934 → ~740 (the 13 pairs → one parametrised set over
`case`, kind-specific tests verbatim). `reference_count` landed in a new
`gui/acheron_gui/read_model.py` (docstring disclaims a general read-model,
ADR 0005); `daemon_stub`'s `_macro_referenced` / `_stepper_referenced` call
it, `_all_bindings` deleted, `_profile_switch_referenced` untouched.
`describe_step` → `library_view.describe_macro_step`, gone from
`binding_editor.py`.

Two `LibraryKind` fields beyond the sketch, both inside the `MACRO` /
`STEPPER` constants: `items_key` (`"steps"` / `"items"` — the generic
builder must *read* the list, not just write it via `set_items`) and
`toast_key` (`"stepper_toast"` / `None` — encodes the protocol's "pop once,
Stepper tab only" as data, not a `kind is STEPPER` test). `build_add_controls`
returns `(add_box, add_btn)` so the generic half can keep "+ Add" pinned
above the scroller.

Gate: `gui/.venv/bin/pytest gui/tests` 400 passed (was 397 — +3 for the new
`test_read_model.py`, net-neutral on `test_library_view.py`); `shot_library.py`
before/after byte-identical on all six shots (grid, macros, macros_delay,
steppers, steppers_controller, macros_controller). `/code-review` Standards
+ Spec both clean — no hard violations, no missing/wrong requirements, the
two extra fields judged faithful not creep. Standards nits fixed before
commit: `_build_macro_middle_reserve` param annotations, an internal
"registry" wording (CONTEXT.md's new entry rules the word out), and the
test's `_KindCase` slimmed to derive stub-method names from the adapter.
No hardware check — GUI-only, no input path touched.
