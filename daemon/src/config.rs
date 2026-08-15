//! The config-facing domain model and `config.toml` lifecycle (ticket 14).
//!
//! Scope is deliberately narrow: one Profile (`Default`), Base Layer only
//! (no `Layer` enum/Held layer yet — that's ticket 18's concern). Every
//! `Action`/`TriggerMode` variant's schema shape was already fully decided
//! in issue 06; ticket 17 wired all of them (`Action::Macro`,
//! `TriggerMode::HoldToRepeat`/`Toggle`) up to actually fire, via
//! `executor::compile` and the shared executor dispatch.rs runs firings
//! through.

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

/// A Profile's Base-layer Bindings (CONTEXT.md: Profile, Layer). Held layer
/// is out of scope for this ticket (issue 18).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Sparse map keyed by `Input`; an absent entry means passthrough.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub base: HashMap<Input, Binding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub trigger: TriggerMode,
    pub action: Action,
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

/// CONTEXT.md: Action. Both variants compile into the shared executor's
/// `Vec<executor::MacroStep>` (ticket 17's `executor::compile`).
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
}
