# 08 — Profile-default deep band

**What to build:** A follow-up to ticket 07, from Charon's testing. `+ Add deep
stage` currently seeds a new deep stage's band from a fixed offset off the key's
primary Actuation point (`primary.actuation + 20` / `+ 35`). Charon wants
**"Set as Profile default"** in the dual-stage panel to also record the current
deep band, so `+ Add deep stage` on any other key of that Profile starts from
the remembered band instead of the offset.

**Blocked by:** 07

**Status:** resolved

- [x] `Profile.default_deep_actuation: Option<ActuationPoint>` — `None` (the
      seed / a pre-feature `config.toml`) means "compute the offset";
      `#[serde(default, skip_serializing_if = "Option::is_none")]` so it's only
      written once set. Validated for hysteresis (`release < actuation`) when
      `Some`, reusing `ConfigError::ReleaseNotBelowActuation` with locus
      `"default deep"` — no disjoint-from-primary check (it's only a GUI seed;
      the real per-key `deep_stages` entry it produces is still checked).
- [x] `Edit::SetDefaultDeepActuation { actuation, release }` — sets
      `default_deep_actuation = Some(..)`. No `Effect` (runtime never reads it),
      mirroring `SetDeepActuation`. D-Bus `SetDefaultDeepActuation(yy)`,
      mirroring `SetDefaultActuation(yy)`.
- [x] `wire.rs` — `profile_to_dict` serializes `default_deep_actuation` only
      when `Some` (a nested `actuation`/`release` dict, like `deep_stages`'
      `actuation` sub-dict); GetConfig round-trips it.
- [x] GUI mirror: `daemon_client.set_default_deep_actuation`,
      `daemon_stub.set_default_deep_actuation` + seed profile (absent key),
      `wire.py`/`read_model.py` surface it.
- [x] `binding_editor.build_dual_stage_panel`: `default_deep_cfg()` reads
      `config["profiles"][profile].get("default_deep_actuation")` and uses it
      (clamped disjoint from *this* key's resolved primary — the stored band may
      pre-date a per-key primary override) when present, else the offset.
      `on_set_default` also calls `client.set_default_deep_actuation(d_act,
      d_rel)` whenever `has_deep()`.
- [x] Tests: daemon `edit`/`wire`/`validate`; GUI `daemon_stub` +
      `binding_editor` (Set-as-default records the deep band; a later
      `+ Add deep stage` on another key seeds from it; the clamp when the stored
      band would overlap a per-key primary override).

## Answer

`Profile.default_deep_actuation: Option<ActuationPoint>` added
(`daemon/src/config.rs`), `#[serde(default, skip_serializing_if =
"Option::is_none")]`, hysteresis-validated when `Some` via the existing
`release_not_below_actuation` find_map (locus `"default deep"`).
`Edit::SetDefaultDeepActuation` (`edit.rs`) sets it, no `Effect`; D-Bus
`SetDefaultDeepActuation(yy)` (`dbus/mod.rs`) mirrors `SetDefaultActuation`.
`wire::profile_to_dict` emits `default_deep_actuation` only when `Some`
(`actuation_point_to_dict` sub-dict).

GUI: `daemon_client`/`daemon_stub` gain `set_default_deep_actuation` (+ the stub
seed profile leaves the key absent, matching a fresh daemon); `wire.py`
`profile_from_wire` and `read_model` pass it through.
`binding_editor.build_dual_stage_panel.default_deep_cfg()` prefers the stored
profile default, clamped so `release > this key's resolved primary actuation`
and `actuation > release`; falls back to the `+20/+35` offset when absent or
after the clamp collapses it. `on_set_default` fires
`client.set_default_deep_actuation` alongside `set_default_actuation` whenever a
deep stage exists on the key.

Tests: `daemon` — `set_default_deep_actuation_records_it`, wire round-trip,
`validate` hysteresis reject. `gui` — stub method + seed shape,
`set_as_profile_default_records_the_deep_band`,
`add_deep_stage_seeds_from_the_profile_default_deep_band`, and the
overlap-clamp case. 468 daemon + 461 GUI tests pass; `cargo clippy
--all-targets` / `cargo fmt --check` clean.
