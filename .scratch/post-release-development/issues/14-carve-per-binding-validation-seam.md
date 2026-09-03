<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 14 — Carve the per-Binding half of `config::validate` behind a pure `check_binding` seam

**What to build:** The per-Binding legality rules in `config::validate` —
the ProfileSwitch / ControllerButton / Step trigger checks, the
ControllerButton gamepad-code check, analog-repeat-needs-Grid,
Chord-can't-ProfileSwitch, Chord-can't-analog-repeat — move behind one pure
function `config::binding::check_binding(BindingSite, &Binding) -> Result<(),
ConfigError>` in a new `daemon/src/config/binding.rs`. The one real
Axis-*placement* rule (`InvalidAxisInput` — an Axis assignment is legal only
on a Grid Input) moves beside it as `check_axis_assignment(Input) ->
Result<(), ConfigError>`. `validate` keeps every whole-`Config` check
(dangling refs, Chord subset/superset, Chord `< 2` members,
`release < actuation`, the two Axis *conflict* checks, empty profile name,
legacy inline macro, `InvalidControllerButtonStepperItem`) and now drives
`check_binding` over a new `config::profile_all_binding_sites` iterator.
`schema.rs`'s `trigger_verdict` / `action_kind_verdict` call the two pure
functions **directly** — `seeded_config()` and the insert-into-a-synthetic-
`Config` dance are deleted.

**No behaviour change.** `validate` returns the same verdict for every
`Config`; `daemon/contract/daemon-schema.json` regenerates byte-identical;
`gui/acheron_gui/rules.py` and `gui/tests/test_rules_contract.py` are
untouched. The one deliberate shift: a `Config` with *simultaneous*
violations of different classes may now report a per-Binding violation where
it previously reported an interleaved dangling-reference one — no test and no
caller depends on that ordering, and single-violation ordering (which the
`parse` tests do pin) is preserved. The `config.rs:1117` doc comment is
updated to say so.

## The friction

Ticket 04 made `config::validate` the single invariant point — called from
both `parse` and `edit::plan`, which is the win and stays. But `validate` is
now a ~220-line wall: ~20 sequential blocks, most of the shape
`config.profiles.values().find_map(|p| profile_all_bindings(p).find_map(…))`,
the six ticket-04 additions literally labelled *"appended so existing `parse`
tests are undisturbed"* (`config.rs:1279`). It conflates two check classes it
never separates:

- **pure per-Binding** — a function of `(one Binding, where it sits, the
  vocab)`: ProfileSwitch⇒FireOnce, ControllerButton≠FireOnce, Step≠Toggle,
  ControllerButton's `button` in the gamepad allowlist, AnalogRepeat needs a
  Grid Input, a Chord Binding may be neither ProfileSwitch nor AnalogRepeat.
- **whole-`Config`** — `active_profile` names a real Profile; dangling
  macro / stepper / profile-switch targets; Chord subset/superset; Chord
  `< 2` members; `release < actuation`; an Axis Input also carrying a
  Binding or a Chord membership.

Two other artifacts reconstruct the per-Binding subset by brute force
because there is no function to call:

- **`daemon/src/schema.rs`** (`trigger_verdict` :184, `action_kind_verdict`
  :202) synthesizes a 2-profile `Config` built precisely so nothing else can
  fail (`seeded_config` :100, *"leaving the combination under test as the
  only thing that can make `validate` fail"*), inserts one Binding, runs the
  entire 20-check monolith, and collapses the result to one bool — **580 +
  174 times** (the `trigger_matrix` and `action_kind_matrix` rows).
- **`gui/acheron_gui/rules.py`** (`valid_action_kinds` :119, `valid_triggers`
  :137) hand-transcribes the same matrix as set algebra, each rule's
  docstring naming the `ConfigError` variant it mirrors.

The GUI side already found this seam worth having — `rules.py` *is* the pure
half, contract-tested against the daemon (ticket 06). The daemon just has no
matching function, so `schema.rs` reverse-engineers one.

**Deletion test.** Deleting `check_binding` moves complexity nowhere — the
per-Binding rules only exist today as a shapeless middle stretch of a
monolith. Concentrating them into one pure, exhaustively table-testable
function is the win: `schema.rs` stops synthesizing throwaway `Config`s,
per-Binding legality becomes a truth table, and `rules.py` mirrors one named
function instead of a matrix inferred from a wall.

## The module

```rust
// daemon/src/config/binding.rs
//
// Pure and synchronous. Imports Binding / TriggerMode / Action / ConfigError
// from the config module and Input / is_gamepad_button from crate::input.
// NOTHING from edit, dispatch, executor, dbus. No Config, no Layer, no
// Profile — any check that needs the rest of the Config stays in validate.

/// Where a Binding sits. The per-Binding checks that care about the Input
/// (analog-repeat needs a Grid key) or about being a Chord's own Binding
/// (no ProfileSwitch, no analog-repeat) have to tell these apart.
/// `rules.py` mirrors this as `input_str: str | None` — `None` == `Chord`.
pub(crate) enum BindingSite {
    Individual(Input),
    Chord,
}

/// Every per-Binding legality rule, applied in the order `validate` applied
/// them — so the one Binding that can trip two rules (a `ControllerButton`
/// naming a non-gamepad `KeyCode` *and* carrying a Fire-once trigger) still
/// reports the bad code, not the bad trigger:
///   1. action-payload — `ControllerButton { button }` must be a gamepad code
///   2. trigger legality — ProfileSwitch⇒FireOnce, ControllerButton≠FireOnce,
///      Step≠Toggle
///   3. site-shape — AnalogRepeat needs `Individual(Grid)`; a `Chord`
///      Binding may be neither ProfileSwitch nor AnalogRepeat
pub(crate) fn check_binding(site: BindingSite, binding: &Binding) -> Result<(), ConfigError>;

/// The one per-assignment Axis rule: an Axis assignment is legal only on a
/// Grid Input (only Grid keys have Depth — ticket 59 §1). The two Axis
/// *conflict* rules (an Axis Input also carrying a Binding, or in a Chord's
/// member set on the same Layer) are cross-map and stay in `validate`.
pub(crate) fn check_axis_assignment(input: Input) -> Result<(), ConfigError>;
```

## Integration in `validate`

- New `config::profile_all_binding_sites(profile) -> impl Iterator<Item =
  (BindingSite, &Binding)>` chaining `base` / `held` (→ `Individual(*input)`)
  and `chords_base` / `chords_held` (→ `Chord`). `profile_all_bindings` is
  **reimplemented** as `profile_all_binding_sites(p).map(|(_, b)| b)` so the
  four-map list lives in exactly one place; `edit.rs`'s `macro_references` /
  `stepper_references` keep calling `profile_all_bindings` unchanged.
- The seven per-Binding / axis-placement blocks (`config.rs:1132–1160`,
  `1183–1240`, `1241–1252`) collapse to two loops, placed where the *first*
  of them sits today — right after the `InvalidActiveProfile` guard:

  ```rust
  for profile in config.profiles.values() {
      for (site, binding) in profile_all_binding_sites(profile) {
          binding::check_binding(site, binding)?;
      }
      for layer in [Layer::Base, Layer::Held] {
          for input in profile.axis_layer(layer).keys() {
              binding::check_axis_assignment(*input)?;
          }
      }
  }
  ```

- The dangling-reference blocks (`UnknownMacro` :1161, `UnknownStepper`
  :1172, `UnknownProfileSwitchTarget` :1303), the Axis *conflict* blocks
  (`AxisBindingConflict` :1253, `AxisChordConflict` :1265), and every other
  whole-`Config` block stay in `validate`, in `parse`'s original order,
  after the per-Binding loop.
- `config.rs:1117` doc: *"Returns the first violation. Per-Binding rules
  (`binding::check_binding` / `check_axis_assignment`) are checked first as a
  group; whole-`Config` rules follow in `parse`'s original order.
  Single-violation ordering is unchanged; a `Config` that violates rules of
  both classes now reports the per-Binding one."*

## `schema.rs`

- `trigger_verdict(input, kind, trigger)` → build `Binding { trigger, action
  }`, then `binding::check_binding(site, &binding).is_ok()` where `site` is
  `BindingSite::Chord` for the `__chord__` sentinel, else
  `BindingSite::Individual(input.parse()?)`.
- `action_kind_verdict(input, kind)` → for `kind == "axis"`: `input ==
  "__chord__"` → `false` (an Axis assignment has no `Config` representation
  on a Chord — that is the type system, not a validation rule), else
  `binding::check_axis_assignment(input.parse()?).is_ok()`. For every other
  kind → `check_binding` as above with `neutral_trigger_for(kind)`.
- Deleted: `seeded_config()`, the `P1`-profile inserts, the `AxisTarget`
  import in that path. Kept: `action_for` / `trigger_for` /
  `neutral_trigger_for` / `chord_members` / the matrix row enumeration.
- `render_schema()` output is unchanged → `daemon-schema.json` byte-identical
  → `test_rules_contract.py` and `rules.py` untouched. If a verdict *does*
  shift, that is a real bug the fixture catches; the fix is `ACHERON_BLESS=1
  cargo test … schema` then mirror the delta into `rules.py`
  (`schema.rs:19–30`, `CONTRIBUTING.md`).

## What moves, what stays

- **Into `config/binding.rs`:** `InvalidProfileSwitchTrigger`,
  `InvalidControllerButtonTrigger`, `InvalidStepTrigger`,
  `InvalidControllerButton` (gamepad-code), `InvalidAnalogRepeatInput` +
  `InvalidChordAnalogRepeat`, `InvalidChordProfileSwitch`, `InvalidAxisInput`.
- **Stays in `validate`:** `InvalidActiveProfile`, `UnknownMacro` /
  `UnknownStepper` / `UnknownProfileSwitchTarget`,
  `InvalidControllerButtonStepperItem` (a Stepper *library* item, not a
  Binding), `AxisBindingConflict` / `AxisChordConflict`,
  `ReleaseNotBelowActuation`, `InvalidActuationOverrideInput`,
  `ChordMemberSetConflict`, `ChordTooFewMembers`, `EmptyProfileName`,
  `LegacyInlineMacroBinding`.
- **`ConfigError` and its `Display` impls stay in `config.rs`.** `edit.rs`'s
  `ConfigError → CommandError::InvalidRequest(err.to_string())` path
  (`edit.rs:793–804`) and its substring assertions are untouched.
- **`daemon/src/config.rs` stays a flat file**, gains `mod binding;`; the
  new code is a sibling `daemon/src/config/binding.rs` (no `config.rs` →
  `config/mod.rs` rename).

## Landing in one pass

Create `config/binding.rs`, add `profile_all_binding_sites` + retarget
`profile_all_bindings`, collapse the seven blocks into the two loops, rewire
`schema.rs`, then the test sweep — one PR (ticket 03–13 precedent). A
half-migrated `validate` with three blocks lifted and four still inline is
harder to read than either end state.

## Behaviour-preservation protocol

Not the latency-critical input path, but `validate` is the config gatekeeper
and a dropped check is a silent latent panic downstream:

- **Diff each lifted predicate line-by-line against `HEAD`.** Load-bearing:
  `InvalidControllerButton` uses `crate::input::is_gamepad_button` (pure);
  `InvalidAnalogRepeatInput`'s stored `String` is `Input::to_string()` — the
  un-quoted `Display` form, `config.rs:2632` / `:2912` assert `== "mode_key"`;
  the ControllerButton *code* check precedes the *trigger* check; the two
  chord blocks never stringify the `ChordKey`; `InvalidAxisInput`'s locus is
  `input.to_string()`.
- **`cargo test -p acheron-daemon` fully green** — the ~19 standalone
  `matches!(err, ConfigError::…)` tests and the 21-row
  `each_structural_invariant_has_a_dedicated_rejection` table
  (`config.rs:2780–3067`) — before any test moves.
- **`ACHERON_BLESS=1 cargo test -p acheron-daemon schema` produces a
  zero-diff `daemon/contract/daemon-schema.json`**;
  `gui/.venv/bin/pytest gui/tests/test_rules_contract.py` green with no
  `rules.py` change.
- **`/code-review` on both the Standards and Spec axes**, as tickets 05–13
  did.

## Tests: replace, don't layer

- **New synchronous `config::binding::tests`** — the primary surface, a
  truth table:
  - `check_binding`: for each `BindingSite` (`Individual(grid)`,
    `Individual(non-grid)`, `Chord`) × each Action kind × each `TriggerMode`
    → the exact `Ok(())` / `Err(ConfigError::X)` expected. The
    `ControllerButton`-bad-code-*and*-bad-trigger cell asserts the *code*
    error.
  - `check_axis_assignment`: `Grid` → `Ok`; `ModeKey` / `Thumbstick` /
    `Wheel` → `Err(InvalidAxisInput(locus))` with the locus string checked.
- **Kept in `config::tests` as integration smoke** — two rows of
  `each_structural_invariant_has_a_dedicated_rejection` proving `validate`
  actually calls the seam (one per-Binding, e.g. `InvalidProfileSwitchTrigger`;
  one axis, `InvalidAxisInput`). The other ~5 per-Binding rows in that table
  are deleted — covered exhaustively at the unit.
- **Unchanged** — `edit.rs`'s substring assertions (`is_invalid(e,
  "analog_repeat")`, `is_invalid(e, "cannot be profile_switch")`, …), every
  whole-`Config` row in the table, the `parse` tests.
- Net: the daemon suite gains the truth-table module, loses ~5 integration
  rows; coverage strictly increases.

## Decisions from the grilling

- **`check_binding(BindingSite, &Binding)`, `enum BindingSite {
  Individual(Input), Chord }`** — not `Option<Input>`. The overloaded `None`
  is exactly the implicit convention this refactor kills; `rules.py` maps
  `None ↔ Chord` in one comment line. (Q1)
- **`check_binding` is pure — no `&Config`.** The dangling-reference checks
  (`UnknownMacro` / `UnknownStepper` / `UnknownProfileSwitchTarget`),
  per-Binding in shape but needing the maps, stay in `validate`'s
  whole-`Config` pass. Purity is what lets `schema.rs` call it per matrix
  cell with no synthetic `Config`. (Q2)
- **Returns `ConfigError` directly** — its per-Binding variants already *are*
  the per-Binding error vocabulary; a second enum + conversion is ceremony.
  The module stays pure regardless. (Q3)
- **Also extract `check_axis_assignment(Input)`** — one real rule (Grid-only),
  one param. No `BindingSite`, no `AxisTarget` (no per-target legality
  exists), no synthetic "Chord can't hold an axis" variant — `schema.rs`
  states that as a hardcoded `false`. The two Axis *conflict* checks are
  cross-map and stay in `validate`. (Q4, Q11)
- **Accept the first-violation reorder** across violation *classes*;
  preserve it within single-violation configs (the `parse` tests' actual
  contract). One-line doc update. (Q5)
- **`profile_all_bindings` reimplemented on `profile_all_binding_sites`** —
  one home for "a profile's Bindings live in these four maps." (Q6)
- **`schema.rs` calls the pure functions directly** — delete `seeded_config`
  + the insert dance (~35 lines). The fixture's job is to pin *exactly* what
  `rules.py` mirrors: per-Binding / per-axis legality, nothing else.
  `__chord__ + axis` → hardcoded `false`. (Q7)
- **Sibling `daemon/src/config/binding.rs` + `mod binding;`** — not a
  `config.rs` → `config/mod.rs` move (a 3000-line file move isn't this
  ticket's job) and not an inline `mod binding { … }`. (Q8)
- **The ~7 per-Binding rejection rows leave
  `each_structural_invariant_has_a_dedicated_rejection`**; 2 stay as wiring
  smoke; the truth table at the unit is the real coverage. (Q9)
- **`CONTRIBUTING.md` only** — note that `check_binding` /
  `check_axis_assignment` are the daemon-side source the `rules.py` mirror
  tracks and that `schema.rs` exercises them directly. **No `CONTEXT.md`
  entry** — this is a validation-locus refactor, not a domain concept. **No
  ADR** — nothing was rejected. (Q10)
- **Internal check order locked**: action-payload → trigger → site-shape.
  (Q12)
- **Filed as a single ticket** in the `post-release-development` campaign —
  same remit as tickets 03–13; the design is fully settled here, so no
  spec/impl split. (Q13)

## Facts dug from the code during the grilling (not asked of the user)

- `profile_all_bindings` (`config.rs:1356`) has **9 call sites**; none needs
  the Input/ChordKey. The four per-Binding checks that use it
  (`config.rs:1133/1142/1153/1184`) are keyspace-agnostic — that is *why*
  they were written against the key-less iterator. `edit.rs`'s
  `macro_references` / `stepper_references` (`edit.rs:690/702`) match on
  `binding.action` only. **No existing iterator** yields `(Input, &Binding)`
  or `(ChordKey, &Binding)` across all four layers.
- `validate` is documented *"returns the first violation … checks run in
  `parse`'s original order"* (`config.rs:1117–1121`). **No test constructs
  two simultaneous violations**; `schema.rs`'s `seeded_config` is built so
  exactly one thing can fail.
- Every daemon test is an inline `#[cfg(test)] mod` — there is **no
  `daemon/tests/`**. ~19 standalone `matches!(err, ConfigError::X)` tests +
  the 21-row `each_structural_invariant_has_a_dedicated_rejection` table
  (`config.rs:2780–3067`) + `edit.rs`'s ~8 `is_invalid(e, needle)`
  Display-substring rows (`edit.rs:1490–1788`).
- **Everything routes through the one `config::validate(&next)?` call**
  (`edit.rs:625`). `edit::plan`'s `SetBinding` / `SetChordBinding` arms do no
  per-Binding legality checks themselves — only operation preconditions
  (`NotFound`, blank name, still-referenced, can't-delete-active). Extracting
  the seam keeps that single-path property automatically.
- `config.rs` is a **flat 3069-line file**; only `dbus/` and `capture/` use
  directory modules (`mod.rs` style). `ConfigError` and its `Display` impls
  live in `config.rs`.
- Among the per-Binding errors, only `InvalidAnalogRepeatInput` and
  `InvalidControllerButton` carry a locus string — both from data the
  `(site, binding)` signature already has (`Input::to_string()`,
  `format!("{button:?}")`). `InvalidChordAnalogRepeat` /
  `InvalidChordProfileSwitch` carry none — `check_binding` never needs the
  `ChordKey`.
- `daemon-schema.json` shape: a top-level object, 6 fixed-order keys —
  `gamepad_buttons` (57), `axis_targets` (17), `trigger_matrix` (580 rows,
  `{action_kind, input, trigger, allowed}`), `action_kind_matrix` (174 rows,
  `{input, action_kind, allowed}`), `slug_examples` (16), `chord_key_examples`
  (12). `test_rules_contract.py` asserts `rules.valid_triggers` /
  `valid_action_kinds` reproduce every row.
- `schema.rs` is `#[cfg(test)]`-only (`lib.rs:33`), not linked into the
  daemon binary.
- `SetAxisAssignment` doesn't need the Axis *conflict* checks live — its
  `edit::plan` arm atomically *clears* the colliding Binding / Chord
  memberships (`edit.rs:575–599`) rather than rejecting.

**Blocked by:** None — ticket 04 (`config::validate` single invariant point)
and ticket 06 (`rules.py` mirror + contract fixture) are resolved.

**Status:** resolved

- [x] `daemon/src/config/binding.rs` exists, `pub(crate) mod binding;` in
      `config.rs`; exposes `BindingSite`, `check_binding`,
      `check_axis_assignment` `pub(crate)` and nothing else; imports only
      `config` (`super`) + `crate::input` types; no `async`, no `Config` /
      `Layer` / `Profile`. (`mod` is `pub(crate)`, not private, so `schema.rs`
      can reach the two functions.)
- [x] `check_binding` covers exactly the six per-Binding rules in the locked
      order (payload → trigger → site-shape) and returns the same
      `ConfigError` variants `validate` did.
- [x] `check_axis_assignment(Input)` holds the `InvalidAxisInput` Grid-only
      rule.
- [x] `config::profile_all_binding_sites` added; `profile_all_bindings`
      reimplemented on it; `edit.rs` call sites unchanged.
- [x] `validate`: the seven per-Binding / axis-placement blocks → two loops
      right after the `InvalidActiveProfile` guard; dangling-ref,
      Axis-conflict and all other blocks stay, same order; the `validate` doc
      comment updated to the prescribed wording.
- [x] `schema.rs::trigger_verdict` / `action_kind_verdict` call
      `check_binding` / `check_axis_assignment` directly (via a new
      `site_for(&str) -> BindingSite` helper); `seeded_config` + the inserts
      deleted; `__chord__ + axis` → `false`.
- [x] `ACHERON_BLESS=1 cargo test -p acheron-daemon schema` → zero-diff
      `daemon/contract/daemon-schema.json`; `rules.py` and
      `test_rules_contract.py` unchanged.
- [x] New `config::binding::tests` truth table (`check_binding` over
      site × kind × trigger; `check_axis_assignment` over `Input` variants);
      6 per-Binding rows removed from
      `each_structural_invariant_has_a_dedicated_rejection`, 2 kept as wiring
      smoke (`InvalidProfileSwitchTrigger`, `InvalidAxisInput`).
- [x] `CONTRIBUTING.md` — `check_binding` / `check_axis_assignment` named as
      the `rules.py` mirror source, with the `schema.rs`-exercises-them-
      directly note. No `CONTEXT.md` change.
- [x] `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, the full
      daemon suite (399) and the GUI suite (410) all green. `/code-review`
      Standards + Spec clean.

## Comments

**2026-09-03** — Filed from the `/improve-codebase-architecture` grilling
(third review, saved at
`.scratch/post-release-development/research/architecture-review-2026-09-03.html`
— candidate 1 of 7; the others — a `ConnectionScopedTask` behind the two
D-Bus disconnect races (2), pushing the firing/toggle handles into
`chord` / `trigger` (3), a `device_control` module for the Interface-2
channel (4), a D-Bus wire-shape contract test (5), one `AxisWrite` emit path
(6), a GUI `ConfigView` read-model (7) — are unfiled). Design tree settled
over three rounds; see "Decisions from the grilling". Not yet implemented.

**2026-09-03** — Implemented. `daemon/src/config/binding.rs`
(`pub(crate) mod`) holds `BindingSite`, `check_binding` (payload → trigger →
site-shape, comment-numbered), `check_axis_assignment`, plus a
site × kind × trigger truth-table test module. `validate`'s seven per-Binding
/ axis blocks became two loops over `profile_all_binding_sites`;
`profile_all_bindings` is now `.map(|(_, b)| b)` over it. `schema.rs` drives
the two pure functions directly through a `site_for` helper — `seeded_config`
and the synthetic-`Config` insert dance are gone.

Two deviations from the letter of the spec, both harmless:
- **`chord_members()` deleted, not kept.** The schema.rs section listed it
  under "Kept", but `BindingSite::Chord` carries no member data, so the
  helper is unreachable and `clippy -D warnings` rejects it. `site_for`
  replaces it. `daemon-schema.json` still regenerates byte-identical.
- **`mod binding` is `pub(crate)`, not private.** `schema.rs` calls
  `config::binding::check_binding` directly, which a private `mod` wouldn't
  expose. Items inside stay `pub(crate)` and the module surface is exactly
  the three the spec named.

`ACHERON_BLESS=1 … schema` → zero diff; `rules.py` /
`test_rules_contract.py` untouched; daemon suite 399 green, GUI suite 410
green; fmt + clippy clean; `/code-review` Standards + Spec both clean (only
judgement-call test-scaffolding smells, no hard findings).
