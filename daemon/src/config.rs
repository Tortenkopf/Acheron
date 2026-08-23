//! The config-facing domain model and `config.toml` lifecycle (ticket 14).
//!
//! Every `Action`/`TriggerMode` variant's schema shape was already fully
//! decided in issue 06; ticket 17 wired all of them (`Action::Macro`,
//! `TriggerMode::HoldToRepeat`/`Toggle`) up to actually fire, via
//! `executor::compile` and the shared executor dispatch.rs runs firings
//! through. Ticket 18 added the `Layer` enum and each Profile's `held`
//! Binding map alongside `base`, plus the per-Profile `mode_key_role`.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

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
    /// The global, named Macro library (ticket 15/51 — CONTEXT.md: Macro).
    /// One shared map across every Profile; an `Action::Macro { macro_id }`
    /// Binding references an entry here rather than carrying its own step
    /// content, so any number of Bindings across any number of Profiles can
    /// reuse the same `MacroDef` at once. Additive/`#[serde(default)]` even
    /// though `Action::Macro`'s own shape change is a breaking one — a
    /// config with no Macros at all still parses.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub macros: HashMap<MacroId, MacroDef>,
    /// The global, named Stepper-list library (ticket 03/54 — CONTEXT.md:
    /// Stepper). One shared map across every Profile; unlike `macros`, at
    /// most one forward/backward Input pair may reference a given
    /// `StepperId` at a time — enforced by `SetBinding` silently moving a
    /// list off its old pair rather than rejecting (ticket 03's Answer), not
    /// by anything in this type. Purely additive — `Action::Step` is
    /// net-new, so a pre-ticket-54 `config.toml` has no `[steppers]` table
    /// at all and still parses.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub steppers: HashMap<StepperId, StepperDef>,
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
            macros: HashMap::new(),
            steppers: HashMap::new(),
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
    /// Chord Bindings active while this Profile's Base Layer is active
    /// (ticket 01/40 — CONTEXT.md: Chord): a `Binding` reused unchanged,
    /// keyed by the `Set<Input>` that must all be down together within the
    /// dispatch task's ~50ms window to fire it instead of any member's own
    /// individual Binding. Mirrors `base`/`held`'s own per-Layer split
    /// exactly — a Chord is "just a Binding keyed by a Set<Input> instead of
    /// one Input" (ticket 01's Answer), not a parallel concept.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub chords_base: HashMap<ChordKey, Binding>,
    /// `chords_base`'s exact mirror for the Held Layer.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub chords_held: HashMap<ChordKey, Binding>,
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

    /// `layer`'s exact mirror for a Profile's Chord Bindings (ticket 40).
    pub fn chords(&self, layer: Layer) -> &HashMap<ChordKey, Binding> {
        match layer {
            Layer::Base => &self.chords_base,
            Layer::Held => &self.chords_held,
        }
    }

    /// `layer_mut`'s exact mirror for a Profile's Chord Bindings.
    pub fn chords_mut(&mut self, layer: Layer) -> &mut HashMap<ChordKey, Binding> {
        match layer {
            Layer::Base => &mut self.chords_base,
            Layer::Held => &mut self.chords_held,
        }
    }
}

/// A Chord's membership key (ticket 01/40 — CONTEXT.md: Chord): the set of
/// physical Inputs that must all be down together within the dispatch
/// task's ~50ms window to fire this Chord's Binding instead of any member's
/// own individual one. `BTreeSet<Input>` per ticket 01's Answer — TOML has
/// no non-string map-key type, so (mirroring `Input`'s own hand-written
/// TOML string-key convention) this marshals as a `+`-joined, sorted string
/// of each member's own `Input` `Display` form (e.g.
/// `"grid_r1c1+grid_r1c2"`). Always at least 2 members — enforced by
/// `dispatch::handle_command`'s `SetChordBinding` handler, not the type
/// itself, matching how `ActuationPoint`'s hysteresis invariant is enforced
/// at the `Command` layer rather than baked into the struct.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ChordKey(BTreeSet<Input>);

impl ChordKey {
    pub fn new(members: BTreeSet<Input>) -> Self {
        ChordKey(members)
    }

    pub fn members(&self) -> &BTreeSet<Input> {
        &self.0
    }
}

impl fmt::Display for ChordKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let joined = self
            .0
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("+");
        write!(f, "{joined}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseChordKeyError(String);

impl fmt::Display for ParseChordKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a valid Chord key", self.0)
    }
}

impl std::error::Error for ParseChordKeyError {}

impl FromStr for ChordKey {
    type Err = ParseChordKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut members = BTreeSet::new();
        for part in s.split('+') {
            let input: Input = part
                .parse()
                .map_err(|_| ParseChordKeyError(s.to_string()))?;
            members.insert(input);
        }
        // A single-member "Chord" is meaningless (ticket 01: "open-ended,
        // N>=2") — rejected here too, not just at the Command layer, so a
        // hand-edited config.toml with a bogus one-member chord key refuses
        // to start rather than silently loading an unreachable entry.
        if members.len() < 2 {
            return Err(ParseChordKeyError(s.to_string()));
        }
        Ok(ChordKey(members))
    }
}

impl Serialize for ChordKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ChordKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
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
    /// References a `MacroDef` in `Config.macros` rather than carrying step
    /// content directly (ticket 15/51) — no inline/unnamed Macro survives as
    /// a second representation.
    Macro { macro_id: MacroId },
    /// Switches the active Profile when fired (ticket 05/34). Validated
    /// (`SetBinding` and `load_or_seed`, via `parse`'s
    /// `InvalidProfileSwitchTrigger` check) to only ever pair with
    /// `TriggerMode::FireOnce` — a held/toggled Profile switch has no
    /// coherent meaning.
    ProfileSwitch { target: String },
    /// Fires a virtual-gamepad button press (ticket 14/43) — an ordinary
    /// Action reusing Binding/Trigger-mode/dispatch/executor exactly like
    /// Keypress (only the target `uinput` device differs, an
    /// executor/injector-level distinction, per `executor::compile` and
    /// `input::is_gamepad_button`). `button` is validated (`SetBinding` and
    /// `load_or_seed`, via `parse`'s `InvalidControllerButton` check)
    /// against `input::gamepad_button_codes()`'s curated 57-entry allowlist
    /// — unlike Keypress's `key`, which accepts any `KeyCode` at all.
    ControllerButton { button: KeyCode },
    /// Advances or retreats a Stepper list's Daemon-side runtime cursor and
    /// fires the newly-selected item, in one motion (ticket 03/54 —
    /// CONTEXT.md: Stepper). References a `StepperDef` in `Config.steppers`
    /// rather than carrying item content directly, same reference-not-inline
    /// shape as `Action::Macro`. Validated (`SetBinding` and `load_or_seed`,
    /// via `parse`'s `UnknownStepper`/`InvalidStepTrigger` checks) to name a
    /// real `StepperId` and to never pair with `TriggerMode::Toggle` — a
    /// cursor advance has no coherent continuously-running state.
    Step {
        stepper: StepperId,
        direction: StepDirection,
    },
}

/// `Action::Step`'s direction field (ticket 03's Answer: "one variant with a
/// direction field, not two separate variants").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepDirection {
    Forward,
    Backward,
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

/// A Macro's identity (CONTEXT.md: Macro; ticket 15's Answer) — a slug
/// derived from `MacroDef.name` at creation time, then frozen: it is the
/// `Config.macros` key and never changes again, even when `name` is later
/// edited via `RenameMacro`. Deliberately opaque/decoupled from `name`
/// (unlike `Profile`, whose name *is* its map key) so a rename is a pure
/// field write with no scan/cascade anywhere else in `config.toml`.
/// `#[serde(transparent)]` round-trips as the plain inner string, matching
/// `Input`'s hand-written TOML/D-Bus string convention — simpler here since
/// there's no fixed grammar to validate against; a lookup miss just becomes
/// `CommandError::NotFound`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MacroId(String);

impl MacroId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MacroId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for MacroId {
    fn from(id: String) -> Self {
        MacroId(id)
    }
}

impl From<&str> for MacroId {
    fn from(id: &str) -> Self {
        MacroId(id.to_string())
    }
}

/// A library entry in `Config.macros` (ticket 15/51 — CONTEXT.md: Macro).
/// `name` is the editable display name (`RenameMacro`); `steps` is exactly
/// the step-sequence content that used to live inline on `Action::Macro`,
/// moved here unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroDef {
    pub name: String,
    pub steps: Vec<MacroStepDto>,
}

/// A Stepper's identity (CONTEXT.md: Stepper; ticket 03's Answer) — a slug
/// derived from `StepperDef.name` at creation time, then frozen, exactly
/// mirroring `MacroId`'s identity model (ticket 15) for consistency across
/// the two libraries: opaque/decoupled from `name` so `RenameStepper` is a
/// pure field write, no scan/cascade.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StepperId(String);

impl StepperId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StepperId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for StepperId {
    fn from(id: String) -> Self {
        StepperId(id)
    }
}

impl From<&str> for StepperId {
    fn from(id: &str) -> Self {
        StepperId(id.to_string())
    }
}

/// A single Stepper list item (ticket 03's Answer — CONTEXT.md: Stepper): "a
/// dedicated type distinct from `Action` — restricted to a single fire-once
/// keyboard key or mouse-button, never a Macro or another Stepper — enforced
/// structurally." Mouse-button output needs no separate variant — ticket 02
/// confirmed live that a mouse button is just another `evdev::KeyCode`
/// (`BTN_LEFT`/etc.), the same way `Action::Keypress.key` already covers it.
/// A dedicated enum (rather than a bare `KeyCode` field on `StepperDef`) so
/// a later joystick/controller-button variant can be added — ticket 03's
/// Answer: "deliberately designed to extend to joystick/controller buttons
/// later" — without reshaping every existing list entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepperItem {
    Key {
        key: KeyCode,
        /// The item's modifier combination (ticket 62's Answer) — e.g.
        /// Ctrl+1 for an MMORPG hotkey page. `#[serde(default)]` so an
        /// omitted `modifiers` key in `config.toml` still parses, matching
        /// `Action::Keypress`'s own convention.
        #[serde(default)]
        modifiers: Modifiers,
    },
}

/// A library entry in `Config.steppers` (ticket 03/54 — CONTEXT.md:
/// Stepper). `name` is the editable display name (`RenameStepper`); `items`
/// is the ordered list a bound pair's forward/backward steps walk. Mirrors
/// `MacroDef`'s shape exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepperDef {
    pub name: String,
    pub items: Vec<StepperItem>,
}

/// The lowercase/hyphenated base of a library-entry slug, derived from a
/// user-supplied name (ticket 15's Answer): every ASCII alphanumeric
/// character is kept (lowercased), every run of anything else collapses to
/// one `-`, and leading/trailing `-` are trimmed. Falls back to `fallback`
/// if that leaves nothing (e.g. a name with no alphanumeric characters at
/// all) — `unique_macro_id`/`unique_stepper_id` still guarantee the final
/// result is collision-free. Shared by both libraries; `fallback` is the
/// only thing that differs between them (`"macro"`/`"stepper"`).
fn slug_base(name: &str, fallback: &str) -> String {
    let mut result = String::with_capacity(name.len());
    let mut last_was_hyphen = true; // suppresses a leading hyphen
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            result.push(ch.to_ascii_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            result.push('-');
            last_was_hyphen = true;
        }
    }
    while result.ends_with('-') {
        result.pop();
    }
    if result.is_empty() {
        result.push_str(fallback);
    }
    result
}

/// Appends `-2`, `-3`, ... to `slug_base(name, "macro")` until the result
/// isn't already a key in `config.macros` — ticket 15's Answer's worked
/// example: `screenshot-combo`, `screenshot-combo-2`. Used by `CreateMacro`'s
/// dispatch handler to derive a new, frozen `MacroId`.
pub(crate) fn unique_macro_id(config: &Config, name: &str) -> MacroId {
    let base = slug_base(name, "macro");
    if !config.macros.contains_key(&MacroId(base.clone())) {
        return MacroId(base);
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}-{n}");
        if !config.macros.contains_key(&MacroId(candidate.clone())) {
            return MacroId(candidate);
        }
        n += 1;
    }
}

/// `unique_macro_id`'s exact mirror for the Stepper library — used by
/// `CreateStepper`'s dispatch handler to derive a new, frozen `StepperId`.
pub(crate) fn unique_stepper_id(config: &Config, name: &str) -> StepperId {
    let base = slug_base(name, "stepper");
    if !config.steppers.contains_key(&StepperId(base.clone())) {
        return StepperId(base);
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}-{n}");
        if !config.steppers.contains_key(&StepperId(candidate.clone())) {
            return StepperId(candidate);
        }
        n += 1;
    }
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
    InvalidControllerButton(String),
    UnknownMacro(String),
    UnknownStepper(String),
    InvalidStepTrigger,
    /// A Chord Binding (`chords_base`/`chords_held`) whose Action is
    /// `ProfileSwitch` (ticket 40) — refused because `executor::compile`
    /// panics on it: unlike an ordinary Binding, `dispatch::fire_chord` has
    /// no `&mut Config`/`config_path` to actually run a Profile switch
    /// through, so this Action never reaches a Chord at all, structurally
    /// enforced both here (a hand-edited `config.toml`) and by
    /// `SetChordBinding`'s own validation (a live D-Bus caller).
    InvalidChordProfileSwitch,
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
            ConfigError::InvalidControllerButton(button) => write!(
                f,
                "config.toml contains an Action::ControllerButton Binding whose button {button:?} is not a valid gamepad button"
            ),
            ConfigError::UnknownMacro(macro_id) => write!(
                f,
                "config.toml contains an Action::Macro Binding whose macro_id {macro_id:?} does not name a Macro in [macros]"
            ),
            ConfigError::UnknownStepper(stepper_id) => write!(
                f,
                "config.toml contains an Action::Step Binding whose stepper {stepper_id:?} does not name a Stepper in [steppers]"
            ),
            ConfigError::InvalidStepTrigger => write!(
                f,
                "config.toml contains an Action::Step Binding whose trigger is toggle"
            ),
            ConfigError::InvalidChordProfileSwitch => write!(
                f,
                "config.toml contains a Chord Binding whose Action is profile_switch, which is not supported on a Chord"
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
        profile_all_bindings(profile).any(|binding| {
            matches!(binding.action, Action::ProfileSwitch { .. })
                && binding.trigger != TriggerMode::FireOnce
        })
    });
    if has_invalid_profile_switch_trigger {
        return Err(ConfigError::InvalidProfileSwitchTrigger);
    }
    let invalid_controller_button = config.profiles.values().find_map(|profile| {
        profile_all_bindings(profile).find_map(|binding| match binding.action {
            Action::ControllerButton { button } if !crate::input::is_gamepad_button(button) => {
                Some(button)
            }
            _ => None,
        })
    });
    if let Some(button) = invalid_controller_button {
        return Err(ConfigError::InvalidControllerButton(format!("{button:?}")));
    }
    let dangling_macro_id = config.profiles.values().find_map(|profile| {
        profile_all_bindings(profile).find_map(|binding| match &binding.action {
            Action::Macro { macro_id } if !config.macros.contains_key(macro_id) => {
                Some(macro_id.clone())
            }
            _ => None,
        })
    });
    if let Some(macro_id) = dangling_macro_id {
        return Err(ConfigError::UnknownMacro(macro_id.to_string()));
    }
    let dangling_stepper_id = config.profiles.values().find_map(|profile| {
        profile_all_bindings(profile).find_map(|binding| match &binding.action {
            Action::Step { stepper, .. } if !config.steppers.contains_key(stepper) => {
                Some(stepper.clone())
            }
            _ => None,
        })
    });
    if let Some(stepper_id) = dangling_stepper_id {
        return Err(ConfigError::UnknownStepper(stepper_id.to_string()));
    }
    let has_invalid_step_trigger = config.profiles.values().any(|profile| {
        profile_all_bindings(profile).any(|binding| {
            matches!(binding.action, Action::Step { .. }) && binding.trigger == TriggerMode::Toggle
        })
    });
    if has_invalid_step_trigger {
        return Err(ConfigError::InvalidStepTrigger);
    }
    let has_chord_profile_switch = config.profiles.values().any(|profile| {
        profile
            .chords_base
            .values()
            .chain(profile.chords_held.values())
            .any(|binding| matches!(binding.action, Action::ProfileSwitch { .. }))
    });
    if has_chord_profile_switch {
        return Err(ConfigError::InvalidChordProfileSwitch);
    }
    Ok(config)
}

/// Every Binding on `profile`, across both ordinary per-`Input` Layers and
/// both per-`ChordKey` Chord Layers (ticket 40) — shared by every
/// cross-cutting validation check `parse` runs, so a hand-edited
/// `config.toml`'s Chord Bindings are held to the exact same
/// ProfileSwitch/ControllerButton/Macro/Stepper invariants as ordinary ones
/// rather than silently skipped.
pub(crate) fn profile_all_bindings(profile: &Profile) -> impl Iterator<Item = &Binding> {
    profile
        .base
        .values()
        .chain(profile.held.values())
        .chain(profile.chords_base.values())
        .chain(profile.chords_held.values())
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

[macros.test-macro]
name = "Test macro"
steps = [
  { key_down = "KEY_A" }, { delay_ms = 50 }, { key_up = "KEY_A" },
]

[profiles.Default]
mode_key_role = "bound"

[profiles.Default.held.grid_r2c1]
trigger = "toggle"
action = { type = "macro", macro_id = "test-macro" }
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
    }

    #[test]
    fn parses_a_controller_button_binding() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "controller_button", button = "BTN_SOUTH" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let binding = &config.profiles["Default"].base[&Input::Grid(1, 1)];
        assert_eq!(
            binding.action,
            Action::ControllerButton {
                button: KeyCode::BTN_SOUTH,
            }
        );
    }

    #[test]
    fn refuses_to_start_when_a_controller_button_is_not_in_the_gamepad_allowlist() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "controller_button", button = "KEY_A" }
"#;
        fs::write(&path, original).unwrap();

        let err =
            load_or_seed(&path).expect_err("a non-gamepad ControllerButton must refuse to start");
        assert!(matches!(err, ConfigError::InvalidControllerButton(_)));

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn held_layer_bindings_survive_a_full_write_and_reparse_round_trip() {
        let (_dir, path) = temp_config_path();
        let mut config = Config::seed();
        config.macros.insert(
            MacroId::from("screenshot-combo"),
            MacroDef {
                name: "Screenshot combo".to_string(),
                steps: vec![
                    MacroStepDto::KeyDown(KeyCode::KEY_A),
                    MacroStepDto::Delay(50),
                    MacroStepDto::KeyUp(KeyCode::KEY_A),
                ],
            },
        );
        let profile = config.active_profile_mut().unwrap();
        profile.mode_key_role = ModeKeyRole::Bound;
        profile.held.insert(
            Input::Grid(2, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action: Action::Macro {
                    macro_id: MacroId::from("screenshot-combo"),
                },
            },
        );

        write(&path, &config).unwrap();
        let reparsed = load_or_seed(&path).unwrap();

        assert_eq!(reparsed, config);
    }

    #[test]
    fn a_pre_ticket_51_config_defaults_an_empty_macro_library() {
        // A config.toml written before ticket 51 has no `[macros]` table at
        // all — it must still parse, defaulting to an empty library, per
        // `Config.macros`'s own `#[serde(default)]`.
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.macros.is_empty());
    }

    #[test]
    fn refuses_to_start_when_a_macro_binding_names_an_unknown_macro_id() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "macro", macro_id = "does-not-exist" }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path).expect_err("a dangling macro_id must refuse to start");
        assert!(matches!(err, ConfigError::UnknownMacro(id) if id == "does-not-exist"));

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn parses_a_macro_binding_shape() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[macros.screenshot-combo]
name = "Screenshot combo"
steps = [
  { key_down = "KEY_A" }, { delay_ms = 50 }, { key_up = "KEY_A" },
]

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "macro", macro_id = "screenshot-combo" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let binding = &config.profiles["Default"].base[&Input::Grid(1, 1)];
        assert_eq!(
            binding.action,
            Action::Macro {
                macro_id: MacroId::from("screenshot-combo"),
            }
        );
        let def = &config.macros[&MacroId::from("screenshot-combo")];
        assert_eq!(def.name, "Screenshot combo");
        assert_eq!(
            def.steps,
            vec![
                MacroStepDto::KeyDown(KeyCode::KEY_A),
                MacroStepDto::Delay(50),
                MacroStepDto::KeyUp(KeyCode::KEY_A),
            ]
        );
    }

    #[test]
    fn slug_base_lowercases_and_hyphenates() {
        assert_eq!(slug_base("Screenshot Combo", "macro"), "screenshot-combo");
        assert_eq!(slug_base("  weird!!__Name--", "macro"), "weird-name");
        assert_eq!(slug_base("こんにちは", "macro"), "macro");
        assert_eq!(slug_base("こんにちは", "stepper"), "stepper");
    }

    #[test]
    fn unique_macro_id_appends_a_numeric_suffix_on_collision() {
        let mut config = Config::seed();
        let first = unique_macro_id(&config, "Screenshot Combo");
        assert_eq!(first, MacroId::from("screenshot-combo"));
        config.macros.insert(
            first.clone(),
            MacroDef {
                name: "Screenshot Combo".to_string(),
                steps: vec![],
            },
        );

        let second = unique_macro_id(&config, "Screenshot Combo");
        assert_eq!(second, MacroId::from("screenshot-combo-2"));
        config.macros.insert(
            second.clone(),
            MacroDef {
                name: "Screenshot Combo".to_string(),
                steps: vec![],
            },
        );

        let third = unique_macro_id(&config, "Screenshot Combo");
        assert_eq!(third, MacroId::from("screenshot-combo-3"));
    }

    #[test]
    fn unique_stepper_id_appends_a_numeric_suffix_on_collision() {
        let mut config = Config::seed();
        let first = unique_stepper_id(&config, "Weapon Wheel");
        assert_eq!(first, StepperId::from("weapon-wheel"));
        config.steppers.insert(
            first.clone(),
            StepperDef {
                name: "Weapon Wheel".to_string(),
                items: vec![],
            },
        );

        let second = unique_stepper_id(&config, "Weapon Wheel");
        assert_eq!(second, StepperId::from("weapon-wheel-2"));
    }

    #[test]
    fn a_pre_ticket_54_config_defaults_an_empty_stepper_library() {
        // A config.toml written before ticket 54 has no `[steppers]` table
        // at all — `Action::Step` is net-new, so it must still parse,
        // defaulting to an empty library, per `Config.steppers`'s own
        // `#[serde(default)]`.
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.steppers.is_empty());
    }

    #[test]
    fn parses_a_step_binding_shape() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[steppers.weapon-wheel]
name = "Weapon Wheel"
items = [
  { type = "key", key = "KEY_1" }, { type = "key", key = "KEY_2" },
]

[profiles.Default.base.wheel_scroll_up]
trigger = "fire_once"
action = { type = "step", stepper = "weapon-wheel", direction = "forward" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let binding =
            &config.profiles["Default"].base[&Input::Wheel(crate::input::WheelEvent::ScrollUp)];
        assert_eq!(
            binding.action,
            Action::Step {
                stepper: StepperId::from("weapon-wheel"),
                direction: StepDirection::Forward,
            }
        );
        let def = &config.steppers[&StepperId::from("weapon-wheel")];
        assert_eq!(def.name, "Weapon Wheel");
        assert_eq!(
            def.items,
            vec![
                StepperItem::Key {
                    key: KeyCode::KEY_1,
                    modifiers: Modifiers::default(),
                },
                StepperItem::Key {
                    key: KeyCode::KEY_2,
                    modifiers: Modifiers::default(),
                },
            ]
        );
    }

    #[test]
    fn a_stepper_item_with_no_modifiers_key_defaults_to_no_modifiers() {
        // A config.toml written before ticket 63 has no `modifiers` field on
        // a stepper item at all — must still parse, defaulting to
        // `Modifiers::default()`, per the field's `#[serde(default)]`.
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base]

[steppers.weapon-wheel]
name = "Weapon Wheel"
items = [
  { type = "key", key = "KEY_1" },
]
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let def = &config.steppers[&StepperId::from("weapon-wheel")];
        assert_eq!(
            def.items,
            vec![StepperItem::Key {
                key: KeyCode::KEY_1,
                modifiers: Modifiers::default(),
            }]
        );
    }

    #[test]
    fn a_stepper_item_round_trips_a_modifier_combination() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base]

[steppers.hotkey-pages]
name = "Hotkey Pages"
items = [
  { type = "key", key = "KEY_1", modifiers = { ctrl = true } },
]
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let def = &config.steppers[&StepperId::from("hotkey-pages")];
        assert_eq!(
            def.items,
            vec![StepperItem::Key {
                key: KeyCode::KEY_1,
                modifiers: Modifiers {
                    ctrl: true,
                    shift: false,
                    alt: false,
                    super_key: false,
                },
            }]
        );
    }

    #[test]
    fn refuses_to_start_when_a_step_binding_names_an_unknown_stepper() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "step", stepper = "does-not-exist", direction = "forward" }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path).expect_err("a dangling stepper id must refuse to start");
        assert!(matches!(err, ConfigError::UnknownStepper(id) if id == "does-not-exist"));

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn refuses_to_start_when_a_step_binding_is_toggle() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[steppers.weapon-wheel]
name = "Weapon Wheel"
items = [{ type = "key", key = "KEY_1" }]

[profiles.Default.base.grid_r1c1]
trigger = "toggle"
action = { type = "step", stepper = "weapon-wheel", direction = "forward" }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path).expect_err("a Toggle Step Binding must refuse to start");
        assert!(matches!(err, ConfigError::InvalidStepTrigger));

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn chord_key_displays_as_a_plus_joined_sorted_string() {
        let key = ChordKey::new(BTreeSet::from([Input::Grid(1, 2), Input::Grid(1, 1)]));
        assert_eq!(key.to_string(), "grid_r1c1+grid_r1c2");
    }

    #[test]
    fn chord_key_round_trips_through_its_display_form() {
        let key = ChordKey::new(BTreeSet::from([
            Input::Thumbstick(crate::input::Direction::Up),
            Input::Thumbstick(crate::input::Direction::Right),
        ]));
        let parsed: ChordKey = key.to_string().parse().unwrap();
        assert_eq!(parsed, key);
    }

    #[test]
    fn chord_key_from_str_rejects_fewer_than_two_members() {
        assert!("grid_r1c1".parse::<ChordKey>().is_err());
        assert!("".parse::<ChordKey>().is_err());
    }

    #[test]
    fn chord_key_from_str_rejects_an_unknown_member() {
        assert!("grid_r1c1+not_an_input".parse::<ChordKey>().is_err());
    }

    #[test]
    fn parses_a_chord_binding_shape_on_both_layers() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.chords_base."grid_r1c1+grid_r1c2"]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_C", modifiers = { ctrl = true } }

[profiles.Default.chords_held."thumbstick_left+thumbstick_up"]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_Q" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let profile = &config.profiles["Default"];
        let base_key = ChordKey::new(BTreeSet::from([Input::Grid(1, 1), Input::Grid(1, 2)]));
        assert_eq!(
            profile.chords(Layer::Base)[&base_key].trigger,
            TriggerMode::FireOnce
        );
        let held_key = ChordKey::new(BTreeSet::from([
            Input::Thumbstick(crate::input::Direction::Left),
            Input::Thumbstick(crate::input::Direction::Up),
        ]));
        assert!(profile.chords(Layer::Held).contains_key(&held_key));
    }

    #[test]
    fn a_pre_ticket_40_config_defaults_empty_chord_maps() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let profile = &config.profiles["Default"];
        assert!(profile.chords_base.is_empty());
        assert!(profile.chords_held.is_empty());
    }

    #[test]
    fn chord_bindings_survive_a_full_write_and_reparse_round_trip() {
        let (_dir, path) = temp_config_path();
        let mut config = Config::seed();
        let profile = config.active_profile_mut().unwrap();
        let key = ChordKey::new(BTreeSet::from([Input::Grid(1, 1), Input::Grid(1, 2)]));
        profile.chords_base.insert(
            key.clone(),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: KeyCode::KEY_C,
                },
            },
        );

        write(&path, &config).unwrap();
        let reparsed = load_or_seed(&path).unwrap();

        assert_eq!(reparsed, config);
        assert!(reparsed.profiles["Default"].chords_base.contains_key(&key));
    }

    #[test]
    fn refuses_to_start_when_a_chord_binding_names_an_unknown_macro_id() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.chords_base."grid_r1c1+grid_r1c2"]
trigger = "fire_once"
action = { type = "macro", macro_id = "does-not-exist" }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path).expect_err("a dangling Chord macro_id must refuse to start");
        assert!(matches!(err, ConfigError::UnknownMacro(id) if id == "does-not-exist"));
    }

    #[test]
    fn refuses_to_start_when_a_chord_binding_is_a_profile_switch() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.chords_base."grid_r1c1+grid_r1c2"]
trigger = "fire_once"
action = { type = "profile_switch", target = "Gaming" }
"#;
        fs::write(&path, original).unwrap();

        let err =
            load_or_seed(&path).expect_err("a ProfileSwitch Chord Binding must refuse to start");
        assert!(matches!(err, ConfigError::InvalidChordProfileSwitch));
    }
}
