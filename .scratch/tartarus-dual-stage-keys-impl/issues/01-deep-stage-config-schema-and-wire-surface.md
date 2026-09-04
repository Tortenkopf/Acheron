# 01 — Deep-stage config schema and `GetConfig` wire surface

**What to build:** Every Profile gains the ability to carry a **deep stage** on any of
its grid keys — a second Actuation/Release point pair plus a per-Layer Binding, plus a
per-Input-per-Profile Staging mode governing the handoff. Stored in `config.toml` as
`deep_base`/`deep_held`/`deep_stages` and surfaced through `GetConfig`. A user
hand-editing `config.toml` can add a `[profiles.<name>.deep_base.<input>]`/
`[profiles.<name>.deep_stages.<input>]` pair and have it round-trip, rejected by
`config::validate` if it violates any of the seven new invariants. A user upgrading
from a build without this feature keeps their existing `config.toml` working
unchanged, with every Profile's three new maps empty. No runtime or D-Bus-mutation
behavior in this ticket — nothing drives a deep stage yet and no caller can create one
over D-Bus; a deep stage can only be authored by hand-editing `config.toml`.

Source of truth: [`spec.md`](../../tartarus-dual-stage-keys/spec.md) §"Config schema".

**Blocked by:** None — can start immediately.

**Status:** resolved

- [x] `DeepStageConfig { actuation: ActuationPoint, mode: StagingMode }` sits beside
      `ActuationPoint` in the daemon config module — `Copy`, bundling the two fields
      because they're only ever meaningful together (mirrors `ActuationPoint` itself).
      `StagingMode` is a four-variant `#[serde(rename_all = "snake_case")]` enum
      (`Handoff` `#[default]`, `NoReturn`, `Additive`, `QuickSkip`) so
      `.entry(input).or_default()` always lands on Handoff regardless of which field
      (`SetDeepActuation`/`SetStagingMode`, ticket 05) arrives first.
- [x] `Profile` gains three fields after `axis_base`/`axis_held`, exactly per
      spec.md's "Config schema": `deep_base: HashMap<Input, Binding>` and
      `deep_held: HashMap<Input, Binding>` (`base`'s exact per-Layer-map sibling, one
      level deeper, mirroring `chords_base`/`axis_base`'s existing idiom), and
      `deep_stages: HashMap<Input, DeepStageConfig>` (per-Input per-Profile, shared
      across Base/Held, mirroring `default_actuation`/`actuation_overrides`). All
      three `#[serde(default, skip_serializing_if = "HashMap::is_empty")]`. No
      `schema_version` bump — the empty-map serde default *is* the migration, the
      codebase's unbroken precedent.
- [x] `Profile::deep_layer`/`deep_layer_mut`, exact mirrors of `layer`/`layer_mut`,
      returning `&deep_base`/`&mut deep_base` or `&deep_held`/`&mut deep_held` by
      `Layer`.
- [x] `profile_all_binding_sites` chains `deep_base`/`deep_held` in as a third
      `BindingSite::Individual` source (not a new `BindingSite` variant — a deep
      Binding is legal only on a Grid Input, the same shape as any other individual
      Binding). Verify by inspection (and a targeted test) that this means
      `binding::check_binding` already applies the exact same payload/Trigger-mode/
      site-shape rules to a deep Binding as to a primary one with zero new code in
      the `binding` submodule — "each stage is a full Binding, all existing
      per-Action validation applies to each stage independently" falls out for free.
- [x] Seven new `ConfigError` variants, appended to the enum and to `validate` after
      the existing checks, `Display` strings and locus conventions exactly as
      spec.md's "Config schema" section specifies (locus is always just the
      `Input`'s `Display` string, matching every existing hysteresis/conflict error —
      never the Layer):
  - `InvalidDeepStageInput(String)` — a `deep_stages` entry keyed by a non-`Grid`
    `Input`.
  - `DeepStageReleaseNotBelowActuation(String)` — a `deep_stages` entry whose own
    `release` is not strictly below its own `actuation`.
  - `DeepStageBandOverlapsPrimary(String)` — a `deep_stages` entry whose `release`
    is not strictly greater than the same Input's resolved primary `actuation` (the
    disjoint-and-stacked constraint, `deep.release > primary.actuation`).
  - `DeepStageWithoutPrimary(String)` — a `deep_base`/`deep_held` Binding with no
    matching Binding in `base`/`held` on the same Layer.
  - `DeepStageMissingConfig(String)` — a `deep_base`/`deep_held` Binding with no
    matching entry in `deep_stages`.
  - `AnalogRepeatOnDualStageKey(String)` — a Binding (primary or deep, either
    Layer) using `analog_repeat` on an Input that has a functioning deep stage.
  - `ChordMemberDeepStageConflict(String)` — an Input that is both a Chord member
    (on some Layer) and carries a `deep_base`/`deep_held` Binding on that same
    Layer.

  All seven are whole-`Config` checks (they need `deep_stages` cross-referenced
  against `deep_base`/`deep_held`, or the primary's resolved Actuation point, or
  Chord membership) — they live in `config::validate` itself, not
  `binding::check_binding`.
- [x] New parse test `a_pre_dual_stage_config_defaults_deep_fields`, mirroring
      `a_pre_status_led_config_defaults_status_leds`: a minimal pre-feature
      `config.toml` (no `deep_base`/`deep_held`/`deep_stages` keys) parses with all
      three empty on every Profile.
- [x] One `config::validate` test per new `ConfigError` variant (seven), following
      the file's existing one-test-per-rule convention, plus a positive test that
      the sample `config.toml` fragment from spec.md's "Config schema" section (a
      Handoff `grid_r1c1` with `deep.release` 200 > `primary.actuation` 128,
      `deep.release` 200 < `deep.actuation` 220) parses and validates clean.
- [x] The D-Bus wire module: `StagingMode` marshals as a flat lowercase string
      (`handoff`/`no_return`/`additive`/`quick_skip`), the exact convention
      `axis_target_str` already uses; `DeepStageConfig` marshals as a flat dict
      bundling its `ActuationPoint` sub-dict plus the mode string. `profile_to_dict`
      gains three entries (`deep_base`, `deep_held`, `deep_stages`) after
      `status_leds`. A `config_to_dict` test asserts the three keys, mirroring the
      existing default-actuation/status-LEDs serialization tests.
- [x] `DaemonStub`'s seed Profile dict gains `"deep_base": {}`, `"deep_held": {}`,
      `"deep_stages": {}` so stub-backed GUI code sees the same shape a real
      `GetConfig` returns.
- [x] `rules.py` gets nothing yet — no GUI mutation path exists in this ticket
      (ticket 05).

## Comments

**2026-09-04** — Implemented as specified, verbatim against ticket 03's Answer
(`../../tartarus-dual-stage-keys/issues/03-config-dbus-surface.md#answer`) for the
exact `Display` strings, validate-check ordering, and wire shapes. `DeepStageConfig`
derives `Default` (clippy's `derivable_impls` flagged the spec's manual `impl
Default`, since `ActuationPoint` and `StagingMode` already both implement it) — same
128/112/Handoff values, mechanical difference only.

One fix beyond the checklist: `profile_all_binding_sites` folding `deep_base`/
`deep_held` in as `Individual` sites (this ticket) changes what
`edit.rs::macro_references`/`stepper_references` count "for free," but
`gui/acheron_gui/read_model.py::_profile_all_bindings` — an explicit client-side
mirror of `config::profile_all_bindings`, used by both `library_view`'s "Used by N"
gate and `daemon_stub.py`'s delete-refusal guard — was left stale, under-counting a
Macro/Stepper reference living only in a hand-authored `deep_base`/`deep_held` entry
(already reachable today via hand-edited `config.toml`, per this ticket's own scope).
Found by `/code-review`; fixed by adding `deep_base`/`deep_held` to the scanned
layer keys, with a regression test.

`cargo fmt --check`, `cargo clippy --all-targets` clean; daemon suite 415 green; GUI
suite 411 green. `/code-review` surfaced the one finding above, fixed and re-verified.
