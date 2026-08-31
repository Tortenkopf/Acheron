<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 04 — One `config::validate` as the single Config-invariant enforcement point

**What to build:** Every structural invariant of a `Config` must be checked in
exactly one place — a new `config::validate(&Config) -> Result<(), ConfigError>`
— called from both `config::parse` (after the strongly-typed deserialize) and
`config::persist_edit` (after the edit closure returns `Ok`, before the disk
write). The dispatch task stops carrying its own copies of those rules.

Today the same invariants are enforced twice, in two files, with no shared
function. `config::parse` runs ~17 post-deserialize checks so a hand-edited
`config.toml` can't put the Daemon into a state its downstream `expect`/
`unreachable!` paths assume away (`executor::compile`, `resolve_step`,
`active_profile().expect(...)`). The live-edit path re-checks a subset by hand:
`dispatch::validate_binding`, `validate_stepper_items`, `chord_conflict`,
`axis_conflict`, `reject_release_above_actuation`, `reject_non_grid_input`, plus
inline arm code in `SetBinding` / `SetChordBinding` / `SetAxisAssignment` /
`SetActuationPoint` / `SetDefaultActuation`. A rule added on one side and missed
on the other is either a latent panic (missed in `parse`) or a corrupt in-memory
`Config` persisted to disk (missed in `dispatch`). None of the dispatch-side
rollback/rejection paths that guard against the latter has test coverage.

This is the `config::persist_edit` follow-through (ticket 03): that ticket made
the *edit + persist* atomic; this one makes *validation* single-sourced on the
same path. Convert in one pass, as ticket 03 did — a half-migrated state with
`validate` live **and** the dispatch copies still present means two
invariant-enforcement points, which is worse than either alone.

## The invariant / precondition split

`validate` owns everything that makes a stored `Config` structurally invalid —
anything that could be written to disk and reloaded:

- ProfileSwitch / ControllerButton / Step trigger-mode legality
- the gamepad-button allowlist, for both `Action::ControllerButton` and
  `StepperItem::ControllerButton`
- dangling `Macro` / `Stepper` references **and** dangling `ProfileSwitch`
  targets (a `ProfileSwitch` naming a Profile not in `[profiles]`)
- Analog-repeat only on grid Inputs; no Analog-repeat or ProfileSwitch on a
  Chord Binding
- Axis assignment only on grid Inputs; no Input carrying both an Axis
  assignment and a Binding, or both an Axis assignment and Chord membership,
  on the same Layer
- two Chords on one Layer in a subset/superset member-set relationship
- a stored Chord Binding with fewer than two member Inputs
- `default_actuation` or any `actuation_overrides` entry with
  `release >= actuation` (defeats hysteresis — chatters on a motionless key)
- an `actuation_overrides` entry keyed by a non-grid Input
- a Profile keyed by an empty or whitespace-only name

The `handle_command` arms keep the checks that are only meaningful relative to
the requested operation, not to the resulting `Config`:

- `NotFound` when clearing / renaming / deleting something that isn't there
- `AlreadyExists` when creating with a name already taken
- "cannot delete the active Profile"
- "cannot delete a Macro / Stepper still referenced by a Binding" — the arm
  keeps its specific, friendly message; `validate` independently forbids the
  dangling reference such a delete would create, so the guarantee holds even
  if an arm check is ever missed

## New `ConfigError` variants

Six invariants have no variant today. Add:

- `ReleaseNotBelowActuation(String)` — locus (`"default"` or the Input) in the
  string
- `InvalidActuationOverrideInput(String)`
- `UnknownProfileSwitchTarget(String)`
- `ChordMemberSetConflict { key: String, other: String }`
- `ChordTooFewMembers(String)`
- `EmptyProfileName`

`validate` returns the first violation it finds (as `parse` does today). Run the
existing checks in `parse`'s current order and append the six new ones, so no
existing single-violation `parse` test changes which error it sees.

## Error routing and messages

`From<ConfigError> for CommandError` currently maps **every** `ConfigError` to
`CommandError::IoError`, because the only path a `ConfigError` takes into
`CommandError` today is a genuine disk-write failure inside `persist_edit`. Once
`validate` feeds this conversion, change it to match: `ConfigError::Io(_)` stays
`IoError`; every other variant becomes `CommandError::InvalidRequest(_)` (→
`DaemonError::InvalidBinding` → the GUI's `InvalidBindingError`). This keeps the
existing `matches!(err, CommandError::InvalidRequest(_))` dispatch tests and the
GUI stub's `InvalidBindingError` expectations valid.

Reword the invariant `ConfigError` `Display` strings to drop the "config.toml
contains …" framing (e.g. "a Profile Switch Binding must use fire_once"), since
the same text now also surfaces over D-Bus to a GUI user who is not hand-editing
a file. `Io`, `Parse`, `MissingSchemaVersion`, `InvalidSchemaVersion`,
`UnsupportedSchemaVersion`, and `LegacyInlineMacroBinding` keep their
file-oriented wording — they genuinely are load-time failures.

## Wiring

- `parse`: delete the inline invariant block; after the typed deserialize, call
  `validate(&config)?` and return. The schema-version check and the raw-TOML
  legacy-inline-macro scan stay ahead of the typed deserialize, unchanged.
- `persist_edit`: `snapshot → edit closure → validate → persist`, restoring the
  snapshot on any of the three failing. No signature change — the existing
  `E: From<ConfigError>` bound already covers the new call. `RenameProfile`'s
  same-name early return still bypasses `persist_edit` entirely and is
  unaffected.
- Delete `validate_binding`, `validate_stepper_items`, `chord_conflict`,
  `axis_conflict`, `reject_release_above_actuation`, and `reject_non_grid_input`
  from the dispatch task, along with the inline invariant checks in the arms.
  Every call site is inside a `handle_command` arm — none is on the live input
  path — so this is a clean removal. Drop the now-unreachable
  `reject_non_grid_input` guards from the `Clear*` arms too; "clear of an
  absent entry" keeps behaving as it does today for the grid case.
- `config::profile_all_bindings` stays `pub(crate)` — shared by `validate` and
  by the surviving `macro_references` / `stepper_references` precondition
  guards.
- Make `validate` `pub(crate)`. Its doc comment states the contract: called
  from `parse` and `persist_edit`, first-error-wins, structural invariants
  only — never operation preconditions. Add one sentence to `CONTRIBUTING.md`
  pointing a contributor who adds a new `Command` arm at it.

## Consequence: stricter startup (intended)

A `config.toml` that boots today will start refusing to load if it has
`release >= actuation`, a non-grid `actuation_overrides` key, a subset/superset
Chord pair, or a `ProfileSwitch` naming a missing Profile. This is the point of
single-sourcing — same contract as the existing refuse-to-start-on-corrupt-TOML
behaviour. No auto-repair; that would be its own ticket.

## Tests: replace, don't layer

- `validate` gets its own synchronous, table-driven test module — one case per
  invariant, no tokio, no tempfile. This is the new test surface.
- `Config::seed()` passes `validate`.
- The existing `config::parse` tests stay — they now exercise `validate`
  transitively — and gain cases for the four newly-enforced invariants.
- In the dispatch task, keep only: one integration test showing an
  invariant-violating edit is rejected **and** the in-memory `Config` is rolled
  back; and the existing tests where a rejection is coupled to a specific side
  effect or rollback (the cross-Profile stepper-direction steal rollback, the
  Mode-key-role change stopping a running Toggle).
- Delete the dispatch `#[tokio::test]`s that only assert "bad Action/Trigger
  combination → `InvalidRequest`" — that coverage moves to `validate`'s module.

## Out of scope

Carving `parse` / `serialize` / `load_or_seed` / `persist` / the legacy scan
into a `config::toml` submodule. Separate architecture-review candidate.

**Blocked by:** None — can start immediately.

**Status:** resolved

- [x] `config::validate(&Config) -> Result<(), ConfigError>` exists, is
      `pub(crate)`, returns the first violation, and its doc comment states the
      parse + persist_edit / invariants-not-preconditions contract.
- [x] `config::parse` performs no inline invariant checking beyond the schema
      and legacy-macro scans — it deserializes, calls `validate`, and returns.
- [x] `config::persist_edit` calls `validate` after the edit closure and before
      the write, and restores the pre-edit snapshot if `validate` fails, with
      no change to its signature.
- [x] `validate_binding`, `validate_stepper_items`, `chord_conflict`,
      `axis_conflict`, `reject_release_above_actuation`, and
      `reject_non_grid_input` no longer exist in the dispatch task, and no
      `handle_command` arm performs its own invariant check; operation
      preconditions (`NotFound`, `AlreadyExists`, active-Profile delete,
      referenced-entry delete) remain in the arms.
- [x] The six new `ConfigError` variants exist; `default_actuation` /
      `actuation_overrides` hysteresis, non-grid actuation overrides,
      subset/superset Chord pairs, sub-two-member stored Chords, dangling
      `ProfileSwitch` targets, and empty Profile names are all rejected by
      `validate` — enforced identically at startup and on a live edit.
- [x] `From<ConfigError> for CommandError` maps `Io` to `IoError` and every
      other variant to `InvalidRequest`; an invariant violation over D-Bus
      reaches the GUI as `InvalidBindingError`, not `DaemonIoError`.
- [x] Invariant `ConfigError` `Display` strings read correctly both at startup
      and over D-Bus (no "config.toml contains …" on the invariant variants);
      load-time variants keep their file-oriented wording.
- [x] A hand-edited `config.toml` with `release >= actuation` (or any other
      newly-enforced invariant) refuses to start with a clear, specific error.
- [x] `validate` has a per-invariant synchronous test module; `Config::seed()`
      is covered; the dispatch suite keeps only the rollback/side-effect-coupled
      rejection tests plus one "reject + roll back" integration test; the
      combo-only dispatch rejection tests are gone.
- [x] `CONTRIBUTING.md` names `config::validate` as the sole place to add a
      Config invariant.
- [x] Full Daemon test suite green; `cargo clippy` clean; GUI and packaging
      suites unaffected.

## Comments

**2026-08-31** — Filed from an architecture-review grilling session (candidate 2
of the review that also produced ticket 03). Design tree settled: error type is
the existing `ConfigError` (no new `ValidationError`); `validate` runs inside
`persist_edit` rather than per-arm; stricter startup is accepted rather than
softened with a lenient subset or auto-repair; the `config::toml` submodule
split is a separate candidate.

**2026-08-31** — Implemented. `config::validate` holds the former ~17 `parse`
checks (unchanged order) plus the six new ones; `parse` and `persist_edit` both
run it, and the six dispatch helpers plus every arm-level invariant check are
gone. `From<ConfigError> for CommandError` splits `Io` → `IoError` / else →
`InvalidRequest`. New synchronous `validate_invariants` test module (one case
per invariant) + `Config::seed()` coverage; four new `parse` tests; 17
combo-only dispatch rejection tests + one dbus one deleted; one
"invariant-violating edit rejected **and** rolled back" dispatch integration
test added; a dbus no-op-success test for `ClearActuationPoint` on a non-Grid
Input replaces the deleted rejection one. `CONTRIBUTING.md` gained the
single-enforcement-point note. Daemon suite 374 green, `clippy --all-targets -D
warnings` clean, `fmt --check` clean, GUI (356) + packaging suites pass.
Code-review (fork) raised the intended-strict-startup consequence and the
one-integration-test coverage trade-off — both are ticket-sanctioned; its note
about the Macro/Stepper display-name check staying arm-side (the slug, not the
name, is the key) is reflected in the `CONTRIBUTING.md` wording.
