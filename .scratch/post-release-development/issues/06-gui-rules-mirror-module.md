<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 06 — One contract-tested `rules` mirror module in the GUI

**What to build:** The daemon's device catalogs and Binding-legality matrix are
copied into the GUI by hand today, across five modules, with no test in either
suite spanning the process seam. Collect the *pure* half of that mirror — the
parts that are a function of `(vocabulary + one Binding/Chord + its Input)` and
nothing else — behind one small interface in a new `gui/acheron_gui/rules.py`,
and make its faithfulness a **test** rather than a comment: the daemon emits its
catalogs and `config::validate` verdicts as a checked-in JSON fixture, and a
GUI-side contract test asserts `rules` agrees with it exactly.

`daemon_stub.py` stops re-implementing `dispatch`/`config` functions and calls
`rules`. `binding_editor.py` keeps its `Gtk.DropDown` mechanics but filters
through `rules` instead of hardcoded literals. The pickers assert their catalogs'
*contents* against `rules`, not just their length.

## The friction

The domain model was **copied** across the daemon↔GUI seam (ADR 0003's
split-language stack) rather than shared, and nothing tests that the copies
still agree. Churn shows the cost: of the last 52 commits touching `gui/`, 22
touch `binding_editor.py` and 22 touch `daemon_stub.py`, and 20 of those
`daemon_stub.py` commits also touch `daemon/src/`. Concretely:

- **`daemon_stub.py` re-implements the daemon's rule core.**
  `_validate_binding_action` mirrors the `Action`/`TriggerMode` checks now in
  `config::validate`; `_input_sort_key` hand-reproduces `Input`'s derived `Ord`
  so `_chord_key` can reproduce `ChordKey`'s `Display` byte-for-byte;
  `_slug_base` mirrors `config::slug_base`; `_chord_conflict` mirrors the
  subset/superset rule. `test_daemon_stub.py` (620 lines) then tests the fake
  against itself — nothing launches the real daemon and checks the fake agrees.
- **The Trigger-mode matrix is encoded in three places** — `config::validate`
  (SSOT), `binding_editor.py`'s `Gtk.DropDown` model surgery (~lines 506–635,
  the "no per-item sensitivity" workaround cited three times), and
  `daemon_stub._validate_binding_action`.
- **Device vocabularies are scattered across the pickers** — `controller_picker`
  and `axis_picker` each carry a hand-maintained catalog guarded only by
  `assert len(LABEL_BY_CODE) == 57` / `== 17`. The **count** is asserted; the
  **contents** never are, so a renamed or swapped entry passes.

## The module: `gui/acheron_gui/rules.py`

One flat module. Every symbol is pure — returns data, never raises, never
touches a `Config` or any stub state. It may import from `inputs.py`
(`is_grid_input`, the grid enumeration); the dependency is one-way,
`rules` → `inputs`.

| Symbol | Signature | Mirrors (daemon) |
|---|---|---|
| `GAMEPAD_BUTTONS` | `frozenset[str]` — 57 `BTN_*` names | `input::gamepad_button_codes()` |
| `AXIS_TARGETS` | `frozenset[str]` — 17 wire strings | `wire::axis_target_str` / `AxisTarget::ALL` |
| `ALL_TRIGGERS` | `frozenset[str]` — `fire_once`, `hold_to_repeat`, `toggle`, `analog_repeat` | `TriggerMode` |
| `ALL_ACTION_KINDS` | `frozenset[str]` — `keypress`, `controller_button`, `axis`, `macro`, `step`, `profile_switch` | `Action` + Axis assignment |
| `valid_action_kinds(input_str: str \| None)` | `-> frozenset[str]` | the Action/Axis-placement parts of `validate` (`axis` grid-only; `profile_switch` not on a Chord) |
| `valid_triggers(action_kind: str, input_str: str \| None)` | `-> frozenset[str]` | the `TriggerMode` matrix (see below) |
| `slug(name: str, fallback: str)` | `-> str` | `config::slug_base` |
| `input_sort_key(input_str: str)` | `-> tuple` | `Input`'s derived `Ord` |
| `chord_key(inputs: Iterable[str])` | `-> str` | `ChordKey`'s `Display` |
| `chord_members_conflict(a: set[str], b: set[str])` | `-> bool` — `True` iff one set contains the other | the subset/superset rule |

Conventions:

- **`input_str=None` means "a Chord's own Binding"**, matching
  `binding_editor.py`'s existing convention. A Chord has no single Input, so
  `valid_action_kinds(None)` excludes `axis` and `profile_switch`, and
  `valid_triggers(k, None)` excludes `analog_repeat`.
- **`valid_triggers("axis", _)` returns `frozenset()`** — an Axis assignment has
  no `TriggerMode` on the daemon at all. `binding_editor` reads the empty set as
  "disable the Trigger-mode dropdown" (what it already does for the `axis` kind).
- The `TriggerMode` matrix `valid_triggers` encodes: `profile_switch` →
  `{fire_once}`; `controller_button` excludes `fire_once`; `step` excludes
  `toggle`; `analog_repeat` only when `input_str` is a grid key.

### Scope: what is *not* in `rules`

The checks that walk a whole `Config` stay in `daemon_stub.py` as operation
logic over its own in-memory state (the stub *is* the thing that holds a whole
config): dangling `macro_id` / `stepper_id` / `profile_switch` target,
`_macro_referenced` / `_stepper_referenced` reference counts, the stepper-steal
loop, the `release < actuation` numeric check, and the axis↔binding↔chord
mutual-exclusion clear. `rules` answers "is this combination structurally
legal in isolation", not "re-run whole-config validation".

**Keycodes are out.** `key_picker.py`'s ~200-entry catalog has no daemon SSOT —
`Action::Keypress.key` accepts any `KeyCode` in `0..=KEY_MAX` by design, so
there is nothing to contract-test. `key_picker` keeps its hand-curated authoring
catalog unchanged. Do not "finish the job" by adding it.

## The contract fixture

A new `daemon/src/schema.rs` (a `pub(crate)` module, wired into `lib.rs`) with a
single `#[test]` that:

1. Builds `GAMEPAD_BUTTONS` from `input::gamepad_button_codes()` (each
   `KeyCode` as its `{:?}` name) and `AXIS_TARGETS` from `AxisTarget::ALL` via
   `wire::axis_target_str`.
2. Derives the verdict matrices by driving **`config::validate`** with synthetic
   minimal `Config`s — one profile, one Binding (or one Axis assignment, or one
   two-member Chord for the `"__chord__"` sentinel) of the combination under
   test — seeded so the out-of-scope checks always pass: a dummy macro `"m"`, a
   dummy stepper `"s"`, and a second profile `"P2"` as a `profile_switch`
   target. `Ok(())` → `allowed: true`; any `Err(_)` → `allowed: false`.
3. Emits `slug_examples` (calling `config::slug_base` — make it `pub(crate)`)
   and `chord_key_examples` (building a `ChordKey` and taking its `Display`)
   from hand-authored input lists covering the transformation edge cases.
4. Serialises the lot to `daemon/contract/daemon-schema.json` (pretty-printed,
   deterministic key order), compares against the on-disk file, and **fails on
   any diff**. `ACHERON_BLESS=1 cargo test` rewrites the file instead of
   asserting — the same golden-file idiom used nowhere else in this repo yet, so
   document it in the test's doc comment.

Fixture shape:

```json
{
  "gamepad_buttons": ["BTN_SOUTH", "..."],
  "axis_targets": ["left_trigger", "..."],
  "trigger_matrix": [
    {"action_kind": "keypress", "input": "grid_r1c1", "trigger": "analog_repeat", "allowed": true}
  ],
  "action_kind_matrix": [
    {"input": "mode_key", "action_kind": "axis", "allowed": false}
  ],
  "slug_examples": [{"name": "My Macro!!", "fallback": "macro", "slug": "my-macro"}],
  "chord_key_examples": [{"members": ["mode_key", "grid_r1c1"], "key": "mode_key+grid_r1c1"}]
}
```

- `input` is one of the 28 real Input strings or the literal `"__chord__"`.
- `trigger_matrix` covers the five real Action kinds (not `axis`) × 29 inputs ×
  4 triggers ≈ 580 rows; `action_kind_matrix` covers all six kinds × 29 inputs
  = 174 rows. Both fully enumerated — the space is small and cheap.
- `slug_examples`: ≈15 entries — Unicode, runs of punctuation/space collapsing
  to one hyphen, leading/trailing hyphen trim, empty-after-strip → `fallback`.
- `chord_key_examples`: ≈12 entries — each `Input` variant's internal order
  (`mode_key` first; `grid` by `(row, col)`; the four `thumbstick_*`; the three
  `wheel_*`) and cross-variant mixes that a plain alphabetical sort gets wrong
  (e.g. `["grid_r1c1", "mode_key"]` → `"mode_key+grid_r1c1"`).

## The GUI contract test

New `gui/tests/test_rules_contract.py`, synchronous, no GTK:

- Loads `../daemon/contract/daemon-schema.json` (relative to the test file).
- `assert rules.GAMEPAD_BUTTONS == set(schema["gamepad_buttons"])` and the same
  for `AXIS_TARGETS`.
- For every `trigger_matrix` row:
  `assert (row["trigger"] in rules.valid_triggers(row["action_kind"], _input(row["input"]))) == row["allowed"]`
  where `_input` maps `"__chord__"` → `None`.
- For every `action_kind_matrix` row: the same against `rules.valid_action_kinds`.
- For every `slug_examples` / `chord_key_examples` row: `rules.slug(...)` /
  `rules.chord_key(...)` equals the recorded output.

New `gui/tests/test_rules.py` holds the module's own unit tests (label-free,
behaviour-focused) — this is where the pure-rule assertions pulled out of
`test_daemon_stub.py` land.

## Caller migration (one pass)

- **`daemon_stub.py`.** Delete `_input_sort_key`, `_chord_key`, `_slug_base`,
  `_chord_conflict`. Rewrite `_validate_binding_action` / `_validate_stepper_items`
  to consult `rules.GAMEPAD_BUTTONS` and `rules.valid_triggers`; rewrite the
  `set_axis_assignment` target guard to consult `rules.AXIS_TARGETS`; rewrite
  `_chord_conflict` as a loop over `rules.chord_members_conflict`. Build
  `_unique_macro_id` / `_unique_stepper_id` on `rules.slug`. **Keep**
  `_macro_referenced` / `_stepper_referenced`, `_reject_if_axis_assigned` and
  the `set_axis_assignment` mutual-exclusion clear, the stepper-steal loop, the
  `release > actuation` guards, and every `raise …Error`. Drop the imports from
  `controller_picker` / `axis_picker` — import `rules` instead.
- **`binding_editor.py`.** Keep the `Gtk.DropDown` model-rebuild mechanics and
  every label/ordering (from `inputs.py`). Replace only the hardcoded rule
  literals: the `k != "analog_repeat"` filter on `base_trigger_options`, the
  `k != "fire_once"` filter in `render_action_editor`, and the `e[0] != "axis"`
  filter on `available_action_types` all become membership tests against
  `rules.valid_triggers(kind, inp)` / `rules.valid_action_kinds(inp)`. Keep the
  defensive `trigger: "fire_once"` lock for `profile_switch` in `on_save`.
- **`controller_picker.py` / `axis_picker.py`.** Replace
  `assert len(LABEL_BY_CODE) == 57` / `== 17` with
  `assert set(LABEL_BY_CODE) == rules.GAMEPAD_BUTTONS` /
  `set(AXIS_LABEL_BY_TARGET) == rules.AXIS_TARGETS`. Keep the label maps and
  geometry.
- **`inputs.py`.** No structural change. Its test asserts
  `{k for k, _ in TRIGGER_OPTIONS} == rules.ALL_TRIGGERS` and
  `{k for k, _ in ACTION_TYPES} == rules.ALL_ACTION_KINDS`. `TRIGGER_SHORT`,
  the label maps, the menu ordering, and `default_trigger_for` (a GUI-authoring
  heuristic the daemon explicitly does not share) stay put.

## Tests: replace, don't layer

Per the ticket 03 / 04 precedent. Move out of `test_daemon_stub.py` into
`test_rules.py` (+ covered by `test_rules_contract.py`):
`test_chord_key_orders_members_like_the_real_daemons_input_ord…`,
`test_set_chord_binding_rejects_a_subset_superset_conflict…` (the predicate
half), `test_set_binding_rejects_a_toggle_step_binding`,
`test_set_binding_rejects_analog_repeat_on_a_non_grid_input`,
`test_set_binding_accepts_analog_repeat_on_a_grid_input`,
`test_stepper_items_accept_a_valid_controller_button_and_reject_a_non_gamepad_code`,
and the slug derivation / collision-base assertions.

Keep in `test_daemon_stub.py` (genuinely about the stub's stateful behaviour):
persistence / reflection, subscriber notifications, the `delete_*`
reference-count guards, the active-Profile delete guard, `switch_profile`
clearing toggles, the stepper-steal, actuation, `get_state` shape, and Chord
*persistence* (keyed correctly) as a thin integration check.

## Recording the decision

- A module docstring on `rules.py` stating plainly that it is the acknowledged
  Python mirror of `config::validate` + the daemon catalogs, contract-tested by
  `test_rules_contract.py`, and why the model is mirrored rather than shared
  (ADR 0003's split-language stack across a D-Bus process seam).
- A `CONTRIBUTING.md` bullet in the house style of the existing "Adding a new
  mutating `Command`" one: **adding a device-catalog entry or a Binding-legality
  rule** — change the daemon, run `ACHERON_BLESS=1 cargo test` to regenerate
  `daemon/contract/daemon-schema.json`, update the `rules.py` mirror, and run
  both suites.
- **No ADR** — nothing is being rejected here, and mirroring across the
  two-language seam is a direct consequence of ADR 0003, not a fresh trade-off.
- **No CONTEXT.md term** — `rules` is an implementation artifact, not domain
  vocabulary.

## Out of scope

- Sharing the model for real (a common schema crate, code generation into
  Python) — ADR 0003 stands; this ticket makes the existing mirror *trusted*,
  not smaller.
- `wire.py` ↔ `dbus/wire.rs` (the pack/unpack codec) — architecture-review
  candidate 4. This ticket touches the rule/vocab mirror only.
- Breaking `build_action_and_trigger_fields` into per-Action-kind editor modules
  — candidate 5. It builds *on* `rules` (each per-kind module will call
  `rules.valid_triggers()`); it does not block this ticket and this ticket does
  not block it beyond landing first.
- The daemon `config::toml` submodule split — daemon-only, unrelated.

**Blocked by:** None. Tickets 04 and 05 (the `config::validate` surface this
mirrors) are merged.

**Status:** resolved

- [x] `gui/acheron_gui/rules.py` exists with the ten symbols in the table, all
      pure (no raises, no `Config`/stub state), depending only one-way on
      `inputs.py`. Its docstring states the mirror contract and the
      mirror-not-share rationale.
- [x] `valid_triggers` / `valid_action_kinds` take `input_str: str | None`
      (`None` = a Chord's own Binding); `valid_triggers("axis", _)` is the
      empty set.
- [x] `daemon/src/schema.rs` exists (`#[cfg(test)] pub(crate) mod`, wired into
      `lib.rs`) with a `#[test]` that regenerates
      `daemon/contract/daemon-schema.json` under `ACHERON_BLESS=1` and asserts
      equality otherwise; `config::slug_base` is `pub(crate)`.
- [x] `daemon/contract/daemon-schema.json` is checked in, holds the two fully
      enumerated verdict matrices (580 + 174 rows) plus the catalogs and the
      slug / chord-key example lists, and is derived by driving
      `config::validate` with synthetic single-item `Config`s (dummy
      macro/stepper/second-profile seeded).
- [x] `gui/tests/test_rules_contract.py` loads that file and asserts
      set-equality on both catalogs and per-row verdict-equality on both
      matrices and both example lists.
- [x] `daemon_stub.py` contains no hand-reimplementation of `input_sort_key`,
      `chord_key`, `slug_base`, or the subset/superset predicate, and no
      hardcoded gamepad / axis-target / trigger-legality literal — each is a
      call into `rules`. It no longer imports from `controller_picker` /
      `axis_picker`. Its Config-walking operation guards and every `raise`
      remain. (`device_overview.py`'s own copy of the subset/superset
      predicate was routed through `rules.chord_members_conflict` too.)
- [x] `binding_editor.py` keeps its `Gtk.DropDown` mechanics but derives every
      trigger / action-kind filter from `rules`; no `"analog_repeat"` /
      `"fire_once"` / `"axis"` rule literal remains in it.
- [x] `controller_picker.py` / `axis_picker.py` assert catalog *contents*
      against `rules.GAMEPAD_BUTTONS` / `rules.AXIS_TARGETS`, not length;
      `inputs.py`'s test (`gui/tests/test_inputs.py`) asserts its option-key
      sets equal `rules.ALL_TRIGGERS` / `rules.ALL_ACTION_KINDS`.
- [x] The pure-rule tests listed above have moved out of `test_daemon_stub.py`
      into `test_rules.py`; `test_daemon_stub.py` keeps only its stateful-stub
      coverage.
- [x] `key_picker.py` is unchanged; no keycode catalog is added to `rules` or
      the fixture.
- [x] `CONTRIBUTING.md` has the catalog/rule-change bullet naming
      `ACHERON_BLESS=1` and the regenerate-then-mirror flow.
- [x] Daemon suite green (incl. the new schema test); `cargo clippy` and
      `cargo fmt --check` clean; full GUI suite green; packaging suite
      unaffected.

## Comments

**2026-08-31** — Filed from an architecture-review grilling session (candidate 3
of the review that produced tickets 03–05; candidates 1/4 → ticket 05, candidate
2 → ticket 04). Design tree settled over three rounds:

- **Scope** is the pure vocab-and-matrix layer only. Whole-`Config` validation,
  reference-counting, the numeric actuation check, and the axis mutual-exclusion
  clear stay in `daemon_stub` as operation logic over its own state — one
  caller, and they need a `Config` the GUI rarely has assembled. Keycodes are
  out (no daemon SSOT).
- **Fixture** is a golden JSON file generated by a daemon `#[test]` (not a
  `--dump-schema` CLI flag — `main.rs` has no arg parsing — and not a build
  script). It lives under `daemon/` because the daemon owns it and the
  dependency direction is GUI → daemon. Verdicts are derived by driving the real
  `config::validate`, not by hand-listing.
- **Drift enforcement** is documentation only — there is no CI in this repo
  (no `.github/workflows`, no workspace). The Rust golden test and the Python
  contract test each live in a suite a contributor already runs; `CONTRIBUTING.md`
  gets one bullet. A `make check` aggregator was considered and declined as
  out of proportion.
- **`inputs.py`** keeps all presentation (labels, menu ordering, `TRIGGER_SHORT`,
  `default_trigger_for`); `rules` owns the label-free predicates and sets.
- **One pass**, per the ticket 03 / 05 precedent — the daemon side is purely
  additive and the GUI side is a focused extraction with no wire change and no
  migration.
- **No ADR, no CONTEXT.md term** — see "Recording the decision".

Facts dug from the code during the grilling (not asked of the user): the
keypress field has no daemon allowlist; `config::validate` is whole-`Config`
only; there is no CI or Cargo workspace; `daemon_stub` currently imports catalogs
from the picker widget modules; `test_daemon_stub.py` is 620 lines and tests the
fake against itself.
