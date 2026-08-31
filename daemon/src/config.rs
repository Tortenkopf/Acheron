// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

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

use evdev::{AbsoluteAxisCode, KeyCode};
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
    /// Axis assignments active while this Profile's Base Layer is active
    /// (ticket 59/71 — CONTEXT.md: Axis assignment): a parallel per-Layer
    /// map alongside `base`, not a new `Action` variant — Trigger-mode has
    /// no coherent meaning for a continuous value. An Input present here is
    /// structurally excluded from `base`'s Binding map *and* from Chord
    /// membership on this Layer, enforced by `SetAxisAssignment`/
    /// `SetBinding`/`SetChordBinding` and, for a hand-edited `config.toml`,
    /// by `parse` below.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub axis_base: HashMap<Input, AxisTarget>,
    /// `axis_base`'s exact mirror for the Held Layer.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub axis_held: HashMap<Input, AxisTarget>,
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
                resolved.insert(input, self.resolved_actuation_point(input));
            }
        }
        resolved
    }

    /// `resolved_actuation_points`'s single-`Input` mirror — `actuation_
    /// overrides` where set, `default_actuation` otherwise. Exists
    /// separately for a hot-path caller (`dispatch::handle_depth_update`,
    /// ticket 71) that only ever needs the handful of Inputs currently
    /// Axis-assigned, not a full 20-entry `HashMap` rebuilt from scratch on
    /// every live-Depth tick (code-review finding).
    pub fn resolved_actuation_point(&self, input: Input) -> ActuationPoint {
        self.actuation_overrides
            .get(&input)
            .copied()
            .unwrap_or(self.default_actuation)
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

    /// `layer`'s exact mirror for a Profile's Axis assignments (ticket 71).
    pub fn axis_layer(&self, layer: Layer) -> &HashMap<Input, AxisTarget> {
        match layer {
            Layer::Base => &self.axis_base,
            Layer::Held => &self.axis_held,
        }
    }

    /// `layer_mut`'s exact mirror for a Profile's Axis assignments.
    pub fn axis_layer_mut(&mut self, layer: Layer) -> &mut HashMap<Input, AxisTarget> {
        match layer {
            Layer::Base => &mut self.axis_base,
            Layer::Held => &mut self.axis_held,
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
    /// `#[serde(default)]` (→ `TriggerMode::HoldToRepeat`, matching the GUI's
    /// own new-binding default, ticket 89) so a hand-edited `config.toml`
    /// binding that omits `trigger` parses instead of failing with serde's
    /// opaque "missing field `trigger`". Backward-compatible: every config
    /// that spells `trigger` out still parses identically, and the field is
    /// still always written back out (no `skip_serializing_if`).
    #[serde(default)]
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

/// CONTEXT.md: Axis assignment. One of the 17 targets ticket 59 §3 settled —
/// 5 unsigned single-key axes (raw Depth 0-255, no polar opposite) and 6
/// signed axes split into two independently-assignable +/- halves each (12
/// half-axis targets), so two independent grid keys can cover both halves of
/// one physical axis. `#[serde(rename_all = "snake_case")]` gives every
/// variant a plain lowercase `config.toml` string (e.g. `"left_trigger"`,
/// `"right_stick_x_pos"`) with no hand-written `Display`/`FromStr` needed,
/// unlike `Input` — there's no map-key grammar to validate here, just a
/// closed enum value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisTarget {
    LeftTrigger,
    RightTrigger,
    Throttle,
    Gas,
    Brake,
    LeftStickXPos,
    LeftStickXNeg,
    LeftStickYPos,
    LeftStickYNeg,
    RightStickXPos,
    RightStickXNeg,
    RightStickYPos,
    RightStickYNeg,
    RudderPos,
    RudderNeg,
    WheelPos,
    WheelNeg,
}

/// A signed `AxisTarget`'s polarity — which independently-assignable half of
/// its underlying `ABS_*` code it drives. `None` (via `AxisTarget::polarity`)
/// for the 5 unsigned targets, which have no opposite half and so never
/// participate in the opposite-half suppression rule (ticket 59 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisPolarity {
    Positive,
    Negative,
}

impl AxisTarget {
    pub const ALL: [AxisTarget; 17] = [
        AxisTarget::LeftTrigger,
        AxisTarget::RightTrigger,
        AxisTarget::Throttle,
        AxisTarget::Gas,
        AxisTarget::Brake,
        AxisTarget::LeftStickXPos,
        AxisTarget::LeftStickXNeg,
        AxisTarget::LeftStickYPos,
        AxisTarget::LeftStickYNeg,
        AxisTarget::RightStickXPos,
        AxisTarget::RightStickXNeg,
        AxisTarget::RightStickYPos,
        AxisTarget::RightStickYNeg,
        AxisTarget::RudderPos,
        AxisTarget::RudderNeg,
        AxisTarget::WheelPos,
        AxisTarget::WheelNeg,
    ];

    /// The underlying uinput `ABS_*` code this target drives (ticket 59 §3)
    /// — the 6 signed axes' two independently-assignable halves share one
    /// code each, distinguished only by `polarity`.
    pub fn abs_code(self) -> AbsoluteAxisCode {
        match self {
            AxisTarget::LeftTrigger => AbsoluteAxisCode::ABS_Z,
            AxisTarget::RightTrigger => AbsoluteAxisCode::ABS_RZ,
            AxisTarget::Throttle => AbsoluteAxisCode::ABS_THROTTLE,
            AxisTarget::Gas => AbsoluteAxisCode::ABS_GAS,
            AxisTarget::Brake => AbsoluteAxisCode::ABS_BRAKE,
            AxisTarget::LeftStickXPos | AxisTarget::LeftStickXNeg => AbsoluteAxisCode::ABS_X,
            AxisTarget::LeftStickYPos | AxisTarget::LeftStickYNeg => AbsoluteAxisCode::ABS_Y,
            AxisTarget::RightStickXPos | AxisTarget::RightStickXNeg => AbsoluteAxisCode::ABS_RX,
            AxisTarget::RightStickYPos | AxisTarget::RightStickYNeg => AbsoluteAxisCode::ABS_RY,
            AxisTarget::RudderPos | AxisTarget::RudderNeg => AbsoluteAxisCode::ABS_RUDDER,
            AxisTarget::WheelPos | AxisTarget::WheelNeg => AbsoluteAxisCode::ABS_WHEEL,
        }
    }

    pub fn polarity(self) -> Option<AxisPolarity> {
        match self {
            AxisTarget::LeftTrigger
            | AxisTarget::RightTrigger
            | AxisTarget::Throttle
            | AxisTarget::Gas
            | AxisTarget::Brake => None,
            AxisTarget::LeftStickXPos
            | AxisTarget::LeftStickYPos
            | AxisTarget::RightStickXPos
            | AxisTarget::RightStickYPos
            | AxisTarget::RudderPos
            | AxisTarget::WheelPos => Some(AxisPolarity::Positive),
            AxisTarget::LeftStickXNeg
            | AxisTarget::LeftStickYNeg
            | AxisTarget::RightStickXNeg
            | AxisTarget::RightStickYNeg
            | AxisTarget::RudderNeg
            | AxisTarget::WheelNeg => Some(AxisPolarity::Negative),
        }
    }
}

/// Live/linear axis resolution (ticket 59 §4/§7's `(Depth, edge_event) ->
/// axis_value` seam — this is its Depth half, reused unmodified as the
/// Digital-mode step fallback's own final-value clamp): 0 below the key's
/// Release point, a linear ramp from 0 (at Release) up to raw Depth (at
/// Actuation), and raw Depth unchanged above Actuation — reuses the key's
/// own already-tunable Actuation/Release points as the axis's start/end
/// thresholds rather than a separate deadzone (ticket 59 §4). Continuous at
/// both boundaries: `resolve_axis_value(release, point) == 0` and
/// `resolve_axis_value(actuation, point) == actuation`.
pub fn resolve_axis_value(depth: u8, point: ActuationPoint) -> u8 {
    if depth <= point.release {
        0
    } else if depth >= point.actuation {
        depth
    } else {
        let span = u32::from(point.actuation - point.release);
        let progress = u32::from(depth - point.release);
        (progress * u32::from(point.actuation) / span) as u8
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
    /// A fourth mode, grid-key-only (ticket 20/39): re-fires at a rate that
    /// varies continuously with Depth rather than at a fixed cadence — see
    /// CONTEXT.md's Analog-repeat entry. Validated (`SetBinding`/
    /// `SetChordBinding` and `parse`) to only ever pair with a Grid `Input`
    /// and never with a Chord's own Binding.
    AnalogRepeat,
}

/// Hold-to-repeat is the default a `config.toml` binding takes when it omits
/// `trigger` entirely (`Binding.trigger`'s `#[serde(default)]`), matching the
/// GUI's own new-binding default (ticket 89).
impl Default for TriggerMode {
    fn default() -> Self {
        TriggerMode::HoldToRepeat
    }
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
    /// A gamepad-button press (ticket 92's Answer) — the joystick/controller
    /// extension CONTEXT.md's Stepper entry always anticipated. Compiled by
    /// `dispatch::resolve_step` to the same down/dwell/up triple as
    /// `Action::ControllerButton` (`executor::controller_button_steps`),
    /// routed to the gamepad `uinput` device by `input::is_gamepad_button`.
    /// `button` is validated against `input::gamepad_button_codes()`'s
    /// 57-entry allowlist at the `CreateStepper`/`SetStepperItems` D-Bus
    /// paths and by `parse`'s `InvalidControllerButtonStepperItem` check,
    /// mirroring `Action::ControllerButton`'s two-place enforcement. **No
    /// `modifiers` field** — a gamepad button takes no modifier combination.
    ControllerButton { button: KeyCode },
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
    /// A Fire-once `Action::ControllerButton` Binding (ticket 78's Answer):
    /// Hold-to-repeat already covers a quick tap (a quick physical Down+Up
    /// naturally produces a quick output Down+Up), leaving nothing for
    /// Fire-once's decoupled-from-hold-duration pulse to uniquely serve — no
    /// real gamepad button press works that way. Same shape as
    /// `InvalidProfileSwitchTrigger`/`InvalidStepTrigger`.
    InvalidControllerButtonTrigger,
    /// A `StepperItem::ControllerButton` list item whose `button` is not in
    /// `input::gamepad_button_codes()`'s 57-entry allowlist (ticket 92) —
    /// the Stepper-item parallel of `InvalidControllerButton`, so a
    /// hand-edited `config.toml` with a `controller_button` item naming
    /// `KEY_A` refuses to start with a clear error.
    InvalidControllerButtonStepperItem(String),
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
    /// An `axis_base`/`axis_held` entry (ticket 71) keyed by an `Input` that
    /// isn't a `Grid` variant — only grid keys have Depth to drive an axis
    /// with (ticket 59 §1).
    InvalidAxisInput(String),
    /// An Input present in both a Layer's Axis map *and* its Binding map
    /// (ticket 59 §2's mutual exclusion) — only reachable via a hand-edited
    /// `config.toml`; `SetBinding`/`SetAxisAssignment` both refuse to create
    /// this live.
    AxisBindingConflict(String),
    /// An Input present in both a Layer's Axis map and some Chord's member
    /// set on that same Layer (ticket 59 §2's mutual exclusion) — only
    /// reachable via a hand-edited `config.toml`; `SetChordBinding`/
    /// `SetAxisAssignment` both refuse to create this live.
    AxisChordConflict(String),
    /// A pre-ticket-51 `config.toml` still carrying the old inline
    /// `Action::Macro { steps: [...] }` shape (`type = "macro"` with a
    /// `steps` array and no `macro_id`) — ticket 51 replaced it with a
    /// reference into `[macros.*]`, and nothing migrates an old file
    /// automatically (issue 06's "sparse data model, no forced migration"
    /// precedent — see `parse`'s other sparse-default tests). Detected
    /// specifically against the raw TOML, ahead of the strongly-typed parse,
    /// so the failure names the affected Binding(s) instead of surfacing as
    /// serde's opaque "missing field `macro_id`" (ticket 57, spawned live
    /// from ticket 53 hitting exactly this against a real config.toml).
    LegacyInlineMacroBinding(Vec<String>),
    /// An ordinary Binding (`base`/`held`) whose trigger is `AnalogRepeat`
    /// but whose `Input` isn't a `Grid` variant — only grid keys have Depth
    /// to drive a rate curve with (ticket 20/39), same reasoning as
    /// `InvalidAxisInput`.
    InvalidAnalogRepeatInput(String),
    /// A Chord Binding (`chords_base`/`chords_held`) whose trigger is
    /// `AnalogRepeat` (ticket 20/39) — refused because a Chord fires on a
    /// discrete member-set completion, not a single grid key's continuous
    /// Depth, mirroring `InvalidChordProfileSwitch`'s exact precedent.
    InvalidChordAnalogRepeat,
    /// A `default_actuation` or `actuation_overrides` entry whose `release`
    /// point is not strictly below its `actuation` point (ticket 04) —
    /// `release >= actuation` defeats hysteresis entirely: a key held at a
    /// steady Depth crosses both thresholds on every report, chattering
    /// Down/Up on a motionless key (`capture::analog::observe`). The string
    /// is the locus — `"default"` for the Profile default, otherwise the
    /// offending Input's `Display` form.
    ReleaseNotBelowActuation(String),
    /// An `actuation_overrides` entry (ticket 17 §3) keyed by an `Input` that
    /// isn't a `Grid` variant — only grid keys have Depth for an actuation
    /// point to threshold against, same reasoning as `InvalidAxisInput`.
    InvalidActuationOverrideInput(String),
    /// An `Action::ProfileSwitch { target }` naming a Profile not present in
    /// `[profiles]` (ticket 34/04) — `switch_profile`'s
    /// `active_profile_mut().expect(...)` path assumes every reachable target
    /// resolves, so a dangling one is a latent panic, exactly like a dangling
    /// `Macro`/`Stepper` reference.
    UnknownProfileSwitchTarget(String),
    /// Two Chords on one Layer whose member sets are in a subset/superset
    /// relationship (ticket 01's amended Answer) — completing the smaller is
    /// indistinguishable from being partway into the larger.
    ChordMemberSetConflict {
        key: String,
        other: String,
    },
    /// A stored Chord Binding with fewer than two member Inputs (ticket 01:
    /// "open-ended, N>=2") — `ChordKey`'s own `FromStr` already refuses this
    /// on the deserialize path; this is the in-memory structural mirror for
    /// the live-edit path (`SetChordBinding` builds a `ChordKey` directly).
    ChordTooFewMembers(String),
    /// A Profile keyed by an empty or whitespace-only name (ticket 19/04) —
    /// the structural mirror of `CreateProfile`/`RenameProfile`'s own
    /// friendly rejection, so a hand-edited `[profiles.""]` refuses to load.
    EmptyProfileName,
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
                "active_profile {name:?} does not name a Profile in [profiles]"
            ),
            ConfigError::InvalidProfileSwitchTrigger => {
                write!(f, "a Profile Switch Binding must use fire_once")
            }
            ConfigError::InvalidControllerButton(button) => write!(
                f,
                "a Controller Button Binding's button {button:?} is not a valid gamepad button"
            ),
            ConfigError::InvalidControllerButtonTrigger => {
                write!(f, "a Controller Button Binding must not use fire_once")
            }
            ConfigError::InvalidControllerButtonStepperItem(button) => write!(
                f,
                "a Stepper list item's controller button {button:?} is not a valid gamepad button"
            ),
            ConfigError::UnknownMacro(macro_id) => write!(
                f,
                "a Macro Binding references macro_id {macro_id:?}, which is not in [macros]"
            ),
            ConfigError::UnknownStepper(stepper_id) => write!(
                f,
                "a Step Binding references stepper {stepper_id:?}, which is not in [steppers]"
            ),
            ConfigError::InvalidStepTrigger => write!(f, "a Step Binding must not use toggle"),
            ConfigError::InvalidChordProfileSwitch => {
                write!(f, "a Chord Binding's Action cannot be profile_switch")
            }
            ConfigError::InvalidAxisInput(input) => write!(
                f,
                "an Axis assignment on {input:?} is not allowed — only Grid Inputs can carry one"
            ),
            ConfigError::AxisBindingConflict(input) => write!(
                f,
                "{input:?} has both an Axis assignment and a Binding on the same Layer"
            ),
            ConfigError::AxisChordConflict(input) => write!(
                f,
                "{input:?} has both an Axis assignment and Chord membership on the same Layer"
            ),
            ConfigError::LegacyInlineMacroBinding(paths) => write!(
                f,
                "config.toml contains an old-style inline Action::Macro Binding (from before named macros were introduced) at: {} — replace each one with {{ type = \"macro\", macro_id = \"...\" }} referencing an entry under [macros.*], or recreate the Binding from the GUI",
                paths.join(", ")
            ),
            ConfigError::InvalidAnalogRepeatInput(input) => write!(
                f,
                "an analog_repeat trigger on {input:?} is not allowed — only Grid Inputs can carry one"
            ),
            ConfigError::InvalidChordAnalogRepeat => {
                write!(f, "a Chord Binding's trigger cannot be analog_repeat")
            }
            ConfigError::ReleaseNotBelowActuation(locus) => write!(
                f,
                "the actuation point for {locus} has its release point at or above its actuation point (defeats hysteresis)"
            ),
            ConfigError::InvalidActuationOverrideInput(input) => write!(
                f,
                "an actuation override on {input:?} is not allowed — only Grid Inputs can carry one"
            ),
            ConfigError::UnknownProfileSwitchTarget(target) => write!(
                f,
                "a Profile Switch Binding targets Profile {target:?}, which is not in [profiles]"
            ),
            ConfigError::ChordMemberSetConflict { key, other } => write!(
                f,
                "Chord {key} conflicts with Chord {other} on the same Layer — one member set fully contains the other"
            ),
            ConfigError::ChordTooFewMembers(key) => {
                write!(f, "Chord {key} has fewer than two member Inputs")
            }
            ConfigError::EmptyProfileName => {
                write!(f, "a Profile has an empty or whitespace-only name")
            }
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

/// Walks `value` looking for the pre-ticket-51 inline `Action::Macro` shape
/// (`type = "macro"` with a `steps` array and no `macro_id`) and returns a
/// dotted breadcrumb (e.g. `profiles.Default.base.grid_r2c1.action`) for
/// each one found — `parse` runs this against the raw TOML, ahead of the
/// strongly-typed deserialize, since that deserialize would otherwise fail
/// on the first one with serde's generic "missing field `macro_id`" instead
/// of naming which Binding(s) need fixing (ticket 57).
fn find_legacy_macro_bindings(value: &toml::Value) -> Vec<String> {
    fn walk(value: &toml::Value, path: &str, out: &mut Vec<String>) {
        let toml::Value::Table(table) = value else {
            return;
        };
        let is_legacy_macro_shape = table.get("type").and_then(toml::Value::as_str)
            == Some("macro")
            && table.contains_key("steps")
            && !table.contains_key("macro_id");
        if is_legacy_macro_shape {
            out.push(path.to_string());
            return;
        }
        for (key, child) in table {
            let child_path = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            walk(child, &child_path, out);
        }
    }
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out
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
    let legacy_macro_bindings = find_legacy_macro_bindings(&value);
    if !legacy_macro_bindings.is_empty() {
        return Err(ConfigError::LegacyInlineMacroBinding(legacy_macro_bindings));
    }

    let config: Config = toml::from_str(contents).map_err(ConfigError::Parse)?;
    validate(&config)?;
    Ok(config)
}

/// The single enforcement point for every structural invariant of a stored
/// `Config` — anything that could be written to `config.toml` and reloaded
/// (ticket 04). Called from `parse` (after the strongly-typed deserialize)
/// and from `persist_edit` (after the edit closure, before the disk write),
/// so a rule lives in exactly one place and a hand-edited file and a live
/// D-Bus edit are held to the identical contract.
///
/// Returns the first violation it finds (as `parse` historically did). The
/// checks run in `parse`'s original order, with the six ticket-04 additions
/// appended, so no existing single-violation `parse` test changes which
/// error it sees.
///
/// This is *only* for invariants of the resulting `Config`. Operation
/// preconditions — "that name isn't taken", "that entry doesn't exist",
/// "you can't delete the active Profile" — are checked in the
/// `handle_command` arm that knows the requested operation, never here.
pub(crate) fn validate(config: &Config) -> Result<(), ConfigError> {
    if !config.profiles.contains_key(&config.active_profile) {
        return Err(ConfigError::InvalidActiveProfile(
            config.active_profile.clone(),
        ));
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
    let has_invalid_controller_button_trigger = config.profiles.values().any(|profile| {
        profile_all_bindings(profile).any(|binding| {
            matches!(binding.action, Action::ControllerButton { .. })
                && binding.trigger == TriggerMode::FireOnce
        })
    });
    if has_invalid_controller_button_trigger {
        return Err(ConfigError::InvalidControllerButtonTrigger);
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
    let invalid_stepper_controller_button = config.steppers.values().find_map(|def| {
        def.items.iter().find_map(|item| match item {
            StepperItem::ControllerButton { button }
                if !crate::input::is_gamepad_button(*button) =>
            {
                Some(*button)
            }
            _ => None,
        })
    });
    if let Some(button) = invalid_stepper_controller_button {
        return Err(ConfigError::InvalidControllerButtonStepperItem(format!(
            "{button:?}"
        )));
    }
    let invalid_analog_repeat_input = config.profiles.values().find_map(|profile| {
        [Layer::Base, Layer::Held].into_iter().find_map(|layer| {
            profile
                .layer(layer)
                .iter()
                .find(|(input, binding)| {
                    binding.trigger == TriggerMode::AnalogRepeat
                        && !matches!(input, Input::Grid(_, _))
                })
                .map(|(input, _)| input.to_string())
        })
    });
    if let Some(input) = invalid_analog_repeat_input {
        return Err(ConfigError::InvalidAnalogRepeatInput(input));
    }
    let has_chord_analog_repeat = config.profiles.values().any(|profile| {
        profile
            .chords_base
            .values()
            .chain(profile.chords_held.values())
            .any(|binding| binding.trigger == TriggerMode::AnalogRepeat)
    });
    if has_chord_analog_repeat {
        return Err(ConfigError::InvalidChordAnalogRepeat);
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
    let invalid_axis_input = config.profiles.values().find_map(|profile| {
        [Layer::Base, Layer::Held].into_iter().find_map(|layer| {
            profile
                .axis_layer(layer)
                .keys()
                .find(|input| !matches!(input, Input::Grid(_, _)))
                .copied()
        })
    });
    if let Some(input) = invalid_axis_input {
        return Err(ConfigError::InvalidAxisInput(input.to_string()));
    }
    let axis_binding_conflict = config.profiles.values().find_map(|profile| {
        [Layer::Base, Layer::Held].into_iter().find_map(|layer| {
            profile
                .axis_layer(layer)
                .keys()
                .find(|input| profile.layer(layer).contains_key(input))
                .copied()
        })
    });
    if let Some(input) = axis_binding_conflict {
        return Err(ConfigError::AxisBindingConflict(input.to_string()));
    }
    let axis_chord_conflict = config.profiles.values().find_map(|profile| {
        [Layer::Base, Layer::Held].into_iter().find_map(|layer| {
            profile.axis_layer(layer).keys().find_map(|input| {
                profile
                    .chords(layer)
                    .keys()
                    .any(|key| key.members().contains(input))
                    .then_some(*input)
            })
        })
    });
    if let Some(input) = axis_chord_conflict {
        return Err(ConfigError::AxisChordConflict(input.to_string()));
    }
    // --- ticket 04 additions, appended so existing `parse` tests are undisturbed ---
    let release_not_below_actuation = config.profiles.values().find_map(|profile| {
        if profile.default_actuation.release >= profile.default_actuation.actuation {
            return Some("default".to_string());
        }
        profile
            .actuation_overrides
            .iter()
            .find(|(_, point)| point.release >= point.actuation)
            .map(|(input, _)| input.to_string())
    });
    if let Some(locus) = release_not_below_actuation {
        return Err(ConfigError::ReleaseNotBelowActuation(locus));
    }
    let invalid_actuation_override_input = config.profiles.values().find_map(|profile| {
        profile
            .actuation_overrides
            .keys()
            .find(|input| !matches!(input, Input::Grid(_, _)))
            .map(ToString::to_string)
    });
    if let Some(input) = invalid_actuation_override_input {
        return Err(ConfigError::InvalidActuationOverrideInput(input));
    }
    let unknown_profile_switch_target = config.profiles.values().find_map(|profile| {
        profile_all_bindings(profile).find_map(|binding| match &binding.action {
            Action::ProfileSwitch { target } if !config.profiles.contains_key(target) => {
                Some(target.clone())
            }
            _ => None,
        })
    });
    if let Some(target) = unknown_profile_switch_target {
        return Err(ConfigError::UnknownProfileSwitchTarget(target));
    }
    let chord_member_set_conflict = config.profiles.values().find_map(|profile| {
        [Layer::Base, Layer::Held].into_iter().find_map(|layer| {
            let chords = profile.chords(layer);
            chords.keys().find_map(|key| {
                chords
                    .keys()
                    .find(|other| {
                        key != *other
                            && (key.members().is_subset(other.members())
                                || other.members().is_subset(key.members()))
                    })
                    .map(|other| (key.to_string(), other.to_string()))
            })
        })
    });
    if let Some((key, other)) = chord_member_set_conflict {
        return Err(ConfigError::ChordMemberSetConflict { key, other });
    }
    let chord_too_few_members = config.profiles.values().find_map(|profile| {
        [Layer::Base, Layer::Held].into_iter().find_map(|layer| {
            profile
                .chords(layer)
                .keys()
                .find(|key| key.members().len() < 2)
                .map(ToString::to_string)
        })
    });
    if let Some(key) = chord_too_few_members {
        return Err(ConfigError::ChordTooFewMembers(key));
    }
    if config.profiles.keys().any(|name| name.trim().is_empty()) {
        return Err(ConfigError::EmptyProfileName);
    }
    Ok(())
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

/// Serializes `config` to its `config.toml` text. Split out so the async
/// `persist` path can do this cheap CPU work on the caller's task and hand
/// only the finished `String` to the blocking pool — no second full `Config`
/// clone just to move it across the `spawn_blocking` boundary.
fn serialize(config: &Config) -> String {
    toml::to_string_pretty(config).expect("Config always serializes to TOML")
}

/// Writes `contents` to `path`, creating the parent directory if needed.
fn write_contents(path: &Path, contents: &str) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(ConfigError::Io)?;
    }
    fs::write(path, contents).map_err(ConfigError::Io)
}

/// Rewrites `config.toml` in full — the only persistence path, used both for
/// the initial seed and for every live D-Bus mutation (ticket 15), so
/// `config.toml` on disk always matches in-memory state immediately.
pub(crate) fn write(path: &Path, config: &Config) -> Result<(), ConfigError> {
    write_contents(path, &serialize(config))
}

/// Rewrites `config.toml` with the `fs` work moved off onto the async worker
/// pool: those `std::fs` calls are synchronous, and running them inline on the
/// dispatch task would stall every queued `PhysicalEvent` behind them for the
/// write's duration — perceptible input lag in a daemon whose whole job is
/// low-latency key remapping. Private to this module: every caller goes
/// through `persist_edit`, which pairs the write with its own
/// snapshot-and-restore.
async fn persist(config: &Config, path: &Path) -> Result<(), ConfigError> {
    let contents = serialize(config);
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || write_contents(&path, &contents))
        .await
        .expect("the config.toml write task must not panic")
}

/// Applies `edit` to `config`, runs `validate` against the result, and
/// persists it as one atomic in-memory unit (ticket 03/04). If `edit`
/// returns `Err`, if `validate` rejects the edited `Config`, or if the
/// `config.toml` write then fails, the in-memory `Config` is restored to
/// exactly its pre-`edit` value before the error propagates — so a failed
/// edit can never leave
/// `GetConfig()` reporting a change that never reached `persist`, for every
/// command, by construction. (`write_contents` itself is a plain truncating
/// `fs::write`, so a write that fails *mid-stream* can still leave the file
/// torn — an orthogonal durability concern, not one snapshot-and-restore
/// addresses.) On success the closure's `T` is returned (e.g. a freshly
/// minted `MacroId`/`StepperId`).
///
/// The helper's contract is strictly "atomic config edit + persist": on-success
/// side effects that aren't `Config` (publishing the actuation snapshot,
/// recomputing axis output, signalling the capture supervisor, …) belong in
/// the caller, after this returns `Ok`. `E: From<ConfigError>` keeps this
/// module from naming `command::CommandError`.
pub(crate) async fn persist_edit<T, E>(
    config: &mut Config,
    path: &Path,
    edit: impl FnOnce(&mut Config) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<ConfigError>,
{
    let snapshot = config.clone();
    let value = match edit(config) {
        Ok(v) => v,
        Err(e) => {
            *config = snapshot;
            return Err(e);
        }
    };
    // Ticket 04: the same single-sourced structural check `parse` runs, on
    // the same path — a live edit that would leave `Config` structurally
    // invalid is rejected and rolled back here, before it can reach disk.
    if let Err(e) = validate(config) {
        *config = snapshot;
        return Err(e.into());
    }
    match persist(config, path).await {
        Ok(()) => Ok(value),
        Err(e) => {
            *config = snapshot;
            Err(e.into())
        }
    }
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
    fn a_binding_that_omits_trigger_parses_as_hold_to_repeat() {
        // Ticket 89: `Binding.trigger` is `#[serde(default)]`, so a
        // hand-edited config.toml binding with no `trigger` line parses
        // (as Hold-to-repeat, matching the GUI's new-binding default)
        // instead of failing with serde's "missing field `trigger`".
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
action = { type = "keypress", key = "KEY_F1" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let binding = &config.profiles["Default"].base[&Input::Grid(1, 1)];
        assert_eq!(binding.trigger, TriggerMode::HoldToRepeat);
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
    fn resolved_actuation_point_matches_the_full_maps_own_entry_default_and_overridden() {
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

        assert_eq!(
            profile.resolved_actuation_point(Input::Grid(2, 3)),
            ActuationPoint {
                actuation: 200,
                release: 190,
            }
        );
        assert_eq!(
            profile.resolved_actuation_point(Input::Grid(1, 1)),
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
    fn refuses_to_start_when_a_controller_button_binding_is_fire_once() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "controller_button", button = "BTN_SOUTH" }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path)
            .expect_err("a Fire-once ControllerButton Binding must refuse to start");
        assert!(matches!(err, ConfigError::InvalidControllerButtonTrigger));

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
    fn refuses_to_start_on_a_pre_ticket_51_inline_macro_binding_and_names_it() {
        // ticket 57: a config.toml written before named macros existed still
        // has `type = "macro"` with a `steps` array and no `macro_id` — this
        // must surface as a specific, readable error naming the Binding,
        // not serde's generic "missing field `macro_id`".
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "macro", steps = [{ key_down = "KEY_A" }, { key_up = "KEY_A" }] }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path)
            .expect_err("a pre-ticket-51 inline Macro Binding must refuse to start");
        assert!(matches!(
            err,
            ConfigError::LegacyInlineMacroBinding(paths)
                if paths == vec!["profiles.Default.base.grid_r1c1.action".to_string()]
        ));

        // Refused, not silently rewritten (this ticket chose a guard, not a
        // migration) — the file on disk is untouched.
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
    fn parses_a_controller_button_stepper_item() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base]

[steppers.weapon-wheel]
name = "Weapon Wheel"
items = [
  { type = "key", key = "KEY_1" },
  { type = "controller_button", button = "BTN_SOUTH" },
]
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let def = &config.steppers[&StepperId::from("weapon-wheel")];
        assert_eq!(
            def.items,
            vec![
                StepperItem::Key {
                    key: KeyCode::KEY_1,
                    modifiers: Modifiers::default(),
                },
                StepperItem::ControllerButton {
                    button: KeyCode::BTN_SOUTH,
                },
            ]
        );
    }

    #[test]
    fn refuses_to_start_when_a_controller_button_stepper_item_is_not_a_gamepad_code() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.base]

[steppers.weapon-wheel]
name = "Weapon Wheel"
items = [{ type = "controller_button", button = "KEY_A" }]
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path)
            .expect_err("a non-gamepad controller_button Stepper item must refuse to start");
        assert!(matches!(
            err,
            ConfigError::InvalidControllerButtonStepperItem(_)
        ));

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
    fn axis_target_serializes_as_a_flat_snake_case_string() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            target: AxisTarget,
        }
        let toml = toml::to_string(&Wrapper {
            target: AxisTarget::RightStickXNeg,
        })
        .unwrap();
        assert_eq!(toml.trim(), "target = \"right_stick_x_neg\"");
        let parsed: Wrapper = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.target, AxisTarget::RightStickXNeg);
    }

    #[test]
    fn every_axis_target_has_a_distinct_abs_code_and_polarity_consistent_with_its_name() {
        assert_eq!(AxisTarget::ALL.len(), 17);
        assert_eq!(AxisTarget::LeftTrigger.polarity(), None);
        assert_eq!(
            AxisTarget::LeftStickXPos.polarity(),
            Some(AxisPolarity::Positive)
        );
        assert_eq!(
            AxisTarget::LeftStickXNeg.polarity(),
            Some(AxisPolarity::Negative)
        );
        assert_eq!(
            AxisTarget::LeftStickXPos.abs_code(),
            AxisTarget::LeftStickXNeg.abs_code(),
            "a signed axis's two halves must share one ABS_* code"
        );
        // Every unsigned target's code is unique to it; every signed axis's
        // code is shared by exactly its own two halves — 5 + 6 = 11 distinct
        // codes across the 17 targets.
        let mut codes: Vec<_> = AxisTarget::ALL.iter().map(|t| t.abs_code()).collect();
        codes.sort_by_key(|c| c.0);
        codes.dedup();
        assert_eq!(codes.len(), 11);
    }

    #[test]
    fn resolve_axis_value_is_zero_below_release_and_raw_depth_above_actuation() {
        let point = ActuationPoint {
            actuation: 128,
            release: 112,
        };
        assert_eq!(resolve_axis_value(0, point), 0);
        assert_eq!(resolve_axis_value(112, point), 0);
        assert_eq!(resolve_axis_value(128, point), 128);
        assert_eq!(resolve_axis_value(255, point), 255);
    }

    #[test]
    fn resolve_axis_value_ramps_linearly_between_release_and_actuation() {
        let point = ActuationPoint {
            actuation: 120,
            release: 100,
        };
        // Halfway between release (100) and actuation (120) ramps to
        // halfway between 0 and the actuation value (120): 60.
        assert_eq!(resolve_axis_value(110, point), 60);
    }

    #[test]
    fn a_pre_ticket_71_config_defaults_empty_axis_maps() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let profile = &config.profiles["Default"];
        assert!(profile.axis_base.is_empty());
        assert!(profile.axis_held.is_empty());
    }

    #[test]
    fn parses_an_axis_assignment_shape_on_both_layers() {
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.axis_base]
grid_r1c1 = "left_trigger"

[profiles.Default.axis_held]
grid_r2c2 = "right_stick_x_pos"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let profile = &config.profiles["Default"];
        assert_eq!(
            profile.axis_layer(Layer::Base)[&Input::Grid(1, 1)],
            AxisTarget::LeftTrigger
        );
        assert_eq!(
            profile.axis_layer(Layer::Held)[&Input::Grid(2, 2)],
            AxisTarget::RightStickXPos
        );
    }

    #[test]
    fn axis_assignments_survive_a_full_write_and_reparse_round_trip() {
        let (_dir, path) = temp_config_path();
        let mut config = Config::seed();
        let profile = config.active_profile_mut().unwrap();
        profile
            .axis_base
            .insert(Input::Grid(1, 1), AxisTarget::Brake);

        write(&path, &config).unwrap();
        let reparsed = load_or_seed(&path).unwrap();

        assert_eq!(reparsed, config);
    }

    #[test]
    fn refuses_to_start_when_an_axis_assignment_targets_a_non_grid_input() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.axis_base]
mode_key = "left_trigger"
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path)
            .expect_err("an Axis assignment on a non-Grid Input must refuse to start");
        assert!(matches!(err, ConfigError::InvalidAxisInput(input) if input == "mode_key"));
    }

    #[test]
    fn refuses_to_start_when_an_input_has_both_an_axis_assignment_and_a_binding_on_the_same_layer()
    {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.axis_base]
grid_r1c1 = "left_trigger"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path)
            .expect_err("an Input with both an Axis assignment and a Binding must refuse to start");
        assert!(matches!(err, ConfigError::AxisBindingConflict(input) if input == "grid_r1c1"));
    }

    #[test]
    fn refuses_to_start_when_an_input_has_both_an_axis_assignment_and_chord_membership_on_the_same_layer()
     {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.axis_base]
grid_r1c1 = "left_trigger"

[profiles.Default.chords_base."grid_r1c1+grid_r1c2"]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_C" }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path).expect_err(
            "an Input with both an Axis assignment and Chord membership must refuse to start",
        );
        assert!(matches!(err, ConfigError::AxisChordConflict(input) if input == "grid_r1c1"));
    }

    #[test]
    fn an_axis_assignment_on_a_different_layer_than_a_conflicting_binding_is_allowed() {
        // Ticket 59 §2: the same physical grid key may be Axis-assigned on
        // one Layer and carry an ordinary Binding on the other — only a
        // same-Layer conflict is refused.
        let toml = r#"
schema_version = 1
active_profile = "Default"

[profiles.Default.axis_base]
grid_r1c1 = "left_trigger"

[profiles.Default.held.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            config.profiles["Default"].axis_base[&Input::Grid(1, 1)],
            AxisTarget::LeftTrigger
        );
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

    #[test]
    fn refuses_to_start_when_an_analog_repeat_binding_is_on_a_non_grid_input() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.base.mode_key]
trigger = "analog_repeat"
action = { type = "keypress", key = "KEY_A" }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path)
            .expect_err("an Analog-repeat Binding on a non-Grid Input must refuse to start");
        assert!(matches!(err, ConfigError::InvalidAnalogRepeatInput(input) if input == "mode_key"));

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn refuses_to_start_when_a_chord_binding_is_analog_repeat() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.chords_base."grid_r1c1+grid_r1c2"]
trigger = "analog_repeat"
action = { type = "keypress", key = "KEY_A" }
"#;
        fs::write(&path, original).unwrap();

        let err =
            load_or_seed(&path).expect_err("an Analog-repeat Chord Binding must refuse to start");
        assert!(matches!(err, ConfigError::InvalidChordAnalogRepeat));

        assert_eq!(fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn refuses_to_start_when_a_release_point_is_not_below_its_actuation_point() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default]
default_actuation = { actuation = 100, release = 120 }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path)
            .expect_err("a Release point at or above Actuation must refuse to start");
        assert!(matches!(err, ConfigError::ReleaseNotBelowActuation(locus) if locus == "default"));
    }

    #[test]
    fn refuses_to_start_when_an_actuation_override_key_is_not_a_grid_input() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.actuation_overrides]
mode_key = { actuation = 200, release = 150 }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path)
            .expect_err("an actuation override on a non-Grid Input must refuse to start");
        assert!(
            matches!(err, ConfigError::InvalidActuationOverrideInput(input) if input == "mode_key")
        );
    }

    #[test]
    fn refuses_to_start_when_two_chords_on_a_layer_are_in_a_subset_superset_relationship() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.chords_base."grid_r1c1+grid_r1c2"]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_A" }

[profiles.Default.chords_base."grid_r1c1+grid_r1c2+grid_r1c3"]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_B" }
"#;
        fs::write(&path, original).unwrap();

        let err =
            load_or_seed(&path).expect_err("a subset/superset Chord pair must refuse to start");
        assert!(matches!(err, ConfigError::ChordMemberSetConflict { .. }));
    }

    #[test]
    fn refuses_to_start_when_a_profile_switch_targets_a_missing_profile() {
        let (_dir, path) = temp_config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = r#"schema_version = 1
active_profile = "Default"

[profiles.Default.base.grid_r1c1]
trigger = "fire_once"
action = { type = "profile_switch", target = "Gaming" }
"#;
        fs::write(&path, original).unwrap();

        let err = load_or_seed(&path)
            .expect_err("a ProfileSwitch naming a missing Profile must refuse to start");
        assert!(
            matches!(err, ConfigError::UnknownProfileSwitchTarget(target) if target == "Gaming")
        );
    }

    // --- `config::validate` (ticket 04) ---------------------------------
    //
    // One synchronous case per structural invariant `validate` owns — no
    // tokio, no tempfile. The `parse` tests above now exercise `validate`
    // transitively; this module is the direct, exhaustive surface.
    mod validate_invariants {
        use super::*;

        fn profile(config: &mut Config) -> &mut Profile {
            config.profiles.get_mut(DEFAULT_PROFILE_NAME).unwrap()
        }

        fn keypress() -> Binding {
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: KeyCode::KEY_A,
                },
            }
        }

        fn chord(members: impl IntoIterator<Item = Input>) -> ChordKey {
            ChordKey::new(members.into_iter().collect())
        }

        fn stepper_def(items: Vec<StepperItem>) -> StepperDef {
            StepperDef {
                name: "S".to_string(),
                items,
            }
        }

        #[test]
        fn the_seed_config_satisfies_every_invariant() {
            validate(&Config::seed()).expect("Config::seed() must pass validate");
        }

        struct Case {
            invariant: &'static str,
            break_it: fn(&mut Config),
            matches: fn(&ConfigError) -> bool,
        }

        #[test]
        fn each_structural_invariant_has_a_dedicated_rejection() {
            let cases = [
                Case {
                    invariant: "unknown active_profile",
                    break_it: |c| c.active_profile = "ghost".to_string(),
                    matches: |e| matches!(e, ConfigError::InvalidActiveProfile(n) if n == "ghost"),
                },
                Case {
                    invariant: "held/toggled ProfileSwitch Binding",
                    break_it: |c| {
                        profile(c).base.insert(
                            Input::Grid(1, 1),
                            Binding {
                                trigger: TriggerMode::HoldToRepeat,
                                action: Action::ProfileSwitch {
                                    target: DEFAULT_PROFILE_NAME.to_string(),
                                },
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::InvalidProfileSwitchTrigger),
                },
                Case {
                    invariant: "ControllerButton outside the gamepad allowlist",
                    break_it: |c| {
                        profile(c).base.insert(
                            Input::Grid(1, 1),
                            Binding {
                                trigger: TriggerMode::HoldToRepeat,
                                action: Action::ControllerButton {
                                    button: KeyCode::KEY_A,
                                },
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::InvalidControllerButton(_)),
                },
                Case {
                    invariant: "Fire-once ControllerButton Binding",
                    break_it: |c| {
                        profile(c).base.insert(
                            Input::Grid(1, 1),
                            Binding {
                                trigger: TriggerMode::FireOnce,
                                action: Action::ControllerButton {
                                    button: KeyCode::BTN_SOUTH,
                                },
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::InvalidControllerButtonTrigger),
                },
                Case {
                    invariant: "dangling Macro reference",
                    break_it: |c| {
                        profile(c).base.insert(
                            Input::Grid(1, 1),
                            Binding {
                                trigger: TriggerMode::HoldToRepeat,
                                action: Action::Macro {
                                    macro_id: MacroId::from("ghost"),
                                },
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::UnknownMacro(_)),
                },
                Case {
                    invariant: "dangling Stepper reference",
                    break_it: |c| {
                        profile(c).base.insert(
                            Input::Grid(1, 1),
                            Binding {
                                trigger: TriggerMode::HoldToRepeat,
                                action: Action::Step {
                                    stepper: StepperId::from("ghost"),
                                    direction: StepDirection::Forward,
                                },
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::UnknownStepper(_)),
                },
                Case {
                    invariant: "Toggle Step Binding",
                    break_it: |c| {
                        c.steppers.insert(
                            StepperId::from("s"),
                            stepper_def(vec![StepperItem::Key {
                                key: KeyCode::KEY_1,
                                modifiers: Modifiers::default(),
                            }]),
                        );
                        profile(c).base.insert(
                            Input::Grid(1, 1),
                            Binding {
                                trigger: TriggerMode::Toggle,
                                action: Action::Step {
                                    stepper: StepperId::from("s"),
                                    direction: StepDirection::Forward,
                                },
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::InvalidStepTrigger),
                },
                Case {
                    invariant: "non-gamepad ControllerButton Stepper item",
                    break_it: |c| {
                        c.steppers.insert(
                            StepperId::from("s"),
                            stepper_def(vec![StepperItem::ControllerButton {
                                button: KeyCode::KEY_A,
                            }]),
                        );
                    },
                    matches: |e| matches!(e, ConfigError::InvalidControllerButtonStepperItem(_)),
                },
                Case {
                    invariant: "Analog-repeat on a non-grid Input",
                    break_it: |c| {
                        profile(c).base.insert(
                            Input::ModeKey,
                            Binding {
                                trigger: TriggerMode::AnalogRepeat,
                                action: Action::Keypress {
                                    modifiers: Modifiers::default(),
                                    key: KeyCode::KEY_A,
                                },
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::InvalidAnalogRepeatInput(i) if i == "mode_key"),
                },
                Case {
                    invariant: "Analog-repeat Chord Binding",
                    break_it: |c| {
                        profile(c).chords_base.insert(
                            chord([Input::Grid(1, 1), Input::Grid(1, 2)]),
                            Binding {
                                trigger: TriggerMode::AnalogRepeat,
                                action: Action::Keypress {
                                    modifiers: Modifiers::default(),
                                    key: KeyCode::KEY_A,
                                },
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::InvalidChordAnalogRepeat),
                },
                Case {
                    invariant: "ProfileSwitch Chord Binding",
                    break_it: |c| {
                        profile(c).chords_base.insert(
                            chord([Input::Grid(1, 1), Input::Grid(1, 2)]),
                            Binding {
                                trigger: TriggerMode::FireOnce,
                                action: Action::ProfileSwitch {
                                    target: DEFAULT_PROFILE_NAME.to_string(),
                                },
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::InvalidChordProfileSwitch),
                },
                Case {
                    invariant: "Axis assignment on a non-grid Input",
                    break_it: |c| {
                        profile(c)
                            .axis_base
                            .insert(Input::ModeKey, AxisTarget::LeftTrigger);
                    },
                    matches: |e| matches!(e, ConfigError::InvalidAxisInput(i) if i == "mode_key"),
                },
                Case {
                    invariant: "Axis assignment and Binding on one Input/Layer",
                    break_it: |c| {
                        let p = profile(c);
                        p.axis_base
                            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);
                        p.base.insert(Input::Grid(1, 1), keypress());
                    },
                    matches: |e| matches!(e, ConfigError::AxisBindingConflict(i) if i == "grid_r1c1"),
                },
                Case {
                    invariant: "Axis assignment and Chord membership on one Input/Layer",
                    break_it: |c| {
                        let p = profile(c);
                        p.axis_base
                            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);
                        p.chords_base
                            .insert(chord([Input::Grid(1, 1), Input::Grid(1, 2)]), keypress());
                    },
                    matches: |e| matches!(e, ConfigError::AxisChordConflict(i) if i == "grid_r1c1"),
                },
                Case {
                    invariant: "release >= actuation on the Profile default",
                    break_it: |c| {
                        profile(c).default_actuation = ActuationPoint {
                            actuation: 100,
                            release: 120,
                        };
                    },
                    matches: |e| matches!(e, ConfigError::ReleaseNotBelowActuation(l) if l == "default"),
                },
                Case {
                    invariant: "release >= actuation on an override",
                    break_it: |c| {
                        profile(c).actuation_overrides.insert(
                            Input::Grid(1, 1),
                            ActuationPoint {
                                actuation: 100,
                                release: 120,
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::ReleaseNotBelowActuation(l) if l == "grid_r1c1"),
                },
                Case {
                    invariant: "actuation override on a non-grid Input",
                    break_it: |c| {
                        profile(c).actuation_overrides.insert(
                            Input::ModeKey,
                            ActuationPoint {
                                actuation: 200,
                                release: 150,
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::InvalidActuationOverrideInput(i) if i == "mode_key"),
                },
                Case {
                    invariant: "dangling ProfileSwitch target",
                    break_it: |c| {
                        profile(c).base.insert(
                            Input::Grid(1, 1),
                            Binding {
                                trigger: TriggerMode::FireOnce,
                                action: Action::ProfileSwitch {
                                    target: "ghost".to_string(),
                                },
                            },
                        );
                    },
                    matches: |e| matches!(e, ConfigError::UnknownProfileSwitchTarget(t) if t == "ghost"),
                },
                Case {
                    invariant: "subset/superset Chord pair on one Layer",
                    break_it: |c| {
                        let p = profile(c);
                        p.chords_base
                            .insert(chord([Input::Grid(1, 1), Input::Grid(1, 2)]), keypress());
                        p.chords_base.insert(
                            chord([Input::Grid(1, 1), Input::Grid(1, 2), Input::Grid(1, 3)]),
                            keypress(),
                        );
                    },
                    matches: |e| matches!(e, ConfigError::ChordMemberSetConflict { .. }),
                },
                Case {
                    invariant: "stored Chord with fewer than two members",
                    break_it: |c| {
                        profile(c)
                            .chords_base
                            .insert(chord([Input::Grid(1, 1)]), keypress());
                    },
                    matches: |e| matches!(e, ConfigError::ChordTooFewMembers(_)),
                },
                Case {
                    invariant: "empty / whitespace-only Profile name",
                    break_it: |c| {
                        c.profiles.insert("   ".to_string(), Profile::default());
                    },
                    matches: |e| matches!(e, ConfigError::EmptyProfileName),
                },
            ];

            for case in cases {
                let mut config = Config::seed();
                (case.break_it)(&mut config);
                let err = validate(&config).expect_err(case.invariant);
                assert!(
                    (case.matches)(&err),
                    "{}: validate returned the wrong error: {err:?}",
                    case.invariant
                );
            }
        }
    }

    // --- `persist_edit` (ticket 03) --------------------------------------

    #[tokio::test]
    async fn persist_edit_closure_error_leaves_config_and_disk_untouched() {
        let (_dir, path) = temp_config_path();
        let mut config = Config::seed();
        let before = config.clone();

        let result: Result<(), ConfigError> = persist_edit(&mut config, &path, |c| {
            c.force_digital = !c.force_digital;
            Err(ConfigError::MissingSchemaVersion)
        })
        .await;

        assert!(matches!(result, Err(ConfigError::MissingSchemaVersion)));
        assert_eq!(config, before, "the closure's mutation must be rolled back");
        assert!(!path.exists(), "a failed edit must not write config.toml");
    }

    #[tokio::test]
    async fn persist_edit_write_failure_restores_config_and_reports_io_error() {
        let dir = tempfile::tempdir().unwrap();
        // A regular file where `write` expects a directory — `create_dir_all`
        // on its parent then fails, so the persist itself errors.
        let blocker = dir.path().join("not-a-dir");
        fs::write(&blocker, b"blocker").unwrap();
        let path = blocker.join("config.toml");

        let mut config = Config::seed();
        let before = config.clone();

        let result: Result<(), ConfigError> = persist_edit(&mut config, &path, |c| {
            c.force_digital = !c.force_digital;
            Ok(())
        })
        .await;

        assert!(matches!(result, Err(ConfigError::Io(_))));
        assert_eq!(
            config, before,
            "an unwritable config.toml must roll the in-memory edit back"
        );
    }

    #[tokio::test]
    async fn persist_edit_success_writes_the_file_and_returns_the_closure_value() {
        let (_dir, path) = temp_config_path();
        let mut config = Config::seed();

        let result: Result<u32, ConfigError> = persist_edit(&mut config, &path, |c| {
            c.force_digital = true;
            Ok(42)
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert!(config.force_digital);
        let reloaded = load_or_seed(&path).expect("config.toml must have been written");
        assert!(
            reloaded.force_digital,
            "the persisted file must reflect the edit"
        );
    }
}
