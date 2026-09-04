Type: grilling
Blocked by: 02
Status: resolved (Charon, 2026-09-04)

## Question

Decide the **`config.toml` schema and the D-Bus `Edit` surface** for the deep stage.

Grilling + `/domain-modeling` + `/codebase-design` against `daemon/src/config.rs`,
`daemon/src/edit.rs`, `daemon/src/dbus/`, and the GUI mirror
(`gui/acheron_gui/daemon_client.py` / `daemon_stub.py` / `wire.py` / `rules.py`).
Decision only. Model on the status-LEDs effort's ticket 04.

### Settled inputs (do not re-open)

- Deep **Binding** per-Layer; deep **ActuationPoint** + **staging mode** per-Input
  per-Profile, shared Base/Held (Q8).
- Constraint `deep.release > primary.actuation`, enforced at `Command` /
  `config::validate` (Q4).
- Analog-repeat banned on both stages of a dual-stage key; Chord member ⇒ no deep stage
  (Q10/Q11).
- Deep stage requires a primary Binding on that Layer; deleting the primary
  cascade-deletes the deep stage (Q6 — cascade detail is ticket 05, but the schema must
  not make a dangling deep stage representable).

### Settle at least

- **Schema shape.** Parallel per-Layer `deep_base` / `deep_held: HashMap<Input, Binding>`
  maps (mirroring `chords_base`/`axis_base`)? Sparse `deep_actuation: HashMap<Input,
  ActuationPoint>`? `staging_mode: HashMap<Input, StagingMode>` — or fold mode into a
  small `DeepStageConfig` struct? A `StagingMode` enum (4 variants, `#[serde(rename_all
  = "snake_case")]`, `Default`?). No `schema_version` bump (the codebase never bumps — the
  serde default *is* the migration; confirm the all-absent default parses cleanly).
- **`config::validate` rules** — variant names + `Display` strings for: disjoint-band
  violation, Analog-repeat on a dual-stage key, deep stage on a Chord member, deep stage
  with no primary. Mirror `ConfigError::ReleaseNotBelowActuation`'s precedent.
- **D-Bus `Edit` variants** — `SetDeepStage` / `ClearDeepStage` / `SetDeepActuation` /
  `SetStagingMode`, or a combined call? Whole-value vs partial. Which emit which
  `edit::Effect` (a republish of the actuation snapshot? a force-release?).
- **`parse` test** — new `a_pre_dual_stage_config_...` mirroring `a_pre_ticket_17_...`.
- **GUI mirror** — `daemon_client` / `daemon_stub` / `wire` mechanical mirror; **exact
  `rules.py` additions** (graduates the "rules.py mirror detail" fog patch).
- Whether `GetState()` / the read model needs *anything* (charting says no new field —
  confirm the deep-stage config reaches the GUI purely through the existing config
  snapshot).

### Output

An `## Answer` with the full schema (Rust types + a sample `config.toml` fragment), the
`ConfigError` additions with their strings, the `Edit` surface, and the GUI mirror diff —
ready for ticket 06.

## Answer

Resolved by a grilling session against `daemon/src/config.rs`, `daemon/src/config/binding.rs`
(post-release ticket 14's `check_binding` seam), `daemon/src/edit.rs`, `daemon/src/dbus/`,
and the GUI mirror (`daemon_client.py`/`daemon_stub.py`/`wire.py`/`rules.py`), modeled on
`tartarus-status-leds` ticket 04's surface-decision precedent, 2026-09-04. Builds on
[ticket 01](./01-staged-event-pipeline.md) (Route (b), `stage::Engine` in dispatch) and
[ticket 02](./02-dual-stage-state-machine.md) (the four state tables). Decisions only — no
build.

### 1. Rust types — `daemon/src/config.rs`

```rust
/// A grid key's deep-stage physical/behavioral configuration — its own
/// Actuation/Release pair plus which staging mode governs the handoff with
/// the primary stage. Bundled as one struct (not two parallel maps) because
/// the two fields are only ever meaningful together, mirroring how
/// `ActuationPoint` itself bundles `actuation`+`release` rather than
/// splitting them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepStageConfig {
    pub actuation: ActuationPoint,
    #[serde(default)]
    pub mode: StagingMode,
}

impl Default for DeepStageConfig {
    fn default() -> Self {
        DeepStageConfig {
            actuation: ActuationPoint::default(),
            mode: StagingMode::default(),
        }
    }
}

/// The four staging modes (charting Q3) governing how a grid key's primary
/// and deep stages hand off as Depth crosses the deep band. `Default =
/// Handoff` — the canonical "camera shutter" mode — so `SetDeepActuation`/
/// `SetStagingMode` (§3) can `.entry(input).or_default()` into a fresh
/// `DeepStageConfig` without special-casing which field arrived first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagingMode {
    #[default]
    Handoff,
    NoReturn,
    Additive,
    QuickSkip,
}
```

`Profile` (`config.rs:138`) gains three fields, placed after `axis_base`/`axis_held`:

```rust
/// Deep-stage Bindings active while this Profile's Base Layer is active —
/// `base`'s exact per-Layer-map sibling, one level deeper (Q8). An entry
/// here requires a matching entry in `base` for the same Input
/// (`ConfigError::DeepStageWithoutPrimary`) and a matching entry in
/// `deep_stages` (`ConfigError::DeepStageMissingConfig`).
#[serde(default, skip_serializing_if = "HashMap::is_empty")]
pub deep_base: HashMap<Input, Binding>,
/// `deep_base`'s exact mirror for the Held Layer.
#[serde(default, skip_serializing_if = "HashMap::is_empty")]
pub deep_held: HashMap<Input, Binding>,
/// Deep-stage Actuation point + staging mode, per-Input per-Profile, shared
/// across Base and Held (Q8 — it interprets physical travel, like
/// `default_actuation`/`actuation_overrides`). Legal with no matching
/// `deep_base`/`deep_held` entry (an unused, inert deep-stage config, same
/// shape as an `actuation_overrides` entry on a key with no primary
/// Binding) — illegal the other way around (§4).
#[serde(default, skip_serializing_if = "HashMap::is_empty")]
pub deep_stages: HashMap<Input, DeepStageConfig>,
```

`Profile` gains `deep_layer`/`deep_layer_mut`, exact mirrors of `layer`/`layer_mut`:

```rust
pub fn deep_layer(&self, layer: Layer) -> &HashMap<Input, Binding> {
    match layer {
        Layer::Base => &self.deep_base,
        Layer::Held => &self.deep_held,
    }
}

pub fn deep_layer_mut(&mut self, layer: Layer) -> &mut HashMap<Input, Binding> {
    match layer {
        Layer::Base => &mut self.deep_base,
        Layer::Held => &mut self.deep_held,
    }
}
```

No `schema_version` bump — every field is `#[serde(default)]`/an empty-default map, following
the codebase's unbroken precedent (status-LEDs, ticket 17/18/51/54): a pre-feature
`config.toml` has none of these keys and parses with `deep_base`/`deep_held`/`deep_stages` all
empty, which *is* "no key has a deep stage."

### 2. `profile_all_binding_sites` extension

`profile_all_binding_sites` (`config.rs:1298`) chains in `deep_base`/`deep_held` as a third
`Individual(input)` source (same `BindingSite`, not a new variant — a deep Binding is legal
only on a Grid Input, same shape as any other individual Binding):

```rust
let deep_individual = profile
    .deep_base
    .iter()
    .chain(profile.deep_held.iter())
    .map(|(input, b)| (BindingSite::Individual(*input), b));
individual.chain(deep_individual).chain(chords)
```

This means `check_binding` (post-release ticket 14's seam) applies the *exact same*
payload/trigger/site-shape rules to a deep Binding as to a primary one, for free — Q10's
"each stage is a full Binding, all existing per-Action validation applies to each stage
independently" falls out of this without a new rule. `check_binding`'s own
`Individual`-AnalogRepeat rule already permits AnalogRepeat on a Grid Individual — the
*ban* on AnalogRepeat for a dual-stage key (§4) is a cross-field rule this extension doesn't
cover, and stays in `validate`.

### 3. `ConfigError` additions — seven new variants

Ordered as `check_binding` orders its own three rules: structural/input-shape first, then the
two internal-hysteresis-style checks, then the two dangling-reference checks, then the two
mode/trigger cross-field bans. Locus is always just the `Input`'s `Display` string — no Layer
in the message, matching every existing locus (`AxisBindingConflict`, `ReleaseNotBelowActuation`
etc. never name the Layer either, despite being just as Layer-scoped).

```rust
/// A `deep_base`/`deep_held`/`deep_stages` entry keyed by an `Input` that
/// isn't a `Grid` variant — only grid keys have Depth for a deep stage to
/// threshold against, same reasoning as `InvalidAxisInput`/
/// `InvalidActuationOverrideInput`.
InvalidDeepStageInput(String),
/// A `deep_stages` entry whose own `release` is not strictly below its own
/// `actuation` — the deep band's internal hysteresis, `ReleaseNotBelow
/// Actuation`'s exact concern one level deeper. A dedicated variant, not a
/// reuse of `ReleaseNotBelowActuation`, so the message doesn't conflate
/// "your primary override is backwards" with "your deep stage's own pair is
/// backwards" on the same key.
DeepStageReleaseNotBelowActuation(String),
/// A `deep_stages` entry whose `release` is not strictly greater than the
/// same Input's resolved primary `actuation` (Q4's disjoint-and-stacked
/// constraint: `deep.release > primary.actuation`) — the two bands overlap.
DeepStageBandOverlapsPrimary(String),
/// A `deep_base`/`deep_held` Binding with no matching Binding in `base`/
/// `held` (same Input, same Layer) — Q6: "no primary ⇒ no deep stage."
DeepStageWithoutPrimary(String),
/// A `deep_base`/`deep_held` Binding with no matching entry in
/// `deep_stages` for that Input — a deep stage with a Binding but no
/// Actuation point or staging mode to run it against.
DeepStageMissingConfig(String),
/// A Binding — primary *or* deep, on either Layer — using `analog_repeat`
/// on an Input that has a functioning deep stage (a `deep_base`/
/// `deep_held` entry on that Layer) — Q10's v1 restriction: the Analog-repeat
/// background task ignores Actuation points and would fight the staging
/// logic.
AnalogRepeatOnDualStageKey(String),
/// An Input that is both a Chord member (on some Layer) and carries a
/// `deep_base`/`deep_held` Binding on that same Layer — Q11: a Chord member
/// cannot have a deep stage in v1.
ChordMemberDeepStageConflict(String),
```

`Display` strings:

```rust
ConfigError::InvalidDeepStageInput(input) => write!(
    f, "a deep stage on {input:?} is not allowed — only Grid Inputs can carry one"
),
ConfigError::DeepStageReleaseNotBelowActuation(input) => write!(
    f, "{input:?}'s deep stage release point is not below its own actuation point"
),
ConfigError::DeepStageBandOverlapsPrimary(input) => write!(
    f, "{input:?}'s deep stage release point must be strictly greater than its primary actuation point — the two bands must not overlap"
),
ConfigError::DeepStageWithoutPrimary(input) => write!(
    f, "{input:?} has a deep stage Binding on a Layer with no primary Binding there"
),
ConfigError::DeepStageMissingConfig(input) => write!(
    f, "{input:?} has a deep stage Binding but no deep Actuation point / staging mode configured"
),
ConfigError::AnalogRepeatOnDualStageKey(input) => write!(
    f, "{input:?} cannot use analog_repeat on either stage while it has a deep stage"
),
ConfigError::ChordMemberDeepStageConflict(input) => write!(
    f, "{input:?} is a Chord member and cannot also carry a deep stage"
),
```

### 4. `config::validate` additions

Appended after the existing `EmptyProfileName` check, right before the final `Ok(())` —
"ticket 03 additions," matching the file's own "ticket 04 additions" convention for undisturbed
existing tests:

1. **`InvalidDeepStageInput`** — any key of `deep_base`, `deep_held`, or `deep_stages` that
   isn't `Input::Grid(_, _)`.
2. **`DeepStageReleaseNotBelowActuation`** — any `deep_stages` value with
   `release >= actuation`.
3. **`DeepStageBandOverlapsPrimary`** — any `deep_stages` entry whose `release` is
   `<= profile.resolved_actuation_point(input).actuation`.
4. **`DeepStageWithoutPrimary`** — for each Layer, any `deep_layer(layer)` key absent from
   `layer(layer)`.
5. **`DeepStageMissingConfig`** — any Input present in `deep_base` or `deep_held` absent from
   `deep_stages`.
6. **`AnalogRepeatOnDualStageKey`** — for each Layer where `deep_layer(layer)` has a Binding
   for `input`: if that deep Binding's trigger is `AnalogRepeat`, **or** if `layer(layer)`'s
   primary Binding for `input` (if any) has trigger `AnalogRepeat` → error.
7. **`ChordMemberDeepStageConflict`** — for each Layer, any Input in `deep_layer(layer)` that
   is also a member of some Chord in `chords(layer)`.

Each is a `find_map` over `config.profiles.values()` in the existing style, one `if let
Some(locus) = ... { return Err(...) }` per rule.

### 5. `Edit` surface — four new variants, `daemon/src/edit.rs`

Granular, mirroring the primary stage's own `SetBinding`/`SetActuationPoint` split exactly —
not one combined call — so the GUI (ticket 04) can reuse the existing drag-the-Actuation-bar
and pick-a-Binding interaction patterns independently for the deep slot.

```rust
/// Creates or edits the deep-stage Binding on the active Profile's `layer`
/// (Q8: deep Binding is per-Layer). Mirrors `SetBinding` exactly, one level
/// deeper. `config::validate` (not an inline check) rejects a deep Binding
/// with no matching primary Binding (`DeepStageWithoutPrimary`) or no
/// matching `deep_stages` entry (`DeepStageMissingConfig`) — this Edit does
/// not reach into `base`/`held` or `deep_stages` itself; sequencing across
/// the three pieces is the caller's (GUI's) job, same as `SetAxisAssignment`
/// leaves "was there already a Binding here" to `config::validate`'s
/// reachable states, not a precondition this checks.
SetDeepStage {
    input: Input,
    layer: Layer,
    binding: Binding,
},
/// Removes the deep-stage Binding. Fails `NotFound` if `input` has no deep
/// Binding on `layer`. Does **not** cascade-clear `deep_stages` or force-
/// release a live deep-stage slot — ticket 05's job (Q6, this ticket's
/// grilling round).
ClearDeepStage { input: Input, layer: Layer },
/// Sets the deep-stage Actuation/Release point for `input` (per-Profile,
/// shared Base/Held — Q8), creating a fresh `DeepStageConfig` with
/// `mode: StagingMode::default()` if none exists yet. No `Effect` — unlike
/// `SetActuationPoint`, nothing needs a live snapshot pushed to it: the
/// deep-band engine (`stage::Engine`, ticket 01) lives in dispatch and reads
/// `Config` directly each tick, never a republished copy.
SetDeepActuation {
    input: Input,
    actuation: u8,
    release: u8,
},
/// Sets the staging mode for `input`, creating a fresh `DeepStageConfig`
/// with `actuation: ActuationPoint::default()` if none exists yet. No
/// `Effect`, same reasoning as `SetDeepActuation`.
SetStagingMode { input: Input, mode: StagingMode },
```

`plan` arms (all rely on the trailing `config::validate(&next)?` for every structural rule,
exactly like `SetActuationPoint`/`SetBinding` do today — no inline checks):

```rust
Edit::SetDeepStage { input, layer, binding } => {
    active_profile_mut(&mut next).deep_layer_mut(layer).insert(input, binding);
}
Edit::ClearDeepStage { input, layer } => {
    if active_profile_mut(&mut next).deep_layer_mut(layer).remove(&input).is_none() {
        return Err(CommandError::NotFound);
    }
}
Edit::SetDeepActuation { input, actuation, release } => {
    active_profile_mut(&mut next)
        .deep_stages
        .entry(input)
        .or_default()
        .actuation = ActuationPoint { actuation, release };
}
Edit::SetStagingMode { input, mode } => {
    active_profile_mut(&mut next).deep_stages.entry(input).or_default().mode = mode;
}
```

### 6. D-Bus methods — `daemon/src/dbus/mod.rs`

Four thin wrappers, exactly the `set_axis_assignment`/`set_actuation_point` shape (parse wire
args, build the `Edit`, `self.apply(...)`):

```rust
async fn set_deep_stage(
    &self, input: String, layer: String, binding: HashMap<String, OwnedValue>,
) -> Result<(), DaemonError> {
    let input = Self::parse_input(&input)?;
    let layer = wire::layer_from_str(&layer).map_err(DaemonError::InvalidBinding)?;
    let binding = wire::binding_from_dict(&binding).map_err(DaemonError::InvalidBinding)?;
    self.apply(Edit::SetDeepStage { input, layer, binding }).await
}

async fn clear_deep_stage(&self, input: String, layer: String) -> Result<(), DaemonError> {
    let input = Self::parse_input(&input)?;
    let layer = wire::layer_from_str(&layer).map_err(DaemonError::InvalidBinding)?;
    self.apply(Edit::ClearDeepStage { input, layer }).await
}

async fn set_deep_actuation(&self, input: String, actuation: u8, release: u8) -> Result<(), DaemonError> {
    let input = Self::parse_input(&input)?;
    self.apply(Edit::SetDeepActuation { input, actuation, release }).await
}

async fn set_staging_mode(&self, input: String, mode: String) -> Result<(), DaemonError> {
    let input = Self::parse_input(&input)?;
    let mode = wire::staging_mode_from_str(&mode).map_err(DaemonError::InvalidBinding)?;
    self.apply(Edit::SetStagingMode { input, mode }).await
}
```

Plus the four matching `#[zbus(proxy)]` trait entries alongside `set_axis_assignment` etc.

### 7. Wire encoding — `daemon/src/dbus/wire.rs`

`StagingMode` marshals as its own flat lowercase string, exactly `axis_target_str`'s
convention:

```rust
pub fn staging_mode_str(mode: StagingMode) -> &'static str {
    match mode {
        StagingMode::Handoff => "handoff",
        StagingMode::NoReturn => "no_return",
        StagingMode::Additive => "additive",
        StagingMode::QuickSkip => "quick_skip",
    }
}

pub fn staging_mode_from_str(s: &str) -> Result<StagingMode, String> {
    match s {
        "handoff" => Ok(StagingMode::Handoff),
        "no_return" => Ok(StagingMode::NoReturn),
        "additive" => Ok(StagingMode::Additive),
        "quick_skip" => Ok(StagingMode::QuickSkip),
        other => Err(format!("{other:?} is not a valid staging mode")),
    }
}
```

`DeepStageConfig` marshals as a flat dict bundling its own `ActuationPoint` sub-dict plus the
mode string (`actuation_point_to_dict`'s bundle-related-scalars convention, one level nested
since `DeepStageConfig` itself bundles two things):

```rust
fn deep_stage_config_to_dict(cfg: DeepStageConfig) -> Dict {
    let mut dict = Dict::new();
    dict.insert("actuation".to_string(), scalar(actuation_point_to_dict(cfg.actuation)));
    dict.insert("mode".to_string(), scalar(staging_mode_str(cfg.mode).to_string()));
    dict
}

fn deep_stages_to_dict(stages: &HashMap<Input, DeepStageConfig>) -> Dict {
    stages
        .iter()
        .map(|(input, cfg)| (input.to_string(), scalar(deep_stage_config_to_dict(*cfg))))
        .collect()
}
```

`profile_to_dict` (`wire.rs:391`) gains three entries after `status_leds`:

```rust
dict.insert("deep_base".to_string(), scalar(bindings_to_dict(&profile.deep_base)));
dict.insert("deep_held".to_string(), scalar(bindings_to_dict(&profile.deep_held)));
dict.insert("deep_stages".to_string(), scalar(deep_stages_to_dict(&profile.deep_stages)));
```

### 8. `parse` test

`a_pre_dual_stage_config_defaults_deep_fields` — mirroring
`a_pre_status_led_config_defaults_status_leds` / `a_pre_ticket_17_config_defaults_actuation_
fields_and_force_digital`: a minimal `schema_version = 1` file with a Profile that has only
`base` set must parse with `deep_base`/`deep_held`/`deep_stages` all empty.

### 9. Sample `config.toml` fragment

```toml
[profiles.Default]
default_actuation = { actuation = 128, release = 112 }

[profiles.Default.base.grid_r1c1]
trigger = "hold_to_repeat"
type = "keypress"
key = "KEY_W"

[profiles.Default.deep_base.grid_r1c1]
trigger = "fire_once"
type = "keypress"
key = "KEY_LSHIFT"

[profiles.Default.deep_stages.grid_r1c1]
actuation = { actuation = 220, release = 200 }
mode = "handoff"
```

(`deep.release` = 200 > `primary.actuation` = 128 — disjoint and stacked, satisfying
`DeepStageBandOverlapsPrimary`; `deep.release` 200 < `deep.actuation` 220, satisfying
`DeepStageReleaseNotBelowActuation`.)

### 10. `GetState()` / `active_toggles` — no change

Confirmed no addition (Q7). The live Depth bar (4 markers: primary release/actuation, deep
release/actuation) is the runtime indicator; `active_toggles` stays `Input`-scoped, covering
only primary Toggles, unchanged.

### 11. GUI mirror

**`daemon_client.py`** — four new methods + `Protocol` stubs, mechanical mirror of
`set_axis_assignment`/`set_actuation_point`:

```python
def set_deep_stage(self, input_str: str, layer: str, binding: dict) -> None: ...
def clear_deep_stage(self, input_str: str, layer: str) -> None: ...
def set_deep_actuation(self, input_str: str, actuation: int, release: int) -> None: ...
def set_staging_mode(self, input_str: str, mode: str) -> None: ...
```

**`daemon_stub.py`** — matching methods, following `set_chord_binding`/`set_actuation_point`'s
existing shape (guard-clause checks *before* mutating, raising `InvalidBindingError`/
`NotFoundError` — the stub's own idiom, mirror of intent rather than byte-identical to the
Rust `Display` strings): a Grid-input check (`InvalidDeepStageInput`), the deep-pair
hysteresis check (`DeepStageReleaseNotBelowActuation`), the disjoint-band check against the
resolved primary point (`DeepStageBandOverlapsPrimary`) for `set_deep_actuation`; `_validate_
binding_action` reused as-is for `set_deep_stage` (same pure rules `check_binding` runs,
already contract-tested via `rules.py`); the two dangling checks
(`DeepStageWithoutPrimary` on `set_deep_stage`, `DeepStageMissingConfig` — a stub-side
choice: raise it if `set_deep_stage` is called before any `deep_stages` entry exists for that
Input) plus `AnalogRepeatOnDualStageKey`/`ChordMemberDeepStageConflict` as `_reject_if_*`-style
helper checks, following `_reject_if_axis_assigned`'s existing pattern. Seed Profile dict gains
`"deep_base": {}`, `"deep_held": {}`, `"deep_stages": {}`.

**`wire.py`/`read_model.py`** — surface `deep_base`/`deep_held`/`deep_stages` in the config
dict the GUI reads, mirroring `profile_to_dict`'s three new entries.

**`rules.py` — nothing new** (Q8, confirmed). All seven new rules are whole-`Config`/cross-map
checks, not pure functions of one Binding — they land in `daemon_stub.py` only, closing the
"rules.py mirror detail" fog patch. The one thing `rules.py` already covers for free: a deep
Binding's own payload/trigger legality (`valid_action_kinds`/`valid_triggers`), reused
unchanged via `_validate_binding_action`, same as every other Binding site.

### 12. Corrections / fog graduated

- The map's **"`rules.py` mirror detail"** fog patch graduates fully into §11 above — nothing
  lands in `rules.py`.
- The map's **"Exact validation-error strings"** fog patch graduates into §3 above.
- No new `CONTEXT.md` term or ADR from this ticket — `Dual-stage keys`/staging-mode glossary
  entries land with [ticket 06](./06-write-dual-stage-spec.md), as already planned.
- Nothing here re-opens ticket 01 or ticket 02; the `stage::Engine`'s `resolved_deep_
  actuation_point(input)` accessor ticket 01 named now has a concrete home:
  `profile.deep_stages.get(&input).map(|c| c.actuation)` (`None` = no deep stage on this
  Input at all).
