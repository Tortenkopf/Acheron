// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The config-transaction module (ticket 05): one pure function, `plan`,
//! decides what a requested mutation does to the stored `Config` — it applies
//! the mutation to a clone, runs `config::validate` against the result, and
//! describes the post-commit effects the dispatch task must run — with no
//! I/O, no async, and no channels. `apply` is the thin async wrapper that
//! `plan`s, then `config::persist`s the planned `Config`, then assigns it on
//! success (rollback is just "don't assign").
//!
//! This is the third step on the path tickets 03 and 04 started: ticket 03
//! made *edit + persist* atomic (`config::persist_edit`), ticket 04
//! single-sourced *validation* on that path (`config::validate`), and this
//! ticket lifts the whole transaction — edit, validate, persist, and the
//! post-commit effect derivation — out of `dispatch.rs` and into one deep,
//! pure, synchronously-testable module. `config::validate` is unchanged and
//! stays the single invariant point; the `handle_command` arms that used to
//! carry preconditions and rollback are now purely mechanical translation.

use std::path::Path;

use crate::command::CommandError;
use crate::config::{
    self, Action, ActuationPoint, AxisTarget, Binding, ChordKey, Config, Layer, MacroId,
    MacroStepDto, ModeKeyRole, Profile, StepDirection, StepperId, StepperItem,
};
use crate::input::Input;

/// A single requested mutation to the stored `Config` — one data-only variant
/// per mutating `Command` (24), carrying the same fields minus the `reply`
/// sender. `GetConfig` / `GetState` / `StopAllToggles` have no `Edit`: they
/// never touch `Config`, so they stay wholly in `dispatch`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Edit {
    SetBinding {
        input: Input,
        layer: Layer,
        binding: Binding,
    },
    ClearBinding {
        input: Input,
        layer: Layer,
    },
    SetModeKeyRole {
        role: ModeKeyRole,
    },
    CreateProfile {
        name: String,
    },
    DeleteProfile {
        name: String,
    },
    RenameProfile {
        old_name: String,
        new_name: String,
    },
    SwitchProfile {
        name: String,
    },
    SetActuationPoint {
        input: Input,
        actuation: u8,
        release: u8,
    },
    ClearActuationPoint {
        input: Input,
    },
    SetDefaultActuation {
        actuation: u8,
        release: u8,
    },
    ResetActuationPoints,
    SetForceDigital {
        force: bool,
    },
    CreateMacro {
        name: String,
        steps: Vec<MacroStepDto>,
    },
    RenameMacro {
        macro_id: MacroId,
        new_name: String,
    },
    DeleteMacro {
        macro_id: MacroId,
    },
    SetMacroSteps {
        macro_id: MacroId,
        steps: Vec<MacroStepDto>,
    },
    CreateStepper {
        name: String,
        items: Vec<StepperItem>,
    },
    RenameStepper {
        stepper_id: StepperId,
        new_name: String,
    },
    DeleteStepper {
        stepper_id: StepperId,
    },
    SetStepperItems {
        stepper_id: StepperId,
        items: Vec<StepperItem>,
    },
    SetChordBinding {
        inputs: std::collections::BTreeSet<Input>,
        layer: Layer,
        binding: Binding,
    },
    ClearChordBinding {
        inputs: std::collections::BTreeSet<Input>,
        layer: Layer,
    },
    SetAxisAssignment {
        input: Input,
        layer: Layer,
        target: AxisTarget,
    },
    ClearAxisAssignment {
        input: Input,
        layer: Layer,
    },
}

/// A post-commit effect the caller must run — described here by `plan`,
/// performed by the dispatch task against the runtime state it owns
/// (`dispatch::run_effects` + its private `EffectCtx`). `edit` never runs
/// these itself: it has no injector, no channels, and no async.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Republish the active Profile's resolved Actuation-point snapshot
    /// (ticket 18 §5) into the live-Depth grid task's watch channel.
    RepublishActuation,
    /// Recompute and re-emit every `ABS_*` code the given Layer's
    /// Axis-assignment map touches. A no-op in `run_effects` when `layer`
    /// isn't the currently-active Layer (the check `plan` can't make).
    RecomputeAxes { layer: Layer },
    /// Drop the given Input's live axis contribution from `AxisState`.
    ForgetAxisContribution(Input),
    /// Tell the capture supervisor to swap the live capture source (ticket
    /// 23) — `SetForceDigital`'s only side effect.
    SignalCaptureMode(bool),
    /// Force-stop the running Toggle on the given Input, if any.
    StopToggle(Input),
    /// Force-stop every running Toggle.
    StopAllToggles,
    /// Force-stop every running Analog-repeat task.
    StopAllAnalogRepeats,
    /// Center every live axis output and clear `AxisState`.
    ResetAxisOutputs,
    /// Drop the given Stepper's Daemon-side runtime cursor.
    DropStepperCursor(StepperId),
    /// Clamp the given Stepper's runtime cursor to `len - 1`, if it has one.
    ClampStepperCursor { stepper: StepperId, len: usize },
    /// Emit `ActiveProfileChanged(name)`.
    AnnounceProfileChange(String),
}

/// The freshly-minted id a `CreateMacro` / `CreateStepper` mints — the D-Bus
/// reply carries it back to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CreatedId {
    Macro(MacroId),
    Stepper(StepperId),
}

/// Everything `plan` derives beyond the resulting `Config` itself: the
/// post-commit effects to run, and (for the two create commands) the id to
/// hand back over D-Bus.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Outcome {
    pub(crate) effects: Vec<Effect>,
    /// Set only by `CreateMacro` / `CreateStepper`.
    pub(crate) created: Option<CreatedId>,
}

/// The deep module. Clones `config`, applies `edit` to the clone, runs
/// `config::validate` against the result, and returns the new `Config` by
/// value plus the effects to run. No I/O, no async — rollback is just
/// dropping the clone.
///
/// Operation preconditions (`NotFound`, `AlreadyExists`, "can't delete the
/// active Profile", "still-referenced Macro/Stepper", blank create/rename
/// name) are explicit early-return `Err` in each arm, with their existing
/// messages preserved verbatim. Structural invariants of the resulting
/// `Config` stay in `config::validate`, run once here at the end.
pub(crate) fn plan(config: &Config, edit: Edit) -> Result<(Config, Outcome), CommandError> {
    let mut next = config.clone();
    let mut effects = Vec::new();
    let mut created = None;

    match edit {
        Edit::SetBinding {
            input,
            layer,
            binding,
        } => {
            // Ticket 03's Answer: assigning a Stepper list to a new Input
            // silently moves it off its old one, in either keyspace (ticket
            // 40 widened it to Chords) — at most one Input *or* Chord may
            // carry a given (stepper, direction) at a time.
            if let Action::Step { stepper, direction } = &binding.action {
                let active_profile = next.active_profile.clone();
                take_stepper_direction_elsewhere(
                    &mut next,
                    stepper,
                    *direction,
                    Some((&active_profile, layer, input)),
                );
                take_stepper_direction_elsewhere_from_chords(&mut next, stepper, *direction, None);
            }
            active_profile_mut(&mut next)
                .layer_mut(layer)
                .insert(input, binding);
        }
        Edit::ClearBinding { input, layer } => {
            if active_profile_mut(&mut next)
                .layer_mut(layer)
                .remove(&input)
                .is_none()
            {
                return Err(CommandError::NotFound);
            }
        }
        Edit::SetModeKeyRole { role } => {
            active_profile_mut(&mut next).mode_key_role = role;
            if role == ModeKeyRole::LayerSwitch {
                // Leaving `Bound`: a Toggle can only ever have been started
                // on the Mode key while `Bound`. Once `LayerSwitch` takes
                // over, `handle_event` intercepts every `Input::ModeKey`
                // press before the stop-toggle check, so a still-running one
                // would become permanently unstoppable via that key.
                effects.push(Effect::StopToggle(Input::ModeKey));
            }
        }
        Edit::CreateProfile { name } => {
            if next.profiles.contains_key(&name) {
                return Err(CommandError::AlreadyExists);
            }
            next.profiles.insert(name, Profile::default());
        }
        Edit::DeleteProfile { name } => {
            if name == next.active_profile {
                return Err(CommandError::InvalidRequest(
                    "cannot delete the active Profile".to_string(),
                ));
            }
            if profile_switch_references(&next, &name) {
                return Err(CommandError::InvalidRequest(format!(
                    "Profile {name:?} is still referenced by a Profile Switch Binding"
                )));
            }
            if next.profiles.remove(&name).is_none() {
                return Err(CommandError::NotFound);
            }
        }
        Edit::RenameProfile { old_name, new_name } => {
            if !next.profiles.contains_key(&old_name) {
                return Err(CommandError::NotFound);
            }
            if old_name != new_name && next.profiles.contains_key(&new_name) {
                return Err(CommandError::AlreadyExists);
            }
            let profile = next
                .profiles
                .remove(&old_name)
                .expect("just checked old_name exists");
            next.profiles.insert(new_name.clone(), profile);
            if next.active_profile == old_name {
                next.active_profile = new_name.clone();
            }
            cascade_rename_profile_switch_targets(&mut next, &old_name, &new_name);
        }
        Edit::SwitchProfile { name } => {
            if !next.profiles.contains_key(&name) {
                return Err(CommandError::NotFound);
            }
            next.active_profile = name.clone();
            // Ordering matters: Toggles and Analog-repeats stop, the new
            // Profile's Actuation snapshot goes out, axes reset, then the
            // signal fires — all after the D-Bus reply, uniformly, which is
            // what deletes `SwitchProfile`'s old bespoke reply-before-signal
            // reasoning (the hazard it dodged is now the default shape).
            //
            // `StopAllToggles` clears only the per-Input `toggles` map, never
            // `dispatch::ChordRuntime`'s `ChordKey`-keyed firings/toggles —
            // an active Chord Toggle survives a Profile switch today. That is
            // pre-existing behaviour, preserved unchanged by post-release
            // ticket 07's mechanical carve; whether a Chord Toggle *should*
            // outlive a Profile switch is an open question for the domain
            // owner, not something to settle here.
            effects.push(Effect::StopAllToggles);
            effects.push(Effect::RepublishActuation);
            effects.push(Effect::ResetAxisOutputs);
            effects.push(Effect::StopAllAnalogRepeats);
            effects.push(Effect::AnnounceProfileChange(name));
        }
        Edit::SetActuationPoint {
            input,
            actuation,
            release,
        } => {
            active_profile_mut(&mut next)
                .actuation_overrides
                .insert(input, ActuationPoint { actuation, release });
            effects.push(Effect::RepublishActuation);
        }
        Edit::ClearActuationPoint { input } => {
            active_profile_mut(&mut next)
                .actuation_overrides
                .remove(&input);
            effects.push(Effect::RepublishActuation);
        }
        Edit::SetDefaultActuation { actuation, release } => {
            active_profile_mut(&mut next).default_actuation = ActuationPoint { actuation, release };
            effects.push(Effect::RepublishActuation);
        }
        Edit::ResetActuationPoints => {
            active_profile_mut(&mut next).actuation_overrides.clear();
            effects.push(Effect::RepublishActuation);
        }
        Edit::SetForceDigital { force } => {
            next.force_digital = force;
            effects.push(Effect::SignalCaptureMode(force));
        }
        Edit::CreateMacro { name, steps } => {
            if name.trim().is_empty() {
                return Err(CommandError::InvalidRequest(
                    "Macro name can't be empty".to_string(),
                ));
            }
            let macro_id = config::unique_macro_id(&next, &name);
            next.macros
                .insert(macro_id.clone(), config::MacroDef { name, steps });
            created = Some(CreatedId::Macro(macro_id));
        }
        Edit::RenameMacro { macro_id, new_name } => {
            if new_name.trim().is_empty() {
                return Err(CommandError::InvalidRequest(
                    "Macro name can't be empty".to_string(),
                ));
            }
            let def = next
                .macros
                .get_mut(&macro_id)
                .ok_or(CommandError::NotFound)?;
            def.name = new_name;
        }
        Edit::DeleteMacro { macro_id } => {
            if macro_references(&next, &macro_id) {
                return Err(CommandError::InvalidRequest(format!(
                    "{macro_id:?} is still referenced by a Macro Binding"
                )));
            }
            if next.macros.remove(&macro_id).is_none() {
                return Err(CommandError::NotFound);
            }
        }
        Edit::SetMacroSteps { macro_id, steps } => {
            let def = next
                .macros
                .get_mut(&macro_id)
                .ok_or(CommandError::NotFound)?;
            def.steps = steps;
        }
        Edit::CreateStepper { name, items } => {
            if name.trim().is_empty() {
                return Err(CommandError::InvalidRequest(
                    "Stepper name can't be empty".to_string(),
                ));
            }
            let stepper_id = config::unique_stepper_id(&next, &name);
            next.steppers
                .insert(stepper_id.clone(), config::StepperDef { name, items });
            created = Some(CreatedId::Stepper(stepper_id));
        }
        Edit::RenameStepper {
            stepper_id,
            new_name,
        } => {
            if new_name.trim().is_empty() {
                return Err(CommandError::InvalidRequest(
                    "Stepper name can't be empty".to_string(),
                ));
            }
            let def = next
                .steppers
                .get_mut(&stepper_id)
                .ok_or(CommandError::NotFound)?;
            def.name = new_name;
        }
        Edit::DeleteStepper { stepper_id } => {
            if stepper_references(&next, &stepper_id) {
                return Err(CommandError::InvalidRequest(format!(
                    "{stepper_id:?} is still referenced by a Step Binding"
                )));
            }
            if next.steppers.remove(&stepper_id).is_none() {
                return Err(CommandError::NotFound);
            }
            // The runtime cursor is Daemon-side-only state — dropped so a
            // later `CreateStepper` landing on the same freed slug starts at
            // the list's first item rather than inheriting a stale position.
            effects.push(Effect::DropStepperCursor(stepper_id));
        }
        Edit::SetStepperItems { stepper_id, items } => {
            let def = next
                .steppers
                .get_mut(&stepper_id)
                .ok_or(CommandError::NotFound)?;
            def.items = items;
            let new_len = def.items.len();
            if new_len == 0 {
                // Nothing left to point at — dropping the entry lets
                // `resolve_step`'s zero-items short-circuit and `GetState`'s
                // own default both agree on "index 0" for free.
                effects.push(Effect::DropStepperCursor(stepper_id));
            } else {
                // A shrink can leave a stored cursor past the new end —
                // clamped so `GetState`'s reported position never outruns
                // the list it's a position *in*.
                effects.push(Effect::ClampStepperCursor {
                    stepper: stepper_id,
                    len: new_len,
                });
            }
        }
        Edit::SetChordBinding {
            inputs,
            layer,
            binding,
        } => {
            let key = ChordKey::new(inputs);
            if let Action::Step { stepper, direction } = &binding.action {
                let active_profile = next.active_profile.clone();
                take_stepper_direction_elsewhere(&mut next, stepper, *direction, None);
                take_stepper_direction_elsewhere_from_chords(
                    &mut next,
                    stepper,
                    *direction,
                    Some((&active_profile, layer, &key)),
                );
            }
            active_profile_mut(&mut next)
                .chords_mut(layer)
                .insert(key, binding);
        }
        Edit::ClearChordBinding { inputs, layer } => {
            let key = ChordKey::new(inputs);
            if active_profile_mut(&mut next)
                .chords_mut(layer)
                .remove(&key)
                .is_none()
            {
                return Err(CommandError::NotFound);
            }
        }
        Edit::SetAxisAssignment {
            input,
            layer,
            target,
        } => {
            // Ticket 59 §2's mutual exclusion: atomically clear any existing
            // Binding *and* any Chord membership for (layer, input) alongside
            // the insert.
            active_profile_mut(&mut next)
                .layer_mut(layer)
                .remove(&input);
            let chords = active_profile_mut(&mut next).chords_mut(layer);
            let member_keys: Vec<ChordKey> = chords
                .keys()
                .filter(|key| key.members().contains(&input))
                .cloned()
                .collect();
            for key in member_keys {
                chords.remove(&key);
            }
            active_profile_mut(&mut next)
                .axis_layer_mut(layer)
                .insert(input, target);
            effects.push(Effect::RecomputeAxes { layer });
        }
        Edit::ClearAxisAssignment { input, layer } => {
            if active_profile_mut(&mut next)
                .axis_layer_mut(layer)
                .remove(&input)
                .is_none()
            {
                return Err(CommandError::NotFound);
            }
            effects.push(Effect::ForgetAxisContribution(input));
            effects.push(Effect::RecomputeAxes { layer });
        }
    }

    config::validate(&next)?;
    Ok((next, Outcome { effects, created }))
}

/// The thin async wrapper: `plan`, then `config::persist` the planned
/// `Config`, then assign it on success. Supersedes `config::persist_edit` —
/// its snapshot-and-restore collapses to "don't assign on failure", since
/// `plan` never touches the caller's `config` and a failed `persist` leaves
/// it equally untouched.
pub(crate) async fn apply(
    config: &mut Config,
    path: &Path,
    edit: Edit,
) -> Result<Outcome, CommandError> {
    let (next, outcome) = plan(config, edit)?;
    config::persist(&next, path).await?;
    *config = next;
    Ok(outcome)
}

/// The `Default` Profile always exists — `load_or_seed` refuses to start a
/// `Config` whose `active_profile` doesn't name a real Profile.
fn active_profile_mut(config: &mut Config) -> &mut Profile {
    config
        .active_profile_mut()
        .expect("load_or_seed validates active_profile names a real profile")
}

/// Every `Action::ProfileSwitch { target }` across every Profile's Base/Held
/// Binding map that targets `old_name` is repointed at `new_name` (ticket
/// 34) — a rename must not silently leave a dangling or wrong reference
/// behind.
fn cascade_rename_profile_switch_targets(config: &mut Config, old_name: &str, new_name: &str) {
    for profile in config.profiles.values_mut() {
        for bindings in [&mut profile.base, &mut profile.held] {
            for binding in bindings.values_mut() {
                if let Action::ProfileSwitch { target } = &mut binding.action
                    && target == old_name
                {
                    *target = new_name.to_string();
                }
            }
        }
    }
}

/// Whether any Profile's Base/Held Binding map contains an
/// `Action::ProfileSwitch { target }` naming `name` — `DeleteProfile`
/// refuses while this is true, so a dangling reference can never exist
/// (ticket 34).
fn profile_switch_references(config: &Config, name: &str) -> bool {
    config.profiles.values().any(|profile| {
        [&profile.base, &profile.held].into_iter().any(|bindings| {
            bindings.values().any(|binding| {
                matches!(&binding.action, Action::ProfileSwitch { target } if target == name)
            })
        })
    })
}

/// Whether any Profile's Base/Held *or Chord* Binding contains an
/// `Action::Macro { macro_id }` naming `macro_id` — `DeleteMacro` refuses
/// while this is true (ticket 15/51/40).
fn macro_references(config: &Config, macro_id: &MacroId) -> bool {
    config.profiles.values().any(|profile| {
        config::profile_all_bindings(profile).any(
            |binding| matches!(&binding.action, Action::Macro { macro_id: id } if id == macro_id),
        )
    })
}

/// `macro_references`'s exact mirror for the Stepper library — whether any
/// Profile's Base/Held *or Chord* Binding contains an `Action::Step {
/// stepper }` naming `stepper_id` (either direction). `DeleteStepper`
/// refuses while this is true (ticket 03/54/40).
fn stepper_references(config: &Config, stepper_id: &StepperId) -> bool {
    config.profiles.values().any(|profile| {
        config::profile_all_bindings(profile)
            .any(|binding| matches!(&binding.action, Action::Step { stepper, .. } if stepper == stepper_id))
    })
}

/// Removes every other Binding, across every Profile/Layer, whose `Action`
/// is `Action::Step { stepper, direction }` matching the one being set —
/// ticket 03's Answer: "assigning it to a new pair silently moves it off its
/// old one." `except` (the Input currently being written) is left untouched
/// even if it already matches; `None` steals from every matching Input.
/// `take_stepper_direction_elsewhere_from_chords` is the exact mirror for a
/// Chord's own Step action — both keyspaces are swept together whenever
/// either kind of caller claims one.
fn take_stepper_direction_elsewhere(
    config: &mut Config,
    stepper: &StepperId,
    direction: StepDirection,
    except: Option<(&str, Layer, Input)>,
) {
    for (profile_name, profile) in config.profiles.iter_mut() {
        for layer in [Layer::Base, Layer::Held] {
            let bindings = profile.layer_mut(layer);
            let matching: Vec<Input> = bindings
                .iter()
                .filter(|(input, binding)| {
                    except != Some((profile_name.as_str(), layer, **input))
                        && matches!(
                            &binding.action,
                            Action::Step { stepper: s, direction: d }
                                if s == stepper && *d == direction
                        )
                })
                .map(|(&input, _)| input)
                .collect();
            for input in matching {
                bindings.remove(&input);
            }
        }
    }
}

/// `take_stepper_direction_elsewhere`'s exact mirror for a Profile's Chord
/// Bindings (ticket 40). `except` is the `ChordKey` currently being written,
/// left untouched even if it already matches; `None` steals from every
/// matching Chord.
fn take_stepper_direction_elsewhere_from_chords(
    config: &mut Config,
    stepper: &StepperId,
    direction: StepDirection,
    except: Option<(&str, Layer, &ChordKey)>,
) {
    for (profile_name, profile) in config.profiles.iter_mut() {
        for layer in [Layer::Base, Layer::Held] {
            let chords = profile.chords_mut(layer);
            let matching: Vec<ChordKey> = chords
                .iter()
                .filter(|(key, binding)| {
                    except != Some((profile_name.as_str(), layer, key))
                        && matches!(
                            &binding.action,
                            Action::Step { stepper: s, direction: d }
                                if s == stepper && *d == direction
                        )
                })
                .map(|(key, _)| key.clone())
                .collect();
            for key in matching {
                chords.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Action, AxisTarget, Binding, DEFAULT_PROFILE_NAME, MacroDef, MacroStepDto, Modifiers,
        Profile, StepDirection, StepperDef, StepperItem, TriggerMode,
    };
    use evdev::KeyCode;
    use std::collections::BTreeSet;

    fn seed() -> Config {
        Config::seed()
    }

    fn active(config: &mut Config) -> &mut Profile {
        let name = config.active_profile.clone();
        config.profiles.get_mut(&name).unwrap()
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

    fn chord(members: impl IntoIterator<Item = Input>) -> BTreeSet<Input> {
        members.into_iter().collect()
    }

    fn step(stepper: &str, direction: StepDirection) -> Binding {
        Binding {
            trigger: TriggerMode::FireOnce,
            action: Action::Step {
                stepper: StepperId::from(stepper),
                direction,
            },
        }
    }

    fn with_stepper(config: &mut Config, id: &str) -> StepperId {
        let sid = StepperId::from(id);
        config.steppers.insert(
            sid.clone(),
            StepperDef {
                name: id.to_string(),
                items: vec![
                    StepperItem::Key {
                        key: KeyCode::KEY_1,
                        modifiers: Modifiers::default(),
                    },
                    StepperItem::Key {
                        key: KeyCode::KEY_2,
                        modifiers: Modifiers::default(),
                    },
                ],
            },
        );
        sid
    }

    fn with_macro(config: &mut Config, id: &str) -> MacroId {
        let mid = MacroId::from(id);
        config.macros.insert(
            mid.clone(),
            MacroDef {
                name: id.to_string(),
                steps: vec![MacroStepDto::KeyDown(KeyCode::KEY_A)],
            },
        );
        mid
    }

    fn plan_ok(config: &Config, edit: Edit) -> (Config, Outcome) {
        plan(config, edit).expect("plan must succeed")
    }

    fn plan_err(config: &Config, edit: Edit) -> CommandError {
        plan(config, edit).expect_err("plan must reject the edit")
    }

    // --- resulting `Config` on the success path -------------------------------

    #[test]
    fn set_binding_inserts_into_the_active_profiles_layer() {
        let (next, outcome) = plan_ok(
            &seed(),
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: keypress(),
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)],
            keypress()
        );
        assert!(outcome.effects.is_empty());
    }

    #[test]
    fn set_binding_with_a_step_action_steals_the_direction_off_its_old_input() {
        let mut config = seed();
        with_stepper(&mut config, "wep");
        active(&mut config)
            .base
            .insert(Input::Grid(1, 1), step("wep", StepDirection::Forward));

        let (next, _) = plan_ok(
            &config,
            Edit::SetBinding {
                input: Input::Grid(2, 2),
                layer: Layer::Base,
                binding: step("wep", StepDirection::Forward),
            },
        );
        let base = &next.profiles[DEFAULT_PROFILE_NAME].base;
        assert!(!base.contains_key(&Input::Grid(1, 1)), "old owner cleared");
        assert!(base.contains_key(&Input::Grid(2, 2)), "new owner set");
    }

    #[test]
    fn set_binding_and_set_chord_binding_steal_a_step_direction_across_both_keyspaces() {
        let mut config = seed();
        with_stepper(&mut config, "wep");
        // A Chord currently owns (wep, Forward)...
        let members = chord([Input::Grid(1, 1), Input::Grid(1, 2)]);
        config
            .profiles
            .get_mut(DEFAULT_PROFILE_NAME)
            .unwrap()
            .chords_base
            .insert(
                ChordKey::new(members.clone()),
                step("wep", StepDirection::Forward),
            );

        // ...a plain SetBinding for the same (stepper, direction) takes it.
        let (next, _) = plan_ok(
            &config,
            Edit::SetBinding {
                input: Input::Grid(3, 3),
                layer: Layer::Base,
                binding: step("wep", StepDirection::Forward),
            },
        );
        assert!(next.profiles[DEFAULT_PROFILE_NAME].chords_base.is_empty());

        // ...and the reverse: a SetChordBinding takes it back off the Input.
        let mut with_input = seed();
        with_stepper(&mut with_input, "wep");
        with_input
            .profiles
            .get_mut(DEFAULT_PROFILE_NAME)
            .unwrap()
            .base
            .insert(Input::Grid(3, 3), step("wep", StepDirection::Forward));
        let (back, _) = plan_ok(
            &with_input,
            Edit::SetChordBinding {
                inputs: members,
                layer: Layer::Base,
                binding: step("wep", StepDirection::Forward),
            },
        );
        assert!(
            !back.profiles[DEFAULT_PROFILE_NAME]
                .base
                .contains_key(&Input::Grid(3, 3))
        );
    }

    #[test]
    fn delete_macro_is_rejected_when_only_a_chord_still_references_it() {
        let mut config = seed();
        let mid = with_macro(&mut config, "m");
        config
            .profiles
            .get_mut(DEFAULT_PROFILE_NAME)
            .unwrap()
            .chords_base
            .insert(
                ChordKey::new(chord([Input::Grid(1, 1), Input::Grid(1, 2)])),
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Macro {
                        macro_id: mid.clone(),
                    },
                },
            );
        assert!(matches!(
            plan_err(&config, Edit::DeleteMacro { macro_id: mid }),
            CommandError::InvalidRequest(_)
        ));
    }

    #[test]
    fn clear_binding_removes_it_and_rejects_an_absent_one() {
        let mut config = seed();
        active(&mut config)
            .base
            .insert(Input::Grid(1, 1), keypress());
        let (next, _) = plan_ok(
            &config,
            Edit::ClearBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
            },
        );
        assert!(next.profiles[DEFAULT_PROFILE_NAME].base.is_empty());

        assert!(matches!(
            plan_err(
                &seed(),
                Edit::ClearBinding {
                    input: Input::Grid(1, 1),
                    layer: Layer::Base,
                }
            ),
            CommandError::NotFound
        ));
    }

    #[test]
    fn set_binding_accepts_the_valid_shapes_config_validate_allows() {
        // Grid Input + AnalogRepeat: only the *non-grid* reject path is a
        // dedicated table row, so pin the accept path too.
        let (next, _) = plan_ok(
            &seed(),
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: Binding {
                    trigger: TriggerMode::AnalogRepeat,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: KeyCode::KEY_A,
                    },
                },
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)].trigger,
            TriggerMode::AnalogRepeat
        );

        // A ControllerButton in the gamepad allowlist is accepted.
        let (next, _) = plan_ok(
            &seed(),
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: Binding {
                    trigger: TriggerMode::HoldToRepeat,
                    action: Action::ControllerButton {
                        button: KeyCode::BTN_SOUTH,
                    },
                },
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)].action,
            Action::ControllerButton {
                button: KeyCode::BTN_SOUTH
            }
        );
    }

    #[test]
    fn set_mode_key_role_flips_the_field_and_emits_stop_toggle_only_for_layer_switch() {
        let mut config = seed();
        active(&mut config).mode_key_role = crate::config::ModeKeyRole::Bound;
        // A Held-layer binding retained while `Bound` makes it unreachable
        // (config serde `skip_serializing_if`) must survive the role flip —
        // `plan` writes only `mode_key_role`, nothing else.
        active(&mut config)
            .held
            .insert(Input::Grid(1, 1), keypress());

        let (next, outcome) = plan_ok(
            &config,
            Edit::SetModeKeyRole {
                role: crate::config::ModeKeyRole::LayerSwitch,
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].mode_key_role,
            crate::config::ModeKeyRole::LayerSwitch
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].held[&Input::Grid(1, 1)],
            keypress(),
            "Held bindings survive the role flip"
        );
        assert_eq!(outcome.effects, vec![Effect::StopToggle(Input::ModeKey)]);

        let (_, outcome) = plan_ok(
            &seed(),
            Edit::SetModeKeyRole {
                role: crate::config::ModeKeyRole::Bound,
            },
        );
        assert!(outcome.effects.is_empty());
    }

    #[test]
    fn create_and_delete_and_rename_profile_round_trip() {
        let (with_gaming, _) = plan_ok(
            &seed(),
            Edit::CreateProfile {
                name: "Gaming".to_string(),
            },
        );
        assert!(with_gaming.profiles.contains_key("Gaming"));

        let (renamed, _) = plan_ok(
            &with_gaming,
            Edit::RenameProfile {
                old_name: "Gaming".to_string(),
                new_name: "Editing".to_string(),
            },
        );
        assert!(!renamed.profiles.contains_key("Gaming"));
        assert!(renamed.profiles.contains_key("Editing"));
        assert_eq!(renamed.active_profile, DEFAULT_PROFILE_NAME);

        let (without_editing, _) = plan_ok(
            &renamed,
            Edit::DeleteProfile {
                name: "Editing".to_string(),
            },
        );
        assert!(!without_editing.profiles.contains_key("Editing"));
    }

    #[test]
    fn rename_profile_follows_active_profile_and_cascades_switch_targets() {
        let mut config = seed();
        plan(
            &config,
            Edit::CreateProfile {
                name: "Gaming".to_string(),
            },
        )
        .map(|(c, _)| config = c)
        .unwrap();
        active(&mut config).base.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::ProfileSwitch {
                    target: "Gaming".to_string(),
                },
            },
        );
        config.active_profile = "Gaming".to_string();

        let (next, _) = plan_ok(
            &config,
            Edit::RenameProfile {
                old_name: "Gaming".to_string(),
                new_name: "Renamed".to_string(),
            },
        );
        assert_eq!(next.active_profile, "Renamed");
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)].action,
            Action::ProfileSwitch {
                target: "Renamed".to_string()
            }
        );
    }

    #[test]
    fn switch_profile_sets_active_and_emits_its_ordered_effect_chain() {
        let mut config = seed();
        config
            .profiles
            .insert("Gaming".to_string(), Profile::default());

        let (next, outcome) = plan_ok(
            &config,
            Edit::SwitchProfile {
                name: "Gaming".to_string(),
            },
        );
        assert_eq!(next.active_profile, "Gaming");
        assert_eq!(
            outcome.effects,
            vec![
                Effect::StopAllToggles,
                Effect::RepublishActuation,
                Effect::ResetAxisOutputs,
                Effect::StopAllAnalogRepeats,
                Effect::AnnounceProfileChange("Gaming".to_string()),
            ]
        );
    }

    #[test]
    fn actuation_edits_all_republish_the_snapshot() {
        for edit in [
            Edit::SetActuationPoint {
                input: Input::Grid(1, 1),
                actuation: 200,
                release: 100,
            },
            Edit::ClearActuationPoint {
                input: Input::Grid(1, 1),
            },
            Edit::SetDefaultActuation {
                actuation: 200,
                release: 100,
            },
            Edit::ResetActuationPoints,
        ] {
            let (_, outcome) = plan_ok(&seed(), edit);
            assert_eq!(outcome.effects, vec![Effect::RepublishActuation]);
        }
    }

    #[test]
    fn set_force_digital_writes_the_flag_and_signals_the_supervisor() {
        let (next, outcome) = plan_ok(&seed(), Edit::SetForceDigital { force: true });
        assert!(next.force_digital);
        assert_eq!(outcome.effects, vec![Effect::SignalCaptureMode(true)]);
    }

    #[test]
    fn create_macro_mints_an_id_and_hands_it_back() {
        let (next, outcome) = plan_ok(
            &seed(),
            Edit::CreateMacro {
                name: "My Macro".to_string(),
                steps: vec![],
            },
        );
        let Some(CreatedId::Macro(id)) = outcome.created else {
            panic!("CreateMacro must set Outcome.created");
        };
        assert!(next.macros.contains_key(&id));
    }

    #[test]
    fn create_stepper_mints_an_id_and_hands_it_back() {
        let (next, outcome) = plan_ok(
            &seed(),
            Edit::CreateStepper {
                name: "My Stepper".to_string(),
                items: vec![],
            },
        );
        let Some(CreatedId::Stepper(id)) = outcome.created else {
            panic!("CreateStepper must set Outcome.created");
        };
        assert!(next.steppers.contains_key(&id));
    }

    #[test]
    fn set_binding_targets_the_named_layer_and_clear_actuation_on_a_non_grid_input_is_a_no_op() {
        let (next, _) = plan_ok(
            &seed(),
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Held,
                binding: keypress(),
            },
        );
        let profile = &next.profiles[DEFAULT_PROFILE_NAME];
        assert!(
            !profile.base.contains_key(&Input::Grid(1, 1)),
            "Base untouched"
        );
        assert!(profile.held.contains_key(&Input::Grid(1, 1)));

        // `reject_non_grid_input` is gone (ticket 04): clearing an override
        // that was never there — a non-grid key never has one — is a silent
        // no-op success, not a rejection.
        let (unchanged, outcome) = plan_ok(
            &seed(),
            Edit::ClearActuationPoint {
                input: Input::ModeKey,
            },
        );
        assert_eq!(unchanged, seed());
        assert_eq!(outcome.effects, vec![Effect::RepublishActuation]);
    }

    #[test]
    fn delete_stepper_drops_its_runtime_cursor() {
        let mut config = seed();
        let sid = with_stepper(&mut config, "wep");
        let (_, outcome) = plan_ok(
            &config,
            Edit::DeleteStepper {
                stepper_id: sid.clone(),
            },
        );
        assert_eq!(outcome.effects, vec![Effect::DropStepperCursor(sid)]);
    }

    #[test]
    fn set_stepper_items_clamps_or_drops_the_cursor_by_the_new_length() {
        let mut config = seed();
        let sid = with_stepper(&mut config, "wep");

        let (_, shrink) = plan_ok(
            &config,
            Edit::SetStepperItems {
                stepper_id: sid.clone(),
                items: vec![StepperItem::Key {
                    key: KeyCode::KEY_1,
                    modifiers: Modifiers::default(),
                }],
            },
        );
        assert_eq!(
            shrink.effects,
            vec![Effect::ClampStepperCursor {
                stepper: sid.clone(),
                len: 1
            }]
        );

        let (_, emptied) = plan_ok(
            &config,
            Edit::SetStepperItems {
                stepper_id: sid.clone(),
                items: vec![],
            },
        );
        assert_eq!(emptied.effects, vec![Effect::DropStepperCursor(sid)]);
    }

    #[test]
    fn rename_and_set_steps_are_pure_field_writes() {
        let mut config = seed();
        let mid = with_macro(&mut config, "m");
        let (next, _) = plan_ok(
            &config,
            Edit::RenameMacro {
                macro_id: mid.clone(),
                new_name: "renamed".to_string(),
            },
        );
        assert_eq!(next.macros[&mid].name, "renamed");
        let (next, _) = plan_ok(
            &config,
            Edit::SetMacroSteps {
                macro_id: mid.clone(),
                steps: vec![MacroStepDto::KeyUp(KeyCode::KEY_B)],
            },
        );
        assert_eq!(
            next.macros[&mid].steps,
            vec![MacroStepDto::KeyUp(KeyCode::KEY_B)]
        );
    }

    #[test]
    fn set_chord_binding_inserts_by_member_set_and_clear_removes_it() {
        let members = [Input::Grid(1, 1), Input::Grid(1, 2)];
        let (next, _) = plan_ok(
            &seed(),
            Edit::SetChordBinding {
                inputs: chord(members),
                layer: Layer::Base,
                binding: keypress(),
            },
        );
        assert_eq!(next.profiles[DEFAULT_PROFILE_NAME].chords_base.len(), 1);

        let (cleared, _) = plan_ok(
            &next,
            Edit::ClearChordBinding {
                inputs: chord(members),
                layer: Layer::Base,
            },
        );
        assert!(
            cleared.profiles[DEFAULT_PROFILE_NAME]
                .chords_base
                .is_empty()
        );
    }

    #[test]
    fn set_axis_assignment_clears_a_colliding_binding_and_asks_for_a_recompute() {
        let mut config = seed();
        active(&mut config)
            .base
            .insert(Input::Grid(1, 1), keypress());

        let (next, outcome) = plan_ok(
            &config,
            Edit::SetAxisAssignment {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                target: AxisTarget::LeftTrigger,
            },
        );
        let profile = &next.profiles[DEFAULT_PROFILE_NAME];
        assert!(!profile.base.contains_key(&Input::Grid(1, 1)));
        assert_eq!(
            profile.axis_base[&Input::Grid(1, 1)],
            AxisTarget::LeftTrigger
        );
        assert_eq!(
            outcome.effects,
            vec![Effect::RecomputeAxes { layer: Layer::Base }]
        );
    }

    #[test]
    fn clear_axis_assignment_forgets_the_contribution_then_recomputes() {
        let mut config = seed();
        active(&mut config)
            .axis_base
            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);

        let (next, outcome) = plan_ok(
            &config,
            Edit::ClearAxisAssignment {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
            },
        );
        assert!(next.profiles[DEFAULT_PROFILE_NAME].axis_base.is_empty());
        assert_eq!(
            outcome.effects,
            vec![
                Effect::ForgetAxisContribution(Input::Grid(1, 1)),
                Effect::RecomputeAxes { layer: Layer::Base },
            ]
        );

        assert!(matches!(
            plan_err(
                &seed(),
                Edit::ClearAxisAssignment {
                    input: Input::Grid(1, 1),
                    layer: Layer::Base,
                }
            ),
            CommandError::NotFound
        ));
    }

    // --- preconditions and invariants, one row each ---------------------------

    struct Case {
        name: &'static str,
        setup: fn(&mut Config),
        edit: fn() -> Edit,
        matches: fn(&CommandError) -> bool,
    }

    fn is_invalid(err: &CommandError, needle: &str) -> bool {
        matches!(err, CommandError::InvalidRequest(m) if m.contains(needle))
    }

    #[test]
    fn every_precondition_and_invariant_path_has_a_dedicated_rejection() {
        let cases = [
            Case {
                name: "SetBinding: ProfileSwitch paired with a non-fire-once trigger (invariant)",
                setup: |_| {},
                edit: || Edit::SetBinding {
                    input: Input::Grid(1, 1),
                    layer: Layer::Base,
                    binding: Binding {
                        trigger: TriggerMode::Toggle,
                        action: Action::ProfileSwitch {
                            target: DEFAULT_PROFILE_NAME.to_string(),
                        },
                    },
                },
                matches: |e| is_invalid(e, "fire_once"),
            },
            Case {
                name: "SetBinding: analog_repeat on a non-grid Input (invariant)",
                setup: |_| {},
                edit: || Edit::SetBinding {
                    input: Input::ModeKey,
                    layer: Layer::Base,
                    binding: Binding {
                        trigger: TriggerMode::AnalogRepeat,
                        action: Action::Keypress {
                            modifiers: Modifiers::default(),
                            key: KeyCode::KEY_A,
                        },
                    },
                },
                matches: |e| is_invalid(e, "analog_repeat"),
            },
            Case {
                name: "CreateProfile: name already taken (precondition)",
                setup: |c| {
                    c.profiles.insert("Gaming".to_string(), Profile::default());
                },
                edit: || Edit::CreateProfile {
                    name: "Gaming".to_string(),
                },
                matches: |e| matches!(e, CommandError::AlreadyExists),
            },
            Case {
                name: "CreateProfile: blank name (invariant)",
                setup: |_| {},
                edit: || Edit::CreateProfile {
                    name: "   ".to_string(),
                },
                matches: |e| is_invalid(e, "whitespace-only name"),
            },
            Case {
                name: "DeleteProfile: the active Profile (precondition)",
                setup: |_| {},
                edit: || Edit::DeleteProfile {
                    name: DEFAULT_PROFILE_NAME.to_string(),
                },
                matches: |e| is_invalid(e, "cannot delete the active Profile"),
            },
            Case {
                name: "DeleteProfile: still referenced by a ProfileSwitch (precondition)",
                setup: |c| {
                    c.profiles.insert("Gaming".to_string(), Profile::default());
                    active(c).base.insert(
                        Input::Grid(1, 1),
                        Binding {
                            trigger: TriggerMode::FireOnce,
                            action: Action::ProfileSwitch {
                                target: "Gaming".to_string(),
                            },
                        },
                    );
                },
                edit: || Edit::DeleteProfile {
                    name: "Gaming".to_string(),
                },
                matches: |e| is_invalid(e, "still referenced by a Profile Switch Binding"),
            },
            Case {
                name: "DeleteProfile: unknown name (precondition)",
                setup: |_| {},
                edit: || Edit::DeleteProfile {
                    name: "Ghost".to_string(),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "RenameProfile: unknown old_name (precondition)",
                setup: |_| {},
                edit: || Edit::RenameProfile {
                    old_name: "Ghost".to_string(),
                    new_name: "Whatever".to_string(),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "RenameProfile: new_name already taken (precondition)",
                setup: |c| {
                    c.profiles.insert("Gaming".to_string(), Profile::default());
                },
                edit: || Edit::RenameProfile {
                    old_name: "Gaming".to_string(),
                    new_name: DEFAULT_PROFILE_NAME.to_string(),
                },
                matches: |e| matches!(e, CommandError::AlreadyExists),
            },
            Case {
                name: "SwitchProfile: unknown name (precondition)",
                setup: |_| {},
                edit: || Edit::SwitchProfile {
                    name: "Ghost".to_string(),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "SetActuationPoint: release >= actuation (invariant)",
                setup: |_| {},
                edit: || Edit::SetActuationPoint {
                    input: Input::Grid(1, 1),
                    actuation: 100,
                    release: 120,
                },
                matches: |e| is_invalid(e, "release point at or above"),
            },
            Case {
                name: "SetActuationPoint: non-grid Input (invariant)",
                setup: |_| {},
                edit: || Edit::SetActuationPoint {
                    input: Input::ModeKey,
                    actuation: 200,
                    release: 100,
                },
                matches: |e| is_invalid(e, "actuation override"),
            },
            Case {
                name: "SetDefaultActuation: release >= actuation (invariant)",
                setup: |_| {},
                edit: || Edit::SetDefaultActuation {
                    actuation: 100,
                    release: 100,
                },
                matches: |e| is_invalid(e, "default"),
            },
            Case {
                name: "CreateMacro: blank name (precondition)",
                setup: |_| {},
                edit: || Edit::CreateMacro {
                    name: "  ".to_string(),
                    steps: vec![],
                },
                matches: |e| is_invalid(e, "Macro name can't be empty"),
            },
            Case {
                name: "RenameMacro: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::RenameMacro {
                    macro_id: MacroId::from("ghost"),
                    new_name: "x".to_string(),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "DeleteMacro: still referenced (precondition)",
                setup: |c| {
                    let mid = with_macro(c, "m");
                    active(c).base.insert(
                        Input::Grid(1, 1),
                        Binding {
                            trigger: TriggerMode::FireOnce,
                            action: Action::Macro { macro_id: mid },
                        },
                    );
                },
                edit: || Edit::DeleteMacro {
                    macro_id: MacroId::from("m"),
                },
                matches: |e| is_invalid(e, "still referenced by a Macro Binding"),
            },
            Case {
                name: "DeleteMacro: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::DeleteMacro {
                    macro_id: MacroId::from("ghost"),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "SetMacroSteps: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::SetMacroSteps {
                    macro_id: MacroId::from("ghost"),
                    steps: vec![],
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "CreateStepper: blank name (precondition)",
                setup: |_| {},
                edit: || Edit::CreateStepper {
                    name: "  ".to_string(),
                    items: vec![],
                },
                matches: |e| is_invalid(e, "Stepper name can't be empty"),
            },
            Case {
                name: "RenameStepper: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::RenameStepper {
                    stepper_id: StepperId::from("ghost"),
                    new_name: "x".to_string(),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "DeleteStepper: still referenced (precondition)",
                setup: |c| {
                    let sid = with_stepper(c, "wep");
                    active(c).base.insert(
                        Input::Grid(1, 1),
                        step(sid.as_str(), StepDirection::Forward),
                    );
                },
                edit: || Edit::DeleteStepper {
                    stepper_id: StepperId::from("wep"),
                },
                matches: |e| is_invalid(e, "still referenced by a Step Binding"),
            },
            Case {
                name: "DeleteStepper: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::DeleteStepper {
                    stepper_id: StepperId::from("ghost"),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "SetStepperItems: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::SetStepperItems {
                    stepper_id: StepperId::from("ghost"),
                    items: vec![],
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "SetChordBinding: fewer than two members (invariant)",
                setup: |_| {},
                edit: || Edit::SetChordBinding {
                    inputs: chord([Input::Grid(1, 1)]),
                    layer: Layer::Base,
                    binding: keypress(),
                },
                matches: |e| is_invalid(e, "fewer than two member"),
            },
            Case {
                name: "SetChordBinding: ProfileSwitch action (invariant)",
                setup: |_| {},
                edit: || Edit::SetChordBinding {
                    inputs: chord([Input::Grid(1, 1), Input::Grid(1, 2)]),
                    layer: Layer::Base,
                    binding: Binding {
                        trigger: TriggerMode::FireOnce,
                        action: Action::ProfileSwitch {
                            target: DEFAULT_PROFILE_NAME.to_string(),
                        },
                    },
                },
                matches: |e| is_invalid(e, "cannot be profile_switch"),
            },
            Case {
                name: "ClearChordBinding: no such member set (precondition)",
                setup: |_| {},
                edit: || Edit::ClearChordBinding {
                    inputs: chord([Input::Grid(1, 1), Input::Grid(1, 2)]),
                    layer: Layer::Base,
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "SetAxisAssignment: non-grid Input (invariant)",
                setup: |_| {},
                edit: || Edit::SetAxisAssignment {
                    input: Input::ModeKey,
                    layer: Layer::Base,
                    target: AxisTarget::LeftTrigger,
                },
                matches: |e| is_invalid(e, "only Grid Inputs"),
            },
        ];

        for case in cases {
            let mut config = seed();
            (case.setup)(&mut config);
            let before = config.clone();
            let err = plan(&config, (case.edit)()).expect_err(case.name);
            assert!((case.matches)(&err), "{}: wrong error {err:?}", case.name);
            assert_eq!(config, before, "{}: caller's Config was mutated", case.name);
        }
    }

    // --- `apply` (async wrapper) --------------------------------------------

    #[tokio::test]
    async fn apply_persists_the_planned_config_and_returns_its_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = seed();

        let outcome = apply(&mut config, &path, Edit::SetForceDigital { force: true })
            .await
            .expect("apply must succeed");

        assert_eq!(outcome.effects, vec![Effect::SignalCaptureMode(true)]);
        assert!(config.force_digital);
        let reloaded = config::load_or_seed(&path).expect("config.toml must exist");
        assert!(
            reloaded.force_digital,
            "the persisted file reflects the edit"
        );
    }

    #[tokio::test]
    async fn apply_leaves_config_untouched_when_the_persist_fails() {
        let dir = tempfile::tempdir().unwrap();
        // A regular file where a directory is expected — `create_dir_all` on
        // the parent then fails, so `persist` errors.
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"blocker").unwrap();
        let path = blocker.join("config.toml");

        let mut config = seed();
        let before = config.clone();

        let err = apply(&mut config, &path, Edit::SetForceDigital { force: true })
            .await
            .expect_err("an unwritable path must fail");

        assert!(matches!(err, CommandError::IoError(_)));
        assert_eq!(config, before, "a failed persist rolls nothing forward");
    }
}
