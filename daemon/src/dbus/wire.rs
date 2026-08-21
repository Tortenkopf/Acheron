//! The D-Bus wire encoding conventions issue 08 settled: `Input` reuses its
//! TOML `Display`/`FromStr` form (a plain string); `Action`/`MacroStep`
//! marshal as `a{sv}` dicts with a `"type"` tag key (hand-written
//! `Serialize`/`Deserialize`, not a JSON-string fallback, so the payload
//! stays introspectable via `dbus-send`/`d-feet`); `Binding` bundles its
//! `TriggerMode` and `Action` into one flat `a{sv}` rather than nesting one
//! inside the other. `GetConfig()`'s return recursively reuses these same
//! conventions (profiles -> layers -> bindings -> `Input` string keys ->
//! `Binding` dict), so `config_to_dict` is built directly on `binding_to_dict`.
//!
//! `Action::Macro` and every `TriggerMode` round-trip through this encoding
//! unchanged from issue 08's original design — ticket 17 wired their real
//! firing semantics into the dispatch task without touching the wire format
//! at all.

use std::collections::HashMap;

use evdev::KeyCode;
use zbus::zvariant::{OwnedValue, Value};

use crate::command::State;
use crate::config::{
    Action, ActuationPoint, Binding, Config, Layer, MacroStepDto, ModeKeyRole, Modifiers, Profile,
    TriggerMode,
};

/// The `a{sv}` shape every `Action`/`MacroStep`/`Binding`/`Config` entity
/// marshals as.
pub type Dict = HashMap<String, OwnedValue>;

/// Wraps a value that converts into a `zvariant::Value` as an `OwnedValue`.
/// Infallible for every type built in this module — `try_to_owned` can only
/// fail for `Value::Fd`, and no `Fd` is ever constructed here.
fn scalar<T>(value: T) -> OwnedValue
where
    T: Into<Value<'static>>,
{
    OwnedValue::try_from(value.into()).expect(
        "only zvariant::Value::Fd can fail to become an OwnedValue, and none are built here",
    )
}

fn get<'a>(dict: &'a Dict, key: &str) -> Result<&'a OwnedValue, String> {
    dict.get(key)
        .ok_or_else(|| format!("missing required field {key:?}"))
}

fn get_str<'a>(dict: &'a Dict, key: &str) -> Result<&'a str, String> {
    <&str>::try_from(get(dict, key)?).map_err(|_| format!("field {key:?} is not a string"))
}

fn get_u64(dict: &Dict, key: &str) -> Result<u64, String> {
    u64::try_from(get(dict, key)?).map_err(|_| format!("field {key:?} is not an integer"))
}

fn get_dict_vec(dict: &Dict, key: &str) -> Result<Vec<Dict>, String> {
    let items: Vec<OwnedValue> = Vec::try_from(get(dict, key)?.clone())
        .map_err(|_| format!("field {key:?} is not an array"))?;
    items
        .into_iter()
        .map(|item| {
            Dict::try_from(item).map_err(|_| format!("field {key:?} contains a non-dict element"))
        })
        .collect()
}

fn key_to_string(key: KeyCode) -> String {
    format!("{key:?}")
}

fn key_from_str(s: &str) -> Result<KeyCode, String> {
    s.parse()
        .map_err(|_| format!("{s:?} is not a valid key code"))
}

/// `Layer` marshals as its own flat lowercase string, produced by
/// `Layer::as_str` directly (no wrapper here — unlike `TriggerMode`/
/// `ModeKeyRole`, `Layer` already exposes its wire string as an inherent
/// method), reused identically for `SetBinding`/`ClearBinding`'s Layer
/// argument and `GetState()`/`ActiveLayerChanged`'s payload (issue 08 /
/// ticket 18).
pub fn layer_from_str(s: &str) -> Result<Layer, String> {
    match s {
        "base" => Ok(Layer::Base),
        "held" => Ok(Layer::Held),
        other => Err(format!("{other:?} is not a valid Layer")),
    }
}

fn mode_key_role_str(role: ModeKeyRole) -> &'static str {
    match role {
        ModeKeyRole::LayerSwitch => "layer_switch",
        ModeKeyRole::Bound => "bound",
    }
}

pub fn mode_key_role_from_str(s: &str) -> Result<ModeKeyRole, String> {
    match s {
        "layer_switch" => Ok(ModeKeyRole::LayerSwitch),
        "bound" => Ok(ModeKeyRole::Bound),
        other => Err(format!("{other:?} is not a valid mode-key role")),
    }
}

fn trigger_mode_str(trigger: TriggerMode) -> &'static str {
    match trigger {
        TriggerMode::FireOnce => "fire_once",
        TriggerMode::HoldToRepeat => "hold_to_repeat",
        TriggerMode::Toggle => "toggle",
    }
}

fn trigger_mode_from_str(s: &str) -> Result<TriggerMode, String> {
    match s {
        "fire_once" => Ok(TriggerMode::FireOnce),
        "hold_to_repeat" => Ok(TriggerMode::HoldToRepeat),
        "toggle" => Ok(TriggerMode::Toggle),
        other => Err(format!("{other:?} is not a valid trigger mode")),
    }
}

/// Only the modifiers actually held are listed (e.g. `["ctrl", "shift"]`),
/// per issue 08's example payload — no false entries for the others.
fn modifiers_to_vec(modifiers: Modifiers) -> Vec<String> {
    let mut names = Vec::new();
    if modifiers.ctrl {
        names.push("ctrl".to_string());
    }
    if modifiers.shift {
        names.push("shift".to_string());
    }
    if modifiers.alt {
        names.push("alt".to_string());
    }
    if modifiers.super_key {
        names.push("super".to_string());
    }
    names
}

fn modifiers_from_slice(names: &[String]) -> Result<Modifiers, String> {
    let mut modifiers = Modifiers::default();
    for name in names {
        match name.as_str() {
            "ctrl" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" => modifiers.alt = true,
            "super" => modifiers.super_key = true,
            other => return Err(format!("{other:?} is not a valid modifier name")),
        }
    }
    Ok(modifiers)
}

fn macro_step_to_dict(step: &MacroStepDto) -> Dict {
    let mut dict = Dict::new();
    match step {
        MacroStepDto::KeyDown(key) => {
            dict.insert("type".to_string(), scalar("key_down".to_string()));
            dict.insert("key".to_string(), scalar(key_to_string(*key)));
        }
        MacroStepDto::KeyUp(key) => {
            dict.insert("type".to_string(), scalar("key_up".to_string()));
            dict.insert("key".to_string(), scalar(key_to_string(*key)));
        }
        MacroStepDto::Delay(ms) => {
            dict.insert("type".to_string(), scalar("delay_ms".to_string()));
            dict.insert("ms".to_string(), scalar(*ms));
        }
    }
    dict
}

fn macro_step_from_dict(dict: &Dict) -> Result<MacroStepDto, String> {
    match get_str(dict, "type")? {
        "key_down" => Ok(MacroStepDto::KeyDown(key_from_str(get_str(dict, "key")?)?)),
        "key_up" => Ok(MacroStepDto::KeyUp(key_from_str(get_str(dict, "key")?)?)),
        "delay_ms" => Ok(MacroStepDto::Delay(get_u64(dict, "ms")?)),
        other => Err(format!("{other:?} is not a valid MacroStep type")),
    }
}

pub fn action_to_dict(action: &Action) -> Dict {
    let mut dict = Dict::new();
    match action {
        Action::Keypress { modifiers, key } => {
            dict.insert("type".to_string(), scalar("keypress".to_string()));
            dict.insert("key".to_string(), scalar(key_to_string(*key)));
            let modifiers = modifiers_to_vec(*modifiers);
            if !modifiers.is_empty() {
                dict.insert("modifiers".to_string(), scalar(modifiers));
            }
        }
        Action::Macro { steps } => {
            dict.insert("type".to_string(), scalar("macro".to_string()));
            let steps: Vec<Dict> = steps.iter().map(macro_step_to_dict).collect();
            dict.insert("steps".to_string(), scalar(steps));
        }
        Action::ProfileSwitch { target } => {
            dict.insert("type".to_string(), scalar("profile_switch".to_string()));
            dict.insert("target".to_string(), scalar(target.clone()));
        }
    }
    dict
}

pub fn action_from_dict(dict: &Dict) -> Result<Action, String> {
    match get_str(dict, "type")? {
        "keypress" => {
            let key = key_from_str(get_str(dict, "key")?)?;
            let modifiers = match dict.get("modifiers") {
                Some(value) => {
                    let names: Vec<String> = Vec::try_from(value.clone()).map_err(|_| {
                        "field \"modifiers\" is not an array of strings".to_string()
                    })?;
                    modifiers_from_slice(&names)?
                }
                None => Modifiers::default(),
            };
            Ok(Action::Keypress { modifiers, key })
        }
        "macro" => {
            let steps = get_dict_vec(dict, "steps")?
                .iter()
                .map(macro_step_from_dict)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Action::Macro { steps })
        }
        "profile_switch" => Ok(Action::ProfileSwitch {
            target: get_str(dict, "target")?.to_string(),
        }),
        other => Err(format!("{other:?} is not a valid Action type")),
    }
}

/// Bundles `Binding`'s `TriggerMode` and `Action` into one flat `a{sv}` —
/// "a single `SetBinding` call carries one self-contained blob rather than
/// parallel trigger/action arguments" (issue 08).
pub fn binding_to_dict(binding: &Binding) -> Dict {
    let mut dict = action_to_dict(&binding.action);
    dict.insert(
        "trigger".to_string(),
        scalar(trigger_mode_str(binding.trigger).to_string()),
    );
    dict
}

pub fn binding_from_dict(dict: &Dict) -> Result<Binding, String> {
    let trigger = trigger_mode_from_str(get_str(dict, "trigger")?)?;
    let action = action_from_dict(dict)?;
    Ok(Binding { trigger, action })
}

fn bindings_to_dict(bindings: &std::collections::HashMap<crate::input::Input, Binding>) -> Dict {
    bindings
        .iter()
        .map(|(input, binding)| (input.to_string(), scalar(binding_to_dict(binding))))
        .collect()
}

/// `ActuationPoint` marshals as a flat two-field `a{sv}` (issue 08's `Binding`
/// convention: bundle related scalars into one dict rather than two parallel
/// arguments), reused identically for a Profile's `default_actuation` and
/// each entry of `actuation_overrides`.
fn actuation_point_to_dict(point: ActuationPoint) -> Dict {
    let mut dict = Dict::new();
    dict.insert("actuation".to_string(), scalar(point.actuation));
    dict.insert("release".to_string(), scalar(point.release));
    dict
}

fn actuation_overrides_to_dict(overrides: &HashMap<crate::input::Input, ActuationPoint>) -> Dict {
    overrides
        .iter()
        .map(|(input, point)| (input.to_string(), scalar(actuation_point_to_dict(*point))))
        .collect()
}

fn profile_to_dict(profile: &Profile) -> Dict {
    let mut dict = Dict::new();
    dict.insert("base".to_string(), scalar(bindings_to_dict(&profile.base)));
    dict.insert("held".to_string(), scalar(bindings_to_dict(&profile.held)));
    dict.insert(
        "mode_key_role".to_string(),
        scalar(mode_key_role_str(profile.mode_key_role).to_string()),
    );
    // Ticket 26: still missing as of ticket 21, which deliberately deferred
    // it — the Actuation & release editor needs both to seed its markers
    // and its "reset to Profile default" affordance from the real Config
    // rather than a hardcoded guess.
    dict.insert(
        "default_actuation".to_string(),
        scalar(actuation_point_to_dict(profile.default_actuation)),
    );
    dict.insert(
        "actuation_overrides".to_string(),
        scalar(actuation_overrides_to_dict(&profile.actuation_overrides)),
    );
    dict
}

/// `GetConfig()`'s full-document return: `schema_version`/`active_profile`
/// as scalars, `profiles` recursively reusing `binding_to_dict`'s
/// conventions (issue 08); each Profile now also carries `held` alongside
/// `base` and its `mode_key_role` (ticket 18).
pub fn config_to_dict(config: &Config) -> Dict {
    let mut dict = Dict::new();
    dict.insert("schema_version".to_string(), scalar(config.schema_version));
    dict.insert(
        "active_profile".to_string(),
        scalar(config.active_profile.clone()),
    );
    let profiles: Dict = config
        .profiles
        .iter()
        .map(|(name, profile)| (name.clone(), scalar(profile_to_dict(profile))))
        .collect();
    dict.insert("profiles".to_string(), scalar(profiles));
    dict.insert("force_digital".to_string(), scalar(config.force_digital));
    dict
}

/// `GetState()`'s live runtime snapshot (ticket 25) — keyed so a new field
/// lands in existing clients for free, unlike the positional tuple it
/// replaces, which broke `app.py`'s `rebuild()` the moment `capture_mode`
/// was added (ticket 21). Flat scalars only, no nested recursion needed
/// unlike `config_to_dict`.
pub fn state_to_dict(state: &State) -> Dict {
    let mut dict = Dict::new();
    dict.insert("profile".to_string(), scalar(state.profile.clone()));
    dict.insert("layer".to_string(), scalar(state.layer.to_string()));
    dict.insert(
        "active_toggles".to_string(),
        scalar(
            state
                .active_toggles
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
        ),
    );
    dict.insert(
        "device_connected".to_string(),
        scalar(state.device_connected),
    );
    dict.insert(
        "capture_mode".to_string(),
        scalar(state.capture_mode.to_string()),
    );
    dict
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict_get_string(dict: &Dict, key: &str) -> String {
        get_str(dict, key).unwrap().to_string()
    }

    #[test]
    fn keypress_action_round_trips_through_a_dict() {
        let action = Action::Keypress {
            modifiers: Modifiers {
                ctrl: true,
                shift: true,
                alt: false,
                super_key: false,
            },
            key: KeyCode::KEY_T,
        };

        let dict = action_to_dict(&action);
        assert_eq!(dict_get_string(&dict, "type"), "keypress");
        assert_eq!(dict_get_string(&dict, "key"), "KEY_T");

        let round_tripped = action_from_dict(&dict).unwrap();
        assert_eq!(round_tripped, action);
    }

    #[test]
    fn keypress_action_with_no_modifiers_omits_the_modifiers_field() {
        let action = Action::Keypress {
            modifiers: Modifiers::default(),
            key: KeyCode::KEY_F1,
        };

        let dict = action_to_dict(&action);
        assert!(!dict.contains_key("modifiers"));
        assert_eq!(action_from_dict(&dict).unwrap(), action);
    }

    #[test]
    fn macro_action_round_trips_through_a_dict() {
        let action = Action::Macro {
            steps: vec![
                MacroStepDto::KeyDown(KeyCode::KEY_A),
                MacroStepDto::Delay(50),
                MacroStepDto::KeyUp(KeyCode::KEY_A),
            ],
        };

        let dict = action_to_dict(&action);
        assert_eq!(dict_get_string(&dict, "type"), "macro");

        let round_tripped = action_from_dict(&dict).unwrap();
        assert_eq!(round_tripped, action);
    }

    #[test]
    fn profile_switch_action_round_trips_through_a_dict() {
        let action = Action::ProfileSwitch {
            target: "Gaming".to_string(),
        };

        let dict = action_to_dict(&action);
        assert_eq!(dict_get_string(&dict, "type"), "profile_switch");
        assert_eq!(dict_get_string(&dict, "target"), "Gaming");

        let round_tripped = action_from_dict(&dict).unwrap();
        assert_eq!(round_tripped, action);
    }

    #[test]
    fn binding_bundles_trigger_and_action_in_one_flat_dict() {
        let binding = Binding {
            trigger: TriggerMode::Toggle,
            action: Action::Keypress {
                modifiers: Modifiers::default(),
                key: KeyCode::KEY_F1,
            },
        };

        let dict = binding_to_dict(&binding);
        assert_eq!(dict_get_string(&dict, "trigger"), "toggle");
        assert_eq!(dict_get_string(&dict, "type"), "keypress");

        let round_tripped = binding_from_dict(&dict).unwrap();
        assert_eq!(round_tripped, binding);
    }

    #[test]
    fn every_trigger_mode_round_trips_through_its_wire_string() {
        for trigger in [
            TriggerMode::FireOnce,
            TriggerMode::HoldToRepeat,
            TriggerMode::Toggle,
        ] {
            let s = trigger_mode_str(trigger);
            assert_eq!(trigger_mode_from_str(s).unwrap(), trigger);
        }
    }

    #[test]
    fn action_from_dict_rejects_an_unknown_type_tag() {
        let mut dict = Dict::new();
        dict.insert("type".to_string(), scalar("bogus".to_string()));
        assert!(action_from_dict(&dict).is_err());
    }

    #[test]
    fn action_from_dict_rejects_a_missing_type_tag() {
        let dict = Dict::new();
        assert!(action_from_dict(&dict).is_err());
    }

    #[test]
    fn config_to_dict_nests_profiles_layers_and_bindings() {
        use crate::input::Input;
        use std::collections::HashMap as StdHashMap;

        let mut base = StdHashMap::new();
        base.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: KeyCode::KEY_F1,
                },
            },
        );
        let mut profiles = StdHashMap::new();
        profiles.insert(
            "Default".to_string(),
            Profile {
                base,
                ..Default::default()
            },
        );
        let config = Config {
            schema_version: 1,
            active_profile: "Default".to_string(),
            profiles,
            force_digital: false,
        };

        let dict = config_to_dict(&config);
        assert_eq!(
            u32::try_from(get(&dict, "schema_version").unwrap()).unwrap(),
            1
        );
        assert_eq!(dict_get_string(&dict, "active_profile"), "Default");

        let profiles_dict: Dict = get(&dict, "profiles").unwrap().clone().try_into().unwrap();
        let default_profile: Dict = profiles_dict
            .get("Default")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let base_dict: Dict = get(&default_profile, "base")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let binding_dict: Dict = base_dict
            .get("grid_r1c1")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(dict_get_string(&binding_dict, "type"), "keypress");

        // Held Layer and mode_key_role are always present, even when empty/
        // default — nothing here hides a Layer that doesn't have Bindings
        // yet (ticket 18).
        let held_dict: Dict = get(&default_profile, "held")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert!(held_dict.is_empty());
        assert_eq!(
            dict_get_string(&default_profile, "mode_key_role"),
            "layer_switch"
        );
    }

    /// Closes the gap ticket 27's live-hardware verification caught:
    /// `force_digital` was never serialized, so `binding_editor.py`'s "Force
    /// digital capture" checkbox could never reflect the Daemon's actual
    /// persisted preference on reopen — it always constructed unchecked.
    #[test]
    fn config_to_dict_serializes_force_digital() {
        let config = Config {
            schema_version: 1,
            active_profile: "Default".to_string(),
            profiles: Default::default(),
            force_digital: true,
        };
        let dict = config_to_dict(&config);
        assert!(bool::try_from(get(&dict, "force_digital").unwrap()).unwrap());
    }

    /// Ticket 26: `config_to_dict` must serialize a Profile's
    /// `default_actuation`/`actuation_overrides` — deliberately deferred by
    /// ticket 21, closing the gap this ticket's `binding_editor.py` section
    /// needs to seed its markers from the real Config.
    #[test]
    fn config_to_dict_serializes_default_actuation_and_actuation_overrides() {
        use crate::input::Input;
        use std::collections::HashMap as StdHashMap;

        let mut overrides = StdHashMap::new();
        overrides.insert(
            Input::Grid(2, 3),
            ActuationPoint {
                actuation: 200,
                release: 180,
            },
        );
        let mut profiles = StdHashMap::new();
        profiles.insert(
            "Default".to_string(),
            Profile {
                default_actuation: ActuationPoint {
                    actuation: 128,
                    release: 112,
                },
                actuation_overrides: overrides,
                ..Default::default()
            },
        );
        let config = Config {
            schema_version: 1,
            active_profile: "Default".to_string(),
            profiles,
            force_digital: false,
        };

        let dict = config_to_dict(&config);
        let profiles_dict: Dict = get(&dict, "profiles").unwrap().clone().try_into().unwrap();
        let default_profile: Dict = profiles_dict
            .get("Default")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();

        let default_actuation: Dict = get(&default_profile, "default_actuation")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(
            u8::try_from(get(&default_actuation, "actuation").unwrap()).unwrap(),
            128
        );
        assert_eq!(
            u8::try_from(get(&default_actuation, "release").unwrap()).unwrap(),
            112
        );

        let overrides_dict: Dict = get(&default_profile, "actuation_overrides")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let override_point: Dict = overrides_dict
            .get("grid_r2c3")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(
            u8::try_from(get(&override_point, "actuation").unwrap()).unwrap(),
            200
        );
        assert_eq!(
            u8::try_from(get(&override_point, "release").unwrap()).unwrap(),
            180
        );
    }

    #[test]
    fn state_to_dict_keys_every_field_by_name() {
        use crate::input::Input;

        let state = State {
            profile: "Gaming".to_string(),
            layer: "held",
            active_toggles: vec![Input::Grid(1, 1)],
            device_connected: true,
            capture_mode: "analog",
        };

        let dict = state_to_dict(&state);
        assert_eq!(dict_get_string(&dict, "profile"), "Gaming");
        assert_eq!(dict_get_string(&dict, "layer"), "held");
        assert_eq!(
            Vec::<String>::try_from(get(&dict, "active_toggles").unwrap().clone()).unwrap(),
            vec!["grid_r1c1".to_string()]
        );
        assert!(bool::try_from(get(&dict, "device_connected").unwrap()).unwrap());
        assert_eq!(dict_get_string(&dict, "capture_mode"), "analog");
    }

    #[test]
    fn every_layer_round_trips_through_its_wire_string() {
        for layer in [Layer::Base, Layer::Held] {
            let s = layer.as_str();
            assert_eq!(layer_from_str(s).unwrap(), layer);
        }
    }

    #[test]
    fn layer_from_str_rejects_an_unknown_string() {
        assert!(layer_from_str("bogus").is_err());
    }

    #[test]
    fn every_mode_key_role_round_trips_through_its_wire_string() {
        for role in [ModeKeyRole::LayerSwitch, ModeKeyRole::Bound] {
            let s = mode_key_role_str(role);
            assert_eq!(mode_key_role_from_str(s).unwrap(), role);
        }
    }
}
