// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! Axis-assignment output resolution (ticket 59/71, post-release ticket 10) —
//! carved out of `dispatch.rs` as a pure, synchronous core so the §5
//! runtime-conflict rule (opposite-half suppression, greater-Depth-wins, the
//! owner tie-break) and the Digital-mode step-increment fallback can be
//! table-tested without spawning `run`, an injector, seven channels and a
//! tempfile — and so a future Sticky/latching mode is a small addition here,
//! not a rewrite (ticket 59 banked this seam forward explicitly).
//!
//! `Engine` owns the per-Input contribution / axis-owner state and answers
//! "given these resolved 0-255 contributions and this Layer's assignment map,
//! what `ABS_*` writes should happen?" with a `Vec<AxisWrite>`. It holds no
//! `&Injector`, contains no `async fn`, touches no channel, and imports
//! nothing from `executor`, `injector`, `edit`, or `dispatch`. The
//! `depth → value` ramp (`config::resolve_axis_value` — it needs the
//! per-Input Actuation point) stays in `dispatch`, which also performs the
//! writes.

use std::collections::HashMap;

use evdev::AbsoluteAxisCode;

use crate::capture::EventState;
use crate::config::{AxisPolarity, AxisTarget};
use crate::input::Input;

/// The Digital Capture mode fallback's per-press step size (ticket 59 §6 /
/// ticket 71): press/release step-increment in place of a continuous Depth
/// stream, since Digital-sourced events carry no Depth at all. A build-time-
/// tuned constant, same precedent as Analog-repeat's constants (ticket 20) —
/// exact feel to be adjusted against real hardware, not designed here.
const AXIS_DIGITAL_STEP: u8 = 64;

/// The per-Input axis output state (ticket 59/71). `contributions` is the
/// live, per-Input 0-255 output value every Axis-assigned Input currently
/// wants to drive its target with — merged in by `resolve` (the continuous
/// Analog half of ticket 59 §7's `(Depth, edge_event) -> axis_value` seam)
/// and by `step_digital` (the Digital-mode step-increment fallback, ticket
/// 59 §6) alike, so both flows feed the exact same conflict-resolution path
/// rather than two drifting copies of it. `owners` is which single Input
/// currently "wins" each signed axis's opposite-half suppression (ticket 59
/// §5) — absent means neither half is currently outputting. Both reset fresh
/// per dispatch task start (ex-`dispatch::AxisState`, verbatim fields).
#[derive(Default)]
pub(crate) struct Engine {
    contributions: HashMap<Input, u8>,
    owners: HashMap<AbsoluteAxisCode, Input>,
}

/// One `ABS_*` write the dispatch executor must emit via
/// `injector.set_axis_value(code, value)`. `value` is already signed
/// (negative for a driven negative half).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AxisWrite {
    pub code: AbsoluteAxisCode,
    pub value: i32,
}

impl Engine {
    /// The continuous Analog path (ticket 59 §7). `resolved` carries the
    /// `depth → value` output for the Inputs dispatch just recomputed off a
    /// fresh `rx_depth` snapshot (`config::resolve_axis_value` per Input).
    /// Merges them into `contributions`, then re-runs §5 resolution for every
    /// `ABS_*` code `axis_map` touches — plus any stale code an owner still
    /// lingers on — and returns the writes. Replaces `handle_depth_update`'s
    /// tail + `recompute_and_emit_axes`.
    pub(crate) fn resolve(
        &mut self,
        axis_map: &HashMap<Input, AxisTarget>,
        resolved: &HashMap<Input, u8>,
    ) -> Vec<AxisWrite> {
        for (&input, &value) in resolved {
            self.contributions.insert(input, value);
        }
        self.recompute(axis_map)
    }

    /// Re-run §5 resolution off the stored `contributions` with no new delta
    /// — `run_effects`' `RecomputeAxes` handler, after a live
    /// `SetAxisAssignment` / `ClearAxisAssignment` changed `axis_map`.
    /// Equivalent to `resolve(axis_map, &HashMap::new())`; named for the call
    /// site's intent.
    pub(crate) fn recompute(&mut self, axis_map: &HashMap<Input, AxisTarget>) -> Vec<AxisWrite> {
        // Positive-polarity contributors, negative-polarity contributors, per
        // `ABS_*` code — see `resolve_axis_contribution`'s own doc comment for
        // why unsigned targets always land in the positive side.
        type Contributors = (Vec<(Input, u8)>, Vec<(Input, u8)>);
        let mut by_code: HashMap<AbsoluteAxisCode, Contributors> = HashMap::new();
        for (&input, &target) in axis_map {
            let value = self.contributions.get(&input).copied().unwrap_or(0);
            let (positive, negative) = by_code.entry(target.abs_code()).or_default();
            match target.polarity() {
                None | Some(AxisPolarity::Positive) => positive.push((input, value)),
                Some(AxisPolarity::Negative) => negative.push((input, value)),
            }
        }

        let mut writes = Vec::new();
        // A code this Input used to own but that no longer has *any*
        // contributor at all in `axis_map` (its last remaining Input was
        // cleared/retargeted to a different `ABS_*` code) would otherwise
        // never be revisited by the loop below, which only ever iterates
        // codes `axis_map` currently names — leaving its last-written value
        // stuck (code-review finding).
        let stale_codes: Vec<AbsoluteAxisCode> = self
            .owners
            .keys()
            .filter(|code| !by_code.contains_key(code))
            .copied()
            .collect();
        for code in stale_codes {
            self.owners.remove(&code);
            writes.push(AxisWrite { code, value: 0 });
        }

        for (code, (positive, negative)) in by_code {
            let current_owner = self.owners.get(&code).copied();
            let (value, new_owner) = resolve_axis_contribution(&positive, &negative, current_owner);
            match new_owner {
                Some(owner) => {
                    self.owners.insert(code, owner);
                }
                None => {
                    self.owners.remove(&code);
                }
            }
            writes.push(AxisWrite { code, value });
        }
        writes
    }

    /// The Digital Capture-mode fallback (ticket 59 §6): step this Input's
    /// contribution up by `AXIS_DIGITAL_STEP` on Down/Repeat (saturating),
    /// reset to 0 on Up, then re-resolve `axis_map`. Replaces
    /// `handle_axis_edge_event`. Only ever reached for a genuinely
    /// Digital-sourced event — dispatch gates on `event.depth.is_none()`.
    pub(crate) fn step_digital(
        &mut self,
        axis_map: &HashMap<Input, AxisTarget>,
        input: Input,
        state: EventState,
    ) -> Vec<AxisWrite> {
        let current = self.contributions.get(&input).copied().unwrap_or(0);
        let next = match state {
            EventState::Down | EventState::Repeat => current.saturating_add(AXIS_DIGITAL_STEP),
            EventState::Up => 0,
        };
        self.contributions.insert(input, next);
        self.recompute(axis_map)
    }

    /// Drop this Input's contribution outright — `run_effects`'
    /// `ForgetAxisContribution`, emitted by `ClearAxisAssignment`. The caller
    /// follows with `recompute`.
    pub(crate) fn forget(&mut self, input: Input) {
        self.contributions.remove(&input);
    }

    /// Center every owned `ABS_*` code and clear all state — a Layer/Profile
    /// switch (`handle_layer_switch`, `run_effects`' `ResetAxisOutputs`).
    /// Returns `vec![]` with no state touched when nothing has ever driven
    /// output (the overwhelmingly common Layer/Profile switch — preserves
    /// `reset_axis_outputs`'s write-free fast path).
    pub(crate) fn reset(&mut self) -> Vec<AxisWrite> {
        if self.owners.is_empty() {
            self.contributions.clear();
            return Vec::new();
        }
        let codes: Vec<AbsoluteAxisCode> = self.owners.keys().copied().collect();
        self.contributions.clear();
        self.owners.clear();
        codes
            .into_iter()
            .map(|code| AxisWrite { code, value: 0 })
            .collect()
    }
}

/// The runtime-conflict half of ticket 59 §5, as a pure/unit-testable
/// function: `positive`/`negative` are every currently-nonzero contributor
/// sharing one `ABS_*` code, split by `AxisTarget::polarity` (an unsigned
/// target's single contribution always lands in `positive` — there is no
/// opposite half for it to conflict with, so this reduces to the same
/// "greater Depth wins" rule ticket 59 §5 gives same-half sharing, with no
/// special-casing needed). Two keys sharing one same-signed target take the
/// greater of the two Depths (`positive`/`negative` are each reduced to their
/// own max independently); two keys on opposite halves resolve by "whichever
/// key is already actively outputting suppresses the other" — `current_owner`
/// (persisted across calls in `Engine::owners`) keeps the already-active half
/// winning once both go nonzero, defaulting to the positive half only the
/// first time both activate with no prior owner at all (an arbitrary but
/// harmless tie-break: ticket 59 doesn't specify one for a genuinely
/// simultaneous first activation, and live tuning against real hardware is a
/// later ticket's job, not this one's).
fn resolve_axis_contribution(
    positive: &[(Input, u8)],
    negative: &[(Input, u8)],
    current_owner: Option<Input>,
) -> (i32, Option<Input>) {
    let pos = positive
        .iter()
        .copied()
        .filter(|&(_, v)| v > 0)
        .max_by_key(|&(_, v)| v);
    let neg = negative
        .iter()
        .copied()
        .filter(|&(_, v)| v > 0)
        .max_by_key(|&(_, v)| v);
    match (pos, neg) {
        (Some(p), Some(n)) => {
            if current_owner == Some(n.0) {
                (-i32::from(n.1), Some(n.0))
            } else {
                (i32::from(p.1), Some(p.0))
            }
        }
        (Some(p), None) => (i32::from(p.1), Some(p.0)),
        (None, Some(n)) => (-i32::from(n.1), Some(n.0)),
        (None, None) => (0, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Input;

    const A: Input = Input::Grid(1, 1);
    const B: Input = Input::Grid(1, 2);

    // ── resolve_axis_contribution table ────────────────────────────────────

    #[test]
    fn lone_unsigned_contributor_drives_its_raw_value() {
        assert_eq!(
            resolve_axis_contribution(&[(A, 200)], &[], None),
            (200, Some(A))
        );
    }

    #[test]
    fn two_same_half_contributors_take_the_greater() {
        assert_eq!(
            resolve_axis_contribution(&[(A, 150), (B, 200)], &[], None),
            (200, Some(B))
        );
    }

    #[test]
    fn opposite_halves_with_an_owner_keep_the_owner_driving() {
        assert_eq!(
            resolve_axis_contribution(&[(A, 200)], &[(B, 220)], Some(B)),
            (-220, Some(B))
        );
    }

    #[test]
    fn opposite_halves_with_no_owner_default_to_the_positive_half() {
        assert_eq!(
            resolve_axis_contribution(&[(A, 200)], &[(B, 220)], None),
            (200, Some(A))
        );
    }

    #[test]
    fn a_stale_owner_that_is_no_longer_a_contributor_does_not_win() {
        // The prior owner (B) has dropped to zero — the positive half is the
        // only live contributor and takes the code.
        assert_eq!(
            resolve_axis_contribution(&[(A, 200)], &[(B, 0)], Some(B)),
            (200, Some(A))
        );
    }

    #[test]
    fn both_halves_zero_centers_the_code_and_clears_the_owner() {
        assert_eq!(
            resolve_axis_contribution(&[(A, 0)], &[(B, 0)], Some(A)),
            (0, None)
        );
    }

    // ── Engine behaviours over Vec<AxisWrite> ──────────────────────────────

    fn map(entries: &[(Input, AxisTarget)]) -> HashMap<Input, AxisTarget> {
        entries.iter().copied().collect()
    }

    #[test]
    fn resolve_drives_an_unsigned_axis_off_a_fresh_contribution() {
        let mut engine = Engine::default();
        let axis_map = map(&[(A, AxisTarget::LeftTrigger)]);
        let writes = engine.resolve(&axis_map, &HashMap::from([(A, 200)]));
        assert_eq!(
            writes,
            vec![AxisWrite {
                code: AbsoluteAxisCode::ABS_Z,
                value: 200
            }]
        );
    }

    #[test]
    fn two_keys_on_one_same_signed_target_take_the_greater_depth() {
        let mut engine = Engine::default();
        let axis_map = map(&[(A, AxisTarget::LeftTrigger), (B, AxisTarget::LeftTrigger)]);
        let writes = engine.resolve(&axis_map, &HashMap::from([(A, 150), (B, 200)]));
        assert_eq!(
            writes,
            vec![AxisWrite {
                code: AbsoluteAxisCode::ABS_Z,
                value: 200
            }]
        );
    }

    #[test]
    fn opposite_signed_halves_let_the_already_active_key_keep_driving() {
        let mut engine = Engine::default();
        let axis_map = map(&[
            (A, AxisTarget::LeftStickXPos),
            (B, AxisTarget::LeftStickXNeg),
        ]);
        // Positive half activates alone first.
        let first = engine.resolve(&axis_map, &HashMap::from([(A, 200)]));
        assert_eq!(
            first,
            vec![AxisWrite {
                code: AbsoluteAxisCode::ABS_X,
                value: 200
            }]
        );
        // Negative half now also activates — the already-active positive half
        // must keep winning (ticket 59 §5).
        let second = engine.resolve(&axis_map, &HashMap::from([(B, 220)]));
        assert_eq!(
            second,
            vec![AxisWrite {
                code: AbsoluteAxisCode::ABS_X,
                value: 200
            }]
        );
    }

    #[test]
    fn a_stale_code_that_dropped_out_of_the_map_is_zeroed_once() {
        let mut engine = Engine::default();
        let with = map(&[(A, AxisTarget::LeftTrigger)]);
        engine.resolve(&with, &HashMap::from([(A, 200)]));

        // The Input is retargeted to a different code — the old code must be
        // swept to zero, and the carried-over contribution drives the new one
        // in the same pass.
        let retargeted = map(&[(A, AxisTarget::RightTrigger)]);
        let writes = engine.recompute(&retargeted);
        assert_eq!(
            writes,
            vec![
                AxisWrite {
                    code: AbsoluteAxisCode::ABS_Z,
                    value: 0
                },
                AxisWrite {
                    code: AbsoluteAxisCode::ABS_RZ,
                    value: 200
                },
            ]
        );
        // A second recompute has no stale code left to zero.
        assert_eq!(
            engine.recompute(&retargeted),
            vec![AxisWrite {
                code: AbsoluteAxisCode::ABS_RZ,
                value: 200
            }]
        );
    }

    #[test]
    fn step_digital_ramps_up_on_repeat_and_resets_on_release() {
        let mut engine = Engine::default();
        let axis_map = map(&[(A, AxisTarget::LeftTrigger)]);

        let down = engine.step_digital(&axis_map, A, EventState::Down);
        assert_eq!(down[0].value, i32::from(AXIS_DIGITAL_STEP));
        let repeat = engine.step_digital(&axis_map, A, EventState::Repeat);
        assert_eq!(repeat[0].value, i32::from(AXIS_DIGITAL_STEP) * 2);
        let up = engine.step_digital(&axis_map, A, EventState::Up);
        assert_eq!(up[0].value, 0);
    }

    #[test]
    fn step_digital_saturates_at_the_top_of_the_range() {
        let mut engine = Engine::default();
        let axis_map = map(&[(A, AxisTarget::LeftTrigger)]);
        for _ in 0..10 {
            engine.step_digital(&axis_map, A, EventState::Repeat);
        }
        let writes = engine.step_digital(&axis_map, A, EventState::Repeat);
        assert_eq!(writes[0].value, i32::from(u8::MAX));
    }

    #[test]
    fn reset_centers_owned_codes_only_and_is_a_no_op_otherwise() {
        let mut engine = Engine::default();
        // Nothing has ever driven output.
        assert!(engine.reset().is_empty());

        let axis_map = map(&[(A, AxisTarget::LeftTrigger)]);
        engine.resolve(&axis_map, &HashMap::from([(A, 200)]));
        let writes = engine.reset();
        assert_eq!(
            writes,
            vec![AxisWrite {
                code: AbsoluteAxisCode::ABS_Z,
                value: 0
            }]
        );
        // State is cleared — a second reset is write-free again.
        assert!(engine.reset().is_empty());
    }

    #[test]
    fn forget_then_recompute_drops_a_contribution_and_zeroes_its_code() {
        let mut engine = Engine::default();
        let axis_map = map(&[(A, AxisTarget::LeftTrigger)]);
        engine.resolve(&axis_map, &HashMap::from([(A, 200)]));

        engine.forget(A);
        // With the Input still assigned but its contribution gone, the code
        // resolves back to zero.
        let writes = engine.recompute(&axis_map);
        assert_eq!(
            writes,
            vec![AxisWrite {
                code: AbsoluteAxisCode::ABS_Z,
                value: 0
            }]
        );
    }
}
