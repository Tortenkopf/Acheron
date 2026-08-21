//! The config-facing domain model and `config.toml` lifecycle (ticket 14).
//!
//! Every `Action`/`TriggerMode` variant's schema shape was already fully
//! decided in issue 06; ticket 17 wired all of them (`Action::Macro`,
//! `TriggerMode::HoldToRepeat`/`Toggle`) up to actually fire, via
//! `executor::compile` and the shared executor dispatch.rs runs firings
//! through. Ticket 18 added the `Layer` enum and each Profile's `held`
//! Binding map alongside `base`, plus the per-Profile `mode_key_role`.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use evdev::KeyCode;
use serde::{Deserialize, Serialize};

use crate::input::Input;

pub const SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_PROFILE_NAME: &str = "Default";

/// The Daemon's single config file, per issue 03: `~/.config/acheron/config.toml`.
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .expect("no config directory available on this platform")
        .join("acheron")
        .join("config.toml")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    pub active_profile: String,
    pub profiles: HashMap<String, Profile>,
    /// A hardware/troubleshooting-level override that forces Digital Capture
    /// mode even when Analog would otherwise unlock (ticket 17 §4) — a
    /// `Config`-level preference, not per-Profile, distinct from
    /// `command::State::capture_mode`'s live-reported actual mode.
    #[serde(default)]
    pub force_digital: bool,
}

impl Config {
    /// The seed config written when `config.toml` is missing, per issue 11:
    /// one `Default` Profile, empty Base-layer Binding map, set active.
    pub fn seed() -> Self {
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), Profile::default());
        Config {
            schema_version: SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
        }
    }

    pub fn active_profile(&self) -> Option<&Profile> {
        self.profiles.get(&self.active_profile)
    }

    pub fn active_profile_mut(&mut self) -> Option<&mut Profile> {
        let name = self.active_profile.clone();
        self.profiles.get_mut(&name)
    }
}

/// CONTEXT.md: Layer — closed 2-variant enum, `Base`/`Held`. Every Profile
/// always has both present at the type level (fixed hardware fact — one
/// Mode key), each with its own sparse Binding map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    Base,
    Held,
}

impl Layer {
    /// The flat lowercase string form used both for `ActiveLayerChanged`'s
    /// payload and the D-Bus wire's Layer argument (issue 08).
    pub const fn as_str(self) -> &'static str {
        match self {
            Layer::Base => "base",
            Layer::Held => "held",
        }
    }
}

/// A per-Profile switch (ticket 18) deciding what the Mode key does:
/// `LayerSwitch` (default) makes it a momentary Hypershift-style Layer
/// activator, intercepted by the dispatch task before any Binding lookup;
/// `Bound` routes it through the identical `(Layer, Input) -> Binding`
/// lookup and Trigger-mode dispatch as any other Input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeKeyRole {
    #[default]
    LayerSwitch,
    Bound,
}

/// A Profile's Base- and Held-layer Bindings (CONTEXT.md: Profile, Layer).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Sparse map keyed by `Input`; an absent entry means passthrough.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub base: HashMap<Input, Binding>,
    /// Retained (not deleted) even while `mode_key_role` is `Bound` makes it
    /// unreachable — flipping back to `LayerSwitch` must not lose these
    /// (ticket 18).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub held: HashMap<Input, Binding>,
    #[serde(default)]
    pub mode_key_role: ModeKeyRole,
    /// The Actuation/Release point every Grid key uses unless it has its own
    /// entry in `actuation_overrides` (ticket 17 §5) — per-Input, per-Profile,
    /// shared across Base and Held (an actuation point describes the key's
    /// physical travel, not what it does when triggered).
    #[serde(default)]
    pub default_actuation: ActuationPoint,
    /// Sparse per-key overrides of `default_actuation`, keyed by `Input`
    /// (always a `Grid` variant — enforced at the `Command` layer, not the
    /// type level, per ticket 17 §3).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub actuation_overrides: HashMap<Input, ActuationPoint>,
}

impl Profile {
    /// The Actuation/Release point every one of the 20 Grid keys resolves to
    /// right now — `actuation_overrides` where set, `default_actuation`
    /// otherwise (ticket 18 §5). Dispatch publishes this into a
    /// `tokio::sync::watch` channel on every mutation that touches either
    /// field, so the analog capture source's grid task can threshold against
    /// current values without reading `Config` itself (single ownership).
    pub fn resolved_actuation_points(&self) -> HashMap<Input, ActuationPoint> {
        let mut resolved = HashMap::with_capacity(20);
        for row in 1..=4u8 {
            for col in 1..=5u8 {
                let input = Input::Grid(row, col);
                let point = self
                    .actuation_overrides
                    .get(&input)
                    .copied()
                    .unwrap_or(self.default_actuation);
                resolved.insert(input, point);
            }
        }
        resolved
    }

    pub fn layer(&self, layer: Layer) -> &HashMap<Input, Binding> {
        match layer {
            Layer::Base => &self.base,
            Layer::Held => &self.held,
        }
    }

    pub fn layer_mut(&mut self, layer: Layer) -> &mut HashMap<Input, Binding> {
        match layer {
            Layer::Base => &mut self.base,
            Layer::Held => &mut self.held,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub trigger: TriggerMode,
    pub action: Action,
}

/// CONTEXT.md: Actuation point, Release point. A Grid key's Binding fires a
/// Down at `actuation` and an Up at `release` — hysteresis, so a single
/// Depth boundary doesn't chatter (ticket 17 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActuationPoint {
    pub actuation: u8,
    pub release: u8,
}

/// 128/112 (half-travel, 16-point margin) is an explicit placeholder, not a
/// tuned value — ticket 19 is where it gets felt through a real UI and
/// adjusted (ticket 17 §5).
impl Default for ActuationPoint {
    fn default() -> Self {
        ActuationPoint {
            actuation: 128,
            release: 112,
        }
    }
}

/// CONTEXT.md: Trigger mode. `FireOnce`/`HoldToRepeat`/`Toggle` firing
/// semantics all live in `dispatch::fire` and `executor` (ticket 17).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerMode {
    FireOnce,
    HoldToRepeat,
    Toggle,
}

/// CONTEXT.md: Action. `Keypress`/`Macro` compile into the shared executor's
/// `Vec<executor::MacroStep>` (ticket 17's `executor::compile`);
/// `ProfileSwitch` (ticket 34) has no `MacroStep` form at all — it's
/// intercepted in `dispatch::handle_event` before `compile` is ever called,
/// the same way `Command::SwitchProfile` mutates `Config` directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Keypress {
        #[serde(default)]
        modifiers: Modifiers,
        key: KeyCode,
    },
    Macro {
        steps: Vec<MacroStepDto>,
    },
    /// Switches the active Profile when fired (ticket 05/34). Validated
    /// (`SetBinding` and `load_or_seed`, via `parse`'s
    /// `InvalidProfileSwitchTrigger` check) to only ever pair with
    /// `TriggerMode::FireOnce` — a held/toggled Profile switch has no
    /// coherent meaning.
    ProfileSwitch {
        target: String,
    },
}

/// A Keypress's modifier chord (e.g. Ctrl+Shift+T). Per issue 06: ctrl,
/// shift, alt, super — implemented as plain flags rather than pulling in
/// the `bitflags` crate for four booleans.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modifiers {
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default, rename = "super")]
    pub super_key: bool,
}

/// `Action::Macro`'s config-facing step DTO, per issue 06 — keyboard-only
/// for MVP. `executor::compile` turns this into the runtime `MacroStep` the
/// shared executor actually walks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacroStepDto {
    KeyDown(KeyCode),
    KeyUp(KeyCode),
    #[serde(rename = "delay_ms")]
    Delay(u64),
}

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Parse(toml::de::Error),
    MissingSchemaVersion,
    InvalidSchemaVersion(String),
    UnsupportedSchemaVersion(i64),
    InvalidActiveProfile(String),
    InvalidProfileSwitchTrigger,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(err) => write!(f, "I/O error: {err}"),
            ConfigError::Parse(err) => write!(f, "failed to parse config.toml: {err}"),
            ConfigError::MissingSchemaVersion => {
                write!(
                    f,
                    "config.toml is missing the required schema_version field"
                )
            }
            ConfigError::InvalidSchemaVersion(repr) => write!(
                f,
                "config.toml's schema_version is not a valid integer: {repr}"
            ),
            ConfigError::UnsupportedSchemaVersion(version) => write!(
                f,
                "config.toml has unsupported schema_version {version} (expected {SCHEMA_VERSION})"
            ),
            ConfigError::InvalidActiveProfile(name) => write!(
                f,
                "config.toml's active_profile {name:?} does not name a profile in [profiles]"
            ),
            ConfigError::InvalidProfileSwitchTrigger => write!(
                f,
                "config.toml contains an Action::ProfileSwitch Binding whose trigger is not fire_once"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Loads `config.toml` from `path`, or seeds it (creating the parent
/// directory and writing the file immediately) if it doesn't exist yet, per
/// issue 11. A file that fails to parse or names an unsupported
/// `schema_version` is refused rather than overwritten or backed up.
pub fn load_or_seed(path: &Path) -> Result<Config, ConfigError> {
    match fs::read_to_string(path) {
        Ok(contents) => parse(&contents),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let config = Config::seed();
            write(path, &config)?;
            Ok(config)
        }
        Err(err) => Err(ConfigError::Io(err)),
    }
}

fn parse(contents: &str) -> Result<Config, ConfigError> {
    // Checked separately (and first) so a version mismatch is reported as
    // such, rather than surfacing as whatever generic deserialize error a
    // shape change happens to produce.
    let value: toml::Value = toml::from_str(contents).map_err(ConfigError::Parse)?;
    match value.get("schema_version") {
        None => return Err(ConfigError::MissingSchemaVersion),
        Some(raw) => match raw.as_integer() {
            Some(version) if version == i64::from(SCHEMA_VERSION) => {}
            Some(version) => return Err(ConfigError::UnsupportedSchemaVersion(version)),
            None => return Err(ConfigError::InvalidSchemaVersion(format!("{raw:?}"))),
        },
    }

    let config: Config = toml::from_str(contents).map_err(ConfigError::Parse)?;
    if !config.profiles.contains_key(&config.active_profile) {
        return Err(ConfigError::InvalidActiveProfile(config.active_profile));
    }
    let has_invalid_profile_switch_trigger = config.profiles.values().any(|profile| {
        [&profile.base, &profile.held].into_iter().any(|bindings| {
            bindings.values().any(|binding| {
                matches!(binding.action, Action::ProfileSwitch { .. })
                    && binding.trigger != TriggerMode::FireOnce
            })
        })
    });
    if has_invalid_profile_switch_trigger {
        return Err(ConfigError::InvalidProfileSwitchTrigger);
    }
    Ok(config)
}

/// Rewrites `config.toml` in full — the only persistence path, used both for
/// the initial seed and for every live D-Bus mutation (ticket 15), so
/// `config.toml` on disk always matches in-memory state immediately.
pub(crate) fn write(path: &Path, config: &Config) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(ConfigError::Io)?;
    }
    let contents = toml::to_string_pretty(config).expect("Config always serializes to TOML");
    fs::write(path, contents).map_err(ConfigError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Input;

    fn temp_config_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acheron").join("config.toml");
        (dir, path)
    }

    #[test]
    fn seeds_a_missing_config_file_immediately() {
        let (_dir, path) = temp_config_path();
        assert!(!path.exists());

        let config = load_or_seed(&path).expect("seeding must succeed");

        assert_eq!(config, Config::seed());
        assert!(path.exists(), "seed file must be written immediately");

        let on_disk = fs::read_to_string(&path).unwrap();
        let reparsed: Config = toml::from_str(&on_disk).unwrap();
        assert_eq!(reparsed, config);
    }

    #[test]
    fn seed_config_has_one_empty_active_default_profile() {
        let config = Config::seed();
        assert_eq!(config.schema_version, SCHEMA_VERSION);
        assert_eq!(config.active_profile, "Default");
        let default = config.active_profile().expect("Default must be active");
        assert!(default.base.is_empty());
        assert!(default.held.is_empty());
        assert_eq!(default.mode_key_role, ModeKeyRole::LayerSwitch);
    }

    #[test]
    fn refuses_to_start_on_corrupt_toml_without_touching_the_file() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "this is not valid toml {{{").unwrap();

        let err = load_or_seed(&path).expect_err("corrupt file must refuse to start");
        assert!(matches!(err, ConfigError::Parse(_)));

        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "this is not valid toml {{{");
    }

    #[test]
    fn refuses_to_start_on_unsupported_schema_version() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = "schema_version = 2\nactive_profile = \"Default\"\n\n[profiles.Default]\n";
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path).expect_err("unsupported version must refuse to start");
        assert!(matches!(err, ConfigError::UnsupportedSchemaVersion(2)));

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn reports_a_non_integer_schema_version_distinctly_from_a_missing_one() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "schema_version = \"1\"\nactive_profile = \"Default\"\n\n[profiles.Default]\n",
        )
        .unwrap();

        let err = load_or_seed(&path).expect_err("non-integer version must refuse to start");
        assert!(matches!(err, ConfigError::InvalidSchemaVersion(_)));
    }

    #[test]
    fn refuses_to_start_when_active_profile_is_unknown() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "schema_version = 1\nactive_profile = \"Nonexistent\"\n\n[profiles.Default]\n",
        )
        .unwrap();

        let err = load_or_seed(&path).expect_err("unknown active_profile must refuse to start");
        assert!(matches!(err, ConfigError::InvalidActiveProfile(name) if name == "Nonexistent"));
    }

    #[test]
    fn parses_the_spec_sample_binding_shape() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let binding = &config.profiles["Default"].base[&Input::Grid(1, 1)];
        assert_eq!(binding.trigger, TriggerMode::FireOnce);
        assert_eq!(
            binding.action,
            Action::Keypress {
                modifiers: Modifiers::default(),
                key: KeyCode::KEY_F1,
            }
        );
    }

    #[test]
    fn parses_a_keypress_with_modifiers() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r2c2]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_T", modifiers = { ctrl = true, shift = true } }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let binding = &config.profiles["Default"].base[&Input::Grid(2, 2)];
        assert_eq!(
            binding.action,
            Action::Keypress {
                modifiers: Modifiers {
                    ctrl: true,
                    shift: true,
                    alt: false,
                    super_key: false,
                },
                key: KeyCode::KEY_T,
            }
        );
    }

    #[test]
    fn parses_a_held_layer_binding_and_mode_key_role() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default]
mode_key_role = "bound"

[profiles.Default.held.grid_r2c1]
trigger = "toggle"
action = { type = "macro", steps = [
  { key_down = "KEY_A" }, { delay_ms = 50 }, { key_up = "KEY_A" },
] }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let profile = &config.profiles["Default"];
        assert_eq!(profile.mode_key_role, ModeKeyRole::Bound);
        assert!(profile.base.is_empty());
        let binding = &profile.held[&Input::Grid(2, 1)];
        assert_eq!(binding.trigger, TriggerMode::Toggle);
    }

    #[test]
    fn a_profile_missing_held_and_mode_key_role_defaults_both() {
        // A hand-written or pre-ticket-18 file that only sets `base` must
        // still parse, defaulting the new fields rather than refusing to
        // start (issue 06's "sparse" data model / no forced migration).
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let profile = &config.profiles["Default"];
        assert!(profile.held.is_empty());
        assert_eq!(profile.mode_key_role, ModeKeyRole::LayerSwitch);
    }

    #[test]
    fn a_pre_ticket_17_config_defaults_actuation_fields_and_force_digital() {
        // A config.toml written before ticket 17 has no `default_actuation`,
        // `actuation_overrides`, or `force_digital` at all — it must still
        // parse unchanged, defaulting all three (ticket 17 §6: additive,
        // no schema_version bump).
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(!config.force_digital);
        let profile = &config.profiles["Default"];
        assert_eq!(profile.default_actuation, ActuationPoint::default());
        assert_eq!(
            profile.default_actuation,
            ActuationPoint {
                actuation: 128,
                release: 112,
            }
        );
        assert!(profile.actuation_overrides.is_empty());
    }

    #[test]
    fn resolved_actuation_points_covers_all_20_grid_keys_default_and_overridden() {
        let mut profile = Profile {
            default_actuation: ActuationPoint {
                actuation: 100,
                release: 80,
            },
            ..Default::default()
        };
        profile.actuation_overrides.insert(
            Input::Grid(2, 3),
            ActuationPoint {
                actuation: 200,
                release: 190,
            },
        );

        let resolved = profile.resolved_actuation_points();

        assert_eq!(resolved.len(), 20);
        assert_eq!(
            resolved[&Input::Grid(2, 3)],
            ActuationPoint {
                actuation: 200,
                release: 190,
            }
        );
        assert_eq!(
            resolved[&Input::Grid(1, 1)],
            ActuationPoint {
                actuation: 100,
                release: 80,
            }
        );
    }

    #[test]
    fn parses_a_fire_once_profile_switch_binding() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "profile_switch", target = "Gaming" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let binding = &config.profiles["Default"].base[&Input::Grid(1, 1)];
        assert_eq!(
            binding.action,
            Action::ProfileSwitch {
                target: "Gaming".to_string(),
            }
        );
    }

    #[test]
    fn refuses_to_start_when_a_profile_switch_binding_is_not_fire_once() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "toggle"
action = { type = "profile_switch", target = "Gaming" }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path).expect_err("a Toggle Profile Switch must refuse to start");
        assert!(matches!(err, ConfigError::InvalidProfileSwitchTrigger));

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn held_layer_bindings_survive_a_full_write_and_reparse_round_trip() {
        let (_dir, path) = temp_config_path();
        let mut config = Config::seed();
        let profile = config.active_profile_mut().unwrap();
        profile.mode_key_role = ModeKeyRole::Bound;
        profile.held.insert(
            Input::Grid(2, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action: Action::Macro {
                    steps: vec![
                        MacroStepDto::KeyDown(KeyCode::KEY_A),
                        MacroStepDto::Delay(50),
                        MacroStepDto::KeyUp(KeyCode::KEY_A),
                    ],
                },
            },
        );

        write(&path, &config).unwrap();
        let reparsed = load_or_seed(&path).unwrap();

        assert_eq!(reparsed, config);
    }
}
