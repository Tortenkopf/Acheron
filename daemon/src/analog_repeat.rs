// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The Analog-repeat rate curve (ticket 20/39, post-release ticket 10) —
//! carved out of `dispatch.rs` so the hardware-tuned numbers (the deadzone /
//! hold-solid bands, the Depth→Hz mapping, the per-fire pulse hold, the
//! spawn/stop policy) are finally table-testable without spawning `run`, an
//! injector, seven channels and a tempfile.
//!
//! The decision core — `tick_plan`, `reconcile`, `pulse_hold_for` — is pure
//! and synchronous. `Engine` owns the spawned tokio tasks and is NOT pure:
//! the same shape as `executor::ActiveToggle`. This module imports nothing
//! from `dispatch`, `edit`, `chord`, `trigger`, or `config::Config` —
//! `dispatch` keeps `compile_action` and hands the engine pre-compiled
//! `Vec<MacroStep>`.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use evdev::KeyCode;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::config::Action;
use crate::executor::{self, MacroStep};
use crate::injector::Injector;
use crate::input::Input;

/// Analog-repeat's fixed start/stop threshold (ticket 20/39) — deliberately
/// not the key's own tunable Actuation point, so the rate curve gets the
/// key's full physical travel to work with. Placeholder: left TBD by ticket
/// 20's Answer, hardware-confirmed as-shipped by ticket 73.
const ANALOG_REPEAT_DEADZONE: u8 = 12;

/// Analog-repeat's minimum/maximum re-fire rate (ticket 20/39), linearly
/// interpolated across the key's full 0-255 Depth range. Same as-shipped
/// status as `ANALOG_REPEAT_DEADZONE`.
const ANALOG_REPEAT_MIN_HZ: f64 = 2.0;
const ANALOG_REPEAT_MAX_HZ: f64 = 20.0;

/// Analog-repeat's fixed per-fire hold duration (ticket 20/39) — the same
/// every tick regardless of Depth; only the tick-to-tick *rate* varies. Used
/// for every output Action except `Action::ControllerButton`, which selects
/// `ANALOG_REPEAT_CONTROLLER_PULSE_HOLD` instead (ticket 78) — Keypress/
/// mouse-button output is interrupt-driven on the receiving side, not subject
/// to the per-frame-polling risk a gamepad read has.
const ANALOG_REPEAT_PULSE_HOLD: Duration = Duration::from_millis(15);

/// `Action::ControllerButton`'s own Analog-repeat pulse-hold floor (ticket
/// 78): the 35ms frame-safe floor ticket 76 already vetted for
/// `Action::ControllerButton` output against a polled 60fps game read.
/// Deliberately its own constant, not shared with `ANALOG_REPEAT_PULSE_HOLD`
/// or `executor::CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD` — three dwells tuned
/// for unrelated jobs.
const ANALOG_REPEAT_CONTROLLER_PULSE_HOLD: Duration = Duration::from_millis(35);

/// Analog-repeat's near-full-travel threshold (ticket 20/39) at or above
/// which the key holds down solid instead of continuing to tap. Same
/// as-shipped status as `ANALOG_REPEAT_DEADZONE`.
const ANALOG_REPEAT_HOLD_SOLID: u8 = 235;

/// What one iteration of the repeat loop should do, given the current Depth
/// and whether the loop is already holding the key solid. Pure — captures
/// every band decision in today's `run_analog_repeat_loop` body in one call,
/// without forcing the loop's `select!` structure to change.
/// `release_solid_first` mirrors today's "if holding_solid { release up
/// steps }" that runs before the deadzone / tapping branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TickPlan {
    /// Depth ≥ HOLD_SOLID: press every Down step solid if not already
    /// holding, then wait on `depth_rx.changed()` / cancel.
    HoldSolid,
    /// Depth < DEADZONE: `update` is about to stop this task (or a stale
    /// wakeup is racing it) — wait, don't fire a spurious minimum-rate pulse.
    Idle { release_solid_first: bool },
    /// In the tapping band: fire one Down/hold/Up pulse, then sleep so the
    /// tick-to-tick spacing is `period` measured from the tick start.
    Tap {
        period: Duration,
        release_solid_first: bool,
    },
}

/// The band decision for one loop iteration (ticket 20/39). Pure and
/// table-tested — the shell in `run_analog_repeat_loop` just performs it.
pub(crate) fn tick_plan(depth: u8, holding_solid: bool) -> TickPlan {
    if depth >= ANALOG_REPEAT_HOLD_SOLID {
        return TickPlan::HoldSolid;
    }
    if depth < ANALOG_REPEAT_DEADZONE {
        return TickPlan::Idle {
            release_solid_first: holding_solid,
        };
    }
    TickPlan::Tap {
        period: rate_period(depth),
        release_solid_first: holding_solid,
    }
}

/// The `1 / lerp(MIN_HZ, MAX_HZ, depth/255)` rate math (ticket 20's Answer:
/// not renormalized to the key's own Actuation/Release band) — a private
/// helper feeding `tick_plan`'s `Tap.period`.
fn rate_period(depth: u8) -> Duration {
    let rate_hz = ANALOG_REPEAT_MIN_HZ
        + (ANALOG_REPEAT_MAX_HZ - ANALOG_REPEAT_MIN_HZ) * (f64::from(depth) / 255.0);
    Duration::from_secs_f64(1.0 / rate_hz)
}

/// One spawn/stop decision for a single Input (ticket 20/39).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reconcile {
    Spawn(Input),
    Stop(Input),
}

/// Spawn/stop policy off one `rx_depth` snapshot — today's
/// `update_analog_repeats` head. `repeat_inputs` is the set of Inputs whose
/// active-Layer Binding is `TriggerMode::AnalogRepeat` (computed by dispatch
/// from `Config`). `active` is the Inputs with a live task. Iterates
/// `snapshot` exactly as today (every grid key is present on every Analog
/// report), so an active task for an Input absent from a later snapshot is
/// not stopped here — harmless because `capture::analog` publishes every grid
/// key on every report, and the explicit `stop_all` on Layer / capture-mode
/// transitions covers the Digital case. Pure, table-tested.
pub(crate) fn reconcile(
    active: &HashSet<Input>,
    repeat_inputs: &HashSet<Input>,
    snapshot: &HashMap<Input, u8>,
) -> Vec<Reconcile> {
    let mut out = Vec::new();
    for (&input, &depth) in snapshot {
        let is_analog_repeat = repeat_inputs.contains(&input);
        if is_analog_repeat && depth >= ANALOG_REPEAT_DEADZONE {
            if !active.contains(&input) {
                out.push(Reconcile::Spawn(input));
            }
        } else if active.contains(&input) {
            out.push(Reconcile::Stop(input));
        }
    }
    out
}

/// Analog-repeat's per-fire hold: the 35 ms frame-safe floor for a
/// `ControllerButton` output (ticket 78), the 15 ms dwell for every other
/// Action. Pure. Dispatch passes `&binding.action`.
pub(crate) fn pulse_hold_for(action: &Action) -> Duration {
    if matches!(action, Action::ControllerButton { .. }) {
        ANALOG_REPEAT_CONTROLLER_PULSE_HOLD
    } else {
        ANALOG_REPEAT_PULSE_HOLD
    }
}

/// A running Analog-repeat background task (ticket 20/39) — structurally
/// closer to `executor::ActiveToggle` than to a `FiringHandle`: its lifetime
/// is driven by Depth crossing `ANALOG_REPEAT_DEADZONE` (see
/// `Engine::update`), not by a single physical press/release.
struct ActiveAnalogRepeat {
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl ActiveAnalogRepeat {
    /// Spawns the task: `steps` is compiled once by dispatch, from the
    /// Binding's Action as of the moment Depth first crossed the deadzone
    /// (mirrors `perform_trigger`'s own once-per-fire `compile_action` call)
    /// — not recompiled per tick, so a Stepper Action's cursor advances once
    /// per "press session" rather than auto-cycling at the tick rate.
    /// `depth_rx` is the caller's own clone of the shared live-Depth watch
    /// channel (ticket 26), read fresh on every tick to drive the rate curve.
    fn spawn(
        injector: Injector,
        input: Input,
        steps: Vec<MacroStep>,
        pulse_hold: Duration,
        depth_rx: watch::Receiver<HashMap<Input, u8>>,
    ) -> Self {
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(run_analog_repeat_loop(
            injector,
            input,
            steps,
            pulse_hold,
            depth_rx,
            cancel.clone(),
        ));
        ActiveAnalogRepeat { cancel, handle }
    }

    /// Stops the task and waits for its force-release to complete, mirroring
    /// `ActiveToggle::stop`'s exact contract.
    async fn stop(self) {
        self.cancel.cancel();
        let _ = self.handle.await;
    }
}

/// Fires `steps`' Down phase, holds for `pulse_hold`, then fires the Up phase
/// in reverse order — matching `keypress_steps`'s own down/up nesting
/// (modifiers released in the reverse of how they were pressed). Deliberately
/// ignores any `MacroStep::Delay` a Macro Action might embed: Analog-repeat's
/// whole idea (ticket 20's Answer) is a single fixed-duration pulse, not a
/// multi-step timed sequence.
async fn fire_analog_repeat_pulse(
    injector: &Injector,
    steps: &[MacroStep],
    pulse_hold: Duration,
    held: &mut HashSet<KeyCode>,
) {
    for step in steps {
        if let MacroStep::KeyDown(_) = step {
            let _ = executor::execute_step(injector, held, *step).await;
        }
    }
    tokio::time::sleep(pulse_hold).await;
    for step in steps.iter().rev() {
        if let MacroStep::KeyUp(_) = step {
            let _ = executor::execute_step(injector, held, *step).await;
        }
    }
}

/// Releases every Up step in reverse — the "if holding_solid { release }"
/// that ran before the deadzone / tapping branches in the old loop body.
async fn release_solid(injector: &Injector, steps: &[MacroStep], held: &mut HashSet<KeyCode>) {
    for step in steps.iter().rev() {
        if let MacroStep::KeyUp(_) = step {
            let _ = executor::execute_step(injector, held, *step).await;
        }
    }
}

/// The task body `ActiveAnalogRepeat::spawn` runs — a thin shell driving
/// `tick_plan`. Exits (force-releasing whatever it's still holding) only on
/// external cancellation — `Engine::update` is the sole owner of *when* that
/// happens, driven by Depth crossing back down through the deadzone.
async fn run_analog_repeat_loop(
    injector: Injector,
    input: Input,
    steps: Vec<MacroStep>,
    pulse_hold: Duration,
    mut depth_rx: watch::Receiver<HashMap<Input, u8>>,
    cancel: CancellationToken,
) {
    let mut held: HashSet<KeyCode> = HashSet::new();
    let mut holding_solid = false;

    loop {
        let depth = *depth_rx.borrow().get(&input).unwrap_or(&0);
        match tick_plan(depth, holding_solid) {
            TickPlan::HoldSolid => {
                if !holding_solid {
                    for step in &steps {
                        if let MacroStep::KeyDown(_) = step {
                            let _ = executor::execute_step(&injector, &mut held, *step).await;
                        }
                    }
                    holding_solid = true;
                }
                tokio::select! {
                    () = cancel.cancelled() => break,
                    _ = depth_rx.changed() => {}
                }
            }
            TickPlan::Idle {
                release_solid_first,
            } => {
                if release_solid_first {
                    release_solid(&injector, &steps, &mut held).await;
                    holding_solid = false;
                }
                tokio::select! {
                    () = cancel.cancelled() => break,
                    _ = depth_rx.changed() => {}
                }
            }
            TickPlan::Tap {
                period,
                release_solid_first,
            } => {
                if release_solid_first {
                    release_solid(&injector, &steps, &mut held).await;
                    holding_solid = false;
                }
                let tick_start = Instant::now();
                let cancelled = tokio::select! {
                    () = cancel.cancelled() => true,
                    () = fire_analog_repeat_pulse(&injector, &steps, pulse_hold, &mut held) => false,
                };
                if cancelled {
                    break;
                }
                let elapsed = tick_start.elapsed();
                if elapsed < period {
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(period - elapsed) => {}
                    }
                }
            }
        }
    }
    executor::force_release(&injector, held).await;
}

/// The task supervisor. Owns `HashMap<Input, ActiveAnalogRepeat>` (each a
/// `JoinHandle` + `CancellationToken`), reset fresh per dispatch task start
/// (ex-`DispatchState::analog_repeats`).
#[derive(Default)]
pub(crate) struct Engine {
    tasks: HashMap<Input, ActiveAnalogRepeat>,
}

impl Engine {
    /// Run `reconcile` against the live task set, perform every `Stop`
    /// (cancelling the token then awaiting the task — the engine owns the
    /// map), and return the Inputs that need a fresh task. Dispatch compiles
    /// each one's steps (`compile_action`, staying dispatch-side) and calls
    /// `spawn`. Replaces `update_analog_repeats`'s body.
    pub(crate) async fn update(
        &mut self,
        repeat_inputs: &HashSet<Input>,
        snapshot: &HashMap<Input, u8>,
    ) -> Vec<Input> {
        let active: HashSet<Input> = self.tasks.keys().copied().collect();
        let mut to_spawn = Vec::new();
        for action in reconcile(&active, repeat_inputs, snapshot) {
            match action {
                Reconcile::Spawn(input) => to_spawn.push(input),
                Reconcile::Stop(input) => {
                    if let Some(task) = self.tasks.remove(&input) {
                        task.stop().await;
                    }
                }
            }
        }
        to_spawn
    }

    /// Compile-once-at-spawn (a Stepper cursor advances per press-session, not
    /// per tick — mirrors `perform_trigger`). `steps` and `pulse_hold` arrive
    /// pre-resolved from dispatch. Sync, like today's
    /// `ActiveAnalogRepeat::spawn`. "Spawn only if absent" — dispatch only
    /// ever calls this for an Input `update` just reported as needing a task.
    pub(crate) fn spawn(
        &mut self,
        injector: Injector,
        input: Input,
        steps: Vec<MacroStep>,
        pulse_hold: Duration,
        depth_rx: watch::Receiver<HashMap<Input, u8>>,
    ) {
        self.tasks.entry(input).or_insert_with(|| {
            ActiveAnalogRepeat::spawn(injector, input, steps, pulse_hold, depth_rx)
        });
    }

    /// Force-stop every task — a Layer/Profile switch, an Analog→Digital
    /// transition (`handle_layer_switch`, `handle_capture_mode_change`,
    /// `run_effects`' `StopAllAnalogRepeats`). Ex-`stop_all_analog_repeats`.
    pub(crate) async fn stop_all(&mut self) {
        for (_, task) in self.tasks.drain() {
            task.stop().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Modifiers;

    // ── tick_plan table ───────────────────────────────────────────────────

    #[test]
    fn tick_plan_below_the_deadzone_is_idle() {
        assert_eq!(
            tick_plan(0, false),
            TickPlan::Idle {
                release_solid_first: false
            }
        );
        assert_eq!(
            tick_plan(ANALOG_REPEAT_DEADZONE - 1, true),
            TickPlan::Idle {
                release_solid_first: true
            }
        );
    }

    #[test]
    fn tick_plan_at_or_above_the_hold_solid_threshold_holds_solid() {
        assert_eq!(
            tick_plan(ANALOG_REPEAT_HOLD_SOLID, false),
            TickPlan::HoldSolid
        );
        assert_eq!(tick_plan(u8::MAX, true), TickPlan::HoldSolid);
    }

    #[test]
    fn tick_plan_in_the_tapping_band_taps_and_carries_the_release_flag() {
        let plan = tick_plan(ANALOG_REPEAT_DEADZONE, false);
        let TickPlan::Tap {
            release_solid_first,
            ..
        } = plan
        else {
            panic!("expected Tap, got {plan:?}");
        };
        assert!(!release_solid_first);

        let plan = tick_plan(ANALOG_REPEAT_HOLD_SOLID - 1, true);
        let TickPlan::Tap {
            release_solid_first,
            ..
        } = plan
        else {
            panic!("expected Tap, got {plan:?}");
        };
        assert!(release_solid_first);
    }

    /// Ticket 73's three verified sample points: depth 12 ≈ 2.85 Hz, 100 ≈
    /// 9 Hz, 235 ≈ 18.6 Hz (the hold-solid threshold sits at 235, so 234 is
    /// the top of the tapping band).
    #[test]
    fn rate_period_matches_the_verified_sample_points() {
        let hz = |d: u8| 1.0 / rate_period(d).as_secs_f64();
        assert!((hz(12) - 2.847).abs() < 0.05, "depth 12: {}", hz(12));
        assert!((hz(100) - 9.06).abs() < 0.05, "depth 100: {}", hz(100));
        assert!((hz(235) - 18.6).abs() < 0.1, "depth 235: {}", hz(235));
    }

    // ── reconcile table ───────────────────────────────────────────────────

    fn set<const N: usize>(inputs: [Input; N]) -> HashSet<Input> {
        inputs.into_iter().collect()
    }

    const K: Input = Input::Grid(1, 1);

    #[test]
    fn reconcile_spawns_a_repeat_input_that_crossed_the_deadzone_and_has_no_task() {
        let out = reconcile(
            &set([]),
            &set([K]),
            &HashMap::from([(K, ANALOG_REPEAT_DEADZONE)]),
        );
        assert_eq!(out, vec![Reconcile::Spawn(K)]);
    }

    #[test]
    fn reconcile_does_not_respawn_an_already_active_input() {
        let out = reconcile(&set([K]), &set([K]), &HashMap::from([(K, 200)]));
        assert!(out.is_empty());
    }

    #[test]
    fn reconcile_stops_an_active_input_that_fell_below_the_deadzone() {
        let out = reconcile(
            &set([K]),
            &set([K]),
            &HashMap::from([(K, ANALOG_REPEAT_DEADZONE - 1)]),
        );
        assert_eq!(out, vec![Reconcile::Stop(K)]);
    }

    #[test]
    fn reconcile_stops_an_active_input_that_is_no_longer_a_repeat_binding() {
        let out = reconcile(&set([K]), &set([]), &HashMap::from([(K, 200)]));
        assert_eq!(out, vec![Reconcile::Stop(K)]);
    }

    #[test]
    fn reconcile_ignores_a_non_repeat_input_with_no_task() {
        let out = reconcile(&set([]), &set([]), &HashMap::from([(K, 200)]));
        assert!(out.is_empty());
    }

    // ── pulse_hold_for ────────────────────────────────────────────────────

    #[test]
    fn pulse_hold_for_a_controller_button_uses_the_frame_safe_floor() {
        let cb = Action::ControllerButton {
            button: KeyCode::BTN_SOUTH,
        };
        assert_eq!(pulse_hold_for(&cb), ANALOG_REPEAT_CONTROLLER_PULSE_HOLD);
    }

    #[test]
    fn pulse_hold_for_every_other_action_uses_the_ordinary_dwell() {
        let kbd = Action::Keypress {
            modifiers: Modifiers::default(),
            key: KeyCode::KEY_A,
        };
        assert_eq!(pulse_hold_for(&kbd), ANALOG_REPEAT_PULSE_HOLD);
    }
}
