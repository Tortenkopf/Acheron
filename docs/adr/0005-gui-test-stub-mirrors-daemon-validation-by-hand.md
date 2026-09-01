# The GUI test stub's hand-mirrored Daemon validation is accepted, not a deepening target

`gui/acheron_gui/daemon_stub.py` is a synchronous, in-memory fake of the Daemon's
D-Bus surface, used only by the GUI test suite. It re-implements by hand the
*stateful* half of the Daemon's edit validation: dangling `macro_id` / `stepper_id`
references, reference-count delete refusals, the cross-keyspace stepper-steal,
`ProfileSwitch` target cascades on profile rename, the `release ≤ actuation` check,
and the axis↔binding↔chord mutual-exclusion clear. The *pure* half — device catalogs,
the Trigger-mode / Action-kind legality matrices, slug derivation, `ChordKey` ordering
— is already single-sourced: `gui/acheron_gui/rules.py` mirrors it and
`gui/tests/test_rules_contract.py` checks that mirror against
`daemon/contract/daemon-schema.json`, a fixture the Daemon generates from the real
`config::validate` (see `.scratch/post-release-development/issues/06-gui-rules-mirror-module.md`).

Architecture reviews keep surfacing the stateful half as a duplication to eliminate —
by extending the contract fixture to `(Config, Edit) → verdict` rows, by porting
`daemon/src/edit.rs::plan` to Python, or by generating one side from the other. This
has been weighed twice (the ticket-06 grilling, and the 2026-09 architecture review)
and declined both times, for reasons that do not change under a normal bug-fix cadence:

- The stub is a test artifact; the entire payoff would be in tests, which already pass.
- The duplicated logic is roughly 150 lines and low-churn — the feature set reached
  v1.0 and the `tartarus-input-expansion` effort is archived. `edit::plan` and
  `config::validate` are effectively stable.
- A faithful Python mirror of `edit::plan` would re-express ~500 lines of Rust,
  including `Effect` / `Outcome` types shaped for Daemon runtime state the GUI never
  touches, to guard reject paths that rarely move.
- ADR 0003 already settled that the domain model is copied across the process seam,
  not shared. This is the same trade-off, not a new one.
- The residual risk is silent drift between the two implementations. Its expected cost
  is low, and when drift does occur the fix is a small edit to the stub — not worth a
  standing harness. Auditing the stub against `edit.rs` / `config.rs::validate` by hand
  is cheap and sufficient, and worth doing whenever the Daemon's edit surface is
  touched.

What remains in force: `rules.py` and its contract fixture are the mechanism for the
pure half and must be kept current when a catalog or a pure legality rule changes
(`CONTRIBUTING.md` has the flow). Actual stub/Daemon drift, when found, is a bug to fix
in the stub. If the Daemon's config/edit semantics ever return to heavy churn — a new
feature epoch — revisit this; short of that, treat the hand-mirrored stub as
deliberate and do not re-open it.

A related, smaller duplication was considered and also left alone: the whole-`Config`
read-model queries computed both in shipped GUI code (`library_view.macro_used_by_count`,
`library_view.stepper_used_by_count`, `device_overview._chord_conflict`,
`device_overview._chords_containing`) and again in the stub
(`_macro_referenced` / `_stepper_referenced` / `_chord_conflict`). Consolidating these
into one read-model module does not warrant a dedicated effort; fold them together
opportunistically if you are already editing those call sites.
