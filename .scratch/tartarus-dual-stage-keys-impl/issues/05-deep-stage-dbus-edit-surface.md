# 05 — D-Bus Edit surface for deep-stage config

**What to build:** A caller can create, edit, and clear a grid key's deep stage — its
Binding, its Actuation/Release pair, and its Staging mode — over D-Bus, in four
granular calls mirroring the primary stage's own `SetBinding`/`SetActuationPoint`
split. No GUI widgets in this ticket; the GUI's `daemon_client`/`daemon_stub` mirror
is wired so the next ticket can build the panel against it, and `wire.py`/
`read_model.py` surface the new config fields the GUI reads.

Source of truth: [`spec.md`](../../tartarus-dual-stage-keys/spec.md) §"D-Bus `Edit`
surface".

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] Four new `edit` module `Edit` variants:
  - `SetDeepStage { input, layer, binding }` — creates/edits the deep Binding on
    `layer`. Mirrors `SetBinding` one level deeper. Relies entirely on the trailing
    `config::validate(&next)?` (no inline check) for `DeepStageWithoutPrimary`/
    `DeepStageMissingConfig` — sequencing across primary Binding / deep Binding /
    `deep_stages` config is the caller's job, the same way `SetAxisAssignment`
    leaves "was there already a Binding here" to `validate`'s reachable states.
  - `ClearDeepStage { input, layer }` — removes the deep Binding. `NotFound` if none
    exists. Does **not** cascade-clear `deep_stages` or force-release a live slot
    (ticket 06's job).
  - `SetDeepActuation { input, actuation, release }` — sets the deep
    Actuation/Release pair, `.entry(input).or_default()`-creating a fresh
    `DeepStageConfig` (mode defaults to Handoff) if none exists. **No `Effect`** —
    `stage::Engine` reads `Config` directly each tick, unlike `SetActuationPoint`'s
    `RepublishActuation`.
  - `SetStagingMode { input, mode }` — sets the Staging mode, same `.or_default()`
    creation, no `Effect`.
  - All four `plan` arms rely solely on the trailing `config::validate(&next)?` —
    no inline checks, no new `Effect`s.
- [ ] Four thin D-Bus method wrappers on `com.acheron.Daemon`, shaped exactly like
      `set_axis_assignment`/`set_actuation_point` (parse wire args, build the
      `Edit`, `self.apply(...)`): `SetDeepStage(input, layer, binding_dict)`,
      `ClearDeepStage(input, layer)`, `SetDeepActuation(input, u8, u8)`,
      `SetStagingMode(input, string)`.
- [ ] **No new signal.** These four emit nothing, following `SetBinding`/
      `SetActuationPoint`'s existing precedent — the GUI rebuilds from `GetConfig`
      after its own calls.
- [ ] `daemon_client.py`: four new methods + `Protocol` stubs, mechanical mirror of
      `set_axis_assignment`/`set_actuation_point`.
- [ ] `daemon_stub.py`: matching methods following `set_chord_binding`/
      `set_actuation_point`'s guard-clause-before-mutate shape — a Grid-input check,
      the deep-pair hysteresis check (`release < actuation`), the disjoint-band
      check against the resolved primary Actuation point, `_validate_binding_action`
      reused as-is for `set_deep_stage`, the two dangling checks
      (`DeepStageWithoutPrimary`/`DeepStageMissingConfig`), and
      `AnalogRepeatOnDualStageKey`/`ChordMemberDeepStageConflict` as
      `_reject_if_*`-style helpers.
- [ ] `wire.py`/`read_model.py` surface `deep_base`/`deep_held`/`deep_stages` in the
      config dict the GUI reads.
- [ ] `rules.py` gets nothing — all seven new rules are whole-`Config`/cross-map
      checks, not pure functions of one Binding; they land in `daemon_stub.py` only
      (a deep Binding's own payload/Trigger-mode legality is already covered for
      free via `_validate_binding_action`).
- [ ] Tests: a unit test per new `Edit` variant confirming it mutates the right
      field and relies on `validate` rather than an inline check; `daemon_stub.py`
      contract tests for all seven rejection paths, contract-tested the same way
      existing `daemon_stub` rules are; a D-Bus round-trip test (`SetDeepStage`/
      `SetDeepActuation`/`SetStagingMode`/`ClearDeepStage` over real D-Bus,
      persisted and visible via `GetConfig`), mirroring
      `set_status_leds_over_real_dbus_persists_the_triple_and_surfaces_it_via_get_config`.
