// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The pure, per-Binding half of `config::validate` (post-release ticket 14).
//!
//! Every legality rule that is a function of *one* Binding, *where it sits*,
//! and the fixed device vocabulary — nothing that needs the rest of the
//! `Config` (dangling macro / stepper / profile-switch targets, the Axis
//! *conflict* checks, Chord subset/superset, …). Those stay in
//! [`super::validate`]'s whole-`Config` pass.
//!
//! Pure and synchronous — no `async`, no `Config` / `Layer` / `Profile`. That
//! purity is what lets `schema.rs` call [`check_binding`] /
//! [`check_axis_assignment`] once per truth-table cell with no synthetic
//! `Config`, and what makes `gui/acheron_gui/rules.py` a mirror of two named
//! functions rather than a matrix inferred from a 220-line wall.

use crate::input::{Input, is_gamepad_button};

use super::{Action, Binding, ConfigError, TriggerMode};

/// Where a Binding sits. The per-Binding checks that care about the `Input`
/// (analog-repeat needs a Grid key) or about being a Chord's own Binding (no
/// ProfileSwitch, no analog-repeat) have to tell these apart.
///
/// Not `Option<Input>` — the overloaded `None` is exactly the implicit
/// convention this seam kills. `rules.py` mirrors this as
/// `input_str: str | None`, with `None` == [`BindingSite::Chord`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingSite {
    Individual(Input),
    Chord,
}

/// Every per-Binding legality rule, applied in the order `validate` applied
/// them — so the one Binding that can trip two rules (a `ControllerButton`
/// naming a non-gamepad `KeyCode` *and* carrying a Fire-once trigger) still
/// reports the bad code, not the bad trigger:
///
///   1. action-payload — `ControllerButton { button }` must be a gamepad code
///   2. trigger legality — ProfileSwitch⇒FireOnce, ControllerButton≠FireOnce,
///      Step≠Toggle
///   3. site-shape — AnalogRepeat needs `Individual(Grid)`; a `Chord` Binding
///      may be neither ProfileSwitch nor analog-repeat
pub(crate) fn check_binding(site: BindingSite, binding: &Binding) -> Result<(), ConfigError> {
    // 1. action-payload
    if let Action::ControllerButton { button } = binding.action
        && !is_gamepad_button(button)
    {
        return Err(ConfigError::InvalidControllerButton(format!("{button:?}")));
    }

    // 2. trigger legality
    match binding.action {
        Action::ProfileSwitch { .. } if binding.trigger != TriggerMode::FireOnce => {
            return Err(ConfigError::InvalidProfileSwitchTrigger);
        }
        Action::ControllerButton { .. } if binding.trigger == TriggerMode::FireOnce => {
            return Err(ConfigError::InvalidControllerButtonTrigger);
        }
        Action::Step { .. } if binding.trigger == TriggerMode::Toggle => {
            return Err(ConfigError::InvalidStepTrigger);
        }
        _ => {}
    }

    // 3. site-shape
    match site {
        BindingSite::Individual(input) => {
            if binding.trigger == TriggerMode::AnalogRepeat && !matches!(input, Input::Grid(_, _)) {
                return Err(ConfigError::InvalidAnalogRepeatInput(input.to_string()));
            }
        }
        BindingSite::Chord => {
            if matches!(binding.action, Action::ProfileSwitch { .. }) {
                return Err(ConfigError::InvalidChordProfileSwitch);
            }
            if binding.trigger == TriggerMode::AnalogRepeat {
                return Err(ConfigError::InvalidChordAnalogRepeat);
            }
        }
    }

    Ok(())
}

/// The one per-assignment Axis rule: an Axis assignment is legal only on a
/// Grid `Input` — only Grid keys have Depth to drive an axis with (ticket 59
/// §1). The two Axis *conflict* rules (an Axis Input also carrying a Binding,
/// or in a Chord's member set on the same Layer) are cross-map and stay in
/// [`super::validate`].
pub(crate) fn check_axis_assignment(input: Input) -> Result<(), ConfigError> {
    if !matches!(input, Input::Grid(_, _)) {
        return Err(ConfigError::InvalidAxisInput(input.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use evdev::KeyCode;

    use super::*;
    use crate::config::{Modifiers, StepDirection};
    use crate::input::{Direction, WheelEvent};

    fn binding(trigger: TriggerMode, action: Action) -> Binding {
        Binding { trigger, action }
    }

    fn keypress() -> Action {
        Action::Keypress {
            modifiers: Modifiers::default(),
            key: KeyCode::KEY_A,
        }
    }

    fn gamepad_button() -> Action {
        Action::ControllerButton {
            button: KeyCode::BTN_SOUTH,
        }
    }

    fn non_gamepad_button() -> Action {
        Action::ControllerButton {
            button: KeyCode::KEY_A,
        }
    }

    fn macro_action() -> Action {
        Action::Macro {
            macro_id: crate::config::MacroId::from("m"),
        }
    }

    fn step() -> Action {
        Action::Step {
            stepper: crate::config::StepperId::from("s"),
            direction: StepDirection::Forward,
        }
    }

    fn profile_switch() -> Action {
        Action::ProfileSwitch {
            target: "P2".to_string(),
        }
    }

    const GRID: BindingSite = BindingSite::Individual(Input::Grid(1, 1));
    const NON_GRID: BindingSite = BindingSite::Individual(Input::ModeKey);
    const CHORD: BindingSite = BindingSite::Chord;

    const ALL_TRIGGERS: [TriggerMode; 4] = [
        TriggerMode::FireOnce,
        TriggerMode::HoldToRepeat,
        TriggerMode::Toggle,
        TriggerMode::AnalogRepeat,
    ];

    /// The exact verdict `check_binding` owes each
    /// `site × action-kind × trigger` cell, transcribed from the rules the
    /// ticket locks (payload → trigger → site-shape). `None` == `Ok(())`.
    fn expected(site: BindingSite, kind: &str, trigger: TriggerMode) -> Option<ConfigError> {
        // 1. action-payload
        if kind == "bad_controller_button" {
            return Some(ConfigError::InvalidControllerButton(format!(
                "{:?}",
                KeyCode::KEY_A
            )));
        }
        // 2. trigger legality
        if kind == "profile_switch" && trigger != TriggerMode::FireOnce {
            return Some(ConfigError::InvalidProfileSwitchTrigger);
        }
        if kind == "controller_button" && trigger == TriggerMode::FireOnce {
            return Some(ConfigError::InvalidControllerButtonTrigger);
        }
        if kind == "step" && trigger == TriggerMode::Toggle {
            return Some(ConfigError::InvalidStepTrigger);
        }
        // 3. site-shape
        match site {
            BindingSite::Individual(Input::Grid(_, _)) => None,
            BindingSite::Individual(_) => (trigger == TriggerMode::AnalogRepeat)
                .then(|| ConfigError::InvalidAnalogRepeatInput("mode_key".to_string())),
            BindingSite::Chord => {
                if kind == "profile_switch" {
                    Some(ConfigError::InvalidChordProfileSwitch)
                } else if trigger == TriggerMode::AnalogRepeat {
                    Some(ConfigError::InvalidChordAnalogRepeat)
                } else {
                    None
                }
            }
        }
    }

    fn action_for(kind: &str) -> Action {
        match kind {
            "keypress" => keypress(),
            "controller_button" => gamepad_button(),
            "bad_controller_button" => non_gamepad_button(),
            "macro" => macro_action(),
            "step" => step(),
            "profile_switch" => profile_switch(),
            other => panic!("unknown kind {other:?}"),
        }
    }

    #[test]
    fn check_binding_truth_table() {
        let kinds = [
            "keypress",
            "controller_button",
            "bad_controller_button",
            "macro",
            "step",
            "profile_switch",
        ];
        for site in [GRID, NON_GRID, CHORD] {
            for kind in kinds {
                for trigger in ALL_TRIGGERS {
                    let got = check_binding(site, &binding(trigger, action_for(kind)));
                    match expected(site, kind, trigger) {
                        None => assert!(
                            got.is_ok(),
                            "{site:?} / {kind} / {trigger:?} should be legal, got {got:?}"
                        ),
                        Some(want) => {
                            let err = got.expect_err(&format!(
                                "{site:?} / {kind} / {trigger:?} should be rejected as {want:?}"
                            ));
                            assert_eq!(
                                std::mem::discriminant(&err),
                                std::mem::discriminant(&want),
                                "{site:?} / {kind} / {trigger:?}: got {err:?}, want {want:?}"
                            );
                            assert_eq!(
                                err.to_string(),
                                want.to_string(),
                                "{site:?} / {kind} / {trigger:?}: locus mismatch"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn controller_button_reports_the_bad_code_before_the_bad_trigger() {
        // A non-gamepad code *and* a Fire-once trigger — the payload check
        // wins, matching the order `validate` ran the two blocks in.
        let err = check_binding(GRID, &binding(TriggerMode::FireOnce, non_gamepad_button()))
            .expect_err("both rules broken");
        assert!(matches!(err, ConfigError::InvalidControllerButton(_)));
    }

    #[test]
    fn check_axis_assignment_allows_only_grid_inputs() {
        check_axis_assignment(Input::Grid(1, 1)).expect("a Grid Input carries an axis");
        check_axis_assignment(Input::Grid(4, 5)).expect("a Grid Input carries an axis");

        for (input, locus) in [
            (Input::ModeKey, "mode_key"),
            (Input::Thumbstick(Direction::Up), "thumbstick_up"),
            (Input::Wheel(WheelEvent::MiddleClick), "wheel_middle"),
        ] {
            let err =
                check_axis_assignment(input).expect_err("a non-Grid Input cannot carry an axis");
            assert!(
                matches!(&err, ConfigError::InvalidAxisInput(l) if l == locus),
                "got {err:?} for {input}"
            );
        }
    }
}
