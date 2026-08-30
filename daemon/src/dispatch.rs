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

use std::collections::{BTreeSet, HashMap, HashSet};
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
use crate::command::{Command, CommandError, State};
use crate::config::{
    self, Action, ActuationPoint, AxisPolarity, AxisTarget, Binding, ChordKey, Config, Layer,
    MacroDef, MacroId, ModeKeyRole, Profile, StepDirection, StepperDef, StepperId, StepperItem,
    TriggerMode,
};
use crate::dbus::Daemon;
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

/// The fixed simultaneity window between a Chord's first and last member
/// going down (ticket 01's Answer, §"Simultaneity detection") — a Rust
/// constant, deliberately not a persisted `Config` value or a v1.0 user
/// setting.
const CHORD_WINDOW: Duration = Duration::from_millis(50);

/// The currently-developing press-combo a Chord may complete from (ticket
/// 01/40): every chord-eligible Input pressed since the window opened, and
/// the absolute instant it closes. At most one window is ever open at a
/// time — a fresh chord-eligible Down either joins the existing window or,
/// if none is open, starts a new one.
struct ChordWindow {
    // `BTreeSet`, not `HashSet`: compared directly against a `ChordKey`'s
    // own `BTreeSet<Input>` membership via `is_subset` below, which requires
    // the same set type on both sides.
    down: BTreeSet<Input>,
    deadline: Instant,
}

/// Every piece of Daemon-owned runtime state the Chord-detection state
/// machine needs (ticket 01/40), mirroring `toggles`/`in_flight`'s existing
/// per-Input shapes but keyed by `ChordKey` — a Chord's own Trigger-mode
/// dispatch is otherwise identical to an ordinary Binding's, just evaluated
/// against a member *set* rather than one Input (see `fire_chord`).
#[derive(Default)]
struct ChordState {
    window: Option<ChordWindow>,
    in_flight: HashMap<ChordKey, FiringHandle>,
    toggles: HashMap<ChordKey, ActiveToggle>,
    /// Every Input currently "owned" by the Chord machinery — either still
    /// inside an open window, or physically held down as a member of a
    /// Chord that has since fired. Routes that Input's later Repeat/Up
    /// events back through the Chord path rather than the ordinary
    /// per-Input one, even after `chords_containing_input` would otherwise
    /// still call it chord-eligible (ticket 01: "the remaining member(s)
    /// don't fall back to their individual Bindings until they're released
    /// and re-pressed fresh").
    claimed: HashSet<Input>,
}

/// Every piece of Daemon-owned runtime state Axis-assignment resolution
/// needs (ticket 59/71), mirroring `toggles`/`chord_state`'s own per-Input
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
    // Owns the ~50ms Chord simultaneity window plus every currently-active
    // Chord's Trigger-mode state (ticket 01/40) — reset fresh on every
    // dispatch task start, same as `toggles`/`active_layer`.
    let mut chord_state = ChordState::default();
    // Owns every Axis-assigned Input's live contribution/opposite-half
    // ownership (ticket 59/71) — reset fresh on every dispatch task start,
    // same as `chord_state`.
    let mut axis_state = AxisState::default();
    // Every currently-running Analog-repeat task (ticket 20/39), keyed by
    // grid Input — reset fresh on every dispatch task start, same as
    // `axis_state`; started/stopped by `update_analog_repeats` off every
    // `rx_depth` snapshot below.
    let mut analog_repeats: HashMap<Input, ActiveAnalogRepeat> = HashMap::new();
    let mut depth_open = true;
    loop {
        tokio::select! {
            event = rx_events.recv() => {
                let Some(event) = event else { break };
                handle_event(
                    &injector,
                    &mut config,
                    &config_path,
                    &mut toggles,
                    &mut in_flight,
                    &mut stepper_cursors,
                    &mut active_layer,
                    &signal_emitter,
                    &actuation_tx,
                    &mut chord_state,
                    &mut axis_state,
                    &mut analog_repeats,
                    toggle_lap_target,
                    event,
                )
                .await?;
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
            () = chord_window_deadline(&chord_state.window) => {
                handle_chord_timeout(
                    &injector,
                    &mut config,
                    &config_path,
                    active_layer,
                    &mut toggles,
                    &mut in_flight,
                    &mut stepper_cursors,
                    &actuation_tx,
                    &signal_emitter,
                    &mut chord_state,
                    &mut axis_state,
                    &mut analog_repeats,
                    toggle_lap_target,
                )
                .await?;
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
                    Some(cmd) => handle_command(&injector, &mut config, &config_path, &mut toggles, &mut stepper_cursors, &mut axis_state, &mut analog_repeats, &active_layer, device_connected, capture_mode, device_info.as_ref(), &signal_emitter, &actuation_tx, &capture_control_tx, cmd).await,
                    None => commands_open = false,
                }
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_event(
    injector: &Injector,
    config: &mut Config,
    config_path: &Path,
    toggles: &mut HashMap<Input, ActiveToggle>,
    in_flight: &mut HashMap<Input, FiringHandle>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    active_layer: &mut Layer,
    signal_emitter: &Option<SignalEmitter<'static>>,
    actuation_tx: &watch::Sender<HashMap<Input, ActuationPoint>>,
    chord_state: &mut ChordState,
    axis_state: &mut AxisState,
    analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>,
    toggle_lap_target: Duration,
    event: PhysicalEvent,
) -> io::Result<()> {
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
        return Ok(());
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
        return Ok(());
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
        return if event.depth.is_some() {
            Ok(())
        } else {
            handle_axis_edge_event(injector, axis_state, axis_map, event.input, event.state).await
        };
    }

    // The Chord-detection machinery (ticket 01/40) takes priority over
    // ordinary Binding lookup for any Input currently "owned" by it —
    // either a fresh chord-eligible Down, or any later Repeat/Up for an
    // Input already claimed by an open window or an active fired Chord
    // (`chord_state.claimed`, not a fresh membership check, so a member
    // that already resolved individually via `fire_individual_retroactively`
    // is never routed back here).
    let owned = chord_state.claimed.contains(&event.input);
    if owned
        || (event.state == EventState::Down
            && !chord_keys_containing(profile.chords(*active_layer), event.input).is_empty())
    {
        return handle_chord_event(
            injector,
            config,
            config_path,
            toggles,
            in_flight,
            stepper_cursors,
            *active_layer,
            signal_emitter,
            actuation_tx,
            chord_state,
            axis_state,
            analog_repeats,
            toggle_lap_target,
            event,
        )
        .await;
    }

    // Cloned rather than matched by reference: an `Action::ProfileSwitch`
    // Binding needs `config` mutably below, and `binding` would otherwise
    // still be borrowing it immutably through `profile`/`bindings` (ticket
    // 34).
    let bindings = profile.layer(*active_layer);
    let binding = bindings.get(&event.input).cloned();
    match binding {
        Some(binding) => {
            if let Action::ProfileSwitch { target } = binding.action {
                // Validated (`SetBinding`/`load_or_seed`) to only ever pair
                // with Fire-once, so only `Down` fires it — mirrors `fire`'s
                // own `(FireOnce, Down)` arm, but this Action has no
                // `MacroStep` form to compile/spawn, so it's handled here
                // instead of reaching `fire`/`executor::compile` at all.
                if event.state == EventState::Down {
                    let succeeded = switch_profile(
                        injector,
                        config,
                        config_path,
                        toggles,
                        actuation_tx,
                        axis_state,
                        analog_repeats,
                        target.clone(),
                    )
                    .await
                    .is_ok();
                    if succeeded && let Some(emitter) = signal_emitter {
                        let _ = Daemon::active_profile_changed(emitter, &target).await;
                    }
                }
                return Ok(());
            }
            // Real firing for Analog-repeat while Depth is available comes
            // entirely from `update_analog_repeats`'s own depth-driven
            // background task (ticket 20/39) — this Analog-sourced
            // Down/Repeat/Up (synthesized from the key's ordinary, *tunable*
            // Actuation/Release points, a different threshold pair than
            // Analog-repeat's own fixed deadzone) is swallowed outright
            // rather than double-firing through `fire()`, mirroring the
            // Axis-assignment swallow above. A Digital-sourced event (no
            // Depth at all) falls through to `fire()`, which treats
            // Analog-repeat exactly like Hold-to-repeat (ticket 20's
            // Digital Capture mode fallback).
            if binding.trigger == TriggerMode::AnalogRepeat && event.depth.is_some() {
                return Ok(());
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
            .await
        }
        None => injector
            .inject_physical(event)
            .await
            .map_err(io::Error::other),
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

/// Every `ChordKey` in `chords` that contains `input` among its members
/// (ticket 01's amended Answer: an Input may belong to any number of
/// Chords, so this can return more than one).
fn chord_keys_containing(chords: &HashMap<ChordKey, Binding>, input: Input) -> Vec<ChordKey> {
    chords
        .keys()
        .filter(|key| key.members().contains(&input))
        .cloned()
        .collect()
}

/// Resolves to the active Chord window's deadline, or never resolves if no
/// window is open — the `tokio::select!` branch in `run` that drives
/// `handle_chord_timeout` evaluates this fresh every loop iteration, so a
/// window opened, extended, or cleared by `handle_event` in between is
/// always picked up on the very next iteration (mirrors `run_depth_stream`'s
/// own `interval_at` reasoning against a similarly-recreated-per-iteration
/// future — recreating a `sleep_until` against the same absolute `Instant`
/// every iteration doesn't lose or reset progress).
async fn chord_window_deadline(window: &Option<ChordWindow>) {
    match window {
        Some(window) => tokio::time::sleep_until(window.deadline).await,
        None => std::future::pending().await,
    }
}

/// `fire`'s exact mirror for a Chord's own Trigger-mode dispatch (ticket
/// 01/40): Fire-once/Hold-to-repeat share one Chord-scoped `FiringHandle`
/// slot per `ChordKey`, Toggle spawns/tracks one `ActiveToggle` per
/// `ChordKey`, both keyed by the Chord's member set rather than by a single
/// Input. `ProfileSwitch` never reaches here — `SetChordBinding`/`parse`
/// both refuse to let a Chord's Action be `ProfileSwitch` (see
/// `ConfigError::InvalidChordProfileSwitch`), since `compile_action` panics
/// on it (it has no `MacroStep` form, only `handle_event`'s single-Input
/// path ever specially handles it).
#[allow(clippy::too_many_arguments)]
async fn fire_chord(
    injector: &Injector,
    chord_toggles: &mut HashMap<ChordKey, ActiveToggle>,
    chord_in_flight: &mut HashMap<ChordKey, FiringHandle>,
    key: ChordKey,
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
            // released by `release_chord_firing` on a member's physical Up.
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
            // released by `release_chord_firing` on a member's physical Up.
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

/// Force-releases a Chord's in-flight Fire-once/Hold-to-repeat firing, if
/// any, on a member's `Up` — those Trigger modes are tied to the physical
/// keys staying down, same as `fire`'s own Up handling. A Chord that never
/// actually fired (still pending, or a FireOnce whose firing already
/// finished on its own) has nothing to release here; the check is a no-op
/// in that case.
///
/// **Toggle-mode Chords are deliberately not touched here** (correction,
/// hardware-verified live in ticket 67): ticket 01's original Answer had
/// any member's `Up` also stop an active Toggle, but that felt wrong on the
/// real device — a Toggle should behave like a toggle, staying on past a
/// release. A Toggle Chord now stops only when its full member set
/// completes again, mirroring how a single Input's own Toggle stops on a
/// second `Down`, never on `Up` — see the `Down` arm of
/// `handle_chord_event`.
async fn release_chord_firing(
    injector: &Injector,
    chord_in_flight: &HashMap<ChordKey, FiringHandle>,
    key: &ChordKey,
) {
    if let Some(firing) = chord_in_flight.get(key) {
        firing.force_release_stuck(injector).await;
    }
}

/// Fires `input`'s own individual Binding as if its Down had just landed
/// fresh — used once a chord-eligible Down never actually completes a Chord
/// (the window elapsed, or the key was released early), per ticket 01's
/// Answer: "the pending member's individual Binding fires retroactively
/// (delayed by the window)". Mirrors `handle_event`'s own ordinary dispatch
/// exactly, including the `ProfileSwitch`/passthrough branches — it must,
/// since a Chord member's *own* individual Binding can be any ordinary
/// Action (unlike a Chord's own Action, which can never be `ProfileSwitch`).
#[allow(clippy::too_many_arguments)]
async fn fire_individual_retroactively(
    injector: &Injector,
    config: &mut Config,
    config_path: &Path,
    toggles: &mut HashMap<Input, ActiveToggle>,
    in_flight: &mut HashMap<Input, FiringHandle>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    actuation_tx: &watch::Sender<HashMap<Input, ActuationPoint>>,
    axis_state: &mut AxisState,
    analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>,
    signal_emitter: &Option<SignalEmitter<'static>>,
    layer: Layer,
    input: Input,
    toggle_lap_target: Duration,
) -> io::Result<()> {
    let profile = config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile");
    let binding = profile.layer(layer).get(&input).cloned();
    match binding {
        Some(binding) => {
            if let Action::ProfileSwitch { target } = binding.action {
                if switch_profile(
                    injector,
                    config,
                    config_path,
                    toggles,
                    actuation_tx,
                    axis_state,
                    analog_repeats,
                    target.clone(),
                )
                .await
                .is_ok()
                    && let Some(emitter) = signal_emitter
                {
                    let _ = Daemon::active_profile_changed(emitter, &target).await;
                }
                return Ok(());
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
            .await
        }
        None => injector
            .inject_physical(PhysicalEvent {
                input,
                state: EventState::Down,
                depth: None,
            })
            .await
            .map_err(io::Error::other),
    }
}

/// The Chord-detection state machine's own event routing (ticket 01/40) —
/// `handle_event` diverts here for any Input currently "owned" by it. See
/// the module's `ChordState`/`ChordWindow` doc comments and ticket 01's
/// Answer for the model this implements.
#[allow(clippy::too_many_arguments)]
async fn handle_chord_event(
    injector: &Injector,
    config: &mut Config,
    config_path: &Path,
    toggles: &mut HashMap<Input, ActiveToggle>,
    in_flight: &mut HashMap<Input, FiringHandle>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    active_layer: Layer,
    signal_emitter: &Option<SignalEmitter<'static>>,
    actuation_tx: &watch::Sender<HashMap<Input, ActuationPoint>>,
    chord_state: &mut ChordState,
    axis_state: &mut AxisState,
    analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>,
    toggle_lap_target: Duration,
    event: PhysicalEvent,
) -> io::Result<()> {
    match event.state {
        EventState::Down => {
            chord_state.claimed.insert(event.input);
            let window = chord_state.window.get_or_insert_with(|| ChordWindow {
                down: BTreeSet::new(),
                deadline: Instant::now() + CHORD_WINDOW,
            });
            window.down.insert(event.input);

            // Every Chord whose full member set is now down, in one pass —
            // firing one can only ever *shrink* `down` (its own members are
            // removed below), never grow it, so a single completion pass is
            // enough; a single Down can complete more than one Chord at once
            // when an Input belongs to several of them (ticket 01's amended
            // Answer — the thumbstick-diagonal worked example).
            let profile = config
                .active_profile()
                .expect("load_or_seed validates active_profile names a real profile");
            let chords = profile.chords(active_layer);
            let down_snapshot = chord_state
                .window
                .as_ref()
                .expect("just inserted above")
                .down
                .clone();
            let starting: Vec<(ChordKey, Binding)> = chords
                .iter()
                .filter(|(key, _)| {
                    // A stale-but-*finished* `chord_in_flight` entry must
                    // not permanently exclude a FireOnce/HoldToRepeat Chord
                    // from ever completing again — `release_chord_firing`
                    // only force-releases it, it never removes the map
                    // entry (mirroring `fire`'s own single-Input
                    // `in_flight`, which is never cleaned up either), so
                    // this must check `is_finished()` itself rather than
                    // bare presence (code-review finding: an earlier
                    // version of this filter treated any entry as
                    // still-active forever).
                    !chord_state.toggles.contains_key(*key)
                        && !chord_state
                            .in_flight
                            .get(*key)
                            .is_some_and(|handle| !handle.is_finished())
                        && key.members().is_subset(&down_snapshot)
                })
                .map(|(key, binding)| (key.clone(), binding.clone()))
                .collect();

            // A Toggle Chord that's already active and whose full member set
            // just completed *again* is the Toggle's own "second Down" —
            // stops it, mirroring a single Input's own Toggle (ticket 67
            // correction; see `release_chord_firing`'s doc comment).
            let stopping: Vec<ChordKey> = chords
                .keys()
                .filter(|key| {
                    chord_state.toggles.contains_key(*key)
                        && key.members().is_subset(&down_snapshot)
                })
                .cloned()
                .collect();

            for (key, binding) in starting {
                fire_chord(
                    injector,
                    &mut chord_state.toggles,
                    &mut chord_state.in_flight,
                    key.clone(),
                    &binding,
                    EventState::Down,
                    &config.macros,
                    &config.steppers,
                    stepper_cursors,
                    toggle_lap_target,
                )
                .await?;
                if let Some(window) = chord_state.window.as_mut() {
                    for member in key.members() {
                        window.down.remove(member);
                    }
                }
            }
            for key in stopping {
                if let Some(toggle) = chord_state.toggles.remove(&key) {
                    toggle.stop().await;
                }
                if let Some(window) = chord_state.window.as_mut() {
                    for member in key.members() {
                        window.down.remove(member);
                    }
                }
            }
            if chord_state
                .window
                .as_ref()
                .is_some_and(|w| w.down.is_empty())
            {
                chord_state.window = None;
            }
            Ok(())
        }
        EventState::Repeat => {
            // A still-pending (not yet completed) member is "held, not
            // fired" — Repeat is a no-op for it, mirroring `fire`'s own
            // FireOnce/Toggle handling of Repeat. Only a member of an
            // already-ACTIVE Hold-to-repeat Chord re-fires.
            //
            // While a Chord is active every member is still physically down
            // (any member's Up would already have ended it via the release
            // path below), so the kernel independently autorepeats *each*
            // member at the same cadence — an N-member Chord otherwise sees
            // up to N interleaved Repeat streams landing on one
            // `chord_in_flight` slot, re-firing N times as fast as a single
            // Input ever would (hardware-verified regression, ticket 67).
            // Only the member sorted first by `ChordKey`'s `BTreeSet`
            // ordering drives the re-fire, so exactly one kernel repeat
            // stream reaches it, matching a single Input's own cadence.
            let profile = config
                .active_profile()
                .expect("load_or_seed validates active_profile names a real profile");
            let chords = profile.chords(active_layer);
            let due: Vec<(ChordKey, Binding)> = chord_keys_containing(chords, event.input)
                .into_iter()
                .filter(|key| key.members().iter().next() == Some(&event.input))
                .filter(|key| chord_state.in_flight.contains_key(key))
                .filter_map(|key| chords.get(&key).cloned().map(|b| (key, b)))
                .filter(|(_, binding)| binding.trigger == TriggerMode::HoldToRepeat)
                .collect();
            for (key, binding) in due {
                fire_chord(
                    injector,
                    &mut chord_state.toggles,
                    &mut chord_state.in_flight,
                    key,
                    &binding,
                    EventState::Repeat,
                    &config.macros,
                    &config.steppers,
                    stepper_cursors,
                    toggle_lap_target,
                )
                .await?;
            }
            Ok(())
        }
        EventState::Up => {
            chord_state.claimed.remove(&event.input);

            // Released before ever completing or timing out: resolves right
            // now rather than waiting out the rest of the window on a key
            // that's no longer even down (ticket 01: a pending member always
            // eventually fires retroactively — an early release just means
            // "now" instead of "at the deadline"), then immediately runs
            // this same Up through the ordinary path to force-release
            // whatever that retroactive Down just started.
            let was_pending = chord_state
                .window
                .as_mut()
                .is_some_and(|window| window.down.remove(&event.input));
            if was_pending {
                if chord_state
                    .window
                    .as_ref()
                    .is_some_and(|w| w.down.is_empty())
                {
                    chord_state.window = None;
                }
                fire_individual_retroactively(
                    injector,
                    config,
                    config_path,
                    toggles,
                    in_flight,
                    stepper_cursors,
                    actuation_tx,
                    axis_state,
                    analog_repeats,
                    signal_emitter,
                    active_layer,
                    event.input,
                    toggle_lap_target,
                )
                .await?;
                if let Some(firing) = in_flight.get(&event.input) {
                    firing.force_release_stuck(injector).await;
                }
                return Ok(());
            }

            let profile = config
                .active_profile()
                .expect("load_or_seed validates active_profile names a real profile");
            let keys = chord_keys_containing(profile.chords(active_layer), event.input);
            for key in keys {
                release_chord_firing(injector, &chord_state.in_flight, &key).await;
            }
            Ok(())
        }
    }
}

/// Runs when the active Chord window's deadline elapses with members still
/// unresolved (ticket 01's Answer): every Input still in `down` never
/// completed a Chord, so each fires its own individual Binding retroactively
/// — delayed by the window, exactly as designed, not a bug. Members already
/// claimed by a fired Chord were removed from `down` when they fired (see
/// `handle_chord_event`'s `Down` arm), so this only ever touches ones that
/// are genuinely still pending.
#[allow(clippy::too_many_arguments)]
async fn handle_chord_timeout(
    injector: &Injector,
    config: &mut Config,
    config_path: &Path,
    active_layer: Layer,
    toggles: &mut HashMap<Input, ActiveToggle>,
    in_flight: &mut HashMap<Input, FiringHandle>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    actuation_tx: &watch::Sender<HashMap<Input, ActuationPoint>>,
    signal_emitter: &Option<SignalEmitter<'static>>,
    chord_state: &mut ChordState,
    axis_state: &mut AxisState,
    analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>,
    toggle_lap_target: Duration,
) -> io::Result<()> {
    let Some(window) = chord_state.window.take() else {
        return Ok(());
    };
    for input in window.down {
        chord_state.claimed.remove(&input);
        fire_individual_retroactively(
            injector,
            config,
            config_path,
            toggles,
            in_flight,
            stepper_cursors,
            actuation_tx,
            axis_state,
            analog_repeats,
            signal_emitter,
            active_layer,
            input,
            toggle_lap_target,
        )
        .await?;
    }
    Ok(())
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

/// Shared by `SetActuationPoint`/`ClearActuationPoint` (ticket 17 §3) and
/// `SetAxisAssignment` (ticket 59 §1): both an actuation point and an Axis
/// assignment are properties of a physical Grid key's Depth, so setting or
/// clearing either on any other `Input` variant is rejected. `what` names
/// the caller's own concept (e.g. `"actuation points"`, `"Axis assignments"`)
/// so the error text stays specific to what was actually being set, rather
/// than every caller sharing one hardcoded noun.
fn reject_non_grid_input(input: Input, what: &str) -> Result<(), CommandError> {
    if matches!(input, Input::Grid(_, _)) {
        Ok(())
    } else {
        Err(CommandError::InvalidRequest(format!(
            "{what} can only be set on Grid Inputs"
        )))
    }
}

/// Shared by `SetActuationPoint`/`SetDefaultActuation`: the hysteresis
/// invariant ticket 17 §2 asks for — a Release point at or above its
/// Actuation point defeats hysteresis entirely rather than merely
/// narrowing it. Ticket 22's `capture::analog::observe` is what actually
/// consumes these points against a live Depth stream: at `release ==
/// actuation`, a key held at a perfectly steady Depth crosses both
/// thresholds on every single report, chattering Down/Up forever on a
/// motionless key (code-review finding on ticket 22 — this check
/// pre-existed from ticket 21, but only became exploitable once ticket 22
/// gave it a real consumer).
fn reject_release_above_actuation(actuation: u8, release: u8) -> Result<(), CommandError> {
    if release < actuation {
        Ok(())
    } else {
        Err(CommandError::InvalidRequest(
            "release point must be strictly below the actuation point".to_string(),
        ))
    }
}

/// Shared by `SetBinding`/`SetChordBinding` (ticket 40): rejects a Binding
/// whose Action/Trigger combination is structurally disallowed —
/// `ProfileSwitch` paired with anything but Fire-once, a `ControllerButton`
/// naming a non-gamepad code, a `Macro`/`Step` naming an unknown library
/// entry, or a `Step` paired with Toggle. A Chord Binding is "just a Binding
/// keyed by a Set<Input>" (ticket 01's Answer), so it's held to the exact
/// same rules rather than a second, drifting copy of them.
fn validate_binding(binding: &Binding, config: &Config) -> Result<(), CommandError> {
    if matches!(binding.action, Action::ProfileSwitch { .. })
        && binding.trigger != TriggerMode::FireOnce
    {
        return Err(CommandError::InvalidRequest(
            "a Profile Switch Binding must use Fire-once".to_string(),
        ));
    }
    if let Action::ControllerButton { button } = binding.action {
        if !crate::input::is_gamepad_button(button) {
            return Err(CommandError::InvalidRequest(format!(
                "{button:?} is not a valid gamepad button"
            )));
        }
        if binding.trigger == TriggerMode::FireOnce {
            return Err(CommandError::InvalidRequest(
                "Fire-once is not allowed for a Controller Button Binding".to_string(),
            ));
        }
    }
    if let Action::Macro { macro_id } = &binding.action
        && !config.macros.contains_key(macro_id)
    {
        return Err(CommandError::InvalidRequest(format!(
            "{macro_id:?} does not name a Macro in the library"
        )));
    }
    if let Action::Step { stepper, .. } = &binding.action {
        if !config.steppers.contains_key(stepper) {
            return Err(CommandError::InvalidRequest(format!(
                "{stepper:?} does not name a Stepper in the library"
            )));
        }
        if binding.trigger == TriggerMode::Toggle {
            return Err(CommandError::InvalidRequest(
                "Toggle is not allowed for a Stepper Binding".to_string(),
            ));
        }
    }
    Ok(())
}

/// Rejects a `StepperItem::ControllerButton` list item naming a non-gamepad
/// code (ticket 92) — the `CreateStepper`/`SetStepperItems` counterpart of
/// `validate_binding`'s `Action::ControllerButton` allowlist guard, and of
/// `config::parse`'s `InvalidControllerButtonStepperItem` check for a
/// hand-edited `config.toml`. A GUI-emitted item is always valid (the
/// picker only ever produces allowlist codes); this catches a hand-crafted
/// D-Bus call, mirroring the two-place enforcement `Action::ControllerButton`
/// already has (ticket 43).
fn validate_stepper_items(items: &[StepperItem]) -> Result<(), CommandError> {
    for item in items {
        if let StepperItem::ControllerButton { button } = item
            && !crate::input::is_gamepad_button(*button)
        {
            return Err(CommandError::InvalidRequest(format!(
                "{button:?} is not a valid gamepad button"
            )));
        }
    }
    Ok(())
}

/// Whether `key`'s member set is a subset or superset of any *other*
/// existing Chord's on `chords` (ticket 01's amended Answer) — the only
/// case that stays genuinely ambiguous once an Input may belong to any
/// number of Chords: completing the smaller one is indistinguishable from
/// being partway into the larger one. Editing the exact same member set
/// back (`key` already present in `chords`) is not a conflict with itself.
fn chord_conflict(chords: &HashMap<ChordKey, Binding>, key: &ChordKey) -> Option<ChordKey> {
    chords
        .keys()
        .find(|other| {
            *other != key
                && (key.members().is_subset(other.members())
                    || other.members().is_subset(key.members()))
        })
        .cloned()
}

/// Whether `input` already carries an Axis assignment on `layer` (ticket
/// 59 §2's mutual exclusion) — `SetBinding`/`SetChordBinding` both reject a
/// grid key already Axis-assigned there with a specific error rather than
/// silently overwriting it, the mirror image of `SetAxisAssignment`'s own
/// atomic steal-from-Binding/Chord-membership behavior.
fn axis_conflict(profile: &Profile, layer: Layer, input: Input) -> bool {
    profile.axis_layer(layer).contains_key(&input)
}

/// The `Default` Profile always exists — `load_or_seed` (issue 11) refuses
/// to start a `Config` whose `active_profile` doesn't name a real Profile.
fn active_profile_mut(config: &mut Config) -> &mut Profile {
    config
        .active_profile_mut()
        .expect("load_or_seed validates active_profile names a real profile")
}

/// Republishes the active Profile's resolved Actuation-point snapshot
/// (ticket 18 §5) — called after every successful mutation that can change
/// it: `SetActuationPoint`/`ClearActuationPoint`/`SetDefaultActuation`/
/// `ResetActuationPoints` (all touch the active Profile's own
/// `actuation_overrides`/`default_actuation`) and `SwitchProfile` (changes
/// which Profile is active). `send_replace` rather than `send`: this must
/// not fail just because no `AnalogCaptureSource` grid task has subscribed
/// yet (ticket 23 wires the real receiver; today's tests hold one only to
/// keep the channel open).
fn publish_actuation_snapshot(
    config: &Config,
    actuation_tx: &watch::Sender<HashMap<Input, ActuationPoint>>,
) {
    let profile = config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile");
    actuation_tx.send_replace(profile.resolved_actuation_points());
}

/// Shared by `Command::SwitchProfile` and firing an `Action::ProfileSwitch`
/// Binding (ticket 34): switches `config.active_profile`, persists, and (on
/// success) force-stops every active Toggle and republishes the new
/// Profile's Actuation-point snapshot — the same effects `SwitchProfile`
/// always had. Reply/`ActiveProfileChanged` handling deliberately stays with
/// each caller: `Command::SwitchProfile` has a D-Bus reply to send (and its
/// own reply-before-signal reentrancy-hazard ordering), while a Binding
/// firing has no reply at all. Self-reference (`name` already active) is not
/// special-cased — it still persists, force-stops Toggles, and republishes,
/// an intentional no-op-except-for-Toggles per ticket 05's design.
#[allow(clippy::too_many_arguments)]
async fn switch_profile(
    injector: &Injector,
    config: &mut Config,
    config_path: &Path,
    toggles: &mut HashMap<Input, ActiveToggle>,
    actuation_tx: &watch::Sender<HashMap<Input, ActuationPoint>>,
    axis_state: &mut AxisState,
    analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>,
    name: String,
) -> Result<(), CommandError> {
    if !config.profiles.contains_key(&name) {
        return Err(CommandError::NotFound);
    }
    let previous = std::mem::replace(&mut config.active_profile, name);
    let result = persist(config, config_path).await;
    if result.is_err() {
        config.active_profile = previous;
    } else {
        stop_all_toggles(toggles).await;
        publish_actuation_snapshot(config, actuation_tx);
        // Ticket 71: the new Profile's Axis-assignment map generally
        // differs from the old one — same reset-on-switch reasoning as
        // `handle_layer_switch`'s own call, just for a Profile switch
        // instead of a Layer one.
        let _ = reset_axis_outputs(injector, axis_state).await;
        // Ticket 39: same reasoning, for the same reason, for every
        // Analog-repeat task — the new Profile's Bindings generally differ
        // from the old one's.
        stop_all_analog_repeats(analog_repeats).await;
    }
    result
}

/// Every `Action::ProfileSwitch { target }` across every Profile's Base/Held
/// Binding map that targets `old_name` is repointed at `new_name` (ticket
/// 34) — a rename must not silently leave a dangling or wrong reference
/// behind. Chords don't exist in code yet (ticket 01 is still a design
/// ticket, not built), so only `base`/`held` are scanned.
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
/// while this is true, so a dangling reference can never exist (ticket 15/
/// 51/40), mirroring `profile_switch_references`'s identical shape.
/// `profile_switch_references` itself doesn't need this same widening:
/// `chords_base`/`chords_held` can never contain a `ProfileSwitch` Action
/// at all (`SetChordBinding`/`parse` both refuse it — see
/// `ConfigError::InvalidChordProfileSwitch`), but Macro/Step Actions are
/// fully allowed on a Chord, so a Chord referencing a since-deleted library
/// entry is exactly as reachable as an ordinary Binding's (code-review
/// finding: the original version of this function only scanned `base`/
/// `held`, letting `DeleteMacro`/`DeleteStepper` leave a dangling Chord
/// reference that then failed `load_or_seed`'s own validation — which does
/// scan Chords — on the next Daemon restart).
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
/// refuses while this is true, so a dangling reference can never exist
/// (ticket 03/54/40).
fn stepper_references(config: &Config, stepper_id: &StepperId) -> bool {
    config.profiles.values().any(|profile| {
        config::profile_all_bindings(profile)
            .any(|binding| matches!(&binding.action, Action::Step { stepper, .. } if stepper == stepper_id))
    })
}

/// Removes every other Binding, across every Profile/Layer, whose `Action`
/// is `Action::Step { stepper, direction }` matching the one `SetBinding` is
/// about to set — ticket 03's Answer: "assigning it to a new pair silently
/// moves it off its old one," no reject-at-save step, since at most one
/// Input may ever carry a given (stepper, direction) at a time. `except`
/// (the Input `SetBinding` is currently writing) is left untouched even if
/// it already matches, so re-saving the same Input's own trigger mode isn't
/// mistaken for a conflicting second owner — `None` (used by
/// `SetChordBinding`, which has no ordinary-Input identity of its own to
/// exclude) steals from every matching Input unconditionally. Returns what
/// was removed so the caller can restore it if the persist that follows
/// fails, mirroring every other mutating Command's rollback-on-failure
/// discipline. `take_stepper_direction_elsewhere_from_chords` is this
/// function's exact mirror for a Chord's own Step action (ticket 40) — a
/// Chord is exactly as exclusive an owner of (stepper, direction) as an
/// ordinary Binding, so both keyspaces must be swept together whenever
/// either kind of caller claims one.
fn take_stepper_direction_elsewhere(
    config: &mut Config,
    stepper: &StepperId,
    direction: StepDirection,
    except: Option<(&str, Layer, Input)>,
) -> Vec<(String, Layer, Input, Binding)> {
    let mut removed = Vec::new();
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
                if let Some(binding) = bindings.remove(&input) {
                    removed.push((profile_name.clone(), layer, input, binding));
                }
            }
        }
    }
    removed
}

/// `take_stepper_direction_elsewhere`'s exact mirror for a Profile's Chord
/// Bindings (ticket 40) — see its doc comment. `except` is the `ChordKey`
/// `SetChordBinding` is currently writing, left untouched even if it
/// already matches (re-saving an existing Step-action Chord's own Trigger
/// mode isn't a conflicting second owner); `None` (used by `SetBinding`,
/// which has no `ChordKey` identity of its own to exclude) steals from
/// every matching Chord unconditionally.
fn take_stepper_direction_elsewhere_from_chords(
    config: &mut Config,
    stepper: &StepperId,
    direction: StepDirection,
    except: Option<(&str, Layer, &ChordKey)>,
) -> Vec<(String, Layer, ChordKey, Binding)> {
    let mut removed = Vec::new();
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
                if let Some(binding) = chords.remove(&key) {
                    removed.push((profile_name.clone(), layer, key, binding));
                }
            }
        }
    }
    removed
}

#[allow(clippy::too_many_arguments)]
async fn handle_command(
    injector: &Injector,
    config: &mut Config,
    config_path: &Path,
    toggles: &mut HashMap<Input, ActiveToggle>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    axis_state: &mut AxisState,
    analog_repeats: &mut HashMap<Input, ActiveAnalogRepeat>,
    active_layer: &Layer,
    device_connected: bool,
    capture_mode: CaptureMode,
    device_info: Option<&DeviceInfo>,
    signal_emitter: &Option<SignalEmitter<'static>>,
    actuation_tx: &watch::Sender<HashMap<Input, ActuationPoint>>,
    capture_control_tx: &mpsc::Sender<bool>,
    cmd: Command,
) {
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
                .map(|id| (id.clone(), stepper_cursors.get(id).copied().unwrap_or(0)))
                .collect();
            let _ = reply.send(State {
                profile: config.active_profile.clone(),
                layer: active_layer.as_str(),
                active_toggles: toggles.keys().copied().collect(),
                device_connected,
                capture_mode: capture_mode.as_str(),
                daemon_version: crate::VERSION,
                firmware_version: device_info.map(|info| info.firmware_version.clone()),
                serial_number: device_info.map(|info| info.serial_number.clone()),
                stepper_cursors,
            });
        }
        Command::SetBinding {
            input,
            layer,
            binding,
            reply,
        } => {
            if let Err(err) = validate_binding(&binding, config) {
                let _ = reply.send(Err(err));
                return;
            }
            if binding.trigger == TriggerMode::AnalogRepeat && !matches!(input, Input::Grid(_, _)) {
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "Analog-repeat is only valid on Grid Inputs".to_string(),
                )));
                return;
            }
            let profile = config
                .active_profile()
                .expect("load_or_seed validates active_profile names a real profile");
            if axis_conflict(profile, layer, input) {
                let _ = reply.send(Err(CommandError::InvalidRequest(format!(
                    "{input} already has an Axis assignment on this Layer — clear it first"
                ))));
                return;
            }
            // Ticket 03's Answer: assigning a Stepper list to a new Input
            // silently moves it off its old one — no reject-at-save step,
            // since at most one Input *or Chord* may carry a given
            // (stepper, direction) at a time (ticket 40 widened this
            // invariant to cover Chords too — a Step-action Chord is just
            // as exclusive an owner). Collected before the target insert
            // below so all three can roll back together on a persist
            // failure.
            let (moved_stepper_bindings, moved_stepper_chord_bindings) =
                if let Action::Step { stepper, direction } = &binding.action {
                    let active_profile = config.active_profile.clone();
                    (
                        take_stepper_direction_elsewhere(
                            config,
                            stepper,
                            *direction,
                            Some((&active_profile, layer, input)),
                        ),
                        take_stepper_direction_elsewhere_from_chords(
                            config, stepper, *direction, None,
                        ),
                    )
                } else {
                    (Vec::new(), Vec::new())
                };
            let previous = active_profile_mut(config)
                .layer_mut(layer)
                .insert(input, binding);
            let result = persist(config, config_path).await;
            if result.is_err() {
                // config.toml on disk must always match in-memory state
                // (spec.md's config lifecycle) — roll the in-memory edit
                // back rather than let GetConfig lie about what's saved.
                let bindings = active_profile_mut(config).layer_mut(layer);
                match previous {
                    Some(prev) => {
                        bindings.insert(input, prev);
                    }
                    None => {
                        bindings.remove(&input);
                    }
                }
                for (profile_name, moved_layer, moved_input, moved_binding) in
                    moved_stepper_bindings
                {
                    if let Some(profile) = config.profiles.get_mut(&profile_name) {
                        profile
                            .layer_mut(moved_layer)
                            .insert(moved_input, moved_binding);
                    }
                }
                for (profile_name, moved_layer, moved_key, moved_binding) in
                    moved_stepper_chord_bindings
                {
                    if let Some(profile) = config.profiles.get_mut(&profile_name) {
                        profile
                            .chords_mut(moved_layer)
                            .insert(moved_key, moved_binding);
                    }
                }
            }
            let _ = reply.send(result);
        }
        Command::ClearBinding {
            input,
            layer,
            reply,
        } => {
            let Some(previous) = active_profile_mut(config).layer_mut(layer).remove(&input) else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let result = persist(config, config_path).await;
            if result.is_err() {
                active_profile_mut(config)
                    .layer_mut(layer)
                    .insert(input, previous);
            }
            let _ = reply.send(result);
        }
        Command::SetModeKeyRole { role, reply } => {
            let previous = std::mem::replace(&mut active_profile_mut(config).mode_key_role, role);
            let result = persist(config, config_path).await;
            if result.is_err() {
                active_profile_mut(config).mode_key_role = previous;
            } else if role == ModeKeyRole::LayerSwitch {
                // Leaving `Bound`: a Toggle can only ever have been started
                // on the Mode key while `Bound` (it's the only role that
                // ever runs it through Trigger-mode dispatch). Once
                // `LayerSwitch` takes over, `handle_event` intercepts every
                // `Input::ModeKey` press before it ever reaches the
                // stop-toggle check — so without this, a still-running
                // Toggle on the Mode key would become permanently
                // unstoppable via that key (code review finding).
                if let Some(toggle) = toggles.remove(&Input::ModeKey) {
                    toggle.stop().await;
                }
            }
            let _ = reply.send(result);
        }
        Command::CreateProfile { name, reply } => {
            if name.trim().is_empty() {
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "Profile name can't be empty".to_string(),
                )));
                return;
            }
            if config.profiles.contains_key(&name) {
                let _ = reply.send(Err(CommandError::AlreadyExists));
                return;
            }
            config.profiles.insert(name.clone(), Profile::default());
            let result = persist(config, config_path).await;
            if result.is_err() {
                config.profiles.remove(&name);
            }
            let _ = reply.send(result);
        }
        Command::DeleteProfile { name, reply } => {
            if name == config.active_profile {
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "cannot delete the active Profile".to_string(),
                )));
                return;
            }
            if profile_switch_references(config, &name) {
                let _ = reply.send(Err(CommandError::InvalidRequest(format!(
                    "Profile {name:?} is still referenced by a Profile Switch Binding"
                ))));
                return;
            }
            let Some(previous) = config.profiles.remove(&name) else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let result = persist(config, config_path).await;
            if result.is_err() {
                config.profiles.insert(name, previous);
            }
            let _ = reply.send(result);
        }
        Command::RenameProfile {
            old_name,
            new_name,
            reply,
        } => {
            if new_name.trim().is_empty() {
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "Profile name can't be empty".to_string(),
                )));
                return;
            }
            if !config.profiles.contains_key(&old_name) {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            }
            if old_name != new_name && config.profiles.contains_key(&new_name) {
                let _ = reply.send(Err(CommandError::AlreadyExists));
                return;
            }
            if old_name == new_name {
                let _ = reply.send(Ok(()));
                return;
            }
            // Snapshotting the whole map (rather than just the renamed
            // Profile, as before ticket 34) is what makes the cascade below
            // cleanly reversible on a persist failure — any other Profile's
            // Bindings can now change too, not just the renamed entry.
            let previous_profiles = config.profiles.clone();
            let previous_active = config.active_profile.clone();

            let profile = config
                .profiles
                .remove(&old_name)
                .expect("just checked old_name exists");
            config.profiles.insert(new_name.clone(), profile);
            if config.active_profile == old_name {
                config.active_profile = new_name.clone();
            }
            cascade_rename_profile_switch_targets(config, &old_name, &new_name);

            let result = persist(config, config_path).await;
            if result.is_err() {
                config.profiles = previous_profiles;
                config.active_profile = previous_active;
            }
            let _ = reply.send(result);
        }
        Command::SwitchProfile { name, reply } => {
            let result = switch_profile(
                injector,
                config,
                config_path,
                toggles,
                actuation_tx,
                axis_state,
                analog_repeats,
                name.clone(),
            )
            .await;
            let succeeded = result.is_ok();
            // The reply is sent *before* the signal, deliberately: the
            // caller's own SwitchProfile call is typically a blocking D-Bus
            // round-trip (e.g. the GUI's `call_sync`) still waiting on this
            // very reply. Emitting ActiveProfileChanged first would let that
            // caller's own subscribed signal handler fire while its
            // SwitchProfile call is still unresolved, on the same
            // connection — a reentrancy hazard (a synchronous callback
            // nested inside a synchronous call already in flight) that
            // doesn't exist for ActiveLayerChanged, since nothing there is
            // emitted as the direct, immediate side effect of the very call
            // that's still awaiting its own reply.
            let _ = reply.send(result);
            if succeeded && let Some(emitter) = signal_emitter {
                let _ = Daemon::active_profile_changed(emitter, &name).await;
            }
        }
        Command::StopAllToggles { reply } => {
            stop_all_toggles(toggles).await;
            let _ = reply.send(());
        }
        Command::SetActuationPoint {
            input,
            actuation,
            release,
            reply,
        } => {
            if let Err(err) = reject_non_grid_input(input, "actuation points") {
                let _ = reply.send(Err(err));
                return;
            }
            if let Err(err) = reject_release_above_actuation(actuation, release) {
                let _ = reply.send(Err(err));
                return;
            }
            let point = ActuationPoint { actuation, release };
            let previous = active_profile_mut(config)
                .actuation_overrides
                .insert(input, point);
            let result = persist(config, config_path).await;
            if result.is_err() {
                let overrides = &mut active_profile_mut(config).actuation_overrides;
                match previous {
                    Some(prev) => {
                        overrides.insert(input, prev);
                    }
                    None => {
                        overrides.remove(&input);
                    }
                }
            } else {
                publish_actuation_snapshot(config, actuation_tx);
            }
            let _ = reply.send(result);
        }
        Command::ClearActuationPoint { input, reply } => {
            if let Err(err) = reject_non_grid_input(input, "actuation points") {
                let _ = reply.send(Err(err));
                return;
            }
            let previous = active_profile_mut(config)
                .actuation_overrides
                .remove(&input);
            let result = persist(config, config_path).await;
            if result.is_err() {
                if let Some(prev) = previous {
                    active_profile_mut(config)
                        .actuation_overrides
                        .insert(input, prev);
                }
            } else {
                publish_actuation_snapshot(config, actuation_tx);
            }
            let _ = reply.send(result);
        }
        Command::SetDefaultActuation {
            actuation,
            release,
            reply,
        } => {
            if let Err(err) = reject_release_above_actuation(actuation, release) {
                let _ = reply.send(Err(err));
                return;
            }
            let previous = std::mem::replace(
                &mut active_profile_mut(config).default_actuation,
                ActuationPoint { actuation, release },
            );
            let result = persist(config, config_path).await;
            if result.is_err() {
                active_profile_mut(config).default_actuation = previous;
            } else {
                publish_actuation_snapshot(config, actuation_tx);
            }
            let _ = reply.send(result);
        }
        Command::ResetActuationPoints { reply } => {
            let previous = std::mem::take(&mut active_profile_mut(config).actuation_overrides);
            let result = persist(config, config_path).await;
            if result.is_err() {
                active_profile_mut(config).actuation_overrides = previous;
            } else {
                publish_actuation_snapshot(config, actuation_tx);
            }
            let _ = reply.send(result);
        }
        Command::SetForceDigital { force, reply } => {
            let previous = std::mem::replace(&mut config.force_digital, force);
            let result = persist(config, config_path).await;
            if result.is_err() {
                config.force_digital = previous;
            } else {
                // Tells the supervisor (ticket 23) to actually swap the live
                // capture source — only on a successful persist, matching
                // every other mutating Command's "config.toml on disk always
                // matches in-memory state" discipline.
                let _ = capture_control_tx.send(force).await;
            }
            let _ = reply.send(result);
        }
        Command::CreateMacro { name, steps, reply } => {
            if name.trim().is_empty() {
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "Macro name can't be empty".to_string(),
                )));
                return;
            }
            let macro_id = config::unique_macro_id(config, &name);
            config
                .macros
                .insert(macro_id.clone(), config::MacroDef { name, steps });
            let result = persist(config, config_path).await;
            if result.is_err() {
                config.macros.remove(&macro_id);
            }
            let _ = reply.send(result.map(|()| macro_id));
        }
        Command::RenameMacro {
            macro_id,
            new_name,
            reply,
        } => {
            if new_name.trim().is_empty() {
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "Macro name can't be empty".to_string(),
                )));
                return;
            }
            let Some(def) = config.macros.get_mut(&macro_id) else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let previous = std::mem::replace(&mut def.name, new_name);
            let result = persist(config, config_path).await;
            if result.is_err() {
                config
                    .macros
                    .get_mut(&macro_id)
                    .expect("just written above")
                    .name = previous;
            }
            let _ = reply.send(result);
        }
        Command::DeleteMacro { macro_id, reply } => {
            if macro_references(config, &macro_id) {
                let _ = reply.send(Err(CommandError::InvalidRequest(format!(
                    "{macro_id:?} is still referenced by a Macro Binding"
                ))));
                return;
            }
            let Some(previous) = config.macros.remove(&macro_id) else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let result = persist(config, config_path).await;
            if result.is_err() {
                config.macros.insert(macro_id, previous);
            }
            let _ = reply.send(result);
        }
        Command::SetMacroSteps {
            macro_id,
            steps,
            reply,
        } => {
            let Some(def) = config.macros.get_mut(&macro_id) else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let previous = std::mem::replace(&mut def.steps, steps);
            let result = persist(config, config_path).await;
            if result.is_err() {
                config
                    .macros
                    .get_mut(&macro_id)
                    .expect("just written above")
                    .steps = previous;
            }
            let _ = reply.send(result);
        }
        Command::CreateStepper { name, items, reply } => {
            if name.trim().is_empty() {
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "Stepper name can't be empty".to_string(),
                )));
                return;
            }
            if let Err(err) = validate_stepper_items(&items) {
                let _ = reply.send(Err(err));
                return;
            }
            let stepper_id = config::unique_stepper_id(config, &name);
            config
                .steppers
                .insert(stepper_id.clone(), config::StepperDef { name, items });
            let result = persist(config, config_path).await;
            if result.is_err() {
                config.steppers.remove(&stepper_id);
            }
            let _ = reply.send(result.map(|()| stepper_id));
        }
        Command::RenameStepper {
            stepper_id,
            new_name,
            reply,
        } => {
            if new_name.trim().is_empty() {
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "Stepper name can't be empty".to_string(),
                )));
                return;
            }
            let Some(def) = config.steppers.get_mut(&stepper_id) else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let previous = std::mem::replace(&mut def.name, new_name);
            let result = persist(config, config_path).await;
            if result.is_err() {
                config
                    .steppers
                    .get_mut(&stepper_id)
                    .expect("just written above")
                    .name = previous;
            }
            let _ = reply.send(result);
        }
        Command::DeleteStepper { stepper_id, reply } => {
            if stepper_references(config, &stepper_id) {
                let _ = reply.send(Err(CommandError::InvalidRequest(format!(
                    "{stepper_id:?} is still referenced by a Step Binding"
                ))));
                return;
            }
            let Some(previous) = config.steppers.remove(&stepper_id) else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let result = persist(config, config_path).await;
            if result.is_err() {
                config.steppers.insert(stepper_id, previous);
            } else {
                // The runtime cursor is Daemon-side-only state, not part of
                // `config`/`persist` — dropped here, on a successful delete,
                // so a later `CreateStepper` that happens to land on the
                // same freed slug (`unique_stepper_id` reassigns it once
                // nothing occupies it) starts at the list's first item
                // rather than inheriting a stale position from the deleted
                // entry (code-review finding).
                stepper_cursors.remove(&stepper_id);
            }
            let _ = reply.send(result);
        }
        Command::SetStepperItems {
            stepper_id,
            items,
            reply,
        } => {
            if let Err(err) = validate_stepper_items(&items) {
                let _ = reply.send(Err(err));
                return;
            }
            let Some(def) = config.steppers.get_mut(&stepper_id) else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let previous = std::mem::replace(&mut def.items, items);
            let new_len = config.steppers[&stepper_id].items.len();
            let result = persist(config, config_path).await;
            if result.is_err() {
                config
                    .steppers
                    .get_mut(&stepper_id)
                    .expect("just written above")
                    .items = previous;
            } else if new_len == 0 {
                // Nothing left to point at — dropping the entry lets
                // `resolve_step`'s zero-items short-circuit and `GetState`'s
                // own default both agree on "index 0" for free, the same
                // convention a never-yet-stepped cursor already uses.
                stepper_cursors.remove(&stepper_id);
            } else {
                // A shrink can leave a stored cursor pointing past the new
                // end — clamped here (mirroring `resolve_step`'s own
                // `.min(len - 1)` guard) so `GetState`'s reported position
                // never outruns the list it's a position *in*, even before
                // this Stepper is next fired (code-review finding).
                if let Some(cursor) = stepper_cursors.get_mut(&stepper_id) {
                    *cursor = (*cursor).min(new_len - 1);
                }
            }
            let _ = reply.send(result);
        }
        Command::SetChordBinding {
            inputs,
            layer,
            binding,
            reply,
        } => {
            if inputs.len() < 2 {
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "a Chord needs at least two member Inputs".to_string(),
                )));
                return;
            }
            if matches!(binding.action, Action::ProfileSwitch { .. }) {
                // See `ConfigError::InvalidChordProfileSwitch` — a Chord's
                // own Action can never be ProfileSwitch, since
                // `fire_chord`/`compile_action` have no `&mut Config` to
                // actually run a switch through (unlike a Chord *member*'s
                // own individual Binding, which can be anything — see
                // `fire_individual_retroactively`).
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "a Chord's Binding can't be a Profile Switch".to_string(),
                )));
                return;
            }
            if binding.trigger == TriggerMode::AnalogRepeat {
                // See `ConfigError::InvalidChordAnalogRepeat` — a Chord
                // fires on a discrete member-set completion, not a single
                // grid key's continuous Depth, same "no coherent runtime
                // owner" reasoning as the Profile Switch rejection above.
                let _ = reply.send(Err(CommandError::InvalidRequest(
                    "a Chord's Binding can't use Analog-repeat".to_string(),
                )));
                return;
            }
            if let Err(err) = validate_binding(&binding, config) {
                let _ = reply.send(Err(err));
                return;
            }
            let profile = config
                .active_profile()
                .expect("load_or_seed validates active_profile names a real profile");
            if let Some(&axis_input) = inputs.iter().find(|&&i| axis_conflict(profile, layer, i)) {
                let _ = reply.send(Err(CommandError::InvalidRequest(format!(
                    "{axis_input} already has an Axis assignment on this Layer — clear it first"
                ))));
                return;
            }
            let key = ChordKey::new(inputs);
            if let Some(conflicting) =
                chord_conflict(active_profile_mut(config).chords(layer), &key)
            {
                let _ = reply.send(Err(CommandError::InvalidRequest(format!(
                    "conflicts with the existing Chord {conflicting}: one member set fully contains the other"
                ))));
                return;
            }
            // Ticket 40: a Step-action Chord is exactly as exclusive an
            // owner of (stepper, direction) as an ordinary Binding's
            // (ticket 03's Answer) — steal it from wherever else it
            // currently lives, in either keyspace, mirroring `SetBinding`'s
            // own handler.
            let (moved_stepper_bindings, moved_stepper_chord_bindings) =
                if let Action::Step { stepper, direction } = &binding.action {
                    let active_profile = config.active_profile.clone();
                    (
                        take_stepper_direction_elsewhere(config, stepper, *direction, None),
                        take_stepper_direction_elsewhere_from_chords(
                            config,
                            stepper,
                            *direction,
                            Some((&active_profile, layer, &key)),
                        ),
                    )
                } else {
                    (Vec::new(), Vec::new())
                };
            let previous = active_profile_mut(config)
                .chords_mut(layer)
                .insert(key.clone(), binding);
            let result = persist(config, config_path).await;
            if result.is_err() {
                let chords = active_profile_mut(config).chords_mut(layer);
                match previous {
                    Some(prev) => {
                        chords.insert(key, prev);
                    }
                    None => {
                        chords.remove(&key);
                    }
                }
                for (profile_name, moved_layer, moved_input, moved_binding) in
                    moved_stepper_bindings
                {
                    if let Some(profile) = config.profiles.get_mut(&profile_name) {
                        profile
                            .layer_mut(moved_layer)
                            .insert(moved_input, moved_binding);
                    }
                }
                for (profile_name, moved_layer, moved_key, moved_binding) in
                    moved_stepper_chord_bindings
                {
                    if let Some(profile) = config.profiles.get_mut(&profile_name) {
                        profile
                            .chords_mut(moved_layer)
                            .insert(moved_key, moved_binding);
                    }
                }
            }
            let _ = reply.send(result);
        }
        Command::ClearChordBinding {
            inputs,
            layer,
            reply,
        } => {
            let key = ChordKey::new(inputs);
            let Some(previous) = active_profile_mut(config).chords_mut(layer).remove(&key) else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let result = persist(config, config_path).await;
            if result.is_err() {
                active_profile_mut(config)
                    .chords_mut(layer)
                    .insert(key, previous);
            }
            let _ = reply.send(result);
        }
        Command::SetAxisAssignment {
            input,
            layer,
            target,
            reply,
        } => {
            if let Err(err) = reject_non_grid_input(input, "Axis assignments") {
                let _ = reply.send(Err(err));
                return;
            }
            // Ticket 59 §2's mutual exclusion: atomically clears any
            // existing Binding *and* any Chord membership for (layer,
            // input) alongside the insert, mirroring `SetBinding`'s own
            // atomic-persist precedent — unlike `SetBinding`/
            // `SetChordBinding` (see `axis_conflict` below), which reject
            // rather than silently steal from an existing Axis assignment.
            let previous_binding = active_profile_mut(config).layer_mut(layer).remove(&input);
            let removed_chords: Vec<(ChordKey, Binding)> = {
                let chords = active_profile_mut(config).chords_mut(layer);
                let keys: Vec<ChordKey> = chords
                    .keys()
                    .filter(|key| key.members().contains(&input))
                    .cloned()
                    .collect();
                keys.into_iter()
                    .filter_map(|key| chords.remove(&key).map(|binding| (key, binding)))
                    .collect()
            };
            let previous_axis = active_profile_mut(config)
                .axis_layer_mut(layer)
                .insert(input, target);
            let result = persist(config, config_path).await;
            if result.is_err() {
                let axis_map = active_profile_mut(config).axis_layer_mut(layer);
                match previous_axis {
                    Some(prev) => {
                        axis_map.insert(input, prev);
                    }
                    None => {
                        axis_map.remove(&input);
                    }
                }
                let chords = active_profile_mut(config).chords_mut(layer);
                for (key, binding) in removed_chords {
                    chords.insert(key, binding);
                }
                if let Some(binding) = previous_binding {
                    active_profile_mut(config)
                        .layer_mut(layer)
                        .insert(input, binding);
                }
            } else if layer == *active_layer {
                let axis_map = active_profile_mut(config).axis_layer(layer).clone();
                let _ = recompute_and_emit_axes(injector, axis_state, &axis_map).await;
            }
            let _ = reply.send(result);
        }
        Command::ClearAxisAssignment {
            input,
            layer,
            reply,
        } => {
            let Some(previous) = active_profile_mut(config)
                .axis_layer_mut(layer)
                .remove(&input)
            else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let result = persist(config, config_path).await;
            if result.is_err() {
                active_profile_mut(config)
                    .axis_layer_mut(layer)
                    .insert(input, previous);
            } else {
                axis_state.contributions.remove(&input);
                if layer == *active_layer {
                    let axis_map = active_profile_mut(config).axis_layer(layer).clone();
                    let _ = recompute_and_emit_axes(injector, axis_state, &axis_map).await;
                }
            }
            let _ = reply.send(result);
        }
    }
}

/// Force-stops every currently running Toggle — shared by `SwitchProfile`
/// (as part of switching) and `StopAllToggles` (ticket 25, on its own,
/// GUI-focus-gain triggered) so the drain-and-stop loop has exactly one
/// implementation.
async fn stop_all_toggles(toggles: &mut HashMap<Input, ActiveToggle>) {
    for (_, toggle) in toggles.drain() {
        toggle.stop().await;
    }
}

/// Rewrites `config.toml` off the async worker pool: `config::write` is a
/// synchronous `std::fs` call, and running it inline on the dispatch task
/// would stall every queued `PhysicalEvent` behind it for the write's
/// duration — perceptible input lag in a daemon whose whole job is
/// low-latency key remapping.
async fn persist(config: &Config, config_path: &Path) -> Result<(), CommandError> {
    let config = config.clone();
    let config_path = config_path.to_path_buf();
    tokio::task::spawn_blocking(move || config::write(&config_path, &config))
        .await
        .expect("the config::write blocking task must not panic")
        .map_err(CommandError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::fake::FakeCaptureSource;
    use crate::capture::{CaptureSource, EventState};
    use crate::config::{Action, ActuationPoint, DEFAULT_PROFILE_NAME, MacroStepDto, Modifiers};
    use crate::injector::testing::RecordingSink;
    use crate::injector::{self};
    use crate::input::{Direction, WheelEvent};
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
    async fn hold_to_repeat_chord_mouse_button_ignores_repeat_and_releases_on_member_up() {
        // Ticket 79/80's Chord blast radius: the same treatment applies
        // uniformly when a Chord's own Action is a mouse-button Keypress,
        // mirrors `hold_to_repeat_chord_controller_button_ignores_repeat_
        // and_releases_on_member_up`.
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

        // Exactly one KeyDown (the completing Down) and one KeyUp (the
        // first member's physical Up) — both members' Repeats produced
        // nothing.
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

        async fn clear_chord_binding(
            &self,
            inputs: impl IntoIterator<Item = Input>,
            layer: Layer,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::ClearChordBinding {
                    inputs: inputs.into_iter().collect(),
                    layer,
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

        async fn create_profile(&self, name: &str) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::CreateProfile {
                    name: name.to_string(),
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn delete_profile(&self, name: &str) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::DeleteProfile {
                    name: name.to_string(),
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn rename_profile(&self, old_name: &str, new_name: &str) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::RenameProfile {
                    old_name: old_name.to_string(),
                    new_name: new_name.to_string(),
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn create_macro(
            &self,
            name: &str,
            steps: Vec<MacroStepDto>,
        ) -> Result<MacroId, CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::CreateMacro {
                    name: name.to_string(),
                    steps,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn rename_macro(
            &self,
            macro_id: MacroId,
            new_name: &str,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::RenameMacro {
                    macro_id,
                    new_name: new_name.to_string(),
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn delete_macro(&self, macro_id: MacroId) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::DeleteMacro { macro_id, reply })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn set_macro_steps(
            &self,
            macro_id: MacroId,
            steps: Vec<MacroStepDto>,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SetMacroSteps {
                    macro_id,
                    steps,
                    reply,
                })
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

        async fn rename_stepper(
            &self,
            stepper_id: StepperId,
            new_name: &str,
        ) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::RenameStepper {
                    stepper_id,
                    new_name: new_name.to_string(),
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
    async fn a_fire_once_chord_fires_again_after_being_fully_released_and_re_pressed() {
        // Regression test (code-review finding on this ticket's own build):
        // `release_chord` only force-releases a FireOnce/HoldToRepeat
        // Chord's in-flight firing, it never removes the `chord_in_flight`
        // map entry (mirroring `fire`'s own single-Input `in_flight`, which
        // is never cleaned up either) — an earlier version of the
        // completion-detection filter treated bare presence in that map as
        // "still active," which permanently excluded the Chord from ever
        // completing again after its very first firing.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                keypress_binding(evdev::KeyCode::KEY_C),
            )
            .await
            .unwrap();

        for _ in 0..2 {
            harness.press(Input::Grid(1, 1)).await;
            harness.press(Input::Grid(1, 2)).await;
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
            harness.release(Input::Grid(1, 1)).await;
            harness.release(Input::Grid(1, 2)).await;
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
        }

        let batches = harness.shut_down().await;

        // Two full firings — not just one — each a KeyDown/KeyUp pair.
        assert_eq!(batches.len(), 4);
        for pair in batches.chunks(2) {
            let evdev::EventSummary::Key(_, down_code, down_value) = pair[0][0].destructure()
            else {
                panic!("expected a key event");
            };
            let evdev::EventSummary::Key(_, up_code, up_value) = pair[1][0].destructure() else {
                panic!("expected a key event");
            };
            assert_eq!((down_code, down_value), (evdev::KeyCode::KEY_C, 1));
            assert_eq!((up_code, up_value), (evdev::KeyCode::KEY_C, 0));
        }
    }

    #[tokio::test]
    async fn delete_macro_command_rejects_deleting_a_macro_still_referenced_only_by_a_chord() {
        // Regression test: `macro_references` originally only scanned
        // `base`/`held`, so a Macro referenced solely by a Chord's Action
        // could be deleted, leaving `chords_base`/`chords_held` with a
        // dangling `macro_id` that then failed `load_or_seed`'s own
        // validation (which does scan Chords) on the next Daemon restart.
        let (action, macros) = macro_action("test-macro", vec![MacroStepDto::Delay(1)]);
        let macro_id = macros.keys().next().unwrap().clone();
        let harness =
            CommandHarness::spawn(config_with_bindings_and_macros(HashMap::new(), macros));
        harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action,
                },
            )
            .await
            .unwrap();

        let result = harness.delete_macro(macro_id).await;

        assert!(matches!(result, Err(CommandError::InvalidRequest(_))));
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_binding_and_set_chord_binding_each_steal_a_stepper_direction_from_the_other() {
        // Regression test: `SetChordBinding` originally never called
        // `take_stepper_direction_elsewhere` at all, so an ordinary Binding
        // and a Chord could both independently own the same
        // `(stepper, direction)` — violating ticket 03's "at most one owner
        // at a time" invariant. Covers both directions of the theft.
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
        let step_binding = Binding {
            trigger: TriggerMode::FireOnce,
            action: Action::Step {
                stepper: stepper_id.clone(),
                direction: StepDirection::Forward,
            },
        };

        // An ordinary Binding owns it first...
        harness
            .set_binding(Input::Grid(1, 1), Layer::Base, step_binding.clone())
            .await
            .unwrap();
        // ...then a Chord steals it.
        harness
            .set_chord_binding(
                [Input::Grid(2, 1), Input::Grid(2, 2)],
                Layer::Base,
                step_binding.clone(),
            )
            .await
            .unwrap();

        let config = harness.get_config().await;
        assert!(
            !config.profiles[DEFAULT_PROFILE_NAME]
                .base
                .contains_key(&Input::Grid(1, 1))
        );
        let chord_key = ChordKey::new(BTreeSet::from([Input::Grid(2, 1), Input::Grid(2, 2)]));
        assert!(
            config.profiles[DEFAULT_PROFILE_NAME]
                .chords_base
                .contains_key(&chord_key)
        );

        // ...then an ordinary Binding steals it back from the Chord.
        harness
            .set_binding(Input::Grid(3, 1), Layer::Base, step_binding)
            .await
            .unwrap();

        let config = harness.get_config().await;
        assert!(
            !config.profiles[DEFAULT_PROFILE_NAME]
                .chords_base
                .contains_key(&chord_key)
        );
        assert!(
            config.profiles[DEFAULT_PROFILE_NAME]
                .base
                .contains_key(&Input::Grid(3, 1))
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn thumbstick_diagonals_fire_independently_and_share_a_member() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_chord_binding(
                [
                    Input::Thumbstick(Direction::Up),
                    Input::Thumbstick(Direction::Right),
                ],
                Layer::Base,
                keypress_binding(evdev::KeyCode::KEY_1),
            )
            .await
            .expect("Up-Right must be settable");
        harness
            .set_chord_binding(
                [
                    Input::Thumbstick(Direction::Up),
                    Input::Thumbstick(Direction::Left),
                ],
                Layer::Base,
                keypress_binding(evdev::KeyCode::KEY_2),
            )
            .await
            .expect("Up-Left must be settable despite sharing Up with Up-Right");

        harness.press(Input::Thumbstick(Direction::Up)).await;
        harness.press(Input::Thumbstick(Direction::Right)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.release(Input::Thumbstick(Direction::Up)).await;
        harness.release(Input::Thumbstick(Direction::Right)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        // Up is reusable across both diagonals once released and re-pressed
        // fresh (ticket 01's Answer).
        harness.press(Input::Thumbstick(Direction::Up)).await;
        harness.press(Input::Thumbstick(Direction::Left)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.release(Input::Thumbstick(Direction::Up)).await;
        harness.release(Input::Thumbstick(Direction::Left)).await;

        let batches = harness.shut_down().await;

        // Two Chords each fired exactly once — KEY_1 (Up-Right), then KEY_2
        // (Up-Left) — never the thumbstick directions' own passthrough.
        assert_eq!(batches.len(), 4);
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_1, 1));
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_1, 0));
        let evdev::EventSummary::Key(_, code, value) = batches[2][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_2, 1));
        let evdev::EventSummary::Key(_, code, value) = batches[3][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_2, 0));
    }

    #[tokio::test]
    async fn hold_to_repeat_chord_refires_only_on_the_leader_members_repeat() {
        // Hardware-verified regression (ticket 67): while a Chord is active
        // every member stays physically down, so the kernel independently
        // autorepeats *each* member at the same cadence. Re-firing on any
        // member's Repeat (the original ticket-40 design) made an N-member
        // Chord repeat up to N times as fast as a single Input ever would.
        // Only the member sorted first by `ChordKey`'s `BTreeSet` ordering —
        // `Input::Grid(1, 1)` here — now drives the re-fire.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                Binding {
                    trigger: TriggerMode::HoldToRepeat,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: evdev::KeyCode::KEY_C,
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
        // A Repeat on the non-leader member is a no-op — no re-fire.
        harness.repeat(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        // A Repeat on the leader member does re-fire.
        harness.repeat(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;

        // The initial completion, plus exactly one Repeat re-fire (from the
        // leader member only) — each a KeyDown/KeyUp pair.
        assert_eq!(batches.len(), 4);
        for pair in batches.chunks(2) {
            let evdev::EventSummary::Key(_, down_code, down_value) = pair[0][0].destructure()
            else {
                panic!("expected a key event");
            };
            let evdev::EventSummary::Key(_, up_code, up_value) = pair[1][0].destructure() else {
                panic!("expected a key event");
            };
            assert_eq!((down_code, down_value), (evdev::KeyCode::KEY_C, 1));
            assert_eq!((up_code, up_value), (evdev::KeyCode::KEY_C, 0));
        }
    }

    #[tokio::test]
    async fn toggle_chord_survives_releasing_one_member_and_stops_on_a_fresh_completion() {
        // Hardware-verified correction (ticket 67): ticket 01's original
        // Answer had releasing any one member end a Chord's Toggle — live on
        // the real device that felt wrong (a Toggle should stay on past a
        // release, like a real toggle). It now stops only when the full
        // member set completes again, mirroring a single Input's own Toggle
        // (a second Down stops it, never an Up).
        let (action, macros) = macro_action(
            "stuck",
            vec![MacroStepDto::KeyDown(evdev::KeyCode::KEY_LEFTCTRL)],
        );
        let harness =
            CommandHarness::spawn(config_with_bindings_and_macros(HashMap::new(), macros));
        harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                Binding {
                    trigger: TriggerMode::Toggle,
                    action,
                },
            )
            .await
            .unwrap();

        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        // Releasing just ONE member must NOT stop the Chord's Toggle.
        harness.release(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        // A fresh completion of the full member set — both members down
        // again — is what stops it.
        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;

        assert_eq!(
            batches.len(),
            2,
            "one KeyDown lap surviving the mid-run release, then the stop's force-release"
        );
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_LEFTCTRL, 1));
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_LEFTCTRL, 0));
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
    async fn set_chord_binding_command_persists_and_clear_chord_binding_removes_it() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                keypress_binding(evdev::KeyCode::KEY_C),
            )
            .await
            .expect("SetChordBinding must succeed");

        let key = ChordKey::new(BTreeSet::from([Input::Grid(1, 1), Input::Grid(1, 2)]));
        let config = harness.get_config().await;
        assert!(
            config.profiles[DEFAULT_PROFILE_NAME]
                .chords(Layer::Base)
                .contains_key(&key)
        );
        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        assert!(on_disk.contains("grid_r1c1+grid_r1c2"));

        harness
            .clear_chord_binding([Input::Grid(1, 1), Input::Grid(1, 2)], Layer::Base)
            .await
            .expect("ClearChordBinding must succeed");

        let config = harness.get_config().await;
        assert!(
            !config.profiles[DEFAULT_PROFILE_NAME]
                .chords(Layer::Base)
                .contains_key(&key)
        );

        let err = harness
            .clear_chord_binding([Input::Grid(1, 1), Input::Grid(1, 2)], Layer::Base)
            .await
            .expect_err("clearing an already-cleared Chord must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_chord_binding_rejects_analog_repeat() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                Binding {
                    trigger: TriggerMode::AnalogRepeat,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: evdev::KeyCode::KEY_A,
                    },
                },
            )
            .await
            .expect_err("an Analog-repeat Chord Binding must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        let key = ChordKey::new(BTreeSet::from([Input::Grid(1, 1), Input::Grid(1, 2)]));
        let config = harness.get_config().await;
        harness.shut_down().await;
        assert!(
            !config.profiles[DEFAULT_PROFILE_NAME]
                .chords(Layer::Base)
                .contains_key(&key),
            "the rejected Chord Binding must not have been applied"
        );
    }

    #[tokio::test]
    async fn set_binding_rejects_analog_repeat_on_a_non_grid_input() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_binding(
                Input::ModeKey,
                Layer::Base,
                Binding {
                    trigger: TriggerMode::AnalogRepeat,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: evdev::KeyCode::KEY_A,
                    },
                },
            )
            .await
            .expect_err("an Analog-repeat Binding on a non-Grid Input must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        let config = harness.get_config().await;
        harness.shut_down().await;
        assert!(
            !config.profiles[DEFAULT_PROFILE_NAME]
                .base
                .contains_key(&Input::ModeKey),
            "the rejected Binding must not have been applied"
        );
    }

    #[tokio::test]
    async fn set_binding_accepts_analog_repeat_on_a_grid_input() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::AnalogRepeat,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: evdev::KeyCode::KEY_A,
                    },
                },
            )
            .await
            .expect("Analog-repeat on a Grid Input must be accepted");

        let config = harness.get_config().await;
        harness.shut_down().await;
        assert_eq!(
            config.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)].trigger,
            TriggerMode::AnalogRepeat
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
    async fn clear_binding_command_removes_live_and_persists_to_disk() {
        let mut bindings = HashMap::new();
        bindings.insert(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_F1));
        let harness = CommandHarness::spawn(config_with_bindings(bindings));

        harness
            .clear_binding(Input::Grid(1, 1), Layer::Base)
            .await
            .expect("ClearBinding must succeed");

        // Live: the Input is passthrough again (grid_r1c1 -> KEY_1).
        harness.press(Input::Grid(1, 1)).await;

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        let batches = harness.shut_down().await;

        assert_eq!(batches.len(), 1, "passthrough is a single batch");
        let evdev::EventSummary::Key(_, code, _) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_1);

        let reparsed: Config = toml::from_str(&on_disk).unwrap();
        assert!(reparsed.profiles[DEFAULT_PROFILE_NAME].base.is_empty());
    }

    #[tokio::test]
    async fn clear_binding_command_on_an_unbound_input_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .clear_binding(Input::Grid(1, 1), Layer::Base)
            .await
            .expect_err("clearing an unbound Input must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
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
    async fn held_layer_bindings_survive_a_bound_layer_switch_bound_round_trip() {
        let mut held = HashMap::new();
        held.insert(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_F1));
        let profile = Profile {
            held,
            mode_key_role: ModeKeyRole::Bound,
            ..Default::default()
        };
        let harness = CommandHarness::spawn(config_with_profile(profile));

        harness
            .set_mode_key_role(ModeKeyRole::LayerSwitch)
            .await
            .expect("SetModeKeyRole must succeed");
        harness
            .set_mode_key_role(ModeKeyRole::Bound)
            .await
            .expect("SetModeKeyRole must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        let profile = &config.profiles[DEFAULT_PROFILE_NAME];
        assert_eq!(profile.mode_key_role, ModeKeyRole::Bound);
        assert_eq!(
            profile.held[&Input::Grid(1, 1)].action,
            Action::Keypress {
                modifiers: Modifiers::default(),
                key: evdev::KeyCode::KEY_F1,
            }
        );
    }

    #[tokio::test]
    async fn set_binding_command_targets_the_held_layer_independently_of_base() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Held,
                keypress_binding(evdev::KeyCode::KEY_F1),
            )
            .await
            .expect("SetBinding must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        let profile = &config.profiles[DEFAULT_PROFILE_NAME];
        assert!(
            !profile.base.contains_key(&Input::Grid(1, 1)),
            "Base layer must be untouched"
        );
        assert!(profile.held.contains_key(&Input::Grid(1, 1)));
    }

    #[tokio::test]
    async fn set_binding_rejects_a_profile_switch_binding_that_is_not_fire_once() {
        for trigger in [TriggerMode::HoldToRepeat, TriggerMode::Toggle] {
            let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

            let err = harness
                .set_binding(
                    Input::Grid(1, 1),
                    Layer::Base,
                    Binding {
                        trigger,
                        action: Action::ProfileSwitch {
                            target: "Gaming".to_string(),
                        },
                    },
                )
                .await
                .expect_err("a non-Fire-once Profile Switch Binding must be rejected");
            assert!(matches!(err, CommandError::InvalidRequest(_)));

            let config = harness.get_config().await;
            harness.shut_down().await;
            assert!(
                !config.profiles[DEFAULT_PROFILE_NAME]
                    .base
                    .contains_key(&Input::Grid(1, 1)),
                "the rejected Binding must not have been applied"
            );
        }
    }

    #[tokio::test]
    async fn set_binding_rejects_a_controller_button_outside_the_gamepad_allowlist() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::ControllerButton {
                        button: evdev::KeyCode::KEY_A,
                    },
                },
            )
            .await
            .expect_err("a non-gamepad ControllerButton Binding must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        let config = harness.get_config().await;
        harness.shut_down().await;
        assert!(
            !config.profiles[DEFAULT_PROFILE_NAME]
                .base
                .contains_key(&Input::Grid(1, 1)),
            "the rejected Binding must not have been applied"
        );
    }

    #[tokio::test]
    async fn set_binding_accepts_a_controller_button_in_the_gamepad_allowlist() {
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
            .expect("a gamepad ControllerButton Binding must be accepted");

        let config = harness.get_config().await;
        harness.shut_down().await;
        assert_eq!(
            config.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)].action,
            Action::ControllerButton {
                button: evdev::KeyCode::BTN_SOUTH,
            }
        );
    }

    #[tokio::test]
    async fn set_binding_rejects_a_fire_once_controller_button_binding() {
        // Ticket 78: Fire-once is locked out for `Action::ControllerButton`
        // at the live-write path too, mirroring `config::parse`'s own check.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::ControllerButton {
                        button: evdev::KeyCode::BTN_SOUTH,
                    },
                },
            )
            .await
            .expect_err("a Fire-once ControllerButton Binding must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        let config = harness.get_config().await;
        harness.shut_down().await;
        assert!(
            !config.profiles[DEFAULT_PROFILE_NAME]
                .base
                .contains_key(&Input::Grid(1, 1)),
            "the rejected Binding must not have been applied"
        );
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
    async fn create_profile_command_adds_an_empty_profile_and_persists() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .create_profile("Gaming")
            .await
            .expect("CreateProfile must succeed");

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        let config = harness.get_config().await;
        harness.shut_down().await;

        let profile = &config.profiles["Gaming"];
        assert!(profile.base.is_empty());
        assert!(profile.held.is_empty());
        assert_eq!(profile.mode_key_role, ModeKeyRole::LayerSwitch);
        let reparsed: Config = toml::from_str(&on_disk).unwrap();
        assert!(reparsed.profiles.contains_key("Gaming"));
    }

    #[tokio::test]
    async fn create_profile_command_rejects_a_duplicate_name() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .create_profile(DEFAULT_PROFILE_NAME)
            .await
            .expect_err("creating a Profile with an existing name must fail");
        assert!(matches!(err, CommandError::AlreadyExists));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn create_profile_command_rejects_an_empty_or_whitespace_name() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        for name in ["", "   "] {
            let err = harness
                .create_profile(name)
                .await
                .expect_err("creating a Profile with an empty name must fail");
            assert!(matches!(err, CommandError::InvalidRequest(_)));
        }

        let config = harness.get_config().await;
        harness.shut_down().await;
        assert_eq!(config.profiles.len(), 1, "no empty-named Profile created");
    }

    #[tokio::test]
    async fn delete_profile_command_removes_a_non_active_profile_and_persists() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness.create_profile("Gaming").await.unwrap();

        harness
            .delete_profile("Gaming")
            .await
            .expect("DeleteProfile must succeed");

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        let config = harness.get_config().await;
        harness.shut_down().await;

        assert!(!config.profiles.contains_key("Gaming"));
        let reparsed: Config = toml::from_str(&on_disk).unwrap();
        assert!(!reparsed.profiles.contains_key("Gaming"));
    }

    #[tokio::test]
    async fn delete_profile_command_rejects_deleting_the_active_profile() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .delete_profile(DEFAULT_PROFILE_NAME)
            .await
            .expect_err("deleting the active Profile must fail");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        let config = harness.get_config().await;
        harness.shut_down().await;
        assert!(config.profiles.contains_key(DEFAULT_PROFILE_NAME));
    }

    #[tokio::test]
    async fn delete_profile_command_on_an_unknown_name_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .delete_profile("Nonexistent")
            .await
            .expect_err("deleting an unknown Profile must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn delete_profile_command_rejects_deleting_a_profile_still_referenced_by_a_profile_switch_binding()
     {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness.create_profile("Gaming").await.unwrap();
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::ProfileSwitch {
                        target: "Gaming".to_string(),
                    },
                },
            )
            .await
            .expect("SetBinding must succeed");

        let err = harness
            .delete_profile("Gaming")
            .await
            .expect_err("deleting a still-referenced Profile must fail");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        let config = harness.get_config().await;
        harness.shut_down().await;
        assert!(
            config.profiles.contains_key("Gaming"),
            "the refused delete must not have removed the Profile"
        );
    }

    #[tokio::test]
    async fn rename_profile_command_renames_the_active_profile_and_updates_active_profile() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .rename_profile(DEFAULT_PROFILE_NAME, "Renamed")
            .await
            .expect("RenameProfile must succeed");

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        let config = harness.get_config().await;
        let state = harness.get_state().await;
        harness.shut_down().await;

        assert!(!config.profiles.contains_key(DEFAULT_PROFILE_NAME));
        assert!(config.profiles.contains_key("Renamed"));
        assert_eq!(config.active_profile, "Renamed");
        assert_eq!(state.profile, "Renamed");
        let reparsed: Config = toml::from_str(&on_disk).unwrap();
        assert_eq!(reparsed.active_profile, "Renamed");
    }

    #[tokio::test]
    async fn rename_profile_command_leaves_active_profile_untouched_when_renaming_a_different_one()
    {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness.create_profile("Gaming").await.unwrap();

        harness
            .rename_profile("Gaming", "Editing")
            .await
            .expect("RenameProfile must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert_eq!(config.active_profile, DEFAULT_PROFILE_NAME);
        assert!(config.profiles.contains_key("Editing"));
    }

    #[tokio::test]
    async fn rename_profile_command_cascades_every_cross_profile_profile_switch_reference() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness.create_profile("Gaming").await.unwrap();
        // A Binding on the (non-renamed) active Profile targeting "Gaming",
        // plus a self-referencing Binding stored on "Gaming" itself — the
        // cascade must reach both, not just the renamed Profile's own
        // Bindings.
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::ProfileSwitch {
                        target: "Gaming".to_string(),
                    },
                },
            )
            .await
            .expect("SetBinding must succeed");
        harness
            .switch_profile("Gaming")
            .await
            .expect("SwitchProfile must succeed");
        harness
            .set_binding(
                Input::Grid(1, 2),
                Layer::Held,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::ProfileSwitch {
                        target: "Gaming".to_string(),
                    },
                },
            )
            .await
            .expect("SetBinding must succeed");
        harness
            .switch_profile(DEFAULT_PROFILE_NAME)
            .await
            .expect("SwitchProfile must succeed");

        harness
            .rename_profile("Gaming", "Renamed")
            .await
            .expect("RenameProfile must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert_eq!(
            config.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)].action,
            Action::ProfileSwitch {
                target: "Renamed".to_string(),
            }
        );
        assert_eq!(
            config.profiles["Renamed"].held[&Input::Grid(1, 2)].action,
            Action::ProfileSwitch {
                target: "Renamed".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn rename_profile_command_rejects_a_duplicate_new_name() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness.create_profile("Gaming").await.unwrap();

        let err = harness
            .rename_profile("Gaming", DEFAULT_PROFILE_NAME)
            .await
            .expect_err("renaming onto an existing name must fail");
        assert!(matches!(err, CommandError::AlreadyExists));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn rename_profile_command_rejects_an_empty_or_whitespace_new_name() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        for new_name in ["", "   "] {
            let err = harness
                .rename_profile(DEFAULT_PROFILE_NAME, new_name)
                .await
                .expect_err("renaming to an empty name must fail");
            assert!(matches!(err, CommandError::InvalidRequest(_)));
        }

        let config = harness.get_config().await;
        harness.shut_down().await;
        assert!(config.profiles.contains_key(DEFAULT_PROFILE_NAME));
    }

    #[tokio::test]
    async fn rename_profile_command_on_an_unknown_old_name_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .rename_profile("Nonexistent", "Whatever")
            .await
            .expect_err("renaming an unknown Profile must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn create_macro_command_derives_a_slug_and_persists_it() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let macro_id = harness
            .create_macro(
                "Screenshot Combo",
                vec![MacroStepDto::KeyDown(evdev::KeyCode::KEY_A)],
            )
            .await
            .expect("CreateMacro must succeed");
        assert_eq!(macro_id, MacroId::from("screenshot-combo"));

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        let config = harness.get_config().await;
        harness.shut_down().await;

        let def = &config.macros[&macro_id];
        assert_eq!(def.name, "Screenshot Combo");
        assert_eq!(
            def.steps,
            vec![MacroStepDto::KeyDown(evdev::KeyCode::KEY_A)]
        );
        assert!(on_disk.contains("screenshot-combo"));
    }

    #[tokio::test]
    async fn create_macro_command_appends_a_numeric_suffix_on_slug_collision() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let first = harness
            .create_macro("Screenshot Combo", vec![])
            .await
            .unwrap();
        let second = harness
            .create_macro("Screenshot Combo", vec![])
            .await
            .unwrap();

        harness.shut_down().await;

        assert_eq!(first, MacroId::from("screenshot-combo"));
        assert_eq!(second, MacroId::from("screenshot-combo-2"));
    }

    #[tokio::test]
    async fn create_macro_command_rejects_an_empty_or_whitespace_name() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        for name in ["", "   "] {
            let err = harness
                .create_macro(name, vec![])
                .await
                .expect_err("an empty/whitespace Macro name must fail");
            assert!(matches!(err, CommandError::InvalidRequest(_)));
        }

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn rename_macro_command_changes_the_name_not_the_macro_id() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let macro_id = harness.create_macro("Old Name", vec![]).await.unwrap();

        harness
            .rename_macro(macro_id.clone(), "New Name")
            .await
            .expect("RenameMacro must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert_eq!(config.macros[&macro_id].name, "New Name");
    }

    #[tokio::test]
    async fn rename_macro_command_on_an_unknown_macro_id_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .rename_macro(MacroId::from("nonexistent"), "New Name")
            .await
            .expect_err("renaming an unknown Macro must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn delete_macro_command_rejects_deleting_a_macro_still_referenced_by_a_binding() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let macro_id = harness
            .create_macro(
                "Test macro",
                vec![MacroStepDto::KeyDown(evdev::KeyCode::KEY_A)],
            )
            .await
            .unwrap();
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Macro {
                        macro_id: macro_id.clone(),
                    },
                },
            )
            .await
            .expect("SetBinding referencing a real macro_id must succeed");

        let err = harness
            .delete_macro(macro_id.clone())
            .await
            .expect_err("deleting a still-referenced Macro must fail");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        harness
            .clear_binding(Input::Grid(1, 1), Layer::Base)
            .await
            .unwrap();
        harness
            .delete_macro(macro_id)
            .await
            .expect("deleting an unreferenced Macro must now succeed");

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn delete_macro_command_on_an_unknown_macro_id_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .delete_macro(MacroId::from("nonexistent"))
            .await
            .expect_err("deleting an unknown Macro must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_macro_steps_command_overwrites_steps_and_persists_but_leaves_name_alone() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let macro_id = harness
            .create_macro(
                "Screenshot Combo",
                vec![MacroStepDto::KeyDown(evdev::KeyCode::KEY_A)],
            )
            .await
            .unwrap();

        harness
            .set_macro_steps(
                macro_id.clone(),
                vec![
                    MacroStepDto::KeyDown(evdev::KeyCode::KEY_B),
                    MacroStepDto::Delay(25),
                    MacroStepDto::KeyUp(evdev::KeyCode::KEY_B),
                ],
            )
            .await
            .expect("SetMacroSteps must succeed");

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        let config = harness.get_config().await;
        harness.shut_down().await;

        let def = &config.macros[&macro_id];
        assert_eq!(def.name, "Screenshot Combo");
        assert_eq!(
            def.steps,
            vec![
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_B),
                MacroStepDto::Delay(25),
                MacroStepDto::KeyUp(evdev::KeyCode::KEY_B),
            ]
        );
        assert!(on_disk.contains("screenshot-combo"));
    }

    #[tokio::test]
    async fn set_macro_steps_command_on_an_unknown_macro_id_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_macro_steps(MacroId::from("nonexistent"), vec![])
            .await
            .expect_err("setting steps on an unknown Macro must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn create_stepper_command_derives_a_slug_and_persists_it() {
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
            .expect("CreateStepper must succeed");
        assert_eq!(stepper_id, StepperId::from("weapon-wheel"));

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        let config = harness.get_config().await;
        harness.shut_down().await;

        let def = &config.steppers[&stepper_id];
        assert_eq!(def.name, "Weapon Wheel");
        assert_eq!(
            def.items,
            vec![crate::config::StepperItem::Key {
                key: evdev::KeyCode::KEY_1,
                modifiers: Modifiers::default(),
            }]
        );
        assert!(on_disk.contains("weapon-wheel"));
    }

    #[tokio::test]
    async fn create_stepper_command_appends_a_numeric_suffix_on_slug_collision() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let first = harness
            .create_stepper("Weapon Wheel", vec![])
            .await
            .unwrap();
        let second = harness
            .create_stepper("Weapon Wheel", vec![])
            .await
            .unwrap();

        harness.shut_down().await;

        assert_eq!(first, StepperId::from("weapon-wheel"));
        assert_eq!(second, StepperId::from("weapon-wheel-2"));
    }

    #[tokio::test]
    async fn create_stepper_command_rejects_an_empty_or_whitespace_name() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        for name in ["", "   "] {
            let err = harness
                .create_stepper(name, vec![])
                .await
                .expect_err("an empty/whitespace Stepper name must fail");
            assert!(matches!(err, CommandError::InvalidRequest(_)));
        }

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn rename_stepper_command_changes_the_name_not_the_stepper_id() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let stepper_id = harness.create_stepper("Old Name", vec![]).await.unwrap();

        harness
            .rename_stepper(stepper_id.clone(), "New Name")
            .await
            .expect("RenameStepper must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert_eq!(config.steppers[&stepper_id].name, "New Name");
    }

    #[tokio::test]
    async fn rename_stepper_command_on_an_unknown_stepper_id_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .rename_stepper(StepperId::from("nonexistent"), "New Name")
            .await
            .expect_err("renaming an unknown Stepper must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn delete_stepper_command_rejects_deleting_a_stepper_still_referenced_by_a_binding() {
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
                        stepper: stepper_id.clone(),
                        direction: StepDirection::Forward,
                    },
                },
            )
            .await
            .expect("SetBinding referencing a real stepper_id must succeed");

        let err = harness
            .delete_stepper(stepper_id.clone())
            .await
            .expect_err("deleting a still-referenced Stepper must fail");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        harness
            .clear_binding(Input::Grid(1, 1), Layer::Base)
            .await
            .unwrap();
        harness
            .delete_stepper(stepper_id)
            .await
            .expect("deleting an unreferenced Stepper must now succeed");

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn delete_stepper_command_on_an_unknown_stepper_id_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .delete_stepper(StepperId::from("nonexistent"))
            .await
            .expect_err("deleting an unknown Stepper must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_stepper_items_command_overwrites_items_and_persists_but_leaves_name_alone() {
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
            .set_stepper_items(
                stepper_id.clone(),
                vec![
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
            .expect("SetStepperItems must succeed");

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        let config = harness.get_config().await;
        harness.shut_down().await;

        let def = &config.steppers[&stepper_id];
        assert_eq!(def.name, "Weapon Wheel");
        assert_eq!(
            def.items,
            vec![
                crate::config::StepperItem::Key {
                    key: evdev::KeyCode::KEY_2,
                    modifiers: Modifiers::default(),
                },
                crate::config::StepperItem::Key {
                    key: evdev::KeyCode::KEY_3,
                    modifiers: Modifiers::default(),
                },
            ]
        );
        assert!(on_disk.contains("weapon-wheel"));
    }

    #[tokio::test]
    async fn set_stepper_items_command_on_an_unknown_stepper_id_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_stepper_items(StepperId::from("nonexistent"), vec![])
            .await
            .expect_err("setting items on an unknown Stepper must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_binding_rejects_a_step_action_naming_an_unknown_stepper_id() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Step {
                        stepper: StepperId::from("nonexistent"),
                        direction: StepDirection::Forward,
                    },
                },
            )
            .await
            .expect_err("SetBinding with an unknown stepper_id must fail");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_binding_rejects_a_toggle_step_binding() {
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

        let err = harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::Toggle,
                    action: Action::Step {
                        stepper: stepper_id,
                        direction: StepDirection::Forward,
                    },
                },
            )
            .await
            .expect_err("a Toggle Step Binding must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        harness.shut_down().await;
    }

    /// Ticket 03's Answer: assigning a Stepper list to a new Input pair
    /// silently moves it off its old one — no reject-at-save step. Only the
    /// same direction is moved; the other direction, bound elsewhere, is
    /// left untouched.
    #[tokio::test]
    async fn set_binding_silently_moves_a_stepper_direction_off_its_old_input() {
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
        let step_binding = |direction| Binding {
            trigger: TriggerMode::FireOnce,
            action: Action::Step {
                stepper: stepper_id.clone(),
                direction,
            },
        };

        harness
            .set_binding(
                Input::Wheel(crate::input::WheelEvent::ScrollUp),
                Layer::Base,
                step_binding(StepDirection::Forward),
            )
            .await
            .unwrap();
        harness
            .set_binding(
                Input::Wheel(crate::input::WheelEvent::ScrollDown),
                Layer::Base,
                step_binding(StepDirection::Backward),
            )
            .await
            .unwrap();

        // Reassign Forward to a new pair of Inputs.
        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                step_binding(StepDirection::Forward),
            )
            .await
            .expect("reassigning Forward to a new Input must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        let default_profile = &config.profiles[config::DEFAULT_PROFILE_NAME];
        assert!(
            !default_profile
                .base
                .contains_key(&Input::Wheel(crate::input::WheelEvent::ScrollUp)),
            "the old Forward Binding must be silently removed"
        );
        assert_eq!(
            default_profile.base[&Input::Grid(1, 1)].action,
            Action::Step {
                stepper: stepper_id.clone(),
                direction: StepDirection::Forward,
            }
        );
        // Backward, untouched by the Forward-only reassignment.
        assert_eq!(
            default_profile.base[&Input::Wheel(crate::input::WheelEvent::ScrollDown)].action,
            Action::Step {
                stepper: stepper_id,
                direction: StepDirection::Backward,
            }
        );
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
    async fn set_stepper_items_rejects_a_controller_button_item_naming_a_non_gamepad_code() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        let stepper_id = harness
            .create_stepper("Weapon Wheel", vec![])
            .await
            .expect("CreateStepper must succeed");

        let err = harness
            .set_stepper_items(
                stepper_id.clone(),
                vec![crate::config::StepperItem::ControllerButton {
                    button: evdev::KeyCode::KEY_A,
                }],
            )
            .await
            .expect_err("a non-gamepad controller button item must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        // The rejected write never lands.
        let config = harness.get_config().await;
        assert!(config.steppers[&stepper_id].items.is_empty());
    }

    #[tokio::test]
    async fn create_stepper_rejects_a_controller_button_item_naming_a_non_gamepad_code() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .create_stepper(
                "Weapon Wheel",
                vec![crate::config::StepperItem::ControllerButton {
                    button: evdev::KeyCode::KEY_A,
                }],
            )
            .await
            .expect_err("a non-gamepad controller button item must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));
        assert!(harness.get_config().await.steppers.is_empty());
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
    async fn set_binding_rejects_a_macro_action_naming_an_unknown_macro_id() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Macro {
                        macro_id: MacroId::from("nonexistent"),
                    },
                },
            )
            .await
            .expect_err("SetBinding with an unknown macro_id must fail");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn switch_profile_command_switches_active_profile_and_persists() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness.create_profile("Gaming").await.unwrap();

        harness
            .switch_profile("Gaming")
            .await
            .expect("SwitchProfile must succeed");

        let on_disk = std::fs::read_to_string(&harness.config_path).unwrap();
        let state = harness.get_state().await;
        harness.shut_down().await;

        assert_eq!(state.profile, "Gaming");
        let reparsed: Config = toml::from_str(&on_disk).unwrap();
        assert_eq!(reparsed.active_profile, "Gaming");
    }

    #[tokio::test]
    async fn switch_profile_command_on_an_unknown_name_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .switch_profile("Nonexistent")
            .await
            .expect_err("switching to an unknown Profile must fail");
        assert!(matches!(err, CommandError::NotFound));

        harness.shut_down().await;
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
    async fn set_actuation_point_command_applies_live_and_persists_to_disk() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_actuation_point(Input::Grid(1, 1), 200, 180)
            .await
            .expect("SetActuationPoint must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        let profile = &config.profiles[DEFAULT_PROFILE_NAME];
        assert_eq!(
            profile.actuation_overrides[&Input::Grid(1, 1)],
            ActuationPoint {
                actuation: 200,
                release: 180,
            }
        );
    }

    #[tokio::test]
    async fn set_actuation_point_rejects_a_non_grid_input() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_actuation_point(Input::ModeKey, 200, 180)
            .await
            .expect_err("a non-Grid Input must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_actuation_point_rejects_a_release_point_above_actuation() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_actuation_point(Input::Grid(1, 1), 100, 150)
            .await
            .expect_err("release > actuation must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_actuation_point_rejects_a_release_point_equal_to_actuation() {
        // Code-review finding on ticket 22: `release == actuation` used to
        // pass this check, but `capture::analog::observe` would then
        // chatter Down/Up forever on a key held at a perfectly steady
        // Depth — hysteresis requires a strict gap, not just `<=`.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_actuation_point(Input::Grid(1, 1), 128, 128)
            .await
            .expect_err("release == actuation must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn clear_actuation_point_command_reverts_to_the_profile_default() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_actuation_point(Input::Grid(1, 1), 200, 180)
            .await
            .expect("SetActuationPoint must succeed");
        harness
            .clear_actuation_point(Input::Grid(1, 1))
            .await
            .expect("ClearActuationPoint must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert!(
            !config.profiles[DEFAULT_PROFILE_NAME]
                .actuation_overrides
                .contains_key(&Input::Grid(1, 1))
        );
    }

    #[tokio::test]
    async fn clear_actuation_point_on_an_unoverridden_key_is_a_no_op_success() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .clear_actuation_point(Input::Grid(1, 1))
            .await
            .expect("clearing an unoverridden key must still succeed");

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn clear_actuation_point_rejects_a_non_grid_input() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .clear_actuation_point(Input::ModeKey)
            .await
            .expect_err("a non-Grid Input must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_default_actuation_command_applies_live_and_persists_to_disk() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_default_actuation(140, 120)
            .await
            .expect("SetDefaultActuation must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert_eq!(
            config.profiles[DEFAULT_PROFILE_NAME].default_actuation,
            ActuationPoint {
                actuation: 140,
                release: 120,
            }
        );
    }

    #[tokio::test]
    async fn set_default_actuation_rejects_a_release_point_above_actuation() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_default_actuation(100, 150)
            .await
            .expect_err("release > actuation must be rejected");
        assert!(matches!(err, CommandError::InvalidRequest(_)));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn reset_actuation_points_clears_every_override_in_one_call() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_actuation_point(Input::Grid(1, 1), 200, 180)
            .await
            .expect("SetActuationPoint must succeed");
        harness
            .set_actuation_point(Input::Grid(2, 2), 90, 70)
            .await
            .expect("SetActuationPoint must succeed");

        harness
            .reset_actuation_points()
            .await
            .expect("ResetActuationPoints must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert!(
            config.profiles[DEFAULT_PROFILE_NAME]
                .actuation_overrides
                .is_empty()
        );
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
    async fn set_axis_assignment_command_persists_and_is_reflected_in_get_config() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        harness
            .set_axis_assignment(Input::Grid(1, 1), Layer::Base, AxisTarget::LeftTrigger)
            .await
            .expect("SetAxisAssignment must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert_eq!(
            config.profiles[DEFAULT_PROFILE_NAME].axis_base[&Input::Grid(1, 1)],
            AxisTarget::LeftTrigger
        );
    }

    #[tokio::test]
    async fn set_axis_assignment_rejects_a_non_grid_input() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .set_axis_assignment(Input::ModeKey, Layer::Base, AxisTarget::LeftTrigger)
            .await
            .expect_err("a non-Grid Input must be rejected");
        harness.shut_down().await;

        assert!(matches!(err, CommandError::InvalidRequest(_)));
    }

    #[tokio::test]
    async fn set_axis_assignment_atomically_clears_an_existing_binding_on_the_same_input_and_layer()
    {
        let mut bindings = HashMap::new();
        bindings.insert(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_F1));
        let harness = CommandHarness::spawn(config_with_bindings(bindings));

        harness
            .set_axis_assignment(Input::Grid(1, 1), Layer::Base, AxisTarget::LeftTrigger)
            .await
            .expect("SetAxisAssignment must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert!(
            !config.profiles[DEFAULT_PROFILE_NAME]
                .base
                .contains_key(&Input::Grid(1, 1))
        );
        assert_eq!(
            config.profiles[DEFAULT_PROFILE_NAME].axis_base[&Input::Grid(1, 1)],
            AxisTarget::LeftTrigger
        );
    }

    #[tokio::test]
    async fn set_axis_assignment_atomically_clears_chord_membership_on_the_same_input_and_layer() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                keypress_binding(evdev::KeyCode::KEY_C),
            )
            .await
            .expect("SetChordBinding must succeed");

        harness
            .set_axis_assignment(Input::Grid(1, 1), Layer::Base, AxisTarget::LeftTrigger)
            .await
            .expect("SetAxisAssignment must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert!(
            config.profiles[DEFAULT_PROFILE_NAME].chords_base.is_empty(),
            "the whole Chord must be removed, not just input's own membership"
        );
    }

    #[tokio::test]
    async fn set_binding_rejects_an_input_already_axis_assigned_on_the_same_layer() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_axis_assignment(Input::Grid(1, 1), Layer::Base, AxisTarget::LeftTrigger)
            .await
            .expect("SetAxisAssignment must succeed");

        let err = harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                keypress_binding(evdev::KeyCode::KEY_F1),
            )
            .await
            .expect_err("SetBinding on an Axis-assigned Input must be rejected");
        harness.shut_down().await;

        assert!(matches!(err, CommandError::InvalidRequest(_)));
    }

    #[tokio::test]
    async fn set_binding_on_a_different_layer_than_an_axis_assignment_is_allowed() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_axis_assignment(Input::Grid(1, 1), Layer::Base, AxisTarget::LeftTrigger)
            .await
            .expect("SetAxisAssignment must succeed");

        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Held,
                keypress_binding(evdev::KeyCode::KEY_F1),
            )
            .await
            .expect("a Binding on the other Layer must be allowed");
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn set_chord_binding_rejects_a_member_already_axis_assigned_on_the_same_layer() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_axis_assignment(Input::Grid(1, 1), Layer::Base, AxisTarget::LeftTrigger)
            .await
            .expect("SetAxisAssignment must succeed");

        let err = harness
            .set_chord_binding(
                [Input::Grid(1, 1), Input::Grid(1, 2)],
                Layer::Base,
                keypress_binding(evdev::KeyCode::KEY_C),
            )
            .await
            .expect_err("a Chord with an Axis-assigned member must be rejected");
        harness.shut_down().await;

        assert!(matches!(err, CommandError::InvalidRequest(_)));
    }

    #[tokio::test]
    async fn clear_axis_assignment_removes_it_and_persists() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));
        harness
            .set_axis_assignment(Input::Grid(1, 1), Layer::Base, AxisTarget::LeftTrigger)
            .await
            .expect("SetAxisAssignment must succeed");

        harness
            .clear_axis_assignment(Input::Grid(1, 1), Layer::Base)
            .await
            .expect("ClearAxisAssignment must succeed");

        let config = harness.get_config().await;
        harness.shut_down().await;

        assert!(config.profiles[DEFAULT_PROFILE_NAME].axis_base.is_empty());
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
    async fn clear_axis_assignment_on_an_unassigned_input_returns_not_found() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let err = harness
            .clear_axis_assignment(Input::Grid(1, 1), Layer::Base)
            .await
            .expect_err("clearing an unassigned Input must fail");
        harness.shut_down().await;

        assert!(matches!(err, CommandError::NotFound));
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
