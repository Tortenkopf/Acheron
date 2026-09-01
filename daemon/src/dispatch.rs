// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The dispatch task: single consumer of both the capture channel and the
//! D-Bus command channel (issue 07's "D-Bus interleaving" — GUI-originated
//! calls push a `Command` alongside `PhysicalEvent`s, so one task remains
//! the sole owner of `Config`, no lock or second copy of state). Resolves
//! each `PhysicalEvent`'s `Input` against the active Profile's active Layer
//! (ticket 18) and, per ticket 17, branches on `TriggerMode` — Fire-once
//! fires once on `Down`, Hold-to-repeat fires on `Down` and every `Repeat`,
//! Toggle starts/stops only on `Down`. Applies `Command`s (ticket 15) by
//! mutating `Config` in place and rewriting `config.toml` immediately,
//! atomically per call.
//!
//! Ticket 18: this task also owns the one piece of Layer runtime state —
//! `active_layer` — since it's momentary (Mode-key-held) rather than
//! persisted `Config`. Under `ModeKeyRole::LayerSwitch` (default), a
//! `PhysicalEvent` on `Input::ModeKey` is intercepted right here, before any
//! Binding lookup: `Down` activates the Held Layer, `Up` reverts to Base,
//! `Repeat` is ignored, and no keycode is ever passed through for it — the
//! whole point of Hypershift-style layering is that the physical Mode key
//! never itself reaches the OS. Under `ModeKeyRole::Bound`, `Input::ModeKey`
//! instead flows through the exact same `(Layer, Input) -> Binding` lookup
//! and Trigger-mode dispatch as any other Input.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use evdev::{AbsoluteAxisCode, KeyCode};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use zbus::object_server::SignalEmitter;

use crate::capture::analog::DeviceInfo;
use crate::capture::{CaptureMode, EventState, PhysicalEvent};
use crate::chord;
use crate::command::{Command, CommandError, State};
use crate::config::{
    self, Action, ActuationPoint, AxisPolarity, AxisTarget, Binding, ChordKey, Config, Layer,
    MacroDef, MacroId, ModeKeyRole, StepDirection, StepperDef, StepperId, StepperItem, TriggerMode,
};
use crate::dbus::Daemon;
use crate::edit;
use crate::executor::{self, ActiveToggle, FiringHandle, MacroStep};
use crate::injector::Injector;
use crate::input::Input;

/// The Digital Capture mode fallback's per-press step size (ticket 59 §6 /
/// ticket 71): press/release step-increment in place of a continuous Depth
/// stream, since Digital-sourced events carry no Depth at all. A build-time-
/// tuned constant, same precedent as Analog-repeat's four TBD constants
/// (ticket 20) — exact feel to be adjusted against real hardware in ticket
/// 72, not designed here.
const AXIS_DIGITAL_STEP: u8 = 64;

/// Every piece of Daemon-owned, `ChordKey`-keyed runtime state a Chord's own
/// Trigger-mode dispatch touches (ticket 01/40) — the firing/toggle *handles*
/// the pure `chord` state machine (post-release ticket 07) never holds. One
/// `run`-local, built fresh per dispatch task start, mirroring how
/// `AxisState` bundles its own two maps; the executor derives the
/// `chord::ChordSlot` liveness snapshot `chord::feed` wants from it.
#[derive(Default)]
struct ChordRuntime {
    firings: HashMap<ChordKey, FiringHandle>,
    toggles: HashMap<ChordKey, ActiveToggle>,
}

/// Every piece of Daemon-owned runtime state Axis-assignment resolution
/// needs (ticket 59/71), mirroring `toggles`/`ChordRuntime`'s own per-Input
/// runtime-state shape. `contributions` is the live, per-Input 0-255 output
/// value every Axis-assigned Input currently wants to drive its target
/// with — written by `handle_depth_update` (the continuous Analog half of
/// ticket 59 §7's `(Depth, edge_event) -> axis_value` seam,
/// `config::resolve_axis_value`) and by `handle_axis_edge_event` (the
/// Digital-mode step-increment fallback, ticket 59 §6) alike, so both flows
/// feed the exact same conflict-resolution/emit path
/// (`recompute_and_emit_axes`) rather than two drifting copies of it.
/// `owners` is which single Input currently "wins" each signed axis's
/// opposite-half suppression (ticket 59 §5) — absent means neither half is
/// currently outputting.
#[derive(Default)]
struct AxisState {
    contributions: HashMap<Input, u8>,
    owners: HashMap<AbsoluteAxisCode, Input>,
}

/// The runtime-conflict half of ticket 59 §5, as a pure/unit-testable
/// function: `positive`/`negative` are every currently-nonzero contributor
/// sharing one `ABS_*` code, split by `AxisTarget::polarity` (an unsigned
/// target's single contribution always lands in `positive` — there is no
/// opposite half for it to conflict with, so this reduces to the same
/// "greater Depth wins" rule ticket 59 §5 gives same-half sharing, with no
/// special-casing needed). Two keys sharing one same-signed target take the
/// greater of the two Depths (`positive`/`negative` are each reduced to
/// their own max independently); two keys on opposite halves resolve by
/// "whichever key is already actively outputting suppresses the other" —
/// `current_owner` (persisted across calls in `AxisState::owners`) keeps the
/// already-active half winning once both go nonzero, defaulting to the
/// positive half only the first time both activate with no prior owner at
/// all (an arbitrary but harmless tie-break: ticket 59 doesn't specify one
/// for a genuinely simultaneous first activation, and live tuning against
/// real hardware is ticket 72's job, not this one's).
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

/// Recomputes and writes every `ABS_*` code `axis_map` (the active Layer's
/// resolved Axis-assignment map) currently touches, from `axis_state`'s
/// latest per-Input contributions — the shared tail end of both the
/// continuous Analog path (`handle_depth_update`) and the Digital-mode edge
/// path (`handle_axis_edge_event`), per `AxisState`'s own doc comment.
async fn recompute_and_emit_axes(
    injector: &Injector,
    axis_state: &mut AxisState,
    axis_map: &HashMap<Input, AxisTarget>,
) -> io::Result<()> {
    // Positive-polarity contributors, negative-polarity contributors, per
    // `ABS_*` code — see `resolve_axis_contribution`'s own doc comment for
    // why unsigned targets always land in the positive side.
    type Contributors = (Vec<(Input, u8)>, Vec<(Input, u8)>);
    let mut by_code: HashMap<AbsoluteAxisCode, Contributors> = HashMap::new();
    for (&input, &target) in axis_map {
        let value = axis_state.contributions.get(&input).copied().unwrap_or(0);
        let (positive, negative) = by_code.entry(target.abs_code()).or_default();
        match target.polarity() {
            None | Some(AxisPolarity::Positive) => positive.push((input, value)),
            Some(AxisPolarity::Negative) => negative.push((input, value)),
        }
    }
    // A code this Input used to own but that no longer has *any* contributor
    // at all in `axis_map` (its last remaining Input was cleared/retargeted
    // to a different `ABS_*` code) would otherwise never be revisited by the
    // loop below, which only ever iterates codes `axis_map` currently names
    // — leaving its last-written value stuck (code-review finding).
    let stale_codes: Vec<AbsoluteAxisCode> = axis_state
        .owners
        .keys()
        .filter(|code| !by_code.contains_key(code))
        .copied()
        .collect();
    for code in stale_codes {
        axis_state.owners.remove(&code);
        injector
            .set_axis_value(code, 0)
            .await
            .map_err(io::Error::other)?;
    }

    for (code, (positive, negative)) in by_code {
        let current_owner = axis_state.owners.get(&code).copied();
        let (value, new_owner) = resolve_axis_contribution(&positive, &negative, current_owner);
        match new_owner {
            Some(owner) => {
                axis_state.owners.insert(code, owner);
            }
            None => {
                axis_state.owners.remove(&code);
            }
        }
        injector
            .set_axis_value(code, value)
            .await
            .map_err(io::Error::other)?;
    }
    Ok(())
}

/// Centers every `ABS_*` code `axis_state` currently has an owner for back
/// to 0 and clears every piece of `AxisState`, so a Layer or Profile switch
/// never leaves a stale axis value driving output for an Input that's no
/// longer even Axis-assigned on the newly-active Layer/Profile — mirrors
/// `stop_all_toggles`'s identical "force-stop on switch" precedent for
/// Toggles. A true no-op (no injector writes at all) when no Axis
/// assignment has ever driven output — the overwhelmingly common case for
/// most Profiles/Layers — so an ordinary Layer/Profile switch that never
/// touches Axis assignment stays exactly as write-free as it was before
/// ticket 71.
async fn reset_axis_outputs(injector: &Injector, axis_state: &mut AxisState) -> io::Result<()> {
    if axis_state.owners.is_empty() {
        axis_state.contributions.clear();
        return Ok(());
    }
    let codes: Vec<AbsoluteAxisCode> = axis_state.owners.keys().copied().collect();
    axis_state.contributions.clear();
    axis_state.owners.clear();
    for code in codes {
        injector
            .set_axis_value(code, 0)
            .await
            .map_err(io::Error::other)?;
    }
    Ok(())
}

/// Analog-repeat's fixed start/stop threshold (ticket 20/39) — deliberately
/// not the key's own tunable Actuation point, so the rate curve gets the
/// key's full physical travel to work with. Placeholder: left TBD by ticket
/// 20's Answer, to be tuned live against the real device — no physical
/// Tartarus Pro was available in the session that built this.
const ANALOG_REPEAT_DEADZONE: u8 = 12;

/// Analog-repeat's minimum/maximum re-fire rate (ticket 20/39), linearly
/// interpolated across the key's full 0-255 Depth range. Placeholders, same
/// live-tuning status as `ANALOG_REPEAT_DEADZONE`.
const ANALOG_REPEAT_MIN_HZ: f64 = 2.0;
const ANALOG_REPEAT_MAX_HZ: f64 = 20.0;

/// Analog-repeat's fixed per-fire hold duration (ticket 20/39) — the same
/// every tick regardless of Depth; only the tick-to-tick *rate* varies.
/// Placeholder, same live-tuning status as `ANALOG_REPEAT_DEADZONE`. Used for
/// every output Action except `Action::ControllerButton`, which selects
/// `ANALOG_REPEAT_CONTROLLER_PULSE_HOLD` instead (ticket 78) — Keypress/
/// mouse-button output is interrupt-driven on the receiving side, not subject
/// to the per-frame-polling risk a gamepad read has.
const ANALOG_REPEAT_PULSE_HOLD: Duration = Duration::from_millis(15);

/// `Action::ControllerButton`'s own Analog-repeat pulse-hold floor (ticket
/// 78): `ANALOG_REPEAT_MAX_HZ`'s 20Hz already yields a 50ms period at the
/// fastest end of the rate curve, comfortably above this 35ms dwell — the
/// same frame-safe floor ticket 76 already vetted for `Action::
/// ControllerButton` output against a polled 60fps game read (the class of
/// problem ticket 74 flagged as unaddressed for Analog-repeat's own
/// pre-existing 15ms dwell). Deliberately its own constant, not shared with
/// `ANALOG_REPEAT_PULSE_HOLD` or `executor::CONTROLLER_BUTTON_DIGITAL_PULSE_
/// HOLD` — three dwells tuned for unrelated jobs.
const ANALOG_REPEAT_CONTROLLER_PULSE_HOLD: Duration = Duration::from_millis(35);

/// Analog-repeat's near-full-travel threshold (ticket 20/39) at or above
/// which the key holds down solid instead of continuing to tap. Placeholder,
/// same live-tuning status as `ANALOG_REPEAT_DEADZONE`.
const ANALOG_REPEAT_HOLD_SOLID: u8 = 235;

/// A running Analog-repeat background task (ticket 20/39), as tracked in
/// dispatch's `HashMap<Input, ActiveAnalogRepeat>` — structurally closer to
/// `ActiveToggle` than to a Fire-once/Hold-to-repeat `FiringHandle`, per
/// ticket 20's Answer: its lifetime is driven by Depth crossing
/// `ANALOG_REPEAT_DEADZONE` (see `update_analog_repeats`), not by a single
/// physical press/release. Never touched from `fire()`, which swallows every
/// Analog-sourced Down/Repeat/Up for an Analog-repeat Binding outright (see
/// `handle_event`) — only a Digital-sourced one (no Depth at all) reaches
/// `fire()`, which treats Analog-repeat exactly like Hold-to-repeat there
/// (ticket 20's Digital Capture mode fallback).
struct ActiveAnalogRepeat {
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl ActiveAnalogRepeat {
    /// Spawns the task: `steps` is compiled once, here, from the Binding's
    /// Action as of the moment Depth first crossed the deadzone (mirrors
    /// `fire()`'s own once-per-press `compile_action` call) — not
    /// recompiled per tick, so a Stepper Action's cursor advances once per
    /// "press session" rather than auto-cycling at the tick rate. `depth_rx`
    /// is the caller's own clone of the shared live-Depth watch channel
    /// (ticket 26), read fresh on every tick to drive the rate curve.
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

/// Fires `steps`' Down phase, holds for `pulse_hold` (`ANALOG_REPEAT_PULSE_
/// HOLD`, or `ANALOG_REPEAT_CONTROLLER_PULSE_HOLD` for `Action::
/// ControllerButton` output, per ticket 78), then fires the Up phase in
/// reverse order — matching `keypress_steps`'s own down/up nesting (modifiers
/// released in the reverse of how they were pressed). Deliberately ignores
/// any `MacroStep::Delay` a Macro Action might embed: Analog-repeat's whole
/// idea (ticket 20's Answer) is a single fixed-duration pulse, not a
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

/// The task body `ActiveAnalogRepeat::spawn` runs. Two states, toggled by
/// `depth_rx`'s own live snapshot on every loop iteration: below
/// `ANALOG_REPEAT_HOLD_SOLID`, fires `fire_analog_repeat_pulse` at a rate
/// linearly interpolated between `ANALOG_REPEAT_MIN_HZ`/`_MAX_HZ` across the
/// full 0-255 Depth range (ticket 20's Answer: not renormalized to the key's
/// own Actuation/Release band); at or above it, holds every Down step solid
/// with no further tapping until Depth drops back below the threshold.
/// Exits (force-releasing whatever it's still holding) only on external
/// cancellation — `update_analog_repeats` is the sole owner of *when* that
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
        if depth >= ANALOG_REPEAT_HOLD_SOLID {
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
            continue;
        }
        if holding_solid {
            for step in steps.iter().rev() {
                if let MacroStep::KeyUp(_) = step {
                    let _ = executor::execute_step(&injector, &mut held, *step).await;
                }
            }
            holding_solid = false;
        }
        // Below the deadzone: `update_analog_repeats` is the sole owner of
        // *stopping* this task and is about to (or a stale wakeup is racing
        // it) — wait rather than firing a spurious pulse at the curve's own
        // minimum rate. Without this check, a `depth_rx.changed()` wakeup
        // that wins its `select!` against the hold-solid branch's own
        // `cancel.cancelled()` above (both become ready around the same
        // depth update that crosses back below the deadzone) would
        // otherwise fall through into the tapping branch below and fire one
        // extra Down/Up pulse before the external stop's cancellation ever
        // lands — reproduced by this module's own `analog_repeat_holds_
        // solid_above_the_hold_threshold` test, intermittently, before this
        // check existed.
        if depth < ANALOG_REPEAT_DEADZONE {
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = depth_rx.changed() => {}
            }
            continue;
        }
        let rate_hz = ANALOG_REPEAT_MIN_HZ
            + (ANALOG_REPEAT_MAX_HZ - ANALOG_REPEAT_MIN_HZ) * (f64::from(depth) / 255.0);
        let period = Duration::from_secs_f64(1.0 / rate_hz);
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
    executor::force_release(&injector, held).await;
}

/// Force-stops every currently running Analog-repeat task — mirrors
/// `stop_all_toggles`'s exact shape, called on Layer switch/Profile switch
/// (an Analog-repeat task is tied to one specific Layer's Binding, closer to
/// a continuous Axis output than to a Toggle's deliberately-persisted latch
/// — same reasoning as `reset_axis_outputs`'s own call sites) and on an
/// Analog-to-Digital capture-mode transition (the live-Depth stream driving
/// every task's rate curve goes stale the moment that happens).
async fn stop_all_analog_repeats(analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>) {
    for (_, task) in analog_repeats.drain() {
        task.stop().await;
    }
}

/// Starts/stops every grid Input's Analog-repeat task from a fresh
/// `depth_tx` snapshot (ticket 20/39) — the depth-driven half of
/// Analog-repeat's firing, parallel to `handle_depth_update`'s own Axis
/// resolution off the same snapshot. A rising edge through
/// `ANALOG_REPEAT_DEADZONE` on an Input whose active-Layer Binding is
/// `TriggerMode::AnalogRepeat` spawns a task (compiling its steps once, the
/// same "once per press" precedent `fire()` already sets); a falling edge —
/// or the Binding no longer being Analog-repeat, best-effort only, see below
/// — stops one. A Binding changed away from Analog-repeat without an
/// intervening depth-crossing (e.g. edited live while the key stays
/// physically pressed) is a known, accepted residual gap: the stale task
/// keeps running with the steps it compiled at spawn time until Depth next
/// crosses the deadzone — the same class of gap ticket 71's Answer accepted
/// for its own opposite-signed-halves tie-break, not engineered around here.
#[allow(clippy::too_many_arguments)]
async fn update_analog_repeats(
    injector: &Injector,
    config: &Config,
    active_layer: Layer,
    analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    depth_rx: &watch::Receiver<HashMap<Input, u8>>,
    snapshot: &HashMap<Input, u8>,
) {
    let profile = config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile");
    let bindings = profile.layer(active_layer);
    for (&input, &depth) in snapshot {
        let binding = bindings.get(&input);
        let is_analog_repeat = binding.is_some_and(|b| b.trigger == TriggerMode::AnalogRepeat);
        if is_analog_repeat && depth >= ANALOG_REPEAT_DEADZONE {
            analog_repeats.entry(input).or_insert_with(|| {
                let action = &binding
                    .expect("is_analog_repeat is only true for Some(binding)")
                    .action;
                let steps =
                    compile_action(action, &config.macros, &config.steppers, stepper_cursors);
                // Ticket 78: a gamepad button gets the 35ms frame-safe floor;
                // every other output Action keeps the original 15ms dwell.
                let pulse_hold = if matches!(action, Action::ControllerButton { .. }) {
                    ANALOG_REPEAT_CONTROLLER_PULSE_HOLD
                } else {
                    ANALOG_REPEAT_PULSE_HOLD
                };
                ActiveAnalogRepeat::spawn(
                    injector.clone(),
                    input,
                    steps,
                    pulse_hold,
                    depth_rx.clone(),
                )
            });
        } else if let Some(task) = analog_repeats.remove(&input) {
            task.stop().await;
        }
    }
}

/// Returns an error once the injector channel closes, or the capture
/// channel closes (meaning the capture task has died) — per issue 07, a
/// genuine, fatal capture-pipeline error rather than something to swallow
/// silently. The command channel closing is not fatal: it only means the
/// D-Bus server side has gone away, and this task's other job (capture ->
/// injector passthrough/remapping) still has work to do.
// Ticket 22 grew this by one parameter (`actuation_tx`) past clippy's
// default arg-count threshold — every parameter here is a distinct channel
// handle or piece of startup state this task needs for the process's whole
// lifetime, not something a struct would meaningfully group.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    mut rx_events: mpsc::Receiver<PhysicalEvent>,
    mut rx_connection: mpsc::Receiver<bool>,
    mut rx_commands: mpsc::Receiver<Command>,
    injector: Injector,
    mut config: Config,
    config_path: PathBuf,
    signal_emitter: Option<SignalEmitter<'static>>,
    actuation_tx: watch::Sender<HashMap<Input, ActuationPoint>>,
    mut rx_capture_mode: mpsc::Receiver<CaptureMode>,
    capture_control_tx: mpsc::Sender<bool>,
    // Ticket 68: resolved once at Daemon startup (`main.rs`, before this
    // task's event loop starts) and threaded down to every `ActiveToggle::
    // spawn` call site as a plain value, rather than re-read per Toggle
    // press — the kernel autorepeat rate it reflects never changes while
    // the Daemon is running.
    toggle_lap_target: Duration,
    // Ticket 71: the same live-Depth watch channel the Analog grid task
    // already publishes into on every incoming report (`capture::analog`,
    // ticket 26) — reused here as the continuous half of Axis-assignment
    // resolution (`(Depth, edge_event) -> axis_value`, ticket 59 §7) rather
    // than growing `PhysicalEvent`'s own contract, since only this task
    // owns the `Config`/active-Layer state needed to know which Inputs are
    // currently Axis-assigned at all.
    mut rx_depth: watch::Receiver<HashMap<Input, u8>>,
    // Ticket 101: the supervisor reads the connected Tartarus Pro's
    // firmware/serial over the Interface-2 control channel once per connect
    // and pushes the result here — `Some(info)` on a successful read,
    // `None` on disconnect (so `GetState()`'s keys go absent). Mirrors
    // `rx_capture_mode`'s "supervisor tells dispatch about the device"
    // shape; this task owns the one canonical value `GetState()` reads.
    mut rx_device_info: mpsc::Receiver<Option<DeviceInfo>>,
) -> io::Result<()> {
    // Published once up front so the analog capture source's grid task
    // (ticket 22/23) has a correct snapshot to threshold against from the
    // moment it starts, not just from the first mutation onward.
    actuation_tx.send_replace(
        config
            .active_profile()
            .expect("load_or_seed validates active_profile names a real profile")
            .resolved_actuation_points(),
    );
    // Sole owner of every currently-running Toggle (spec.md: "Active
    // toggles: HashMap<Input, ActiveToggle>"), mutated only from this task —
    // no Mutex, matching the rest of the dispatch task's state.
    let mut toggles: HashMap<Input, ActiveToggle> = HashMap::new();
    // Tracks the most recent Fire-once/Hold-to-repeat firing spawned per
    // Input, so a fast HoldToRepeat autorepeat (or a rapid re-Down) can't
    // spawn a second overlapping firing for the same Input while a slow
    // Macro's raw uinput writes for the first are still in flight — two
    // concurrent firings racing on the shared Injector channel could
    // interleave their KeyDown/KeyUp steps out of order on the wire.
    let mut in_flight: HashMap<Input, FiringHandle> = HashMap::new();
    // Every Stepper library entry's Daemon-side-only runtime cursor (ticket
    // 03/54), owned exclusively by this task — an absent entry means "at the
    // list's first item," matching the always-resets-to-first-item-on-
    // restart semantics for free rather than needing an explicit
    // zero-fill-on-startup pass.
    let mut stepper_cursors: HashMap<StepperId, usize> = HashMap::new();
    // The one piece of momentary Layer runtime state (ticket 18) — not part
    // of `Config`, reset to `Base` whenever this task starts.
    let mut active_layer = Layer::Base;
    // The dispatch task's live view of device connectivity (ticket 20),
    // updated from `rx_connection` — the `CaptureSource`'s poll loop reports
    // transitions there, this task owns the one canonical value `GetState()`
    // reads and `DeviceConnectionChanged` fires from. Starts optimistic
    // (matches the pre-ticket-20 hardcoded default): the real
    // `EvdevCaptureSource` reports its actual initial view within
    // milliseconds of this task starting, so this only briefly matters at
    // startup.
    let mut device_connected = true;
    // The dispatch task's live view of which capture path is running
    // (ticket 23), updated from `rx_capture_mode` — the supervisor pushes a
    // value on every mode transition, mirroring `device_connected` above.
    // Starts optimistic as `Digital` for the same reason `device_connected`
    // starts `true`: the supervisor reports its real startup choice within
    // milliseconds, so this only briefly matters before the first push.
    let mut capture_mode = CaptureMode::Digital;
    // The dispatch task's live view of the connected device's firmware/
    // serial (ticket 101), updated from `rx_device_info` — `None` until the
    // supervisor's first successful read after a connect, back to `None` on
    // disconnect. Unlike `device_connected`/`capture_mode` there is no
    // optimistic startup value: absent is the honest state until a read
    // actually lands.
    let mut device_info: Option<DeviceInfo> = None;
    let mut commands_open = true;
    let mut connection_open = true;
    let mut capture_mode_open = true;
    let mut device_info_open = true;
    // The pure Chord-detection state machine (the ~50ms simultaneity window
    // plus its `claimed` bookkeeping, ticket 01/40) and the `ChordKey`-keyed
    // firing/toggle handles its effects run against — both reset fresh on
    // every dispatch task start, same as `toggles`/`active_layer`.
    let mut chord_machine = chord::ChordMachine::default();
    let mut chord_runtime = ChordRuntime::default();
    // Owns every Axis-assigned Input's live contribution/opposite-half
    // ownership (ticket 59/71) — reset fresh on every dispatch task start,
    // same as the Chord runtime state above.
    let mut axis_state = AxisState::default();
    // Every currently-running Analog-repeat task (ticket 20/39), keyed by
    // grid Input — reset fresh on every dispatch task start, same as
    // `axis_state`; started/stopped by `update_analog_repeats` off every
    // `rx_depth` snapshot below.
    let mut analog_repeats: HashMap<Input, ActiveAnalogRepeat> = HashMap::new();
    let mut depth_open = true;
    // Bundles the task-local runtime state an `edit::Effect` can touch into
    // the `EffectCtx` `run_effects`/`handle_command` want (ticket 05). Built
    // fresh at each use site — after the input-path handler that borrows the
    // same locals `&mut` has returned — never held across a `select!` poll.
    macro_rules! effect_ctx {
        () => {
            EffectCtx {
                injector: &injector,
                toggles: &mut toggles,
                stepper_cursors: &mut stepper_cursors,
                axis_state: &mut axis_state,
                analog_repeats: &mut analog_repeats,
                actuation_tx: &actuation_tx,
                capture_control_tx: &capture_control_tx,
                signal_emitter: &signal_emitter,
                active_layer,
            }
        };
    }
    loop {
        tokio::select! {
            event = rx_events.recv() => {
                let Some(event) = event else { break };
                let edits = handle_event(
                    &injector,
                    &config,
                    &mut toggles,
                    &mut in_flight,
                    &mut stepper_cursors,
                    &mut active_layer,
                    &signal_emitter,
                    &mut chord_machine,
                    &mut chord_runtime,
                    &mut axis_state,
                    &mut analog_repeats,
                    toggle_lap_target,
                    event,
                )
                .await?;
                if !edits.is_empty() {
                    let mut ctx = effect_ctx!();
                    commit_input_edits(edits, &mut config, &config_path, &mut ctx).await;
                }
            }
            changed = rx_depth.changed(), if depth_open => {
                match changed {
                    Ok(()) => {
                        let snapshot = rx_depth.borrow_and_update().clone();
                        handle_depth_update(&injector, &config, active_layer, &mut axis_state, snapshot.clone()).await?;
                        update_analog_repeats(
                            &injector,
                            &config,
                            active_layer,
                            &mut analog_repeats,
                            &mut stepper_cursors,
                            &rx_depth,
                            &snapshot,
                        )
                        .await;
                    }
                    Err(_) => depth_open = false,
                }
            }
            () = wait_for_chord_deadline(chord::next_deadline(&chord_machine)) => {
                let edits = match chord::tick(&mut chord_machine, Instant::now()) {
                    chord::ChordOutcome::Handled(effects) => {
                        run_chord_effects(
                            effects,
                            &injector,
                            &config,
                            &mut chord_runtime,
                            &mut toggles,
                            &mut in_flight,
                            &mut stepper_cursors,
                            active_layer,
                            toggle_lap_target,
                        )
                        .await?
                    }
                    chord::ChordOutcome::NotMine => Vec::new(),
                };
                if !edits.is_empty() {
                    let mut ctx = effect_ctx!();
                    commit_input_edits(edits, &mut config, &config_path, &mut ctx).await;
                }
            }
            connected = rx_connection.recv(), if connection_open => {
                match connected {
                    Some(connected) => handle_connection_change(&mut device_connected, &signal_emitter, connected).await,
                    None => connection_open = false,
                }
            }
            mode = rx_capture_mode.recv(), if capture_mode_open => {
                match mode {
                    Some(mode) => handle_capture_mode_change(&mut capture_mode, &signal_emitter, &mut analog_repeats, mode).await,
                    None => capture_mode_open = false,
                }
            }
            info = rx_device_info.recv(), if device_info_open => {
                match info {
                    // Ticket 101: no signal — the About dialog reads the
                    // fields straight from a `GetState()` snapshot it takes
                    // when it opens, and they never change within a
                    // connection. `Some(None)` is the disconnect case
                    // clearing the cache.
                    Some(update) => device_info = update,
                    None => device_info_open = false,
                }
            }
            cmd = rx_commands.recv(), if commands_open => {
                match cmd {
                    Some(cmd) => {
                        let mut ctx = effect_ctx!();
                        handle_command(
                            &mut config,
                            &config_path,
                            device_connected,
                            capture_mode,
                            device_info.as_ref(),
                            &mut ctx,
                            cmd,
                        )
                        .await;
                    }
                    None => commands_open = false,
                }
            }
        }
    }
    Ok(())
}

/// Resolves one `PhysicalEvent` against the active Profile/Layer. Returns
/// the `Edit`s (if any) the `run` loop must commit — in practice empty, or a
/// single `Edit::SwitchProfile` when a Fire-once `Action::ProfileSwitch`
/// binding fires on `Down` (ticket 05). Takes `&Config`, never `&mut` — the
/// `run` loop is the sole commit point.
#[allow(clippy::too_many_arguments)]
async fn handle_event(
    injector: &Injector,
    config: &Config,
    toggles: &mut HashMap<Input, ActiveToggle>,
    in_flight: &mut HashMap<Input, FiringHandle>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    active_layer: &mut Layer,
    signal_emitter: &Option<SignalEmitter<'static>>,
    chord_machine: &mut chord::ChordMachine,
    chord_runtime: &mut ChordRuntime,
    axis_state: &mut AxisState,
    analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>,
    toggle_lap_target: Duration,
    event: PhysicalEvent,
) -> io::Result<Vec<edit::Edit>> {
    let profile = config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile");

    if event.input == Input::ModeKey && profile.mode_key_role == ModeKeyRole::LayerSwitch {
        handle_layer_switch(
            injector,
            active_layer,
            signal_emitter,
            axis_state,
            analog_repeats,
            event.state,
        )
        .await?;
        return Ok(Vec::new());
    }

    // A Down on an Input with an active Toggle always stops that Toggle
    // first, regardless of what Binding the Input's current Layer nominally
    // assigns — this press is consumed entirely by the stop, per spec.md's
    // "Toggle behavior across Layer/Profile switches". Only a later press
    // resumes normal evaluation.
    if event.state == EventState::Down
        && let Some(toggle) = toggles.remove(&event.input)
    {
        toggle.stop().await;
        return Ok(Vec::new());
    }

    // An Axis-assigned Input (ticket 59/71) is structurally excluded from
    // both `bindings_*` and Chord membership on this Layer (enforced
    // atomically by `SetAxisAssignment`/rejected up front by `SetBinding`/
    // `SetChordBinding`), so it must never reach the ordinary Binding lookup
    // or passthrough below. An Analog-sourced event (`event.depth` is
    // `Some`) is swallowed here — the continuous `rx_depth` watch-channel
    // path (`handle_depth_update`) already drives this Input's output on
    // every report, not just on a Down/Up/Repeat transition; a Digital-
    // sourced one (`None`) runs the press/release step-increment fallback
    // (ticket 59 §6).
    let axis_map = profile.axis_layer(*active_layer);
    if axis_map.contains_key(&event.input) {
        if event.depth.is_none() {
            handle_axis_edge_event(injector, axis_state, axis_map, event.input, event.state)
                .await?;
        }
        return Ok(Vec::new());
    }

    // The Chord-detection state machine (ticket 01/40, post-release ticket
    // 07) runs unconditionally, after the guards above and before ordinary
    // Binding lookup — it owns the "is this event mine?" predicate now
    // (`ChordOutcome::NotMine` when it isn't), rather than `handle_event`
    // reaching into `claimed` / `chord_keys_containing` itself.
    let live = chord_slots(chord_runtime);
    match chord::feed(chord_machine, profile.chords(*active_layer), &live, event) {
        chord::ChordOutcome::Handled(effects) => {
            return run_chord_effects(
                effects,
                injector,
                config,
                chord_runtime,
                toggles,
                in_flight,
                stepper_cursors,
                *active_layer,
                toggle_lap_target,
            )
            .await;
        }
        chord::ChordOutcome::NotMine => {}
    }

    let bindings = profile.layer(*active_layer);
    let binding = bindings.get(&event.input).cloned();

    // Real firing for an Analog-repeat Binding while Depth is available comes
    // entirely from `update_analog_repeats`'s own depth-driven background task
    // (ticket 20/39) — this Analog-sourced edge event (synthesized from the
    // key's ordinary, *tunable* Actuation point) is swallowed outright rather
    // than double-firing, mirroring the Axis-assignment swallow above. Never
    // fires for the Chord machine's synthetic retroactive Down (`depth: None`).
    if let Some(binding) = &binding
        && binding.trigger == TriggerMode::AnalogRepeat
        && event.depth.is_some()
    {
        return Ok(Vec::new());
    }

    match event.state {
        EventState::Down => {
            // The bound → `fire` / `ProfileSwitch` → `Edit` / unbound →
            // passthrough tail, shared verbatim with the Chord machine's
            // `FireIndividual` executor so the retroactive-fire logic exists
            // once.
            dispatch_individual_down(
                injector,
                config,
                toggles,
                in_flight,
                stepper_cursors,
                *active_layer,
                event.input,
                toggle_lap_target,
            )
            .await
        }
        EventState::Repeat | EventState::Up => {
            let Some(binding) = binding else {
                injector
                    .inject_physical(event)
                    .await
                    .map_err(io::Error::other)?;
                return Ok(Vec::new());
            };
            // A `ProfileSwitch` binding is validated Fire-once, so only its
            // `Down` fires it (handled above) — a later Repeat/Up is inert.
            if matches!(binding.action, Action::ProfileSwitch { .. }) {
                return Ok(Vec::new());
            }
            fire(
                injector,
                toggles,
                in_flight,
                event.input,
                &binding,
                event.state,
                &config.macros,
                &config.steppers,
                stepper_cursors,
                toggle_lap_target,
            )
            .await?;
            Ok(Vec::new())
        }
    }
}

/// Builds the `chord::ChordSlot` liveness snapshot `chord::feed` wants from
/// dispatch's `ChordRuntime` — `toggles` → `Toggle`, `firings` →
/// `FiringUnfinished` / `FiringFinished` by `handle.is_finished()`. Firings
/// are inserted first so a `Toggle` entry wins if a live re-bind ever left
/// both (matching the old `starting`/`stopping` filters, which checked the
/// toggle map first).
fn chord_slots(runtime: &ChordRuntime) -> HashMap<ChordKey, chord::ChordSlot> {
    let mut live = HashMap::new();
    for (key, handle) in &runtime.firings {
        let slot = if handle.is_finished() {
            chord::ChordSlot::FiringFinished
        } else {
            chord::ChordSlot::FiringUnfinished
        };
        live.insert(key.clone(), slot);
    }
    for key in runtime.toggles.keys() {
        live.insert(key.clone(), chord::ChordSlot::Toggle);
    }
    live
}

/// Performs each `chord::ChordEffect` the pure machine decided on, in order,
/// against the runtime state dispatch owns (ticket 07). Returns any
/// `edit::Edit`s a `FireIndividual` produced (a member's individual Binding
/// resolving to `Action::ProfileSwitch`) for the `run` loop to commit, same
/// as the old `handle_chord_event` / `handle_chord_timeout` return.
#[allow(clippy::too_many_arguments)]
async fn run_chord_effects(
    effects: Vec<chord::ChordEffect>,
    injector: &Injector,
    config: &Config,
    chord_runtime: &mut ChordRuntime,
    toggles: &mut HashMap<Input, ActiveToggle>,
    in_flight: &mut HashMap<Input, FiringHandle>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    active_layer: Layer,
    toggle_lap_target: Duration,
) -> io::Result<Vec<edit::Edit>> {
    let mut edits = Vec::new();
    for effect in effects {
        match effect {
            chord::ChordEffect::FireChord {
                key,
                binding,
                state,
            } => {
                execute_chord_fire(
                    injector,
                    chord_runtime,
                    key,
                    &binding,
                    state,
                    &config.macros,
                    &config.steppers,
                    stepper_cursors,
                    toggle_lap_target,
                )
                .await?;
            }
            chord::ChordEffect::ReleaseChordFiring { key } => {
                // Fire-once / Hold-to-repeat only — a Toggle Chord is
                // deliberately not stopped by a member's `Up` (ticket 67).
                if let Some(firing) = chord_runtime.firings.get(&key) {
                    firing.force_release_stuck(injector).await;
                }
            }
            chord::ChordEffect::StopChordToggle { key } => {
                if let Some(toggle) = chord_runtime.toggles.remove(&key) {
                    toggle.stop().await;
                }
            }
            chord::ChordEffect::FireIndividual { input } => {
                edits.extend(
                    dispatch_individual_down(
                        injector,
                        config,
                        toggles,
                        in_flight,
                        stepper_cursors,
                        active_layer,
                        input,
                        toggle_lap_target,
                    )
                    .await?,
                );
            }
            chord::ChordEffect::ForceReleaseIndividual { input } => {
                if let Some(firing) = in_flight.get(&input) {
                    firing.force_release_stuck(injector).await;
                }
            }
        }
    }
    Ok(edits)
}

/// Awaits the active Chord window's deadline, or never resolves if none is
/// open — the `select!` branch in `run` re-creates this future every loop
/// iteration, so a window opened, extended, or cleared by `handle_event` in
/// between is always picked up on the very next iteration (recreating a
/// `sleep_until` against the same absolute `Instant` doesn't lose progress).
/// Replaces the old `chord_window_deadline`.
async fn wait_for_chord_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// The `LayerSwitch` interception itself: `Down` activates Held, `Up`
/// reverts to Base, `Repeat` is a steady-state no-op (evdev autorepeat on a
/// held modifier key carries no new information here). Emits
/// `ActiveLayerChanged` only on an actual transition — a `signal_emitter` of
/// `None` (unit tests with no live D-Bus connection) simply skips the push.
/// Also resets every Axis output (ticket 71) on an actual transition — the
/// outgoing Layer's Axis-assignment map generally differs from the incoming
/// one, so any live output must not be left driving a target the newly-
/// active Layer no longer even assigns. Force-stops every Analog-repeat task
/// for the same reason (ticket 39) — an incoming Layer's Bindings generally
/// differ from the outgoing one's, so a task compiled against the old
/// Layer's Action must not keep firing under the new one.
async fn handle_layer_switch(
    injector: &Injector,
    active_layer: &mut Layer,
    signal_emitter: &Option<SignalEmitter<'static>>,
    axis_state: &mut AxisState,
    analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>,
    state: EventState,
) -> io::Result<()> {
    let new_layer = match state {
        EventState::Down => Layer::Held,
        EventState::Up => Layer::Base,
        EventState::Repeat => return Ok(()),
    };
    if new_layer == *active_layer {
        return Ok(());
    }
    *active_layer = new_layer;
    reset_axis_outputs(injector, axis_state).await?;
    stop_all_analog_repeats(analog_repeats).await;
    if let Some(emitter) = signal_emitter {
        let _ = Daemon::active_layer_changed(emitter, new_layer.as_str()).await;
    }
    Ok(())
}

/// Updates the dispatch task's view of device connectivity (ticket 20) and
/// emits `DeviceConnectionChanged` only on an actual transition — mirrors
/// `handle_layer_switch`'s pattern for `ActiveLayerChanged` above, including
/// skipping the push when `signal_emitter` is `None` (unit tests with no
/// live D-Bus connection).
async fn handle_connection_change(
    device_connected: &mut bool,
    signal_emitter: &Option<SignalEmitter<'static>>,
    connected: bool,
) {
    if connected == *device_connected {
        return;
    }
    *device_connected = connected;
    if let Some(emitter) = signal_emitter {
        let _ = Daemon::device_connection_changed(emitter, connected).await;
    }
}

/// Updates the dispatch task's view of which capture path is running
/// (ticket 23) and emits `CaptureModeChanged` only on an actual transition —
/// mirrors `handle_connection_change` above exactly, including skipping the
/// push when `signal_emitter` is `None` (unit tests with no live D-Bus
/// connection). Also force-stops every Analog-repeat task on a transition to
/// Digital (ticket 39): the live-Depth stream every task's rate curve reads
/// goes stale the moment analog capture stops, and Digital-sourced
/// Down/Repeat/Up events for the same Bindings are about to start reaching
/// `fire()`'s own Hold-to-repeat-equivalent fallback instead — a still-
/// running task would otherwise double-fire alongside it.
async fn handle_capture_mode_change(
    capture_mode: &mut CaptureMode,
    signal_emitter: &Option<SignalEmitter<'static>>,
    analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>,
    mode: CaptureMode,
) {
    if mode == *capture_mode {
        return;
    }
    *capture_mode = mode;
    if mode == CaptureMode::Digital {
        stop_all_analog_repeats(analog_repeats).await;
    }
    if let Some(emitter) = signal_emitter {
        let _ = Daemon::capture_mode_changed(emitter, mode.as_str()).await;
    }
}

/// The Digital Capture mode fallback (ticket 59 §6/71): press/release
/// step-increment for an Axis-assigned Input carrying no Depth at all —
/// `handle_event` only ever routes a genuinely Digital-sourced event here
/// (`event.depth` is `None`); an Analog-sourced one is fully handled by the
/// continuous `handle_depth_update` path instead. Reuses the kernel's own
/// autorepeat cadence (the same `Repeat` stream Hold-to-repeat rides) to
/// ramp up by `AXIS_DIGITAL_STEP` on every Down/Repeat, saturating at 255;
/// `Up` resets to 0 — the closest digital emulation of "Depth rises while
/// held, drops to 0 on release" ticket 59 §6 asks for.
async fn handle_axis_edge_event(
    injector: &Injector,
    axis_state: &mut AxisState,
    axis_map: &HashMap<Input, AxisTarget>,
    input: Input,
    state: EventState,
) -> io::Result<()> {
    let current = axis_state.contributions.get(&input).copied().unwrap_or(0);
    let next = match state {
        EventState::Down | EventState::Repeat => current.saturating_add(AXIS_DIGITAL_STEP),
        EventState::Up => 0,
    };
    axis_state.contributions.insert(input, next);
    recompute_and_emit_axes(injector, axis_state, axis_map).await
}

/// The continuous Analog half of ticket 59 §7's `(Depth, edge_event) ->
/// axis_value` seam: reacts to every change of the live-Depth watch channel
/// (`capture::analog`'s grid task, ticket 26) by resolving
/// `config::resolve_axis_value` for every Input the active Layer currently
/// Axis-assigns, then running the shared conflict-resolution/emit path.
/// Every Grid key's raw depth is published on every incoming hidraw report
/// regardless of Binding/Axis status (`capture::analog::relay_grid_blocking`),
/// so this only ever *reads* `depths` for the subset that's actually
/// Axis-assigned right now — an empty Axis map (the common case) short-
/// circuits immediately, doing no work on every ordinary depth tick.
async fn handle_depth_update(
    injector: &Injector,
    config: &Config,
    active_layer: Layer,
    axis_state: &mut AxisState,
    depths: HashMap<Input, u8>,
) -> io::Result<()> {
    let profile = config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile");
    let axis_map = profile.axis_layer(active_layer);
    if axis_map.is_empty() {
        return Ok(());
    }
    // Ticket 71 code-review finding: reads each relevant Input's own
    // Actuation/Release point directly, rather than building
    // `resolved_actuation_points()`'s full 20-entry `HashMap` just to read
    // the 1-4 entries an Axis-assigned Profile actually needs — this runs on
    // every live-Depth tick (sub-millisecond while a key is moving, per
    // ticket 13), so the redundant O(20) rebuild was real hot-path waste.
    for &input in axis_map.keys() {
        if let Some(&depth) = depths.get(&input) {
            let point = profile.resolved_actuation_point(input);
            axis_state
                .contributions
                .insert(input, config::resolve_axis_value(depth, point));
        }
    }
    recompute_and_emit_axes(injector, axis_state, axis_map).await
}

/// The executor half of a `chord::ChordEffect::FireChord` — `fire`'s exact
/// mirror for a Chord's own Trigger-mode dispatch (ticket 01/40):
/// Fire-once/Hold-to-repeat share one Chord-scoped `FiringHandle` slot per
/// `ChordKey`, Toggle spawns/tracks one `ActiveToggle` per `ChordKey`, both
/// keyed by the Chord's member set rather than by a single Input.
/// `ProfileSwitch` never reaches here — `SetChordBinding`/`parse` both refuse
/// to let a Chord's Action be `ProfileSwitch` (see
/// `ConfigError::InvalidChordProfileSwitch`), since `compile_action` panics
/// on it (it has no `MacroStep` form, only the individual-Input path ever
/// specially handles it). Formerly `fire_chord`; the routing decision it
/// used to make inline is now `chord::feed` / `chord::tick`.
#[allow(clippy::too_many_arguments)]
async fn execute_chord_fire(
    injector: &Injector,
    chord_runtime: &mut ChordRuntime,
    key: ChordKey,
    binding: &Binding,
    state: EventState,
    macros: &HashMap<MacroId, MacroDef>,
    steppers: &HashMap<StepperId, StepperDef>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    toggle_lap_target: Duration,
) -> io::Result<()> {
    let chord_in_flight = &mut chord_runtime.firings;
    let chord_toggles = &mut chord_runtime.toggles;
    match (binding.trigger, state) {
        (TriggerMode::HoldToRepeat, EventState::Repeat)
            if matches!(binding.action, Action::ControllerButton { .. }) =>
        {
            // Mirrors `fire`'s own ControllerButton/HoldToRepeat Repeat
            // arm (ticket 75/76) — the Chord's leader member's Repeat is
            // ignored outright rather than re-firing.
            Ok(())
        }
        (TriggerMode::HoldToRepeat, EventState::Down)
            if matches!(binding.action, Action::ControllerButton { .. }) =>
        {
            if let Some(handle) = chord_in_flight.get(&key)
                && !handle.is_finished()
            {
                return Ok(());
            }
            let Action::ControllerButton { button } = binding.action else {
                unreachable!("guarded by this arm's own match guard above")
            };
            // Mirrors `fire`'s own bare-KeyDown ControllerButton hold —
            // released by the `ReleaseChordFiring` effect on a member's physical Up.
            let steps = vec![MacroStep::KeyDown(button)];
            let handle = executor::spawn_fire_once(injector.clone(), steps);
            chord_in_flight.insert(key, handle);
            Ok(())
        }
        (TriggerMode::HoldToRepeat, EventState::Repeat)
            if matches!(
                binding.action,
                Action::Keypress { key, .. } if crate::input::is_mouse_button(key)
            ) =>
        {
            // Mirrors `fire`'s own mouse-button/HoldToRepeat Repeat arm
            // (ticket 79/80) — the Chord's leader member's Repeat is
            // ignored outright rather than re-firing.
            Ok(())
        }
        (TriggerMode::HoldToRepeat, EventState::Down)
            if matches!(
                binding.action,
                Action::Keypress { key, .. } if crate::input::is_mouse_button(key)
            ) =>
        {
            if let Some(handle) = chord_in_flight.get(&key)
                && !handle.is_finished()
            {
                return Ok(());
            }
            let Action::Keypress { key: button, .. } = binding.action else {
                unreachable!("guarded by this arm's own match guard above")
            };
            // Mirrors `fire`'s own bare-KeyDown mouse-button hold —
            // released by the `ReleaseChordFiring` effect on a member's physical Up.
            let steps = vec![MacroStep::KeyDown(button)];
            let handle = executor::spawn_fire_once(injector.clone(), steps);
            chord_in_flight.insert(key, handle);
            Ok(())
        }
        (TriggerMode::FireOnce, EventState::Down)
        | (TriggerMode::HoldToRepeat, EventState::Down | EventState::Repeat) => {
            if let Some(handle) = chord_in_flight.get(&key)
                && !handle.is_finished()
            {
                return Ok(());
            }
            let steps = compile_action(&binding.action, macros, steppers, stepper_cursors);
            let handle = executor::spawn_fire_once(injector.clone(), steps);
            chord_in_flight.insert(key, handle);
            Ok(())
        }
        (TriggerMode::Toggle, EventState::Down)
            if matches!(
                binding.action,
                Action::Keypress { key, .. } if crate::input::is_mouse_button(key)
            ) =>
        {
            let Action::Keypress { key: button, .. } = binding.action else {
                unreachable!("guarded by this arm's own match guard above")
            };
            // Ticket 82: a mouse-button Chord Toggle gets the same
            // sustained-hold treatment as a plain Input's own Toggle below
            // — a single held KeyDown rather than a repeat-tap loop.
            chord_toggles.insert(key, ActiveToggle::spawn_held(injector.clone(), button));
            Ok(())
        }
        (TriggerMode::Toggle, EventState::Down)
            if matches!(binding.action, Action::ControllerButton { .. }) =>
        {
            let Action::ControllerButton { button } = binding.action else {
                unreachable!("guarded by this arm's own match guard above")
            };
            // Ticket 78: a gamepad button Chord Toggle gets the same
            // sustained-hold treatment as a plain Input's own ControllerButton
            // Toggle above, and as the mouse-button Chord Toggle carve-out
            // above it.
            chord_toggles.insert(key, ActiveToggle::spawn_held(injector.clone(), button));
            Ok(())
        }
        (TriggerMode::Toggle, EventState::Down) => {
            let steps = compile_action(&binding.action, macros, steppers, stepper_cursors);
            chord_toggles.insert(
                key,
                ActiveToggle::spawn(injector.clone(), steps, toggle_lap_target),
            );
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Dispatches a single fresh `Down` on `input` against the active Layer —
/// the `ProfileSwitch → Edit` / bound → `fire` / unbound → passthrough tail
/// carved out of `handle_event`, shared verbatim by the ordinary input path
/// and the Chord machine's `FireIndividual` executor (a member's individual
/// Binding firing retroactively — the window elapsed, or the member was
/// released before completing — per ticket 01's Answer: "the pending
/// member's individual Binding fires retroactively, delayed by the window").
/// It is *not* a re-entry into `handle_event`: that would re-run the
/// layer-switch / toggle-stop / axis / chord guards against a synthetic
/// Down, which is wrong. Returns any `Edit::SwitchProfile` the member's own
/// Binding produces — a Chord member's individual Binding can be any Action,
/// unlike a Chord's own, which can never be `ProfileSwitch`.
#[allow(clippy::too_many_arguments)]
async fn dispatch_individual_down(
    injector: &Injector,
    config: &Config,
    toggles: &mut HashMap<Input, ActiveToggle>,
    in_flight: &mut HashMap<Input, FiringHandle>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    layer: Layer,
    input: Input,
    toggle_lap_target: Duration,
) -> io::Result<Vec<edit::Edit>> {
    let profile = config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile");
    let binding = profile.layer(layer).get(&input).cloned();
    match binding {
        Some(binding) => {
            if let Action::ProfileSwitch { target } = binding.action {
                // The switch is an `Edit` for the `run` loop to commit
                // (ticket 05).
                return Ok(vec![edit::Edit::SwitchProfile { name: target }]);
            }
            // Accepted gap (ticket 39): a member's own individual Binding
            // set to Analog-repeat fires once here through `fire()`'s
            // ordinary one-shot path, rather than starting the depth-driven
            // background task `update_analog_repeats` normally would — this
            // retroactive Down is synthetic (no real live Depth to hand a
            // task), and a grid key that's both a Chord member *and*
            // individually Analog-repeat-triggered is a narrow combination
            // this fast-follow doesn't specially engineer for.
            fire(
                injector,
                toggles,
                in_flight,
                input,
                &binding,
                EventState::Down,
                &config.macros,
                &config.steppers,
                stepper_cursors,
                toggle_lap_target,
            )
            .await?;
            Ok(Vec::new())
        }
        None => {
            injector
                .inject_physical(PhysicalEvent {
                    input,
                    state: EventState::Down,
                    depth: None,
                })
                .await
                .map_err(io::Error::other)?;
            Ok(Vec::new())
        }
    }
}

/// Compiles a Binding's `Action` into the flat step sequence `fire` spawns —
/// `executor::compile` for every ordinary Action, or `resolve_step` for
/// `Action::Step`, whose steps depend on Daemon-owned runtime cursor state
/// `executor::compile` has no access to (ticket 03/54).
fn compile_action(
    action: &Action,
    macros: &HashMap<MacroId, MacroDef>,
    steppers: &HashMap<StepperId, StepperDef>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
) -> Vec<executor::MacroStep> {
    match action {
        Action::Step { stepper, direction } => {
            resolve_step(steppers, stepper_cursors, stepper, *direction)
        }
        other => executor::compile(other, macros),
    }
}

/// Advances/retreats a Stepper's per-list cursor (Daemon-side-only runtime
/// state, ticket 03/54 — CONTEXT.md: Stepper) and compiles the
/// newly-selected item — "one motion moves the cursor and fires," ticket
/// 03's Answer's firing semantics. A `Key` item reuses `Action::Keypress`'s
/// mods-down/key/mods-up compile path, carrying its own modifier
/// combination if it has one (ticket 62); a `ControllerButton` item (ticket
/// 92) reuses `Action::ControllerButton`'s down/dwell/up triple. A missing
/// cursor entry
/// means "at the list's first item" (index 0), matching `stepper_cursors`'s
/// own always-resets-to-first-item-on-restart convention. Wraps at either
/// end. A `stepper` with zero items compiles to no steps at all — nothing to
/// select, nothing to fire, cursor left untouched.
fn resolve_step(
    steppers: &HashMap<StepperId, StepperDef>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    stepper: &StepperId,
    direction: StepDirection,
) -> Vec<executor::MacroStep> {
    let def = steppers.get(stepper).expect(
        "SetBinding/config::parse validate every Action::Step references an existing StepperDef",
    );
    let len = def.items.len();
    if len == 0 {
        return Vec::new();
    }
    let current = stepper_cursors
        .get(stepper)
        .copied()
        .unwrap_or(0)
        .min(len - 1);
    let next = match direction {
        StepDirection::Forward => (current + 1) % len,
        StepDirection::Backward => (current + len - 1) % len,
    };
    stepper_cursors.insert(stepper.clone(), next);
    match def.items[next] {
        // A keyboard/mouse item: the item's own modifier combination
        // (ticket 62) through `Action::Keypress`'s canned compile path.
        StepperItem::Key { key, modifiers } => executor::keypress_steps(modifiers, key),
        // A gamepad-button item (ticket 92): the same atomic down/dwell/up
        // triple as `Action::ControllerButton`'s digital path, routed to
        // the gamepad `uinput` device by the injector's own
        // `input::is_gamepad_button` check.
        StepperItem::ControllerButton { button } => executor::controller_button_steps(button),
    }
}

/// Branches on `TriggerMode` x event state, per ticket 17: Fire-once fires
/// only on `Down`; Hold-to-repeat fires on `Down` and every subsequent
/// `Repeat` (the device's own evdev autorepeat, no separate repeat-interval
/// config); Toggle starts its own looping task on `Down` (stopping is
/// handled earlier, in `handle_event`, before a Binding is even looked up).
/// `Repeat` for Fire-once/Toggle is ignored outright — no passthrough of the
/// original key for a bound Input, matching the pre-ticket-17 Fire-once
/// behavior. `Up` for Fire-once/Hold-to-repeat is ticket 33's stuck-key fix:
/// force-releases anything that Input's most recent firing left down (a
/// no-op for an already-self-released, balanced Macro); Toggle's own `Up` is
/// still a no-op, since a Toggle's stop is a second `Down`, not a release.
/// Analog-repeat rides the exact same Down/Repeat/Up arms as Hold-to-repeat
/// (ticket 20's Digital Capture mode fallback) — the only way this function
/// ever sees an Analog-repeat Binding at all, since `handle_event` swallows
/// every Analog-*sourced* Down/Repeat/Up for one outright, before `fire` is
/// ever called (real Analog-mode firing is `update_analog_repeats`'s own
/// depth-driven background task).
///
/// `Action::ControllerButton` + Hold-to-repeat is a carved-out exception
/// (ticket 75/76): a real gamepad button doesn't autorepeat in hardware, so
/// `Down` fires a bare, unbalanced `KeyDown` (not `compile_action`'s own
/// pulse) that mirrors the physical hold, every `Repeat` is ignored outright
/// (no re-fire), and the existing `Up` arm below force-releases it — the
/// same "held until the physical Up force-releases it" shape ticket 33
/// already relies on for an unbalanced Macro, reused rather than invented
/// fresh. `Action::ControllerButton` + Toggle gets the analogous carve-out
/// (ticket 78): `spawn_held`'s single sustained KeyDown rather than
/// `compile_action`'s repeat-tap loop, mirroring the mouse-button Toggle fix
/// (ticket 82/83) below — a real gamepad button doesn't have a "turbo" Toggle
/// mode any more than it autorepeats, so a latched Toggle should just hold it
/// down. Fire-once is disallowed for `Action::ControllerButton` entirely
/// (ticket 78, enforced at config-load/write time, not here).
#[allow(clippy::too_many_arguments)]
async fn fire(
    injector: &Injector,
    toggles: &mut HashMap<Input, ActiveToggle>,
    in_flight: &mut HashMap<Input, FiringHandle>,
    input: Input,
    binding: &Binding,
    state: EventState,
    macros: &HashMap<MacroId, MacroDef>,
    steppers: &HashMap<StepperId, StepperDef>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    toggle_lap_target: Duration,
) -> io::Result<()> {
    match (binding.trigger, state) {
        (TriggerMode::HoldToRepeat, EventState::Repeat)
            if matches!(binding.action, Action::ControllerButton { .. }) =>
        {
            // Ticket 75/76: a real gamepad button doesn't autorepeat in
            // hardware — held down, it just stays down — so once the
            // physical Down's own KeyDown below is holding it, every
            // intervening kernel-autorepeat Repeat is ignored outright, no
            // re-fire.
            Ok(())
        }
        (TriggerMode::HoldToRepeat, EventState::Down)
            if matches!(binding.action, Action::ControllerButton { .. }) =>
        {
            // Same overlap guard as the ordinary arm below.
            if let Some(handle) = in_flight.get(&input)
                && !handle.is_finished()
            {
                return Ok(());
            }
            let Action::ControllerButton { button } = binding.action else {
                unreachable!("guarded by this arm's own match guard above")
            };
            // Deliberately not `compile_action`'s own KeyDown/Delay/KeyUp
            // pulse (that's Fire-once's shape): a bare, unbalanced `KeyDown`
            // that mirrors the physical press for as long as it's actually
            // held, released by the physical Up's own arm below — reusing
            // the same "leaves a key held, a later force-release cleans it
            // up" mechanism ticket 33 already relies on, rather than
            // inventing new architecture.
            let steps = vec![MacroStep::KeyDown(button)];
            let handle = executor::spawn_fire_once(injector.clone(), steps);
            in_flight.insert(input, handle);
            Ok(())
        }
        (TriggerMode::HoldToRepeat, EventState::Repeat)
            if matches!(
                binding.action,
                Action::Keypress { key, .. } if crate::input::is_mouse_button(key)
            ) =>
        {
            // Ticket 79/80: a mouse-button Keypress gets the same
            // sustained-hold treatment as ControllerButton above, so a
            // Hold-to-repeat mouse Binding supports click-and-drag instead
            // of a repeat-tap train — once the physical Down's own KeyDown
            // below is holding it, every intervening kernel-autorepeat
            // Repeat is ignored outright, no re-fire.
            Ok(())
        }
        (TriggerMode::HoldToRepeat, EventState::Down)
            if matches!(
                binding.action,
                Action::Keypress { key, .. } if crate::input::is_mouse_button(key)
            ) =>
        {
            // Same overlap guard as the ordinary arm below.
            if let Some(handle) = in_flight.get(&input)
                && !handle.is_finished()
            {
                return Ok(());
            }
            let Action::Keypress { key, .. } = binding.action else {
                unreachable!("guarded by this arm's own match guard above")
            };
            // Deliberately not `compile_action`'s own KeyDown/Delay/KeyUp
            // pulse: a bare, unbalanced `KeyDown` that mirrors the physical
            // press for as long as it's actually held, released by the
            // physical Up's own arm below (ticket 33's force-release path).
            let steps = vec![MacroStep::KeyDown(key)];
            let handle = executor::spawn_fire_once(injector.clone(), steps);
            in_flight.insert(input, handle);
            Ok(())
        }
        (TriggerMode::FireOnce, EventState::Down)
        | (
            TriggerMode::HoldToRepeat | TriggerMode::AnalogRepeat,
            EventState::Down | EventState::Repeat,
        ) => {
            // Same-Input firings must never run concurrently — their raw
            // steps share one Injector channel, and two interleaved firings
            // could land their KeyDown/KeyUp writes out of order. A still-
            // running previous firing (a slow Macro outlasting a fast
            // HoldToRepeat autorepeat) means this one is dropped rather than
            // queued: the previous firing already reproduces the intended
            // effect, and queuing would only build an ever-growing backlog
            // while the key stays held. For a Stepper Binding this also
            // means a dropped firing must never advance the cursor — nothing
            // fired, so nothing moved — which is exactly what falls out of
            // `compile_action` only running once this guard has passed.
            if let Some(handle) = in_flight.get(&input)
                && !handle.is_finished()
            {
                return Ok(());
            }
            let steps = compile_action(&binding.action, macros, steppers, stepper_cursors);
            let handle = executor::spawn_fire_once(injector.clone(), steps);
            in_flight.insert(input, handle);
            Ok(())
        }
        (TriggerMode::Toggle, EventState::Down)
            if matches!(
                binding.action,
                Action::Keypress { key, .. } if crate::input::is_mouse_button(key)
            ) =>
        {
            let Action::Keypress { key, .. } = binding.action else {
                unreachable!("guarded by this arm's own match guard above")
            };
            // Ticket 82: a mouse-button Toggle gets the same sustained-hold
            // treatment as HoldToRepeat's own mouse-button carve-out above —
            // a single held KeyDown rather than a repeat-tap loop.
            toggles.insert(input, ActiveToggle::spawn_held(injector.clone(), key));
            Ok(())
        }
        (TriggerMode::Toggle, EventState::Down)
            if matches!(binding.action, Action::ControllerButton { .. }) =>
        {
            let Action::ControllerButton { button } = binding.action else {
                unreachable!("guarded by this arm's own match guard above")
            };
            // Ticket 78: a gamepad button Toggle gets the same sustained-hold
            // treatment as the mouse-button carve-out above (and as
            // ControllerButton's own Hold-to-repeat carve-out, ticket 75/76)
            // — a single held KeyDown rather than a repeat-tap loop, matching
            // a real held gamepad button (e.g. a latched sprint/aim).
            toggles.insert(input, ActiveToggle::spawn_held(injector.clone(), button));
            Ok(())
        }
        (TriggerMode::Toggle, EventState::Down) => {
            let steps = compile_action(&binding.action, macros, steppers, stepper_cursors);
            toggles.insert(
                input,
                ActiveToggle::spawn(injector.clone(), steps, toggle_lap_target),
            );
            Ok(())
        }
        (
            TriggerMode::FireOnce | TriggerMode::HoldToRepeat | TriggerMode::AnalogRepeat,
            EventState::Up,
        ) => {
            if let Some(firing) = in_flight.get(&input) {
                firing.force_release_stuck(injector).await;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Republishes the active Profile's resolved Actuation-point snapshot
/// (ticket 18 §5) — `run_effects`'s handler for `edit::Effect::RepublishActuation`,
/// which `edit::plan` emits from `SetActuationPoint` / `ClearActuationPoint` /
/// `SetDefaultActuation` / `ResetActuationPoints` (all touch the active
/// Profile's own `actuation_overrides`/`default_actuation`) and
/// `SwitchProfile` (changes which Profile is active). `send_replace` rather
/// than `send`: this must not fail just because no `AnalogCaptureSource` grid
/// task has subscribed yet (ticket 23 wires the real receiver; today's tests
/// hold one only to keep the channel open).
fn publish_actuation_snapshot(
    config: &Config,
    actuation_tx: &watch::Sender<HashMap<Input, ActuationPoint>>,
) {
    let profile = config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile");
    actuation_tx.send_replace(profile.resolved_actuation_points());
}

/// The runtime state `run_effects` performs an `edit::Effect` against — the
/// pieces of the dispatch task's own state an effect can touch. Built fresh
/// per call site (the `run` loop's command / input-path arms) from the
/// task's locals; `edit` never sees it. `active_layer` is copied in by value
/// so `RecomputeAxes` can no-op when its Layer isn't the active one — the
/// check `edit::plan` can't make.
struct EffectCtx<'a> {
    injector: &'a Injector,
    toggles: &'a mut HashMap<Input, ActiveToggle>,
    stepper_cursors: &'a mut HashMap<StepperId, usize>,
    axis_state: &'a mut AxisState,
    analog_repeats: &'a mut HashMap<Input, ActiveAnalogRepeat>,
    actuation_tx: &'a watch::Sender<HashMap<Input, ActuationPoint>>,
    capture_control_tx: &'a mpsc::Sender<bool>,
    signal_emitter: &'a Option<SignalEmitter<'static>>,
    active_layer: Layer,
}

/// Runs each `edit::Effect` an `edit::plan` derived, in order, against the
/// runtime state `ctx` borrows (ticket 05). `config` is the just-committed
/// `Config` — every effect that reads the new state (`RepublishActuation`,
/// `RecomputeAxes`) reads it from here. Errors on the axis-output path are
/// swallowed exactly as the pre-ticket-05 arm code swallowed them
/// (`let _ = recompute_and_emit_axes(...)` / `reset_axis_outputs`).
async fn run_effects(effects: Vec<edit::Effect>, config: &Config, ctx: &mut EffectCtx<'_>) {
    for effect in effects {
        match effect {
            edit::Effect::RepublishActuation => {
                publish_actuation_snapshot(config, ctx.actuation_tx)
            }
            edit::Effect::RecomputeAxes { layer } => {
                // `RecomputeAxes` for a Layer that isn't the active one is a
                // no-op — the resulting `Config` already carries the edit,
                // but nothing is driving that Layer's axes right now.
                if layer == ctx.active_layer {
                    let axis_map = config
                        .active_profile()
                        .expect("load_or_seed validates active_profile names a real profile")
                        .axis_layer(layer)
                        .clone();
                    let _ = recompute_and_emit_axes(ctx.injector, ctx.axis_state, &axis_map).await;
                }
            }
            edit::Effect::ForgetAxisContribution(input) => {
                ctx.axis_state.contributions.remove(&input);
            }
            edit::Effect::SignalCaptureMode(force) => {
                // Only on a successful persist (which is where `run_effects`
                // runs) — the supervisor swaps the live capture source to
                // match `config.toml` on disk.
                let _ = ctx.capture_control_tx.send(force).await;
            }
            edit::Effect::StopToggle(input) => {
                if let Some(toggle) = ctx.toggles.remove(&input) {
                    toggle.stop().await;
                }
            }
            edit::Effect::StopAllToggles => stop_all_toggles(ctx.toggles).await,
            edit::Effect::StopAllAnalogRepeats => stop_all_analog_repeats(ctx.analog_repeats).await,
            edit::Effect::ResetAxisOutputs => {
                let _ = reset_axis_outputs(ctx.injector, ctx.axis_state).await;
            }
            edit::Effect::DropStepperCursor(stepper) => {
                ctx.stepper_cursors.remove(&stepper);
            }
            edit::Effect::ClampStepperCursor { stepper, len } => {
                if let Some(cursor) = ctx.stepper_cursors.get_mut(&stepper) {
                    *cursor = (*cursor).min(len - 1);
                }
            }
            edit::Effect::AnnounceProfileChange(name) => {
                if let Some(emitter) = ctx.signal_emitter {
                    let _ = Daemon::active_profile_changed(emitter, &name).await;
                }
            }
        }
    }
}

/// Commits each `Edit` the input path returned (an `Action::ProfileSwitch`
/// binding firing — empty or one in practice, only a Fire-once on `Down`),
/// in order: `edit::apply` then `run_effects`. The `run` loop is the sole
/// commit point for an input-originated `Config` mutation (ticket 05). For
/// several returned `Edit::SwitchProfile`s (a genuinely retroactive
/// multi-switch) last-write-wins order is unchanged. One narrow shift: when a
/// single retroactive chord miss fires a `ProfileSwitch` member *alongside*
/// non-switch members, every member's binding now resolves against the
/// pre-switch Profile and the switch's effects (stop Toggles, reset axes,
/// stop Analog-repeats) run after them, rather than interleaved as the old
/// inline `switch_profile` call did — an accepted consequence of the input
/// path no longer holding `&mut Config` (see ticket 05's Answer). A failed
/// apply is logged and ignored — a dangling `ProfileSwitch` target is
/// impossible post-`validate`, so this only ever absorbs a genuine
/// `config.toml` write failure.
async fn commit_input_edits(
    edits: Vec<edit::Edit>,
    config: &mut Config,
    config_path: &Path,
    ctx: &mut EffectCtx<'_>,
) {
    for edit in edits {
        match edit::apply(config, config_path, edit).await {
            Ok(outcome) => run_effects(outcome.effects, config, ctx).await,
            Err(err) => eprintln!(
                "acheron-daemon: dispatch: ignoring a failed input-path Config edit: {err:?}"
            ),
        }
    }
}

/// Translates each `Command` into its `edit::Edit` and commits it (ticket
/// 05): the 3 non-edit arms (`GetConfig`/`GetState`/`StopAllToggles`) inline,
/// the 24 edit arms each a mechanical `edit::apply` → `reply.send` →
/// `run_effects`, with `reply` sent before effects run for every arm — which
/// is what deletes `SwitchProfile`'s old special-case reply-before-signal
/// reasoning: that ordering is now the default shape.
async fn handle_command(
    config: &mut Config,
    config_path: &Path,
    device_connected: bool,
    capture_mode: CaptureMode,
    device_info: Option<&DeviceInfo>,
    ctx: &mut EffectCtx<'_>,
    cmd: Command,
) {
    /// `edit::apply` → send `Ok(())` / `Err` → run effects on success. Every
    /// edit arm whose reply is `Result<(), CommandError>` (i.e. all but the
    /// two create commands, which carry a fresh id back).
    macro_rules! commit {
        ($reply:expr, $edit:expr) => {{
            match edit::apply(config, config_path, $edit).await {
                Ok(outcome) => {
                    let _ = $reply.send(Ok(()));
                    run_effects(outcome.effects, config, ctx).await;
                }
                Err(err) => {
                    let _ = $reply.send(Err(err));
                }
            }
        }};
    }

    match cmd {
        Command::GetConfig(reply) => {
            let _ = reply.send(config.clone());
        }
        Command::GetState(reply) => {
            // Every library entry gets a reported cursor, defaulting to `0`
            // ("the list's first item") for one never yet stepped — richer
            // for the GUI than only reporting entries this task has actually
            // touched (ticket 03/54).
            let stepper_cursors = config
                .steppers
                .keys()
                .map(|id| {
                    (
                        id.clone(),
                        ctx.stepper_cursors.get(id).copied().unwrap_or(0),
                    )
                })
                .collect();
            let _ = reply.send(State {
                profile: config.active_profile.clone(),
                layer: ctx.active_layer.as_str(),
                active_toggles: ctx.toggles.keys().copied().collect(),
                device_connected,
                capture_mode: capture_mode.as_str(),
                daemon_version: crate::VERSION,
                firmware_version: device_info.map(|info| info.firmware_version.clone()),
                serial_number: device_info.map(|info| info.serial_number.clone()),
                stepper_cursors,
            });
        }
        Command::StopAllToggles { reply } => {
            stop_all_toggles(ctx.toggles).await;
            let _ = reply.send(());
        }
        Command::SetBinding {
            input,
            layer,
            binding,
            reply,
        } => commit!(
            reply,
            edit::Edit::SetBinding {
                input,
                layer,
                binding
            }
        ),
        Command::ClearBinding {
            input,
            layer,
            reply,
        } => commit!(reply, edit::Edit::ClearBinding { input, layer }),
        Command::SetModeKeyRole { role, reply } => {
            commit!(reply, edit::Edit::SetModeKeyRole { role })
        }
        Command::CreateProfile { name, reply } => {
            commit!(reply, edit::Edit::CreateProfile { name })
        }
        Command::DeleteProfile { name, reply } => {
            commit!(reply, edit::Edit::DeleteProfile { name })
        }
        Command::RenameProfile {
            old_name,
            new_name,
            reply,
        } => {
            // The one arm with an `Ok` early-return — routing a rename to the
            // same name through `edit::apply` would add a spurious
            // `config.toml` write for a no-op, so it stays a guard ahead of
            // the helper (ticket 03 / 05 precedent). `AlreadyExists` moved
            // into `edit::plan`; the same-name `NotFound` check stays here so
            // a same-name rename of a missing Profile still fails exactly as
            // it did pre-ticket-05 (the guard sat *after* the existence check
            // in ticket 03's arm).
            if old_name == new_name {
                let reply_value = if config.profiles.contains_key(&old_name) {
                    Ok(())
                } else {
                    Err(CommandError::NotFound)
                };
                let _ = reply.send(reply_value);
                return;
            }
            commit!(reply, edit::Edit::RenameProfile { old_name, new_name })
        }
        Command::SwitchProfile { name, reply } => {
            commit!(reply, edit::Edit::SwitchProfile { name })
        }
        Command::SetActuationPoint {
            input,
            actuation,
            release,
            reply,
        } => commit!(
            reply,
            edit::Edit::SetActuationPoint {
                input,
                actuation,
                release,
            }
        ),
        Command::ClearActuationPoint { input, reply } => {
            commit!(reply, edit::Edit::ClearActuationPoint { input })
        }
        Command::SetDefaultActuation {
            actuation,
            release,
            reply,
        } => commit!(
            reply,
            edit::Edit::SetDefaultActuation { actuation, release }
        ),
        Command::ResetActuationPoints { reply } => {
            commit!(reply, edit::Edit::ResetActuationPoints)
        }
        Command::SetForceDigital { force, reply } => {
            commit!(reply, edit::Edit::SetForceDigital { force })
        }
        Command::CreateMacro { name, steps, reply } => {
            match edit::apply(config, config_path, edit::Edit::CreateMacro { name, steps }).await {
                Ok(outcome) => {
                    let Some(edit::CreatedId::Macro(macro_id)) = outcome.created else {
                        unreachable!("CreateMacro always sets Outcome.created to a Macro id")
                    };
                    let _ = reply.send(Ok(macro_id));
                    run_effects(outcome.effects, config, ctx).await;
                }
                Err(err) => {
                    let _ = reply.send(Err(err));
                }
            }
        }
        Command::RenameMacro {
            macro_id,
            new_name,
            reply,
        } => commit!(reply, edit::Edit::RenameMacro { macro_id, new_name }),
        Command::DeleteMacro { macro_id, reply } => {
            commit!(reply, edit::Edit::DeleteMacro { macro_id })
        }
        Command::SetMacroSteps {
            macro_id,
            steps,
            reply,
        } => commit!(reply, edit::Edit::SetMacroSteps { macro_id, steps }),
        Command::CreateStepper { name, items, reply } => {
            match edit::apply(
                config,
                config_path,
                edit::Edit::CreateStepper { name, items },
            )
            .await
            {
                Ok(outcome) => {
                    let Some(edit::CreatedId::Stepper(stepper_id)) = outcome.created else {
                        unreachable!("CreateStepper always sets Outcome.created to a Stepper id")
                    };
                    let _ = reply.send(Ok(stepper_id));
                    run_effects(outcome.effects, config, ctx).await;
                }
                Err(err) => {
                    let _ = reply.send(Err(err));
                }
            }
        }
        Command::RenameStepper {
            stepper_id,
            new_name,
            reply,
        } => commit!(
            reply,
            edit::Edit::RenameStepper {
                stepper_id,
                new_name
            }
        ),
        Command::DeleteStepper { stepper_id, reply } => {
            commit!(reply, edit::Edit::DeleteStepper { stepper_id })
        }
        Command::SetStepperItems {
            stepper_id,
            items,
            reply,
        } => commit!(reply, edit::Edit::SetStepperItems { stepper_id, items }),
        Command::SetChordBinding {
            inputs,
            layer,
            binding,
            reply,
        } => commit!(
            reply,
            edit::Edit::SetChordBinding {
                inputs,
                layer,
                binding
            }
        ),
        Command::ClearChordBinding {
            inputs,
            layer,
            reply,
        } => commit!(reply, edit::Edit::ClearChordBinding { inputs, layer }),
        Command::SetAxisAssignment {
            input,
            layer,
            target,
            reply,
        } => commit!(
            reply,
            edit::Edit::SetAxisAssignment {
                input,
                layer,
                target
            }
        ),
        Command::ClearAxisAssignment {
            input,
            layer,
            reply,
        } => commit!(reply, edit::Edit::ClearAxisAssignment { input, layer }),
    }
}

/// Force-stops every currently running Toggle — shared by `SwitchProfile`'s
/// `StopAllToggles` effect and the `StopAllToggles` command (ticket 25, on
/// its own, GUI-focus-gain triggered) so the drain-and-stop loop has exactly
/// one implementation.
async fn stop_all_toggles(toggles: &mut HashMap<Input, ActiveToggle>) {
    for (_, toggle) in toggles.drain() {
        toggle.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::fake::FakeCaptureSource;
    use crate::capture::{CaptureSource, EventState};
    use crate::config::{
        Action, ActuationPoint, DEFAULT_PROFILE_NAME, MacroStepDto, Modifiers, Profile,
    };
    use crate::injector::testing::RecordingSink;
    use crate::injector::{self};
    use crate::input::{Direction, WheelEvent};
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::oneshot;

    /// Same helper as `executor`'s own test module — used by the newer,
    /// terser ControllerButton Hold-to-repeat tests (ticket 75/76) rather
    /// than this file's older, more verbose `destructure()`/`else { panic!
    /// }` inline pattern.
    fn key_and_value(event: evdev::InputEvent) -> (evdev::KeyCode, i32) {
        match event.destructure() {
            evdev::EventSummary::Key(_, code, value) => (code, value),
            other => panic!("expected a key event, got {other:?}"),
        }
    }

    fn config_with_bindings(bindings: HashMap<Input, Binding>) -> Config {
        config_with_profile(Profile {
            base: bindings,
            ..Default::default()
        })
    }

    fn config_with_profile(profile: Profile) -> Config {
        config_with_profile_and_macros(profile, HashMap::new())
    }

    fn config_with_profile_and_macros(
        profile: Profile,
        macros: HashMap<MacroId, MacroDef>,
    ) -> Config {
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), profile);
        Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros,
            steppers: HashMap::new(),
        }
    }

    fn config_with_bindings_and_macros(
        bindings: HashMap<Input, Binding>,
        macros: HashMap<MacroId, MacroDef>,
    ) -> Config {
        config_with_profile_and_macros(
            Profile {
                base: bindings,
                ..Default::default()
            },
            macros,
        )
    }

    /// Registers one `MacroDef` under `macro_id` and returns the map
    /// alongside the `Action::Macro` referencing it — this test module's
    /// shorthand for what used to be an inline `Action::Macro { steps }`
    /// before ticket 51 moved step content off the Binding and into the
    /// library. None of these tests exercise Macro-library behavior itself
    /// (that's covered separately, in the `Command::CreateMacro`/
    /// `RenameMacro`/`DeleteMacro` tests below) — they're reusing a
    /// multi-step Macro Action as a convenient way to exercise Trigger-mode/
    /// timing behavior.
    fn macro_action(
        macro_id: &str,
        steps: Vec<MacroStepDto>,
    ) -> (Action, HashMap<MacroId, MacroDef>) {
        let id = MacroId::from(macro_id);
        let mut macros = HashMap::new();
        macros.insert(
            id.clone(),
            MacroDef {
                name: macro_id.to_string(),
                steps,
            },
        );
        (Action::Macro { macro_id: id }, macros)
    }

    /// A `config_path` no test in this module ever writes to (persistence
    /// via `Command`s is covered separately, with a real `tempfile` path).
    fn unused_config_path() -> PathBuf {
        PathBuf::from("/nonexistent/acheron-dispatch-test/config.toml")
    }

    /// A fresh Actuation-point watch channel for tests that don't care about
    /// its published value — `dispatch::run` requires the `Sender` half
    /// unconditionally (ticket 22), even for tests with no real
    /// `AnalogCaptureSource` grid task on the paired `Receiver`.
    fn actuation_channel() -> watch::Sender<HashMap<Input, ActuationPoint>> {
        watch::channel(HashMap::new()).0
    }

    /// A fresh capture-mode `Receiver` for tests that don't care about the
    /// supervisor pushing real transitions (ticket 23) — the paired `Sender`
    /// is dropped immediately, so `dispatch::run`'s `rx_capture_mode.recv()`
    /// arm just closes on its first poll, same as `commands_open`/
    /// `connection_open` do when their own senders are dropped.
    fn capture_mode_channel() -> mpsc::Receiver<CaptureMode> {
        mpsc::channel(8).1
    }

    /// A fresh device-info `Receiver` for tests that don't exercise the
    /// supervisor's firmware/serial read (ticket 101) — the paired `Sender`
    /// is dropped immediately, so `dispatch::run`'s `rx_device_info.recv()`
    /// arm just closes on its first poll, mirroring `capture_mode_channel`.
    fn device_info_channel() -> mpsc::Receiver<Option<DeviceInfo>> {
        mpsc::channel(8).1
    }

    /// A fresh capture-control `Sender` for tests that don't exercise
    /// `SetForceDigital`'s live supervisor swap (ticket 23) — sends into it
    /// just fail silently once the paired `Receiver` (dropped here) is gone,
    /// matching `dispatch::run`'s own `let _ = capture_control_tx.send(...)`.
    fn capture_control_channel() -> mpsc::Sender<bool> {
        mpsc::channel(8).0
    }

    /// A fresh live-Depth `Receiver` for tests that don't exercise the
    /// continuous Analog axis-resolution path (ticket 71) — the paired
    /// `Sender` is dropped immediately, so `dispatch::run`'s
    /// `rx_depth.changed()` arm just closes on its first poll, mirroring
    /// `capture_mode_channel`.
    fn depth_channel() -> watch::Receiver<HashMap<Input, u8>> {
        watch::channel(HashMap::new()).1
    }

    async fn run_scripted(
        scripted: Vec<PhysicalEvent>,
        bindings: HashMap<Input, Binding>,
    ) -> Vec<Vec<evdev::InputEvent>> {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        let (tx, rx) = mpsc::channel(8);
        let (conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings(bindings),
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_channel(),
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            depth_channel(),
            device_info_channel(),
        ));

        FakeCaptureSource::new(scripted)
            .run(tx, conn_tx)
            .await
            .unwrap();

        drop(inj);
        dispatch_handle.await.unwrap().unwrap();
        inj_handle.await.unwrap().unwrap();

        sink.batches()
    }

    #[tokio::test]
    async fn passthrough_reinjects_every_captured_event_unchanged_when_unbound() {
        // `Input::ModeKey` is deliberately excluded here — under ticket 18's
        // default `LayerSwitch` role it's intercepted before any passthrough
        // decision (see the dedicated `under_layer_switch_...` test below)
        // rather than behaving like a generic unbound Input.
        let scripted = vec![
            PhysicalEvent {
                input: Input::Grid(3, 1),
                state: EventState::Down,
                depth: None,
            },
            PhysicalEvent {
                input: Input::Grid(2, 3),
                state: EventState::Repeat,
                depth: None,
            },
            PhysicalEvent {
                input: Input::Thumbstick(Direction::Up),
                state: EventState::Up,
                depth: None,
            },
            PhysicalEvent {
                input: Input::Wheel(WheelEvent::ScrollDown),
                state: EventState::Down,
                depth: None,
            },
        ];

        let batches = run_scripted(scripted.clone(), HashMap::new()).await;
        assert_eq!(batches.len(), scripted.len());

        // Grid(2,3) -> KEY_W, value 2 (Repeat).
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_W);
        assert_eq!(value, 2);

        // Thumbstick Up -> KEY_UP, value 0 (Up).
        let evdev::EventSummary::Key(_, code, value) = batches[2][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_UP);
        assert_eq!(value, 0);

        // Wheel ScrollDown -> paired REL_WHEEL(-1)/REL_WHEEL_HI_RES(-120).
        assert_eq!(batches[3].len(), 2);
    }

    #[tokio::test]
    async fn bound_input_fires_the_remapped_keypress_instead_of_passthrough() {
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_F1,
                },
            },
        );

        let scripted = vec![PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Down,
            depth: None,
        }];

        let batches = run_scripted(scripted, bindings).await;

        // One press batch + one release batch of KEY_F1 — not the grid
        // key's own passthrough code (KEY_1).
        assert_eq!(batches.len(), 2);
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_F1);
        assert_eq!(value, 1);
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_F1);
        assert_eq!(value, 0);
    }

    #[tokio::test]
    async fn fire_once_binding_ignores_repeat_and_up_fires_only_on_down() {
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_F1,
                },
            },
        );

        let scripted = vec![
            PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Down,
                depth: None,
            },
            PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Repeat,
                depth: None,
            },
            PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Up,
                depth: None,
            },
        ];

        let batches = run_scripted(scripted, bindings).await;

        // Only the Down produced output: one press batch + one release batch.
        assert_eq!(batches.len(), 2);
    }

    #[tokio::test]
    async fn hold_to_repeat_fires_on_down_and_every_repeat_but_not_up() {
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_F1,
                },
            },
        );

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (tx, rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings(bindings),
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_channel(),
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            depth_channel(),
            device_info_channel(),
        ));

        // Real evdev autorepeat events land tens of milliseconds apart —
        // comfortably enough for a same-Input firing's steps to finish. Send
        // each event with yields in between so the previous firing's spawned
        // task runs to completion first, exercising the code review fix
        // (ticket 17): overlapping same-Input firings are dropped, not
        // queued, so back-to-back events with no gap would otherwise only
        // produce the first firing's output.
        for state in [
            EventState::Down,
            EventState::Repeat,
            EventState::Repeat,
            EventState::Up,
        ] {
            tx.send(PhysicalEvent {
                input: Input::Grid(1, 1),
                state,
                depth: None,
            })
            .await
            .unwrap();
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
        }

        drop(tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();

        // Down + two Repeats = three firings, each a KeyDown/KeyUp pair; the
        // trailing Up produced nothing.
        assert_eq!(batches.len(), 6);
        for pair in batches.chunks(2) {
            let evdev::EventSummary::Key(_, down_code, down_value) = pair[0][0].destructure()
            else {
                panic!("expected a key event");
            };
            let evdev::EventSummary::Key(_, up_code, up_value) = pair[1][0].destructure() else {
                panic!("expected a key event");
            };
            assert_eq!((down_code, down_value), (evdev::KeyCode::KEY_F1, 1));
            assert_eq!((up_code, up_value), (evdev::KeyCode::KEY_F1, 0));
        }
    }

    #[tokio::test]
    async fn hold_to_repeats_unbalanced_macro_is_force_released_on_physical_up() {
        // Ticket 33's reproduction, verbatim: a single-step Macro
        // (`KeyDown` with no matching `KeyUp`) under Hold-to-repeat, used to
        // fake a sustained "hold" — pre-fix, this left KEY_LEFTCTRL held at
        // the OS level forever, surviving even a rebind, requiring a reboot.
        let (action, macros) = macro_action(
            "test-macro",
            vec![MacroStepDto::KeyDown(evdev::KeyCode::KEY_LEFTCTRL)],
        );
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action,
            },
        );

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (tx, rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings_and_macros(bindings, macros),
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_channel(),
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            depth_channel(),
            device_info_channel(),
        ));

        tx.send(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Down,
            depth: None,
        })
        .await
        .unwrap();
        // Let the one-step firing (no Delay) finish before the physical
        // release lands — the realistic case, since a physical press/release
        // cycle vastly outlasts an instant single-step Macro.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        tx.send(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Up,
            depth: None,
        })
        .await
        .unwrap();

        drop(tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();

        // The firing's own KeyDown, then a force-released KeyUp triggered by
        // the physical Up — no stuck key, no reboot needed.
        assert_eq!(batches.len(), 2);
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_LEFTCTRL, 1));
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_LEFTCTRL, 0));
    }

    #[tokio::test]
    async fn hold_to_repeat_controller_button_ignores_repeat_and_releases_on_physical_up() {
        // Ticket 75/76: unlike an ordinary Hold-to-repeat Binding (see
        // `hold_to_repeat_fires_on_down_and_every_repeat_but_not_up` above),
        // `Action::ControllerButton` fires exactly one KeyDown on the
        // physical Down, ignores every kernel-autorepeat Repeat outright
        // (no re-fire), and only releases on the physical Up.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::HoldToRepeat,
                    action: Action::ControllerButton {
                        button: evdev::KeyCode::BTN_SOUTH,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        for _ in 0..3 {
            harness.repeat(Input::Grid(1, 1)).await;
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
        }
        harness.release(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.gamepad_batches();
        harness.shut_down().await;

        // Exactly one KeyDown (the physical Down) and one KeyUp (the
        // physical Up) — the three Repeats produced nothing.
        assert_eq!(batches.len(), 2);
        assert_eq!(key_and_value(batches[0][0]), (evdev::KeyCode::BTN_SOUTH, 1));
        assert_eq!(key_and_value(batches[1][0]), (evdev::KeyCode::BTN_SOUTH, 0));
    }

    #[tokio::test]
    async fn hold_to_repeat_chord_controller_button_ignores_repeat_and_releases_on_member_up() {
        // Ticket 75/76's Chord blast radius: the same treatment applies
        // uniformly when a Chord's own Action is `ControllerButton`, mirrors
        // `hold_to_repeat_chord_refires_only_on_the_leader_members_repeat`.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                Binding {
                    trigger: TriggerMode::HoldToRepeat,
                    action: Action::ControllerButton {
                        button: evdev::KeyCode::BTN_SOUTH,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.repeat(Input::Grid(1, 1)).await;
        harness.repeat(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.release(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.gamepad_batches();
        harness.shut_down().await;

        // Exactly one KeyDown (the completing Down) and one KeyUp (the
        // first member's physical Up) — both members' Repeats produced
        // nothing.
        assert_eq!(batches.len(), 2);
        assert_eq!(key_and_value(batches[0][0]), (evdev::KeyCode::BTN_SOUTH, 1));
        assert_eq!(key_and_value(batches[1][0]), (evdev::KeyCode::BTN_SOUTH, 0));
    }

    #[tokio::test]
    async fn hold_to_repeat_chord_mouse_button_ignores_repeat_and_releases_on_member_up() {
        // Ticket 79/80's Chord blast radius, kept at the byte level (post-
        // release ticket 07): `execute_chord_fire`'s mouse-button Keypress
        // Hold-to-repeat arm (`is_mouse_button` → bare `KeyDown`) is a
        // distinct executor branch from the `ControllerButton` one above, so
        // it keeps its own uinput-level test; the *decision* (leader-only
        // re-fire, release on member Up) is covered synchronously in
        // `chord::tests`.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                Binding {
                    trigger: TriggerMode::HoldToRepeat,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: evdev::KeyCode::BTN_LEFT,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.repeat(Input::Grid(1, 1)).await;
        harness.repeat(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.release(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;

        // One KeyDown (the completing Down), one KeyUp (the first member's
        // physical Up) — both members' Repeats produced nothing.
        assert_eq!(batches.len(), 2);
        assert_eq!(key_and_value(batches[0][0]), (evdev::KeyCode::BTN_LEFT, 1));
        assert_eq!(key_and_value(batches[1][0]), (evdev::KeyCode::BTN_LEFT, 0));
    }

    #[tokio::test]
    async fn hold_to_repeat_mouse_button_ignores_repeat_and_releases_on_physical_up() {
        // Ticket 79/80: unlike an ordinary Hold-to-repeat Binding (see
        // `hold_to_repeat_fires_on_down_and_every_repeat_but_not_up` above),
        // a mouse-button `Action::Keypress` (`BTN_LEFT`/etc.) fires exactly
        // one KeyDown on the physical Down, ignores every kernel-autorepeat
        // Repeat outright (no re-fire), and only releases on the physical
        // Up — the same sustained-hold treatment ticket 75/76 gave
        // `ControllerButton`, now supporting click-and-drag.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::HoldToRepeat,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: evdev::KeyCode::BTN_LEFT,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        for _ in 0..3 {
            harness.repeat(Input::Grid(1, 1)).await;
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
        }
        harness.release(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;

        // Exactly one KeyDown (the physical Down) and one KeyUp (the
        // physical Up) — the three Repeats produced nothing.
        assert_eq!(batches.len(), 2);
        assert_eq!(key_and_value(batches[0][0]), (evdev::KeyCode::BTN_LEFT, 1));
        assert_eq!(key_and_value(batches[1][0]), (evdev::KeyCode::BTN_LEFT, 0));
    }

    #[tokio::test]
    async fn hold_to_repeat_keyboard_key_still_refires_on_every_repeat() {
        // Regression coverage (ticket 79/80): the mouse-button-only
        // carve-out must not bleed onto keyboard-key output — `is_mouse_
        // button` rejects an ordinary keyboard `KeyCode`, so the ordinary
        // Hold-to-repeat arm still applies. Mirrors ticket 76's own
        // `hold_to_repeat_mouse_button_still_refires_on_every_repeat`
        // negative test, but in the other direction.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::HoldToRepeat,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: evdev::KeyCode::KEY_A,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.repeat(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.release(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;

        // Down + one Repeat = two firings, each a KeyDown/KeyUp pair; the
        // trailing Up produced nothing — unchanged from before ticket 79/80.
        assert_eq!(batches.len(), 4);
        for pair in batches.chunks(2) {
            assert_eq!(key_and_value(pair[0][0]), (evdev::KeyCode::KEY_A, 1));
            assert_eq!(key_and_value(pair[1][0]), (evdev::KeyCode::KEY_A, 0));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_mouse_button_holds_a_single_keydown_and_the_same_key_stops_it() {
        // Ticket 82/83: a mouse-button Keypress under Toggle gets a real
        // sustained hold instead of the ordinary repeat-tap loop — one
        // KeyDown while toggled on, no matter how long, released by exactly
        // one KeyUp when the same key stops it.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::Toggle,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: evdev::KeyCode::BTN_LEFT,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        // Advance well past several ordinary Toggle laps' worth of time —
        // a looping Toggle would have re-pressed several times by now.
        for _ in 0..7 {
            tokio::time::advance(executor::MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }

        let state = harness.get_state().await;
        assert_eq!(state.active_toggles, vec![Input::Grid(1, 1)]);

        // Same physical key, still toggled on: stops it rather than
        // starting a second one.
        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;

        assert_eq!(
            batches.len(),
            2,
            "exactly one KeyDown, one KeyUp — no re-fires in between"
        );
        assert_eq!(key_and_value(batches[0][0]), (evdev::KeyCode::BTN_LEFT, 1));
        assert_eq!(key_and_value(batches[1][0]), (evdev::KeyCode::BTN_LEFT, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_controller_button_holds_a_single_keydown_and_the_same_key_stops_it() {
        // Ticket 78: a gamepad button under Toggle gets the same
        // sustained-hold treatment as a mouse-button Toggle (ticket 82/83)
        // above, and as ControllerButton's own Hold-to-repeat carve-out
        // (ticket 75/76) — one KeyDown while toggled on, no matter how long,
        // released by exactly one KeyUp when the same key stops it. Before
        // this ticket, this fell through to the ordinary looping Toggle arm
        // instead.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::Toggle,
                    action: Action::ControllerButton {
                        button: evdev::KeyCode::BTN_SOUTH,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        // Advance well past several ordinary Toggle laps' worth of time —
        // a looping Toggle would have re-pressed several times by now.
        for _ in 0..7 {
            tokio::time::advance(executor::MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }

        let state = harness.get_state().await;
        assert_eq!(state.active_toggles, vec![Input::Grid(1, 1)]);

        // Same physical key, still toggled on: stops it rather than
        // starting a second one.
        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.gamepad_batches();
        harness.shut_down().await;

        assert_eq!(
            batches.len(),
            2,
            "exactly one KeyDown, one KeyUp — no re-fires in between"
        );
        assert_eq!(key_and_value(batches[0][0]), (evdev::KeyCode::BTN_SOUTH, 1));
        assert_eq!(key_and_value(batches[1][0]), (evdev::KeyCode::BTN_SOUTH, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_keyboard_key_still_loops_at_dispatch_level() {
        // Regression coverage (ticket 82/83): the mouse-button-only
        // carve-out must not bleed onto keyboard-key output — `is_mouse_
        // button` rejects an ordinary keyboard `KeyCode`, so the ordinary
        // looping Toggle arm still applies.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::Toggle,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: evdev::KeyCode::KEY_A,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        for _ in 0..7 {
            tokio::time::advance(executor::MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }

        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;

        assert!(
            batches.len() > 2,
            "a keyboard-key Toggle must still loop (mash), unlike the mouse-button held variant: got {batches:?}"
        );
    }

    #[tokio::test]
    async fn analog_repeat_digital_sourced_behaves_like_hold_to_repeat() {
        // Ticket 20's Digital Capture mode fallback: with no Depth at all
        // (`event.depth: None`, exactly mirroring `hold_to_repeat_fires_on_
        // down_and_every_repeat_but_not_up` above), Analog-repeat fires on
        // Down/Repeat and force-releases on Up, identically to Hold-to-repeat.
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::AnalogRepeat,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_F1,
                },
            },
        );

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (tx, rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings(bindings),
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_channel(),
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            depth_channel(),
            device_info_channel(),
        ));

        for state in [
            EventState::Down,
            EventState::Repeat,
            EventState::Repeat,
            EventState::Up,
        ] {
            tx.send(PhysicalEvent {
                input: Input::Grid(1, 1),
                state,
                depth: None,
            })
            .await
            .unwrap();
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
        }

        drop(tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(batches.len(), 6);
        for pair in batches.chunks(2) {
            let evdev::EventSummary::Key(_, down_code, down_value) = pair[0][0].destructure()
            else {
                panic!("expected a key event");
            };
            let evdev::EventSummary::Key(_, up_code, up_value) = pair[1][0].destructure() else {
                panic!("expected a key event");
            };
            assert_eq!((down_code, down_value), (evdev::KeyCode::KEY_F1, 1));
            assert_eq!((up_code, up_value), (evdev::KeyCode::KEY_F1, 0));
        }
    }

    #[tokio::test]
    async fn analog_repeat_analog_sourced_events_are_swallowed() {
        // The opposite case from the test above: an Analog-*sourced* Down/
        // Repeat/Up (`event.depth: Some(_)`, synthesized from the key's
        // ordinary Actuation/Release points) must never reach `fire()` at
        // all for an Analog-repeat Binding — real firing is
        // `update_analog_repeats`'s own depth-driven background task,
        // exercised separately below. No depth-watch crossing is ever
        // published here (`depth_channel()`'s Sender is dropped
        // immediately), so if this Binding fell through to `fire()` instead
        // of being swallowed, it would produce ordinary Hold-to-repeat
        // output — this asserts zero output instead.
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::AnalogRepeat,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_F1,
                },
            },
        );

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (tx, rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings(bindings),
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_channel(),
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            depth_channel(),
            device_info_channel(),
        ));

        for state in [EventState::Down, EventState::Repeat, EventState::Up] {
            tx.send(PhysicalEvent {
                input: Input::Grid(1, 1),
                state,
                depth: Some(200),
            })
            .await
            .unwrap();
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
        }

        drop(tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        assert!(sink.batches().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn analog_repeat_task_fires_periodically_above_the_deadzone_and_stops_below_it() {
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::AnalogRepeat,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_F1,
                },
            },
        );

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (tx, rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let (depth_tx, depth_rx) = watch::channel(HashMap::new());
        let dispatch_handle = tokio::spawn(run(
            rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings(bindings),
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_channel(),
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            depth_rx,
            device_info_channel(),
        ));

        // A mid-travel Depth, comfortably between the deadzone and the
        // hold-solid threshold — the rising edge spawns the task.
        let depth: u8 = 100;
        let rate_hz = ANALOG_REPEAT_MIN_HZ
            + (ANALOG_REPEAT_MAX_HZ - ANALOG_REPEAT_MIN_HZ) * (f64::from(depth) / 255.0);
        let period = Duration::from_secs_f64(1.0 / rate_hz);
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), depth)]));
        tokio::task::yield_now().await;

        // Two full ticks: each is a KeyDown, a PULSE_HOLD sleep, a KeyUp,
        // then the rest of the tick's own period.
        for _ in 0..2 {
            tokio::time::advance(ANALOG_REPEAT_PULSE_HOLD).await;
            tokio::task::yield_now().await;
            tokio::time::advance(period - ANALOG_REPEAT_PULSE_HOLD).await;
            tokio::task::yield_now().await;
        }

        // Falling back below the deadzone stops the task — a no-op
        // force-release here, since every pulse above already self-released.
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 0u8)]));
        tokio::task::yield_now().await;
        let after_stop = sink.batches().len();

        // Advancing well past another tick's worth of time produces
        // nothing further — the task is genuinely gone, not just paused
        // between ticks.
        tokio::time::advance(period * 3).await;
        tokio::task::yield_now().await;
        assert_eq!(sink.batches().len(), after_stop);

        drop(tx);
        drop(depth_tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(batches.len(), 4);
        for pair in batches.chunks(2) {
            let evdev::EventSummary::Key(_, down_code, down_value) = pair[0][0].destructure()
            else {
                panic!("expected a key event");
            };
            let evdev::EventSummary::Key(_, up_code, up_value) = pair[1][0].destructure() else {
                panic!("expected a key event");
            };
            assert_eq!((down_code, down_value), (evdev::KeyCode::KEY_F1, 1));
            assert_eq!((up_code, up_value), (evdev::KeyCode::KEY_F1, 0));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn analog_repeat_controller_button_uses_the_controller_pulse_hold_floor() {
        // Ticket 78: Analog-repeat on a Binding whose Action is
        // `ControllerButton` holds each pulse for `ANALOG_REPEAT_CONTROLLER_
        // PULSE_HOLD` (35ms), not the ordinary `ANALOG_REPEAT_PULSE_HOLD`
        // (15ms) every other output Action uses.
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::AnalogRepeat,
                action: Action::ControllerButton {
                    button: evdev::KeyCode::BTN_SOUTH,
                },
            },
        );

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (tx, rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let (depth_tx, depth_rx) = watch::channel(HashMap::new());
        let dispatch_handle = tokio::spawn(run(
            rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings(bindings),
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_channel(),
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            depth_rx,
            device_info_channel(),
        ));

        let depth: u8 = 100;
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), depth)]));
        tokio::task::yield_now().await;

        assert_eq!(sink.batches().len(), 1, "the Down must fire immediately");

        tokio::time::advance(ANALOG_REPEAT_PULSE_HOLD).await;
        tokio::task::yield_now().await;
        assert_eq!(
            sink.batches().len(),
            1,
            "the ordinary 15ms dwell must not release a ControllerButton pulse"
        );

        tokio::time::advance(ANALOG_REPEAT_CONTROLLER_PULSE_HOLD - ANALOG_REPEAT_PULSE_HOLD).await;
        tokio::task::yield_now().await;

        // Fall back below the deadzone to let `update_analog_repeats` stop
        // the task (dropping its own `Injector` clone) before shutdown —
        // otherwise the still-running task's clone keeps the injector's own
        // channel open forever, hanging `inj_handle.await` below (mirrors
        // `analog_repeat_task_fires_periodically_above_the_deadzone_and_
        // stops_below_it`'s own shutdown sequence).
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 0u8)]));
        tokio::task::yield_now().await;

        drop(tx);
        drop(depth_tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(
            batches.len(),
            2,
            "the Up must fire once the 35ms controller floor elapses"
        );
        assert_eq!(key_and_value(batches[0][0]), (evdev::KeyCode::BTN_SOUTH, 1));
        assert_eq!(key_and_value(batches[1][0]), (evdev::KeyCode::BTN_SOUTH, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn analog_repeat_holds_solid_above_the_hold_threshold() {
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::AnalogRepeat,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_F1,
                },
            },
        );

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (tx, rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let (depth_tx, depth_rx) = watch::channel(HashMap::new());
        let dispatch_handle = tokio::spawn(run(
            rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings(bindings),
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_channel(),
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            depth_rx,
            device_info_channel(),
        ));

        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), u8::MAX)]));
        tokio::task::yield_now().await;
        // Well past several ordinary ticks' worth of time — still holding
        // solid the whole way through, not tapping.
        tokio::time::advance(Duration::from_millis(500)).await;
        tokio::task::yield_now().await;

        let batches = sink.batches();
        assert_eq!(batches.len(), 1, "expected exactly one KeyDown, no taps");
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_F1, 1));

        // Falling back below the deadzone force-releases the held key.
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 0u8)]));
        tokio::task::yield_now().await;

        drop(tx);
        drop(depth_tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(batches.len(), 2);
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_F1, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn overlapping_same_input_firings_are_dropped_not_queued() {
        let (action, macros) = macro_action(
            "test-macro",
            vec![
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                MacroStepDto::Delay(20),
                MacroStepDto::KeyUp(evdev::KeyCode::KEY_A),
            ],
        );
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action,
            },
        );

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (tx, rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings_and_macros(bindings, macros),
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_channel(),
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            depth_channel(),
            device_info_channel(),
        ));

        // Down starts a firing that immediately sends KeyDown, then sleeps
        // 20ms. A Repeat that lands before that firing finishes must be
        // dropped, not spawn a second overlapping firing.
        tx.send(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Down,
            depth: None,
        })
        .await
        .unwrap();
        tokio::task::yield_now().await;
        tx.send(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Repeat,
            depth: None,
        })
        .await
        .unwrap();
        tokio::task::yield_now().await;

        // Let the first firing's Delay elapse and its KeyUp land.
        tokio::time::advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;

        // A later Repeat, after the first firing has fully finished, starts
        // a genuinely new firing.
        tx.send(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Repeat,
            depth: None,
        })
        .await
        .unwrap();
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(20)).await;
        tokio::task::yield_now().await;

        drop(tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();

        // Two firings' worth of output (KeyDown/KeyUp pairs), not three —
        // the overlapping Repeat produced nothing.
        assert_eq!(batches.len(), 4);
        for pair in batches.chunks(2) {
            let evdev::EventSummary::Key(_, down_code, down_value) = pair[0][0].destructure()
            else {
                panic!("expected a key event");
            };
            let evdev::EventSummary::Key(_, up_code, up_value) = pair[1][0].destructure() else {
                panic!("expected a key event");
            };
            assert_eq!((down_code, down_value), (evdev::KeyCode::KEY_A, 1));
            assert_eq!((up_code, up_value), (evdev::KeyCode::KEY_A, 0));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fire_once_macro_action_runs_its_delayed_steps_in_order() {
        let (action, macros) = macro_action(
            "test-macro",
            vec![
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                MacroStepDto::Delay(20),
                MacroStepDto::KeyUp(evdev::KeyCode::KEY_A),
            ],
        );
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::FireOnce,
                action,
            },
        );
        let harness = CommandHarness::spawn(config_with_bindings_and_macros(bindings, macros));

        harness.press(Input::Grid(1, 1)).await;
        tokio::time::advance(Duration::from_millis(25)).await;
        tokio::task::yield_now().await;

        let batches = harness.shut_down().await;

        assert_eq!(batches.len(), 2);
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_A, 1));
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_A, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_starts_on_down_and_the_same_key_stops_it_on_the_next_down() {
        // Deliberately unbalanced within the window we stop in: KeyDown
        // fires, then a long Delay, so KEY_A is still held when the second
        // press stops the Toggle.
        let (action, macros) = macro_action(
            "test-macro",
            vec![
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                MacroStepDto::Delay(50),
            ],
        );
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action,
            },
        );
        let harness = CommandHarness::spawn(config_with_bindings_and_macros(bindings, macros));

        harness.press(Input::Grid(1, 1)).await;
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;

        let state = harness.get_state().await;
        assert_eq!(state.active_toggles, vec![Input::Grid(1, 1)]);

        // Same physical key, still Down: stops the Toggle instead of
        // starting a second one — this press is consumed entirely by the
        // stop, no re-fire. `shut_down` closes the event channel right
        // after this send and awaits dispatch to drain it, which only
        // happens once this stop (including its force-release) has fully
        // run, so there's nothing racy left to synchronize on here.
        harness.press(Input::Grid(1, 1)).await;

        let batches = harness.shut_down().await;

        // One KeyDown from the loop's single lap, then a force-released
        // KeyUp for exactly that key on stop — no stuck key, no extra output.
        assert_eq!(batches.len(), 2);
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_A, 1));
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_A, 0));
    }

    /// Harness for the `Command` tests below: a real `tempfile` config path
    /// (so `SetBinding`/`ClearBinding` persistence is genuinely exercised),
    /// live handles to send `Command`s and read back injected batches, and a
    /// clean shutdown via closing both channels.
    struct CommandHarness {
        _dir: tempfile::TempDir,
        config_path: PathBuf,
        cmd_tx: mpsc::Sender<Command>,
        event_tx: mpsc::Sender<PhysicalEvent>,
        conn_tx: mpsc::Sender<bool>,
        actuation_rx: watch::Receiver<HashMap<Input, ActuationPoint>>,
        depth_tx: watch::Sender<HashMap<Input, u8>>,
        device_info_tx: mpsc::Sender<Option<DeviceInfo>>,
        sink: RecordingSink,
        gamepad_sink: RecordingSink,
        dispatch_handle: tokio::task::JoinHandle<io::Result<()>>,
        inj_handle: tokio::task::JoinHandle<io::Result<()>>,
    }

    impl CommandHarness {
        fn spawn(config: Config) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let config_path = dir.path().join("config.toml");
            config::write(&config_path, &config).unwrap();
            Self::spawn_at(config, dir, config_path)
        }

        /// Spawns the dispatch task pointed at an unwritable `config_path`
        /// (`/nonexistent/...`) so every `config::persist_edit` call fails at
        /// the write — the seam for exercising a persist-failure rollback
        /// through the full dispatch harness (ticket 03). The in-memory
        /// `Config` still starts correct; only the disk write is broken.
        fn spawn_with_failing_persist(config: Config) -> Self {
            let dir = tempfile::tempdir().unwrap();
            Self::spawn_at(config, dir, unused_config_path())
        }

        fn spawn_at(config: Config, dir: tempfile::TempDir, config_path: PathBuf) -> Self {
            let sink = RecordingSink::new();
            let gamepad_sink = RecordingSink::new();
            let (inj, inj_handle) = injector::spawn(sink.clone(), gamepad_sink.clone());
            let (event_tx, event_rx) = mpsc::channel(8);
            let (conn_tx, conn_rx) = mpsc::channel(8);
            let (cmd_tx, cmd_rx) = mpsc::channel(8);
            let (actuation_tx, actuation_rx) = watch::channel(HashMap::new());
            let (depth_tx, depth_rx) = watch::channel(HashMap::new());
            let (device_info_tx, device_info_rx) = mpsc::channel(8);
            let dispatch_handle = tokio::spawn(run(
                event_rx,
                conn_rx,
                cmd_rx,
                inj,
                config,
                config_path.clone(),
                None,
                actuation_tx,
                capture_mode_channel(),
                capture_control_channel(),
                executor::MIN_TOGGLE_LAP,
                depth_rx,
                device_info_rx,
            ));

            CommandHarness {
                _dir: dir,
                config_path,
                cmd_tx,
                event_tx,
                actuation_rx,
                depth_tx,
                device_info_tx,
                conn_tx,
                sink,
                gamepad_sink,
                dispatch_handle,
                inj_handle,
            }
        }

        async fn set_binding(
            &self,
            input: Input,
            layer: Layer,
            binding: Binding,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SetBinding {
                    input,
                    layer,
                    binding,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn clear_binding(&self, input: Input, layer: Layer) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::ClearBinding {
                    input,
                    layer,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn set_chord_binding(
            &self,
            inputs: impl IntoIterator<Item = Input>,
            layer: Layer,
            binding: Binding,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SetChordBinding {
                    inputs: inputs.into_iter().collect(),
                    layer,
                    binding,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn set_mode_key_role(&self, role: ModeKeyRole) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SetModeKeyRole { role, reply })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn create_stepper(
            &self,
            name: &str,
            items: Vec<crate::config::StepperItem>,
        ) -> Result<StepperId, CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::CreateStepper {
                    name: name.to_string(),
                    items,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn delete_stepper(&self, stepper_id: StepperId) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::DeleteStepper { stepper_id, reply })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn set_stepper_items(
            &self,
            stepper_id: StepperId,
            items: Vec<crate::config::StepperItem>,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SetStepperItems {
                    stepper_id,
                    items,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn switch_profile(&self, name: &str) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SwitchProfile {
                    name: name.to_string(),
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn stop_all_toggles(&self) {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::StopAllToggles { reply })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn set_actuation_point(
            &self,
            input: Input,
            actuation: u8,
            release: u8,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SetActuationPoint {
                    input,
                    actuation,
                    release,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn clear_actuation_point(&self, input: Input) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::ClearActuationPoint { input, reply })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn set_default_actuation(
            &self,
            actuation: u8,
            release: u8,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SetDefaultActuation {
                    actuation,
                    release,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn reset_actuation_points(&self) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::ResetActuationPoints { reply })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn set_force_digital(&self, force: bool) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SetForceDigital { force, reply })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn get_config(&self) -> Config {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx.send(Command::GetConfig(reply)).await.unwrap();
            rx.await.unwrap()
        }

        async fn get_state(&self) -> State {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx.send(Command::GetState(reply)).await.unwrap();
            rx.await.unwrap()
        }

        /// The latest resolved Actuation-point snapshot dispatch has
        /// published (ticket 18 §5) — the seam an `AnalogCaptureSource` grid
        /// task's `watch::Receiver` would read `.borrow()` from.
        fn actuation_snapshot(&self) -> HashMap<Input, ActuationPoint> {
            self.actuation_rx.borrow().clone()
        }

        async fn press(&self, input: Input) {
            self.event_tx
                .send(PhysicalEvent {
                    input,
                    state: EventState::Down,
                    depth: None,
                })
                .await
                .unwrap();
        }

        async fn release(&self, input: Input) {
            self.event_tx
                .send(PhysicalEvent {
                    input,
                    state: EventState::Up,
                    depth: None,
                })
                .await
                .unwrap();
        }

        async fn repeat(&self, input: Input) {
            self.event_tx
                .send(PhysicalEvent {
                    input,
                    state: EventState::Repeat,
                    depth: None,
                })
                .await
                .unwrap();
        }

        /// An Analog-sourced transition (`depth: Some(_)`) — used to exercise
        /// `handle_event`'s "swallow rather than passthrough" branch for an
        /// Axis-assigned Input (ticket 71), distinct from `press`/`release`/
        /// `repeat`'s Digital-sourced (`depth: None`) shape.
        async fn press_analog(&self, input: Input, depth: u8) {
            self.event_tx
                .send(PhysicalEvent {
                    input,
                    state: EventState::Down,
                    depth: Some(depth),
                })
                .await
                .unwrap();
        }

        /// Publishes a fresh live-Depth snapshot (ticket 26/71) — the same
        /// seam `capture::analog`'s grid task drives via `depth_tx.
        /// send_replace(...)` on every incoming report; `dispatch::run`'s
        /// continuous axis-resolution path (`handle_depth_update`) reacts to
        /// this exactly as it would the real channel.
        fn push_depth(&self, values: impl IntoIterator<Item = (Input, u8)>) {
            self.depth_tx.send_replace(values.into_iter().collect());
        }

        async fn set_axis_assignment(
            &self,
            input: Input,
            layer: Layer,
            target: AxisTarget,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SetAxisAssignment {
                    input,
                    layer,
                    target,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn clear_axis_assignment(
            &self,
            input: Input,
            layer: Layer,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::ClearAxisAssignment {
                    input,
                    layer,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        /// The gamepad device's own recorded batches (ticket 71) — every
        /// `Action::ControllerButton`/Axis write lands here, never in
        /// `self.sink` (the keyboard/mouse device), mirroring `injector.rs`'s
        /// own two-sink routing split.
        fn gamepad_batches(&self) -> Vec<Vec<evdev::InputEvent>> {
            self.gamepad_sink.batches()
        }

        /// Stands in for the `CaptureSource`'s poll loop reporting a
        /// device-connection transition (ticket 20) — there's no real
        /// evdev poll loop in these tests, so this is the seam that drives
        /// `device_connected`/`DeviceConnectionChanged`.
        async fn set_device_connected(&self, connected: bool) {
            self.conn_tx.send(connected).await.unwrap();
        }

        /// Stands in for `capture::supervisor` pushing a firmware/serial
        /// read result (ticket 101) — `Some` after a successful read on
        /// connect, `None` on disconnect.
        async fn set_device_info(&self, info: Option<DeviceInfo>) {
            self.device_info_tx.send(info).await.unwrap();
        }

        async fn shut_down(self) -> Vec<Vec<evdev::InputEvent>> {
            drop(self.cmd_tx);
            drop(self.event_tx);
            drop(self.conn_tx);
            self.dispatch_handle.await.unwrap().unwrap();
            self.inj_handle.await.unwrap().unwrap();
            self.sink.batches()
        }
    }

    fn keypress_binding(key: evdev::KeyCode) -> Binding {
        Binding {
            trigger: TriggerMode::FireOnce,
            action: Action::Keypress {
                modifiers: Modifiers::default(),
                key,
            },
        }
    }

    #[tokio::test]
    async fn set_binding_persist_failure_rolls_back_the_cross_profile_stepper_steal() {
        // A `Step` Binding on the active Profile that steals a (stepper,
        // direction) from a *different* Profile, then fails to persist: the
        // whole edit — the steal *and* the target insert — must roll back, so
        // the donor Profile keeps its Binding and the active Profile's target
        // Layer stays empty. No persist-failure rollback had dispatch-harness
        // coverage before ticket 03's `config::persist_edit`; this locks in
        // the cross-Profile case the old hand-rolled `SetBinding` block
        // reversed by replaying a `Vec` of moved Bindings.
        let stepper_id = StepperId::from("wheel");
        let step_forward = Binding {
            trigger: TriggerMode::FireOnce,
            action: Action::Step {
                stepper: stepper_id.clone(),
                direction: StepDirection::Forward,
            },
        };

        let mut donor = Profile::default();
        donor.base.insert(Input::Grid(5, 5), step_forward.clone());
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), Profile::default());
        profiles.insert("Alt".to_string(), donor);

        let mut steppers = HashMap::new();
        steppers.insert(
            stepper_id.clone(),
            StepperDef {
                name: "Wheel".to_string(),
                items: vec![StepperItem::Key {
                    key: evdev::KeyCode::KEY_1,
                    modifiers: Modifiers::default(),
                }],
            },
        );

        let harness = CommandHarness::spawn_with_failing_persist(Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros: HashMap::new(),
            steppers,
        });

        let result = harness
            .set_binding(Input::Grid(1, 1), Layer::Base, step_forward)
            .await;
        assert!(matches!(result, Err(CommandError::IoError(_))));

        let config = harness.get_config().await;
        assert!(
            !config.profiles[DEFAULT_PROFILE_NAME]
                .base
                .contains_key(&Input::Grid(1, 1)),
            "the failed target insert must have rolled back"
        );
        assert_eq!(
            config.profiles["Alt"]
                .base
                .get(&Input::Grid(5, 5))
                .map(|binding| &binding.action),
            Some(&Action::Step {
                stepper: stepper_id,
                direction: StepDirection::Forward,
            }),
            "the donor Profile's Binding must have been restored on rollback"
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn an_invariant_violating_edit_is_rejected_and_the_in_memory_config_is_rolled_back() {
        // Ticket 04: the single "reject + roll back" integration test for the
        // dispatch path. `config::validate` now runs inside `persist_edit`
        // after the edit closure — a closure that mutates `Config` into a
        // structurally invalid state (here: a Chord whose member set is a
        // superset of an existing Chord's, *and* which steals a (stepper,
        // direction) from another Binding on the way in) is rejected, and the
        // whole edit — the steal included — is rolled back in memory. The
        // per-invariant "which error" coverage lives in `config::validate`'s
        // own synchronous test module; this is the one test that exercises
        // the rejection through the full dispatch harness.
        let stepper_id = StepperId::from("wheel");
        let step_forward = Binding {
            trigger: TriggerMode::FireOnce,
            action: Action::Step {
                stepper: stepper_id.clone(),
                direction: StepDirection::Forward,
            },
        };
        let mut profile = Profile::default();
        profile.base.insert(Input::Grid(5, 5), step_forward.clone());
        profile.chords_base.insert(
            ChordKey::new(BTreeSet::from([Input::Grid(1, 1), Input::Grid(1, 2)])),
            keypress_binding(evdev::KeyCode::KEY_1),
        );
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), profile);
        let mut steppers = HashMap::new();
        steppers.insert(
            stepper_id.clone(),
            StepperDef {
                name: "Wheel".to_string(),
                items: vec![StepperItem::Key {
                    key: evdev::KeyCode::KEY_1,
                    modifiers: Modifiers::default(),
                }],
            },
        );
        let harness = CommandHarness::spawn(Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros: HashMap::new(),
            steppers,
        });

        let result = harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2), Input::Grid(1, 3)],
                Layer::Base,
                step_forward,
            )
            .await;
        assert!(matches!(result, Err(CommandError::InvalidRequest(_))));

        let config = harness.get_config().await;
        harness.shut_down().await;
        let base = &config.profiles[DEFAULT_PROFILE_NAME];
        assert!(
            !base
                .chords_base
                .contains_key(&ChordKey::new(BTreeSet::from([
                    Input::Grid(1, 1),
                    Input::Grid(1, 2),
                    Input::Grid(1, 3),
                ]))),
            "the rejected superset Chord must not have been inserted"
        );
        assert!(
            base.chords_base
                .contains_key(&ChordKey::new(BTreeSet::from([
                    Input::Grid(1, 1),
                    Input::Grid(1, 2),
                ]))),
            "the pre-existing Chord must be untouched"
        );
        assert_eq!(
            base.base.get(&Input::Grid(5, 5)).map(|b| &b.action),
            Some(&Action::Step {
                stepper: stepper_id,
                direction: StepDirection::Forward,
            }),
            "the (stepper, direction) steal must have rolled back with the rejected insert"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_chord_mouse_button_holds_a_single_keydown_and_full_completion_stops_it() {
        // Ticket 82/83's Chord blast radius: the same sustained-hold
        // treatment applies when a Chord's own Action is a mouse-button
        // Keypress under Toggle, mirroring a single Input's own Toggle
        // above.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                Binding {
                    trigger: TriggerMode::Toggle,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: evdev::KeyCode::BTN_LEFT,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        for _ in 0..7 {
            tokio::time::advance(executor::MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }

        // A fresh completion of the full member set stops it, mirroring a
        // single Input's own Toggle.
        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;

        assert_eq!(batches.len(), 2, "no re-fires between the two completions");
        assert_eq!(key_and_value(batches[0][0]), (evdev::KeyCode::BTN_LEFT, 1));
        assert_eq!(key_and_value(batches[1][0]), (evdev::KeyCode::BTN_LEFT, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_chord_controller_button_holds_a_single_keydown_and_full_completion_stops_it() {
        // Ticket 78's Chord blast radius: the same sustained-hold treatment
        // applies when a Chord's own Action is `ControllerButton` under
        // Toggle, mirroring a single Input's own ControllerButton Toggle
        // above and the mouse-button Chord Toggle carve-out above it.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                Binding {
                    trigger: TriggerMode::Toggle,
                    action: Action::ControllerButton {
                        button: evdev::KeyCode::BTN_SOUTH,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        for _ in 0..7 {
            tokio::time::advance(executor::MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }

        // A fresh completion of the full member set stops it, mirroring a
        // single Input's own Toggle.
        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.gamepad_batches();
        harness.shut_down().await;

        assert_eq!(batches.len(), 2, "no re-fires between the two completions");
        assert_eq!(key_and_value(batches[0][0]), (evdev::KeyCode::BTN_SOUTH, 1));
        assert_eq!(key_and_value(batches[1][0]), (evdev::KeyCode::BTN_SOUTH, 0));
    }

    #[tokio::test]
    async fn a_chord_member_whose_individual_binding_is_a_profile_switch_switches_on_early_release()
    {
        // The one full `feed → FireIndividual → ProfileSwitch` input-path
        // commit (post-release ticket 07): the pure Chord machine only ever
        // emits `FireIndividual`, and the dispatch executor resolves it
        // through the same `dispatch_individual_down` the ordinary Down path
        // uses — so a Chord member whose *own* individual Binding is
        // `Action::ProfileSwitch` still produces an `Edit::SwitchProfile` the
        // `run` loop commits (a Chord's *own* Action can never be a switch,
        // but a member's individual one can be anything).
        let mut base = HashMap::new();
        base.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::ProfileSwitch {
                    target: "Gaming".to_string(),
                },
            },
        );
        let mut profile = Profile {
            base,
            ..Default::default()
        };
        profile.chords_base.insert(
            ChordKey::new(BTreeSet::from([Input::Grid(1, 1), Input::Grid(1, 2)])),
            keypress_binding(evdev::KeyCode::KEY_C),
        );
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), profile);
        profiles.insert("Gaming".to_string(), Profile::default());
        let harness = CommandHarness::spawn(Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros: HashMap::new(),
            steppers: HashMap::new(),
        });

        // Press one member (opens the window), then release it before the
        // rest of the Chord joins — the pending member resolves right now.
        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.release(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let state = harness.get_state().await;
        harness.shut_down().await;
        assert_eq!(
            state.profile, "Gaming",
            "the member's individual ProfileSwitch Binding fired retroactively and committed"
        );
    }

    #[tokio::test]
    async fn set_binding_command_applies_live_and_persists_to_disk() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                keypress_binding(evdev::KeyCode::KEY_F1),
            )
            .await
            .expect("SetBinding must succeed");

        // Live: a Down on the now-bound Input fires the new Keypress.
        harness.press(Input::Grid(1, 1)).await;

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        let batches = harness.shut_down().await;

        assert_eq!(batches.len(), 2, "one press batch + one release batch");
        let evdev::EventSummary::Key(_, code, _) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_F1);

        // On disk: config.toml reflects the new binding immediately, no
        // separate save step.
        let reparsed: Config = toml::from_str(&on_disk).unwrap();
        let binding = &reparsed.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)];
        assert_eq!(
            binding.action,
            Action::Keypress {
                modifiers: Modifiers::default(),
                key: evdev::KeyCode::KEY_F1,
            }
        );
    }

    #[tokio::test]
    async fn get_config_command_returns_the_live_in_memory_config() {
        let mut bindings = HashMap::new();
        bindings.insert(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_F1));
        let expected = config_with_bindings(bindings);
        let harness = CommandHarness::spawn(expected.clone());

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert_eq!(config, expected);
    }

    #[tokio::test]
    async fn get_state_command_returns_live_values() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let state = harness.get_state().await;
        harness.shut_down().await;

        // `active_toggles` is real as of ticket 17; with no Toggle running
        // it's correctly empty here. `layer` is real as of ticket 18 — with
        // no ModeKey press this task's dispatch loop starts and stays at
        // Base. `device_connected` starts optimistic (ticket 20 — no
        // connection transition has been reported yet in this test).
        // `capture_mode` is hardcoded to "digital" as of ticket 21 — there
        // is no real analog CaptureSource yet to report on.
        assert_eq!(state.profile, DEFAULT_PROFILE_NAME);
        assert_eq!(state.layer, "base");
        assert!(state.active_toggles.is_empty());
        assert!(state.device_connected);
        assert_eq!(state.capture_mode, "digital");
    }

    #[tokio::test]
    async fn get_state_reflects_a_reported_device_disconnection() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness.set_device_connected(false).await;
        // `PhysicalEvent`s/`Command`s/connection transitions arrive on
        // separate channels the dispatch task `select!`s over with no
        // ordering guarantee between them — same caveat the Layer tests
        // above document, same fix.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert!(!harness.get_state().await.device_connected);

        harness.set_device_connected(true).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert!(harness.get_state().await.device_connected);

        harness.shut_down().await;
    }

    /// Ticket 101: a firmware/serial read result pushed by the supervisor
    /// shows up in `GetState()`; a subsequent disconnect (`None`) clears it
    /// so the About dialog's keys go absent again.
    #[tokio::test]
    async fn get_state_reflects_a_reported_device_info_read_and_its_clearing() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let state = harness.get_state().await;
        assert_eq!(state.firmware_version, None);
        assert_eq!(state.serial_number, None);

        harness
            .set_device_info(Some(DeviceInfo {
                firmware_version: "v1.2".to_string(),
                serial_number: "PM2443F36300141".to_string(),
            }))
            .await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        let state = harness.get_state().await;
        assert_eq!(state.firmware_version.as_deref(), Some("v1.2"));
        assert_eq!(state.serial_number.as_deref(), Some("PM2443F36300141"));

        harness.set_device_info(None).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        let state = harness.get_state().await;
        assert_eq!(state.firmware_version, None);
        assert_eq!(state.serial_number, None);

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn redundant_connection_reports_are_idempotent() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        // The `CaptureSource` seam can report the same combined value
        // redundantly (e.g. two of three nodes independently reconfirming
        // "still connected") — `handle_connection_change` must treat this
        // as a no-op, not error or misbehave.
        harness.set_device_connected(true).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert!(harness.get_state().await.device_connected);
        harness.shut_down().await;
    }

    fn profile_with_held_bindings(bindings: HashMap<Input, Binding>) -> Profile {
        Profile {
            held: bindings,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn held_layer_binding_fires_only_while_the_mode_key_is_down() {
        let mut held = HashMap::new();
        held.insert(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_F1));
        let harness = CommandHarness::spawn(config_with_profile(profile_with_held_bindings(held)));

        // Base layer: Grid(1,1) is unbound there, so pressing it while the
        // Mode key is up must passthrough (KEY_1), never the Held binding.
        harness.press(Input::Grid(1, 1)).await;

        harness
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Down,
                depth: None,
            })
            .await
            .unwrap();
        // `PhysicalEvent`s and `Command`s arrive on separate channels the
        // dispatch task `select!`s over with no ordering guarantee between
        // them, so a `GetState` sent right after this event isn't
        // guaranteed to observe it applied yet — yield a few times first,
        // same pattern the Toggle tests above use.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(harness.get_state().await.layer, "held");

        // Held layer active: the same physical key now fires the Held
        // Binding instead.
        harness.press(Input::Grid(1, 1)).await;

        let batches = harness.shut_down().await;

        assert_eq!(
            batches.len(),
            3,
            "one passthrough + one press + one release"
        );
        let evdev::EventSummary::Key(_, code, _) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_1, "Base layer: passthrough");
        let evdev::EventSummary::Key(_, code, _) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_F1, "Held layer: remapped");
    }

    #[tokio::test]
    async fn releasing_the_mode_key_reverts_to_the_base_layer() {
        let mut held = HashMap::new();
        held.insert(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_F1));
        let harness = CommandHarness::spawn(config_with_profile(profile_with_held_bindings(held)));

        harness
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Down,
                depth: None,
            })
            .await
            .unwrap();
        harness
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Up,
                depth: None,
            })
            .await
            .unwrap();
        // Same ordering caveat as the test above.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let state = harness.get_state().await;
        assert_eq!(state.layer, "base");

        // Base resumed: Grid(1,1) is unbound there, so this passes through.
        harness.press(Input::Grid(1, 1)).await;

        let batches = harness.shut_down().await;
        assert_eq!(batches.len(), 1);
        let evdev::EventSummary::Key(_, code, _) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_1);
    }

    #[tokio::test]
    async fn under_layer_switch_the_mode_key_never_passes_through_its_own_keycode() {
        // Unbound ModeKey, default LayerSwitch role: pressing and releasing
        // it must produce no injected output at all — it's consumed
        // entirely by the Layer transition, never passed through as
        // KEY_LEFTALT the way an ordinary unbound Input would be.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Down,
                depth: None,
            })
            .await
            .unwrap();
        harness
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Up,
                depth: None,
            })
            .await
            .unwrap();

        let batches = harness.shut_down().await;
        assert!(batches.is_empty());
    }

    #[tokio::test]
    async fn bound_mode_key_role_routes_the_mode_key_through_full_trigger_mode_dispatch() {
        let mut base = HashMap::new();
        base.insert(
            Input::ModeKey,
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_F1,
                },
            },
        );
        let profile = Profile {
            base,
            mode_key_role: ModeKeyRole::Bound,
            ..Default::default()
        };
        let harness = CommandHarness::spawn(config_with_profile(profile));

        harness
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Down,
                depth: None,
            })
            .await
            .unwrap();
        // Real evdev autorepeat events land tens of milliseconds apart —
        // yield so the Down firing's spawned task actually completes before
        // the Repeat lands, matching the same-Input overlap-drop behavior
        // the Hold-to-repeat test above already exercises.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Repeat,
                depth: None,
            })
            .await
            .unwrap();

        // Bound routes through the normal lookup, so the Layer never
        // switches — GetState().layer stays "base" throughout.
        let state = harness.get_state().await;
        assert_eq!(state.layer, "base");

        let batches = harness.shut_down().await;

        // Down + one Repeat = two Hold-to-repeat firings, each a
        // KeyDown/KeyUp pair of the bound Keypress — not KEY_LEFTALT, and
        // not a Layer switch.
        assert_eq!(batches.len(), 4);
        for pair in batches.chunks(2) {
            let evdev::EventSummary::Key(_, down_code, down_value) = pair[0][0].destructure()
            else {
                panic!("expected a key event");
            };
            let evdev::EventSummary::Key(_, up_code, up_value) = pair[1][0].destructure() else {
                panic!("expected a key event");
            };
            assert_eq!((down_code, down_value), (evdev::KeyCode::KEY_F1, 1));
            assert_eq!((up_code, up_value), (evdev::KeyCode::KEY_F1, 0));
        }
    }

    #[tokio::test]
    async fn leaving_bound_role_stops_an_active_toggle_on_the_mode_key() {
        // A code-review-caught edge case: a Toggle can only ever have been
        // started on the Mode key while `Bound`. Once `LayerSwitch` takes
        // over, every `Input::ModeKey` press is intercepted for Layer
        // switching before it ever reaches the stop-toggle check — so
        // without an explicit stop here, this Toggle would run forever with
        // no physical key able to stop it.
        let (action, macros) = macro_action(
            "test-macro",
            vec![
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                MacroStepDto::Delay(50),
            ],
        );
        let mut base = HashMap::new();
        base.insert(
            Input::ModeKey,
            Binding {
                trigger: TriggerMode::Toggle,
                action,
            },
        );
        let profile = Profile {
            base,
            mode_key_role: ModeKeyRole::Bound,
            ..Default::default()
        };
        let harness = CommandHarness::spawn(config_with_profile_and_macros(profile, macros));

        harness
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Down,
                depth: None,
            })
            .await
            .unwrap();
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            harness.get_state().await.active_toggles,
            vec![Input::ModeKey]
        );

        harness
            .set_mode_key_role(ModeKeyRole::LayerSwitch)
            .await
            .expect("SetModeKeyRole must succeed");

        let state = harness.get_state().await;
        assert!(
            state.active_toggles.is_empty(),
            "the Toggle must be stopped, not orphaned"
        );

        let batches = harness.shut_down().await;

        // One KeyDown from the loop's single lap, then a force-released
        // KeyUp for exactly that key on stop — no stuck key.
        assert_eq!(batches.len(), 2);
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_A, 1));
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_A, 0));
    }

    #[tokio::test]
    async fn fire_once_step_binding_advances_the_cursor_forward_and_fires_the_new_item() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let stepper_id = harness
            .create_stepper(
                "Weapon Wheel",
                vec![
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_1,
                        modifiers: Modifiers::default(),
                    },
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_2,
                        modifiers: Modifiers::default(),
                    },
                ],
            )
            .await
            .unwrap();
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Step {
                        stepper: stepper_id.clone(),
                        direction: StepDirection::Forward,
                    },
                },
            )
            .await
            .unwrap();

        // Reset to the list's first item (index 0); the first step must
        // move to index 1 and fire KEY_2 — "the newly-selected item," not
        // the resting position.
        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        let state = harness.get_state().await;
        assert_eq!(state.stepper_cursors[&stepper_id], 1);

        let batches = harness.shut_down().await;
        assert_eq!(batches.len(), 2, "one press batch + one release batch");
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_2, 1));
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_2, 0));
    }

    /// Ticket 63: `resolve_step` no longer hardcodes a bare KeyDown/KeyUp
    /// pair — a modifier-bearing item compiles through the same canned
    /// mods-down/key/mods-up sequence as `Action::Keypress`, mirroring
    /// `executor::tests::compile_keypress_is_a_canned_modifier_key_sequence`'s
    /// shape.
    #[test]
    fn resolve_step_with_modifiers_compiles_the_canned_mods_down_key_up_sequence() {
        let stepper_id = StepperId::from("hotkey-pages");
        let mut steppers = HashMap::new();
        steppers.insert(
            stepper_id.clone(),
            StepperDef {
                name: "Hotkey Pages".to_string(),
                items: vec![crate::config::StepperItem::Key {
                    key: evdev::KeyCode::KEY_3,
                    modifiers: Modifiers {
                        ctrl: true,
                        shift: true,
                        alt: false,
                        super_key: false,
                    },
                }],
            },
        );
        let mut cursors = HashMap::new();

        let steps = resolve_step(&steppers, &mut cursors, &stepper_id, StepDirection::Forward);

        assert_eq!(
            steps,
            vec![
                executor::MacroStep::KeyDown(evdev::KeyCode::KEY_LEFTCTRL),
                executor::MacroStep::KeyDown(evdev::KeyCode::KEY_LEFTSHIFT),
                executor::MacroStep::KeyDown(evdev::KeyCode::KEY_3),
                executor::MacroStep::KeyUp(evdev::KeyCode::KEY_3),
                executor::MacroStep::KeyUp(evdev::KeyCode::KEY_LEFTSHIFT),
                executor::MacroStep::KeyUp(evdev::KeyCode::KEY_LEFTCTRL),
            ]
        );
    }

    /// Ticket 92: a `StepperItem::ControllerButton` compiles to the same
    /// down/dwell/up triple as `Action::ControllerButton`'s digital path.
    #[test]
    fn resolve_step_compiles_a_controller_button_item_to_the_dwell_triple() {
        let stepper_id = StepperId::from("weapon-wheel");
        let mut steppers = HashMap::new();
        steppers.insert(
            stepper_id.clone(),
            StepperDef {
                name: "Weapon Wheel".to_string(),
                items: vec![crate::config::StepperItem::ControllerButton {
                    button: evdev::KeyCode::BTN_SOUTH,
                }],
            },
        );
        let mut cursors = HashMap::new();

        let steps = resolve_step(&steppers, &mut cursors, &stepper_id, StepDirection::Forward);

        assert_eq!(
            steps,
            vec![
                executor::MacroStep::KeyDown(evdev::KeyCode::BTN_SOUTH),
                executor::MacroStep::Delay(executor::CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD),
                executor::MacroStep::KeyUp(evdev::KeyCode::BTN_SOUTH),
            ]
        );
    }

    #[tokio::test]
    async fn step_binding_wraps_around_at_either_end() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let stepper_id = harness
            .create_stepper(
                "Weapon Wheel",
                vec![
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_1,
                        modifiers: Modifiers::default(),
                    },
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_2,
                        modifiers: Modifiers::default(),
                    },
                ],
            )
            .await
            .unwrap();
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Step {
                        stepper: stepper_id.clone(),
                        direction: StepDirection::Backward,
                    },
                },
            )
            .await
            .unwrap();

        // Backward from index 0 wraps to the last item (index 1 of a
        // 2-item list) rather than clamping or panicking.
        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        let state = harness.get_state().await;
        harness.shut_down().await;
        assert_eq!(state.stepper_cursors[&stepper_id], 1);
    }

    #[tokio::test]
    async fn hold_to_repeat_step_binding_advances_the_cursor_on_every_repeat() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let stepper_id = harness
            .create_stepper(
                "Weapon Wheel",
                vec![
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_1,
                        modifiers: Modifiers::default(),
                    },
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_2,
                        modifiers: Modifiers::default(),
                    },
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_3,
                        modifiers: Modifiers::default(),
                    },
                ],
            )
            .await
            .unwrap();
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::HoldToRepeat,
                    action: Action::Step {
                        stepper: stepper_id.clone(),
                        direction: StepDirection::Forward,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.repeat(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let state = harness.get_state().await;
        harness.shut_down().await;
        // Down, then one Repeat = two advances from index 0: 0 -> 1 -> 2.
        assert_eq!(state.stepper_cursors[&stepper_id], 2);
    }

    #[tokio::test]
    async fn get_state_reports_zero_for_a_stepper_never_yet_stepped() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let stepper_id = harness
            .create_stepper(
                "Weapon Wheel",
                vec![crate::config::StepperItem::Key {
                    key: evdev::KeyCode::KEY_1,
                    modifiers: Modifiers::default(),
                }],
            )
            .await
            .unwrap();

        let state = harness.get_state().await;
        harness.shut_down().await;

        assert_eq!(state.stepper_cursors[&stepper_id], 0);
    }

    /// Regression test for a `/code-review` finding: `DeleteStepper` used to
    /// leave the deleted Stepper's runtime cursor sitting in
    /// `stepper_cursors` — since `unique_stepper_id` can reassign a freed
    /// slug to a brand-new, unrelated `CreateStepper` call, a stale nonzero
    /// cursor would leak into that new entry's very first `GetState()`,
    /// violating "always resets to the list's first item."
    #[tokio::test]
    async fn delete_stepper_command_clears_its_runtime_cursor() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let stepper_id = harness
            .create_stepper(
                "Weapon Wheel",
                vec![
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_1,
                        modifiers: Modifiers::default(),
                    },
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_2,
                        modifiers: Modifiers::default(),
                    },
                ],
            )
            .await
            .unwrap();
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Step {
                        stepper: stepper_id.clone(),
                        direction: StepDirection::Forward,
                    },
                },
            )
            .await
            .unwrap();
        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(harness.get_state().await.stepper_cursors[&stepper_id], 1);

        harness
            .clear_binding(Input::Grid(1, 1), Layer::Base)
            .await
            .unwrap();
        harness.delete_stepper(stepper_id.clone()).await.unwrap();

        // A brand-new, unrelated Stepper that happens to land on the exact
        // same freed slug must start at index 0, not inherit the deleted
        // entry's stale cursor.
        let reused_id = harness
            .create_stepper("Weapon Wheel", vec![])
            .await
            .unwrap();
        assert_eq!(reused_id, stepper_id);
        let state = harness.get_state().await;
        harness.shut_down().await;
        assert_eq!(state.stepper_cursors[&reused_id], 0);
    }

    /// Regression test for a `/code-review` finding: `SetStepperItems`
    /// shrinking a list used to leave a stored cursor pointing past the new
    /// end, so `GetState()` reported an out-of-range index until the
    /// Stepper was next fired (only `resolve_step` clamped).
    #[tokio::test]
    async fn set_stepper_items_clamps_a_cursor_left_stranded_by_a_shrink() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let stepper_id = harness
            .create_stepper(
                "Weapon Wheel",
                vec![
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_1,
                        modifiers: Modifiers::default(),
                    },
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_2,
                        modifiers: Modifiers::default(),
                    },
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_3,
                        modifiers: Modifiers::default(),
                    },
                ],
            )
            .await
            .unwrap();
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Step {
                        stepper: stepper_id.clone(),
                        direction: StepDirection::Forward,
                    },
                },
            )
            .await
            .unwrap();
        // Advance to index 2 (the last item of the 3-item list).
        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.release(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(harness.get_state().await.stepper_cursors[&stepper_id], 2);

        // Shrink to a single item — the stranded index-2 cursor must be
        // clamped immediately, not just on the Stepper's next fire.
        harness
            .set_stepper_items(
                stepper_id.clone(),
                vec![crate::config::StepperItem::Key {
                    key: evdev::KeyCode::KEY_9,
                    modifiers: Modifiers::default(),
                }],
            )
            .await
            .unwrap();
        let state = harness.get_state().await;
        harness.shut_down().await;
        assert_eq!(state.stepper_cursors[&stepper_id], 0);
    }

    /// Companion to the shrink-clamp test above: shrinking a Stepper's item
    /// list to zero must drop its cursor entirely, matching a never-yet-
    /// stepped/never-created list's own `GetState()` default.
    #[tokio::test]
    async fn set_stepper_items_to_empty_resets_the_cursor_to_the_default() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let stepper_id = harness
            .create_stepper(
                "Weapon Wheel",
                vec![
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_1,
                        modifiers: Modifiers::default(),
                    },
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_2,
                        modifiers: Modifiers::default(),
                    },
                ],
            )
            .await
            .unwrap();
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Step {
                        stepper: stepper_id.clone(),
                        direction: StepDirection::Forward,
                    },
                },
            )
            .await
            .unwrap();
        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(harness.get_state().await.stepper_cursors[&stepper_id], 1);

        harness
            .set_stepper_items(stepper_id.clone(), vec![])
            .await
            .unwrap();
        let state = harness.get_state().await;
        harness.shut_down().await;
        assert_eq!(state.stepper_cursors[&stepper_id], 0);
    }

    #[tokio::test]
    async fn fire_once_step_binding_produces_no_extra_output_on_physical_release() {
        // Mirrors `fire_once_binding_ignores_repeat_and_up_fires_only_on_down`
        // for a Stepper: a Step compiles to an already-balanced
        // KeyDown/KeyUp pair, so the physical `Up`'s force-release check
        // (ticket 33) finds nothing left held and produces no extra output.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let stepper_id = harness
            .create_stepper(
                "Weapon Wheel",
                vec![crate::config::StepperItem::Key {
                    key: evdev::KeyCode::KEY_1,
                    modifiers: Modifiers::default(),
                }],
            )
            .await
            .unwrap();
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Step {
                        stepper: stepper_id,
                        direction: StepDirection::Forward,
                    },
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.release(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;
        assert_eq!(batches.len(), 2, "one press batch + one release batch");
    }

    #[tokio::test]
    async fn each_profile_carries_independent_binding_sets() {
        let mut gaming_base = HashMap::new();
        gaming_base.insert(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_F1));
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), Profile::default());
        profiles.insert(
            "Gaming".to_string(),
            Profile {
                base: gaming_base,
                ..Default::default()
            },
        );
        let config = Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros: HashMap::new(),
            steppers: HashMap::new(),
        };
        let harness = CommandHarness::spawn(config);

        // Base layer, Default Profile: Grid(1,1) is unbound there, so it
        // passes through — Gaming's own Binding must not leak across.
        harness.press(Input::Grid(1, 1)).await;
        // `PhysicalEvent`s and `Command`s arrive on separate channels the
        // dispatch task `select!`s over with no ordering guarantee between
        // them (issue 07), so the switch below isn't guaranteed to be
        // processed after this press without yielding first — same pattern
        // the Held-layer tests above use.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        harness.switch_profile("Gaming").await.unwrap();

        // Same physical key, now evaluated under Gaming's own independent
        // Binding set.
        harness.press(Input::Grid(1, 1)).await;

        let batches = harness.shut_down().await;

        assert_eq!(
            batches.len(),
            3,
            "one passthrough + one press + one release"
        );
        let evdev::EventSummary::Key(_, code, _) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_1, "Default Profile: passthrough");
        let evdev::EventSummary::Key(_, code, _) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_F1, "Gaming Profile: remapped");
    }

    #[tokio::test(start_paused = true)]
    async fn switch_profile_force_stops_every_active_toggle_with_exact_key_release() {
        // Deliberately unbalanced within the window we switch in: KeyDown
        // fires, then a long Delay, so KEY_A is still held when the Profile
        // switch stops it.
        let (action, macros) = macro_action(
            "test-macro",
            vec![
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                MacroStepDto::Delay(50),
            ],
        );
        let mut base = HashMap::new();
        base.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action,
            },
        );
        let mut profiles = HashMap::new();
        profiles.insert(
            DEFAULT_PROFILE_NAME.to_string(),
            Profile {
                base,
                ..Default::default()
            },
        );
        profiles.insert("Gaming".to_string(), Profile::default());
        let config = Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros,
            steppers: HashMap::new(),
        };
        let harness = CommandHarness::spawn(config);

        harness.press(Input::Grid(1, 1)).await;
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;

        assert_eq!(
            harness.get_state().await.active_toggles,
            vec![Input::Grid(1, 1)]
        );

        harness
            .switch_profile("Gaming")
            .await
            .expect("SwitchProfile must succeed");

        let state = harness.get_state().await;
        assert!(
            state.active_toggles.is_empty(),
            "the Toggle must be force-stopped by the switch, not orphaned"
        );

        let batches = harness.shut_down().await;

        // One KeyDown lap, then the force-released KeyUp for exactly that
        // key — no stuck key, no continued looping into the new Profile.
        assert_eq!(batches.len(), 2);
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_A, 1));
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_A, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn firing_a_profile_switch_binding_switches_the_active_profile_and_force_stops_toggles() {
        // Mirrors switch_profile_force_stops_every_active_toggle_with_exact_key_release
        // above, but drives the switch through a real PhysicalEvent firing an
        // Action::ProfileSwitch Binding (ticket 34) instead of the
        // Command::SwitchProfile D-Bus path — `handle_event`'s interception
        // must produce the exact same effects the shared `switch_profile`
        // gives `Command::SwitchProfile`.
        let (action, macros) = macro_action(
            "test-macro",
            vec![
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                MacroStepDto::Delay(50),
            ],
        );
        let mut base = HashMap::new();
        base.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action,
            },
        );
        base.insert(
            Input::Grid(1, 2),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::ProfileSwitch {
                    target: "Gaming".to_string(),
                },
            },
        );
        let mut profiles = HashMap::new();
        profiles.insert(
            DEFAULT_PROFILE_NAME.to_string(),
            Profile {
                base,
                ..Default::default()
            },
        );
        profiles.insert("Gaming".to_string(), Profile::default());
        let config = Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros,
            steppers: HashMap::new(),
        };
        let harness = CommandHarness::spawn(config);

        harness.press(Input::Grid(1, 1)).await;
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            harness.get_state().await.active_toggles,
            vec![Input::Grid(1, 1)]
        );

        harness.press(Input::Grid(1, 2)).await;
        tokio::task::yield_now().await;

        let state = harness.get_state().await;
        assert_eq!(state.profile, "Gaming");
        assert!(
            state.active_toggles.is_empty(),
            "firing a Profile Switch Binding must force-stop every active Toggle, same as Command::SwitchProfile"
        );

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        harness.shut_down().await;
        let reparsed: Config = toml::from_str(&on_disk).unwrap();
        assert_eq!(reparsed.active_profile, "Gaming");
    }

    #[tokio::test(start_paused = true)]
    async fn stop_all_toggles_force_stops_every_active_toggle_without_switching_profile() {
        // Ticket 25's live-hardware finding: the GUI needs to be able to
        // kill a Toggle left running once its own window gains focus,
        // without that also being a Profile switch — same force-stop
        // mechanism as SwitchProfile, minus the Profile change.
        let (action, macros) = macro_action(
            "test-macro",
            vec![
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                MacroStepDto::Delay(50),
            ],
        );
        let mut base = HashMap::new();
        base.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action,
            },
        );
        let mut profiles = HashMap::new();
        profiles.insert(
            DEFAULT_PROFILE_NAME.to_string(),
            Profile {
                base,
                ..Default::default()
            },
        );
        let config = Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros,
            steppers: HashMap::new(),
        };
        let harness = CommandHarness::spawn(config);

        harness.press(Input::Grid(1, 1)).await;
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            harness.get_state().await.active_toggles,
            vec![Input::Grid(1, 1)]
        );

        harness.stop_all_toggles().await;

        let state = harness.get_state().await;
        assert!(
            state.active_toggles.is_empty(),
            "the Toggle must be force-stopped"
        );
        assert_eq!(
            state.profile, DEFAULT_PROFILE_NAME,
            "stopping Toggles must not change the active Profile"
        );

        let batches = harness.shut_down().await;
        assert_eq!(
            batches.len(),
            2,
            "one KeyDown lap, then the force-released KeyUp"
        );
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_A, 0));
    }

    #[tokio::test]
    async fn set_actuation_point_publishes_the_resolved_snapshot() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        // Published once, up front, before any Command — the default for
        // every one of the 20 Grid keys. `dispatch::run`'s startup publish
        // races this test's own setup (spawned as a separate task), same
        // caveat every other cross-channel test in this module documents.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            harness.actuation_snapshot()[&Input::Grid(1, 1)],
            ActuationPoint::default()
        );

        harness
            .set_actuation_point(Input::Grid(1, 1), 200, 180)
            .await
            .expect("SetActuationPoint must succeed");

        assert_eq!(
            harness.actuation_snapshot()[&Input::Grid(1, 1)],
            ActuationPoint {
                actuation: 200,
                release: 180,
            }
        );
        // Every other key is untouched — still the Profile default.
        assert_eq!(
            harness.actuation_snapshot()[&Input::Grid(1, 2)],
            ActuationPoint::default()
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn clear_actuation_point_publishes_the_reverted_snapshot() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_actuation_point(Input::Grid(1, 1), 200, 180)
            .await
            .unwrap();

        harness
            .clear_actuation_point(Input::Grid(1, 1))
            .await
            .expect("ClearActuationPoint must succeed");

        assert_eq!(
            harness.actuation_snapshot()[&Input::Grid(1, 1)],
            ActuationPoint::default()
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_default_actuation_publishes_the_new_default_for_every_unoverridden_key() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness.set_default_actuation(140, 120).await.unwrap();

        let snapshot = harness.actuation_snapshot();
        assert_eq!(snapshot.len(), 20);
        for point in snapshot.values() {
            assert_eq!(
                *point,
                ActuationPoint {
                    actuation: 140,
                    release: 120,
                }
            );
        }

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn reset_actuation_points_publishes_the_profile_default_for_every_key() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_actuation_point(Input::Grid(1, 1), 200, 180)
            .await
            .unwrap();

        harness.reset_actuation_points().await.unwrap();

        assert_eq!(
            harness.actuation_snapshot()[&Input::Grid(1, 1)],
            ActuationPoint::default()
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn switch_profile_publishes_the_new_profiles_own_actuation_points() {
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), Profile::default());
        profiles.insert(
            "Gaming".to_string(),
            Profile {
                default_actuation: ActuationPoint {
                    actuation: 90,
                    release: 60,
                },
                ..Default::default()
            },
        );
        let config = Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros: HashMap::new(),
            steppers: HashMap::new(),
        };
        let harness = CommandHarness::spawn(config);

        harness.switch_profile("Gaming").await.unwrap();

        assert_eq!(
            harness.actuation_snapshot()[&Input::Grid(1, 1)],
            ActuationPoint {
                actuation: 90,
                release: 60,
            }
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_force_digital_command_applies_live_and_persists_to_disk() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_force_digital(true)
            .await
            .expect("SetForceDigital must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert!(config.force_digital);
    }

    // --- Axis assignment (ticket 59/71) ---

    fn abs_axis_and_value(event: evdev::InputEvent) -> (evdev::AbsoluteAxisCode, i32) {
        match event.destructure() {
            evdev::EventSummary::AbsoluteAxis(_, axis, value) => (axis, value),
            other => panic!("expected an absolute-axis event, got {other:?}"),
        }
    }

    /// Every `AbsoluteAxisCode, value` pair across every gamepad batch, in
    /// order — the shape most axis tests below want to assert against,
    /// rather than each batch's own boundaries (every axis write is its own
    /// single-event batch, mirroring `set_key_state`'s one-`SYN_REPORT`-per-
    /// transition shape).
    fn flat_axis_writes(
        batches: Vec<Vec<evdev::InputEvent>>,
    ) -> Vec<(evdev::AbsoluteAxisCode, i32)> {
        batches
            .into_iter()
            .flatten()
            .map(abs_axis_and_value)
            .collect()
    }

    #[tokio::test]
    async fn clear_axis_assignment_zeroes_a_still_live_output_that_dropped_out_of_the_map() {
        // Code-review finding: `recompute_and_emit_axes` used to only ever
        // walk the codes `axis_map` currently names — a code that drops out
        // entirely (its last remaining Input cleared) was never revisited,
        // so its last-written nonzero value stuck forever.
        let mut config = config_with_bindings(HashMap::new());
        config
            .active_profile_mut()
            .unwrap()
            .axis_base
            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);
        let harness = CommandHarness::spawn(config);

        harness.push_depth([(Input::Grid(1, 1), 200)]);
        tokio::task::yield_now().await;

        harness
            .clear_axis_assignment(Input::Grid(1, 1), Layer::Base)
            .await
            .expect("ClearAxisAssignment must succeed");

        let writes = flat_axis_writes(harness.gamepad_batches());
        harness.shut_down().await;

        assert_eq!(
            writes,
            vec![
                (evdev::AbsoluteAxisCode::ABS_Z, 200),
                (evdev::AbsoluteAxisCode::ABS_Z, 0),
            ]
        );
    }

    #[tokio::test]
    async fn retargeting_an_axis_assignment_zeroes_the_old_abs_code() {
        let mut config = config_with_bindings(HashMap::new());
        config
            .active_profile_mut()
            .unwrap()
            .axis_base
            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);
        let harness = CommandHarness::spawn(config);

        harness.push_depth([(Input::Grid(1, 1), 200)]);
        tokio::task::yield_now().await;

        harness
            .set_axis_assignment(Input::Grid(1, 1), Layer::Base, AxisTarget::RightTrigger)
            .await
            .expect("SetAxisAssignment must succeed");

        let writes = flat_axis_writes(harness.gamepad_batches());
        harness.shut_down().await;

        // The old code (ABS_Z) is zeroed by the stale-code sweep; the
        // now-retargeted Input's carried-over Depth immediately drives the
        // new code (ABS_RZ) in the same recompute pass.
        assert_eq!(
            writes,
            vec![
                (evdev::AbsoluteAxisCode::ABS_Z, 200),
                (evdev::AbsoluteAxisCode::ABS_Z, 0),
                (evdev::AbsoluteAxisCode::ABS_RZ, 200),
            ]
        );
    }

    #[tokio::test]
    async fn an_analog_sourced_event_on_an_axis_assigned_key_never_passes_through() {
        let mut config = config_with_bindings(HashMap::new());
        config
            .active_profile_mut()
            .unwrap()
            .axis_base
            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 200).await;
        tokio::task::yield_now().await;

        let batches = harness.shut_down().await;
        assert!(
            batches.is_empty(),
            "an Axis-assigned key's own discrete transition must never fall through to passthrough"
        );
    }

    #[tokio::test]
    async fn digital_mode_step_fallback_ramps_up_on_repeat_and_resets_on_release() {
        let mut config = config_with_bindings(HashMap::new());
        config
            .active_profile_mut()
            .unwrap()
            .axis_base
            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);
        let harness = CommandHarness::spawn(config);

        harness.press(Input::Grid(1, 1)).await;
        harness.repeat(Input::Grid(1, 1)).await;
        harness.release(Input::Grid(1, 1)).await;
        tokio::task::yield_now().await;

        let writes = flat_axis_writes(harness.gamepad_batches());
        harness.shut_down().await;

        assert_eq!(
            writes,
            vec![
                (evdev::AbsoluteAxisCode::ABS_Z, i32::from(AXIS_DIGITAL_STEP)),
                (
                    evdev::AbsoluteAxisCode::ABS_Z,
                    i32::from(AXIS_DIGITAL_STEP) * 2
                ),
                (evdev::AbsoluteAxisCode::ABS_Z, 0),
            ]
        );
    }

    #[tokio::test]
    async fn continuous_depth_updates_drive_the_assigned_axis_live() {
        let mut config = config_with_bindings(HashMap::new());
        config
            .active_profile_mut()
            .unwrap()
            .axis_base
            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);
        let harness = CommandHarness::spawn(config);

        harness.push_depth([(Input::Grid(1, 1), 200)]);
        tokio::task::yield_now().await;

        let writes = flat_axis_writes(harness.gamepad_batches());
        harness.shut_down().await;

        assert_eq!(writes, vec![(evdev::AbsoluteAxisCode::ABS_Z, 200)]);
    }

    #[tokio::test]
    async fn two_keys_sharing_one_same_signed_target_take_the_greater_depth() {
        let mut config = config_with_bindings(HashMap::new());
        {
            let profile = config.active_profile_mut().unwrap();
            profile
                .axis_base
                .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);
            profile
                .axis_base
                .insert(Input::Grid(1, 2), AxisTarget::LeftTrigger);
        }
        let harness = CommandHarness::spawn(config);

        harness.push_depth([(Input::Grid(1, 1), 150), (Input::Grid(1, 2), 200)]);
        tokio::task::yield_now().await;

        let writes = flat_axis_writes(harness.gamepad_batches());
        harness.shut_down().await;

        assert_eq!(writes, vec![(evdev::AbsoluteAxisCode::ABS_Z, 200)]);
    }

    #[tokio::test]
    async fn opposite_signed_halves_let_the_already_active_key_keep_driving() {
        let mut config = config_with_bindings(HashMap::new());
        {
            let profile = config.active_profile_mut().unwrap();
            profile
                .axis_base
                .insert(Input::Grid(1, 1), AxisTarget::LeftStickXPos);
            profile
                .axis_base
                .insert(Input::Grid(1, 2), AxisTarget::LeftStickXNeg);
        }
        let harness = CommandHarness::spawn(config);

        // Positive half activates alone first.
        harness.push_depth([(Input::Grid(1, 1), 200)]);
        tokio::task::yield_now().await;
        // Negative half now also activates — the already-active positive
        // half must keep winning (ticket 59 §5).
        harness.push_depth([(Input::Grid(1, 1), 200), (Input::Grid(1, 2), 220)]);
        tokio::task::yield_now().await;

        let writes = flat_axis_writes(harness.gamepad_batches());
        harness.shut_down().await;

        assert_eq!(
            writes,
            vec![
                (evdev::AbsoluteAxisCode::ABS_X, 200),
                (evdev::AbsoluteAxisCode::ABS_X, 200),
            ],
            "the positive half must keep suppressing the newcomer negative half"
        );
    }

    #[tokio::test]
    async fn a_layer_switch_centers_any_live_axis_output() {
        let mut config = config_with_bindings(HashMap::new());
        config
            .active_profile_mut()
            .unwrap()
            .axis_base
            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);
        let harness = CommandHarness::spawn(config);

        harness.push_depth([(Input::Grid(1, 1), 200)]);
        tokio::task::yield_now().await;
        harness.press(Input::ModeKey).await;
        tokio::task::yield_now().await;

        let writes = flat_axis_writes(harness.gamepad_batches());
        harness.shut_down().await;

        assert_eq!(
            writes,
            vec![
                (evdev::AbsoluteAxisCode::ABS_Z, 200),
                (evdev::AbsoluteAxisCode::ABS_Z, 0),
            ]
        );
    }
}
