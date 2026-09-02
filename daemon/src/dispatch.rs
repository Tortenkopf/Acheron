// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The dispatch task: single consumer of both the capture channel and the
//! D-Bus command channel (issue 07's "D-Bus interleaving" — GUI-originated
//! calls push a `Command` alongside `PhysicalEvent`s, so one task remains
//! the sole owner of `Config`, no lock or second copy of state). Resolves
//! each `PhysicalEvent`'s `Input` against the active Profile's active Layer
//! (ticket 18) and, per ticket 17, branches on `TriggerMode` — Fire-once
//! fires once on `Down`, Hold-to-repeat fires on `Down` and every `Repeat`,
//! Toggle starts/stops only on `Down`. Applies a `Command::Apply` (ticket
//! 15/11) by handing its `edit::Edit` to `edit::apply` — mutating `Config` in
//! place and rewriting `config.toml` immediately, atomically per call.
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
use std::hash::Hash;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use zbus::object_server::SignalEmitter;

use crate::analog_repeat;
use crate::axis;
use crate::capture::analog::DeviceInfo;
use crate::capture::{CaptureMode, EventState, PhysicalEvent};
use crate::chord;
use crate::command::{Command, State};
use crate::config::{
    self, Action, ActuationPoint, Binding, ChordKey, Config, Layer, MacroDef, MacroId, ModeKeyRole,
    StepperDef, StepperId, TriggerMode,
};
use crate::dbus::Daemon;
use crate::edit;
use crate::executor::{self, ActiveToggle, FiringHandle, MacroStep};
use crate::injector::Injector;
use crate::input::Input;
use crate::stepper;
use crate::trigger;

/// Builds a `TriggerCtx` (below) from a `(firings, toggles)` pair plus the
/// shared `config` view and the `DispatchState` fields it needs — a
/// per-call-site borrow struct built from a macro, so the three
/// `trigger::decide` + `perform_trigger` sites don't each spell the struct
/// literal. Each site passes a disjoint `&mut self.<map>` borrow (`in_flight`
/// on the individual path, `chord_runtime.{firings,toggles}` on the Chord
/// path) — a `&mut self` method can't be generic over which map type `K`
/// selects, so `perform_trigger<K>` stays a free function. Defined up here
/// because `macro_rules!` is textually scoped and the first user
/// (`handle_event`) precedes `TriggerCtx`'s own definition.
macro_rules! trigger_ctx {
    ($injector:expr, $config:expr, $firings:expr, $toggles:expr, $cursors:expr, $lap:expr $(,)?) => {
        TriggerCtx {
            injector: $injector,
            firings: $firings,
            toggles: $toggles,
            macros: &$config.macros,
            steppers: &$config.steppers,
            cursors: $cursors,
            toggle_lap_target: $lap,
        }
    };
}

/// Every piece of Daemon-owned, `ChordKey`-keyed runtime state a Chord's own
/// Trigger-mode dispatch touches (ticket 01/40) — the firing/toggle *handles*
/// the pure `chord` state machine (post-release ticket 07) never holds. One
/// nested `DispatchState` field, reset fresh per dispatch task start,
/// mirroring how `axis::Engine` bundles its own two maps; the executor derives
/// the `trigger::Slot` liveness snapshot `chord::feed` wants from it.
#[derive(Default)]
struct ChordRuntime {
    firings: HashMap<ChordKey, FiringHandle>,
    toggles: HashMap<ChordKey, ActiveToggle>,
}

/// Every piece of ephemeral runtime state the dispatch task owns — built
/// fresh on every task start, the same lifetime as the loose `run` locals it
/// replaces. NOT `Config` (committed state; stays a `run` local so the input
/// path keeps only `&Config` — ticket 05). NOT the `rx_*` receivers or their
/// `*_open` liveness flags (pure `select!`-loop plumbing; no handler touches
/// them). No lifetime parameter — every handle below is owned and `'static`.
/// Dispatch-internal: never part of any module's interface. Adding a new
/// piece of dispatch runtime state means a field here, not a fresh `run`
/// local or a new `handle_*` parameter (CONTRIBUTING.md).
struct DispatchState {
    toggles: HashMap<Input, ActiveToggle>,
    in_flight: HashMap<Input, FiringHandle>,
    stepper: stepper::Cursors,
    active_layer: Layer,
    chord_machine: chord::ChordMachine,
    chord_runtime: ChordRuntime,
    axis: axis::Engine,
    analog_repeat: analog_repeat::Engine,
    device_connected: bool,
    capture_mode: CaptureMode,
    device_info: Option<DeviceInfo>,
    injector: Injector,
    signal_emitter: Option<SignalEmitter<'static>>,
    actuation_tx: watch::Sender<HashMap<Input, ActuationPoint>>,
    capture_control_tx: mpsc::Sender<bool>,
    toggle_lap_target: Duration,
}

impl DispatchState {
    /// Builds the struct with every ephemeral field at its task-start value;
    /// the five owned collaborators come from `run`'s startup parameters (and
    /// from stub channels in the test seam). `run` calls this once and then
    /// only drives the `select!` loop.
    fn new(
        injector: Injector,
        signal_emitter: Option<SignalEmitter<'static>>,
        actuation_tx: watch::Sender<HashMap<Input, ActuationPoint>>,
        capture_control_tx: mpsc::Sender<bool>,
        toggle_lap_target: Duration,
    ) -> Self {
        DispatchState {
            toggles: HashMap::new(),
            in_flight: HashMap::new(),
            stepper: stepper::Cursors::default(),
            active_layer: Layer::Base,
            chord_machine: chord::ChordMachine::default(),
            chord_runtime: ChordRuntime::default(),
            axis: axis::Engine::default(),
            analog_repeat: analog_repeat::Engine::default(),
            device_connected: true,
            capture_mode: CaptureMode::Digital,
            device_info: None,
            injector,
            signal_emitter,
            actuation_tx,
            capture_control_tx,
            toggle_lap_target,
        }
    }

    /// Resolves one `PhysicalEvent` against the active Profile/Layer. Returns
    /// the `Edit`s (if any) the `run` loop must commit — in practice empty, or
    /// a single `Edit::SwitchProfile` when a Fire-once `Action::ProfileSwitch`
    /// binding fires on `Down` (ticket 05). Takes `&Config`, never `&mut` —
    /// the `run` loop is the sole commit point.
    async fn handle_event(
        &mut self,
        config: &Config,
        event: PhysicalEvent,
    ) -> io::Result<Vec<edit::Edit>> {
        let profile = config
            .active_profile()
            .expect("load_or_seed validates active_profile names a real profile");

        if event.input == Input::ModeKey && profile.mode_key_role == ModeKeyRole::LayerSwitch {
            handle_layer_switch(
                &self.injector,
                &mut self.active_layer,
                &self.signal_emitter,
                &mut self.axis,
                &mut self.analog_repeat,
                event.state,
            )
            .await;
            return Ok(Vec::new());
        }

        // A Down on an Input with an active Toggle always stops that Toggle
        // first, regardless of what Binding the Input's current Layer nominally
        // assigns — this press is consumed entirely by the stop, per spec.md's
        // "Toggle behavior across Layer/Profile switches". Only a later press
        // resumes normal evaluation.
        if event.state == EventState::Down
            && trigger::stop_toggle(&mut self.toggles, &event.input).await
        {
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
        let axis_map = profile.axis_layer(self.active_layer);
        if axis_map.contains_key(&event.input) {
            if event.depth.is_none() {
                for w in self.axis.step_digital(axis_map, event.input, event.state) {
                    let _ = self.injector.set_axis_value(w.code, w.value).await;
                }
            }
            return Ok(Vec::new());
        }

        // The Chord-detection state machine (ticket 01/40, post-release ticket
        // 07) runs unconditionally, after the guards above and before ordinary
        // Binding lookup — it owns the "is this event mine?" predicate now
        // (`ChordOutcome::NotMine` when it isn't), rather than `handle_event`
        // reaching into `claimed` / `chord_keys_containing` itself.
        let live = chord_slots(&self.chord_runtime);
        match chord::feed(
            &mut self.chord_machine,
            profile.chords(self.active_layer),
            &live,
            event,
        ) {
            chord::ChordOutcome::Handled(effects) => {
                return self.run_chord_effects(config, effects).await;
            }
            chord::ChordOutcome::NotMine => {}
        }

        let bindings = profile.layer(self.active_layer);
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
                // The bound → `trigger::decide` + `perform_trigger` /
                // `ProfileSwitch` → `Edit` / unbound → passthrough tail, shared
                // verbatim with the Chord machine's `FireIndividual` executor so
                // the retroactive-fire logic exists once.
                self.dispatch_individual_down(config, event.input).await
            }
            EventState::Repeat | EventState::Up => {
                let Some(binding) = binding else {
                    self.injector
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
                let slot = slot_for(&self.in_flight, &self.toggles, &event.input);
                let decision = trigger::decide(&binding, event.state, slot);
                let mut ctx = trigger_ctx!(
                    &self.injector,
                    config,
                    &mut self.in_flight,
                    &mut self.toggles,
                    &mut self.stepper,
                    self.toggle_lap_target,
                );
                perform_trigger(decision, event.input, &binding, &mut ctx).await?;
                Ok(Vec::new())
            }
        }
    }

    /// Performs each `chord::ChordEffect` the pure machine decided on, in
    /// order, against the runtime state dispatch owns (ticket 07). Returns any
    /// `edit::Edit`s a `FireIndividual` produced (a member's individual
    /// Binding resolving to `Action::ProfileSwitch`) for the `run` loop to
    /// commit, same as the old `handle_chord_event` / `handle_chord_timeout`
    /// return.
    async fn run_chord_effects(
        &mut self,
        config: &Config,
        effects: Vec<chord::ChordEffect>,
    ) -> io::Result<Vec<edit::Edit>> {
        let mut edits = Vec::new();
        for effect in effects {
            match effect {
                chord::ChordEffect::FireChord {
                    key,
                    binding,
                    state,
                } => {
                    // The Chord path's own Trigger-mode dispatch — `trigger::
                    // decide` (the same matrix the individual path runs) against
                    // this Chord's `ChordKey`-keyed liveness, performed by the
                    // generic `perform_trigger`. A Chord's Action is never
                    // `AnalogRepeat` or `ProfileSwitch`, and `chord::feed` only
                    // ever emits `Down` / `Repeat`, so those `decide` arms are
                    // unreachable here.
                    let slot = slot_for(
                        &self.chord_runtime.firings,
                        &self.chord_runtime.toggles,
                        &key,
                    );
                    let decision = trigger::decide(&binding, state, slot);
                    let mut ctx = trigger_ctx!(
                        &self.injector,
                        config,
                        &mut self.chord_runtime.firings,
                        &mut self.chord_runtime.toggles,
                        &mut self.stepper,
                        self.toggle_lap_target,
                    );
                    perform_trigger(decision, key, &binding, &mut ctx).await?;
                }
                chord::ChordEffect::ReleaseChordFiring { key } => {
                    // Fire-once / Hold-to-repeat only — a Toggle Chord is
                    // deliberately not stopped by a member's `Up` (ticket 67).
                    trigger::force_release_stuck(&self.chord_runtime.firings, &key, &self.injector)
                        .await;
                }
                chord::ChordEffect::StopChordToggle { key } => {
                    trigger::stop_toggle(&mut self.chord_runtime.toggles, &key).await;
                }
                chord::ChordEffect::FireIndividual { input } => {
                    edits.extend(self.dispatch_individual_down(config, input).await?);
                }
                chord::ChordEffect::ForceReleaseIndividual { input } => {
                    trigger::force_release_stuck(&self.in_flight, &input, &self.injector).await;
                }
            }
        }
        Ok(edits)
    }

    /// The continuous Analog half of ticket 59 §7's `(Depth, edge_event) ->
    /// axis_value` seam: reacts to every change of the live-Depth watch
    /// channel (`capture::analog`'s grid task, ticket 26) by resolving
    /// `config::resolve_axis_value` for every Input the active Layer currently
    /// Axis-assigns, then running the shared conflict-resolution/emit path.
    /// Every Grid key's raw depth is published on every incoming hidraw report
    /// regardless of Binding/Axis status
    /// (`capture::analog::relay_grid_blocking`), so this only ever *reads*
    /// `depths` for the subset that's actually Axis-assigned right now — an
    /// empty Axis map (the common case) short-circuits immediately, doing no
    /// work on every ordinary depth tick.
    async fn handle_depth_update(&mut self, config: &Config, depths: &HashMap<Input, u8>) {
        let profile = config
            .active_profile()
            .expect("load_or_seed validates active_profile names a real profile");
        let axis_map = profile.axis_layer(self.active_layer);
        if axis_map.is_empty() {
            return;
        }
        // Ticket 71 code-review finding: reads each relevant Input's own
        // Actuation/Release point directly, rather than building
        // `resolved_actuation_points()`'s full 20-entry `HashMap` just to read
        // the 1-4 entries an Axis-assigned Profile actually needs — this runs on
        // every live-Depth tick (sub-millisecond while a key is moving, per
        // ticket 13), so the redundant O(20) rebuild was real hot-path waste.
        // The `depth → value` ramp stays dispatch-side (it needs the per-Input
        // Actuation point); the engine's inputs are already-resolved 0-255
        // contributions.
        let mut resolved: HashMap<Input, u8> = HashMap::new();
        for &input in axis_map.keys() {
            if let Some(&depth) = depths.get(&input) {
                let point = profile.resolved_actuation_point(input);
                resolved.insert(input, config::resolve_axis_value(depth, point));
            }
        }
        for w in self.axis.resolve(axis_map, &resolved) {
            let _ = self.injector.set_axis_value(w.code, w.value).await;
        }
    }

    /// Dispatches a single fresh `Down` on `input` against the active Layer —
    /// the `ProfileSwitch → Edit` / bound → `trigger::decide` +
    /// `perform_trigger` / unbound → passthrough tail carved out of
    /// `handle_event`, shared verbatim by the ordinary input path and the
    /// Chord machine's `FireIndividual` executor (a member's individual
    /// Binding firing retroactively — the window elapsed, or the member was
    /// released before completing — per ticket 01's Answer: "the pending
    /// member's individual Binding fires retroactively, delayed by the
    /// window"). It is *not* a re-entry into `handle_event`: that would re-run
    /// the layer-switch / toggle-stop / axis / chord guards against a
    /// synthetic Down, which is wrong. Returns any `Edit::SwitchProfile` the
    /// member's own Binding produces — a Chord member's individual Binding can
    /// be any Action, unlike a Chord's own, which can never be `ProfileSwitch`.
    async fn dispatch_individual_down(
        &mut self,
        config: &Config,
        input: Input,
    ) -> io::Result<Vec<edit::Edit>> {
        let profile = config
            .active_profile()
            .expect("load_or_seed validates active_profile names a real profile");
        let binding = profile.layer(self.active_layer).get(&input).cloned();
        match binding {
            Some(binding) => {
                if let Action::ProfileSwitch { target } = binding.action {
                    // The switch is an `Edit` for the `run` loop to commit
                    // (ticket 05).
                    return Ok(vec![edit::Edit::SwitchProfile { name: target }]);
                }
                // Accepted gap (ticket 39): a member's own individual Binding
                // set to Analog-repeat fires once here through the ordinary
                // one-shot path (`decide` treats `AnalogRepeat` as `HoldToRepeat`
                // for a Down), rather than starting the depth-driven background
                // task `update_analog_repeats` normally would — this retroactive
                // Down is synthetic (no real live Depth to hand a task), and a
                // grid key that's both a Chord member *and* individually
                // Analog-repeat-triggered is a narrow combination this
                // fast-follow doesn't specially engineer for.
                let slot = slot_for(&self.in_flight, &self.toggles, &input);
                let decision = trigger::decide(&binding, EventState::Down, slot);
                let mut ctx = trigger_ctx!(
                    &self.injector,
                    config,
                    &mut self.in_flight,
                    &mut self.toggles,
                    &mut self.stepper,
                    self.toggle_lap_target,
                );
                perform_trigger(decision, input, &binding, &mut ctx).await?;
                Ok(Vec::new())
            }
            None => {
                self.injector
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

    /// Starts/stops every grid Input's Analog-repeat task from a fresh
    /// `depth_tx` snapshot (ticket 20/39) — the depth-driven half of
    /// Analog-repeat's firing, parallel to `handle_depth_update`'s own Axis
    /// resolution off the same snapshot. A rising edge through
    /// `ANALOG_REPEAT_DEADZONE` on an Input whose active-Layer Binding is
    /// `TriggerMode::AnalogRepeat` spawns a task (compiling its steps once,
    /// the same "once per fire" precedent `perform_trigger` follows); a
    /// falling edge — or the Binding no longer being Analog-repeat,
    /// best-effort only, see below — stops one. A Binding changed away from
    /// Analog-repeat without an intervening depth-crossing (e.g. edited live
    /// while the key stays physically pressed) is a known, accepted residual
    /// gap: the stale task keeps running with the steps it compiled at spawn
    /// time until Depth next crosses the deadzone — the same class of gap
    /// ticket 71's Answer accepted for its own opposite-signed-halves
    /// tie-break, not engineered around here.
    async fn update_analog_repeats(
        &mut self,
        config: &Config,
        depth_rx: &watch::Receiver<HashMap<Input, u8>>,
        snapshot: &HashMap<Input, u8>,
    ) {
        let profile = config
            .active_profile()
            .expect("load_or_seed validates active_profile names a real profile");
        let bindings = profile.layer(self.active_layer);
        // The set of Inputs whose active-Layer Binding is Analog-repeat —
        // computed dispatch-side from `Config`; the engine never sees a
        // `Config`, `CaptureMode`, or `Layer`.
        let repeat_inputs: HashSet<Input> = bindings
            .iter()
            .filter(|(_, b)| b.trigger == TriggerMode::AnalogRepeat)
            .map(|(&input, _)| input)
            .collect();
        for input in self.analog_repeat.update(&repeat_inputs, snapshot).await {
            let binding = bindings
                .get(&input)
                .expect("reconcile only returns Spawn for a repeat_inputs member");
            // Compiled once, here, from the Binding's Action as of the moment
            // Depth first crossed the deadzone (mirrors `perform_trigger`'s
            // own once-per-fire `compile_action` call) — `compile_action`
            // stays dispatch-side so the engine needn't depend on `dispatch`
            // or drag `Config` + the `stepper::Cursors` in.
            let steps = compile_action(
                &binding.action,
                &config.macros,
                &config.steppers,
                &mut self.stepper,
            );
            self.analog_repeat.spawn(
                self.injector.clone(),
                input,
                steps,
                analog_repeat::pulse_hold_for(&binding.action),
                depth_rx.clone(),
            );
        }
    }

    /// Runs each `edit::Effect` an `edit::plan` derived, in order, against the
    /// `DispatchState` fields it touches (ticket 05). `config` is the
    /// just-committed `Config` — every effect that reads the new state
    /// (`RepublishActuation`, `RecomputeAxes`) reads it from here. Every axis
    /// `ABS_*` write goes through one dispatch-side emit loop over the
    /// engine's `Vec<AxisWrite>` with the injector error swallowed (`let _ =`)
    /// — a deliberate unification (ticket 10) of the old inconsistent `?` /
    /// `let _ =` on the axis-output path.
    async fn run_effects(&mut self, effects: Vec<edit::Effect>, config: &Config) {
        for effect in effects {
            match effect {
                edit::Effect::RepublishActuation => {
                    publish_actuation_snapshot(config, &self.actuation_tx)
                }
                edit::Effect::RecomputeAxes { layer } => {
                    // `RecomputeAxes` for a Layer that isn't the active one is a
                    // no-op — the resulting `Config` already carries the edit,
                    // but nothing is driving that Layer's axes right now.
                    if layer == self.active_layer {
                        let axis_map = config
                            .active_profile()
                            .expect("load_or_seed validates active_profile names a real profile")
                            .axis_layer(layer)
                            .clone();
                        for w in self.axis.recompute(&axis_map) {
                            let _ = self.injector.set_axis_value(w.code, w.value).await;
                        }
                    }
                }
                edit::Effect::ForgetAxisContribution(input) => {
                    self.axis.forget(input);
                }
                edit::Effect::SignalCaptureMode(force) => {
                    // Only on a successful persist (which is where `run_effects`
                    // runs) — the supervisor swaps the live capture source to
                    // match `config.toml` on disk.
                    let _ = self.capture_control_tx.send(force).await;
                }
                edit::Effect::StopToggle(input) => {
                    if let Some(toggle) = self.toggles.remove(&input) {
                        toggle.stop().await;
                    }
                }
                edit::Effect::StopAllToggles => stop_all_toggles(&mut self.toggles).await,
                edit::Effect::StopAllAnalogRepeats => self.analog_repeat.stop_all().await,
                edit::Effect::ResetAxisOutputs => {
                    for w in self.axis.reset() {
                        let _ = self.injector.set_axis_value(w.code, w.value).await;
                    }
                }
                edit::Effect::ReconcileStepperCursor(stepper_id) => {
                    // Against the just-committed `Config`: `id` gone → drop
                    // the cursor, list shorter → clamp, list empty → drop.
                    self.stepper.reconcile(&config.steppers, &stepper_id);
                }
                edit::Effect::AnnounceProfileChange(name) => {
                    if let Some(emitter) = &self.signal_emitter {
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
    /// multi-switch) last-write-wins order is unchanged. One narrow shift: when
    /// a single retroactive chord miss fires a `ProfileSwitch` member
    /// *alongside* non-switch members, every member's binding now resolves
    /// against the pre-switch Profile and the switch's effects (stop Toggles,
    /// reset axes, stop Analog-repeats) run after them, rather than interleaved
    /// as the old inline `switch_profile` call did — an accepted consequence of
    /// the input path no longer holding `&mut Config` (see ticket 05's Answer).
    /// A failed apply is logged and ignored — a dangling `ProfileSwitch` target
    /// is impossible post-`validate`, so this only ever absorbs a genuine
    /// `config.toml` write failure.
    async fn commit_input_edits(
        &mut self,
        edits: Vec<edit::Edit>,
        config: &mut Config,
        config_path: &Path,
    ) {
        for edit in edits {
            match edit::apply(config, config_path, edit).await {
                Ok(outcome) => self.run_effects(outcome.effects, config).await,
                Err(err) => eprintln!(
                    "acheron-daemon: dispatch: ignoring a failed input-path Config edit: {err:?}"
                ),
            }
        }
    }

    /// Four arms (ticket 11): `GetConfig` / `GetState` / `StopAllToggles`
    /// inline, and one `Apply` arm — the sole mutating path — that `edit::apply`s
    /// the `Edit`, sends the reply (carrying `Outcome.created`) **before**
    /// `run_effects`, and runs effects only on success. Reply-before-effects is
    /// uniform, which is what deleted `SwitchProfile`'s old special-case
    /// reply-before-signal reasoning: that ordering is now the default shape.
    async fn handle_command(&mut self, config: &mut Config, config_path: &Path, cmd: Command) {
        match cmd {
            Command::GetConfig(reply) => {
                let _ = reply.send(config.clone());
            }
            Command::GetState(reply) => {
                // One reported cursor per library entry, `0` ("the list's
                // first item") for one never yet stepped (ticket 03/54).
                let stepper_cursors = self.stepper.snapshot(&config.steppers);
                let _ = reply.send(State {
                    profile: config.active_profile.clone(),
                    layer: self.active_layer.as_str(),
                    active_toggles: self.toggles.keys().copied().collect(),
                    device_connected: self.device_connected,
                    capture_mode: self.capture_mode.as_str(),
                    daemon_version: crate::VERSION,
                    firmware_version: self
                        .device_info
                        .as_ref()
                        .map(|info| info.firmware_version.clone()),
                    serial_number: self
                        .device_info
                        .as_ref()
                        .map(|info| info.serial_number.clone()),
                    stepper_cursors,
                });
            }
            Command::StopAllToggles { reply } => {
                stop_all_toggles(&mut self.toggles).await;
                let _ = reply.send(());
            }
            Command::Apply { edit, reply } => {
                // The sole mutating path (ticket 11): the old `commit!` body
                // inlined once. `reply` carries `Outcome.created` — `None` for
                // the 22 non-create edits, `Some` for `CreateMacro` /
                // `CreateStepper` — and is sent before effects run.
                match edit::apply(config, config_path, edit).await {
                    Ok(outcome) => {
                        let _ = reply.send(Ok(outcome.created));
                        self.run_effects(outcome.effects, config).await;
                    }
                    Err(err) => {
                        let _ = reply.send(Err(err));
                    }
                }
            }
        }
    }
}

/// Returns an error once the injector channel closes, or the capture
/// channel closes (meaning the capture task has died) — per issue 07, a
/// genuine, fatal capture-pipeline error rather than something to swallow
/// silently. The command channel closing is not fatal: it only means the
/// D-Bus server side has gone away, and this task's other job (capture ->
/// injector passthrough/remapping) still has work to do.
// The 13 startup parameters: the `rx_*` receivers (plus `config` /
// `config_path`) stay `run` locals — pure `select!`-loop plumbing no handler
// touches — and the rest are threaded once into `DispatchState` below.
// Clippy's arg-count lint fires only here now (ticket 09): the struct literal
// that consumes them trips nothing, and every `handle_*` helper is a
// `&mut self` method.
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
    // Every piece of the dispatch task's ephemeral runtime state, built fresh
    // on this task start (see `DispatchState` / `DispatchState::new` for the
    // task-start values). Each `select!` arm's handler is a `&mut self` method
    // on it.
    let mut state = DispatchState::new(
        injector,
        signal_emitter,
        actuation_tx,
        capture_control_tx,
        toggle_lap_target,
    );
    // Pure `select!`-loop plumbing — the `rx_*` receivers stay `run` locals
    // (so no `select!` branch expression borrows `state`) and these liveness
    // flags travel with them; no handler reads either.
    let mut commands_open = true;
    let mut connection_open = true;
    let mut capture_mode_open = true;
    let mut device_info_open = true;
    let mut depth_open = true;
    loop {
        tokio::select! {
            event = rx_events.recv() => {
                let Some(event) = event else { break };
                let edits = state.handle_event(&config, event).await?;
                if !edits.is_empty() {
                    state.commit_input_edits(edits, &mut config, &config_path).await;
                }
            }
            changed = rx_depth.changed(), if depth_open => {
                match changed {
                    Ok(()) => {
                        // Two independent engines sharing only the snapshot
                        // value — axis-assignment resolution and the
                        // Analog-repeat spawn/stop policy.
                        let snapshot = rx_depth.borrow_and_update().clone();
                        state.handle_depth_update(&config, &snapshot).await;
                        state.update_analog_repeats(&config, &rx_depth, &snapshot).await;
                    }
                    Err(_) => depth_open = false,
                }
            }
            () = wait_for_chord_deadline(chord::next_deadline(&state.chord_machine)) => {
                let edits = match chord::tick(&mut state.chord_machine, Instant::now()) {
                    chord::ChordOutcome::Handled(effects) => {
                        state.run_chord_effects(&config, effects).await?
                    }
                    chord::ChordOutcome::NotMine => Vec::new(),
                };
                if !edits.is_empty() {
                    state.commit_input_edits(edits, &mut config, &config_path).await;
                }
            }
            connected = rx_connection.recv(), if connection_open => {
                match connected {
                    Some(connected) => handle_connection_change(
                        &mut state.device_connected,
                        &state.signal_emitter,
                        connected,
                    )
                    .await,
                    None => connection_open = false,
                }
            }
            mode = rx_capture_mode.recv(), if capture_mode_open => {
                match mode {
                    Some(mode) => handle_capture_mode_change(
                        &mut state.capture_mode,
                        &state.signal_emitter,
                        &mut state.analog_repeat,
                        mode,
                    )
                    .await,
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
                    Some(update) => state.device_info = update,
                    None => device_info_open = false,
                }
            }
            cmd = rx_commands.recv(), if commands_open => {
                match cmd {
                    Some(cmd) => state.handle_command(&mut config, &config_path, cmd).await,
                    None => commands_open = false,
                }
            }
        }
    }
    Ok(())
}

/// Builds the `trigger::Slot` liveness snapshot `chord::feed` wants from
/// dispatch's `ChordRuntime` — `toggles` → `Toggle`, `firings` →
/// `FiringUnfinished` / `FiringFinished` by `handle.is_finished()`. Firings
/// are inserted first so a `Toggle` entry wins if a live re-bind ever left
/// both (matching the old `starting`/`stopping` filters, which checked the
/// toggle map first). `slot_for` reads the same three states for a single key
/// on the executor paths, but with the opposite tie-break — see its doc.
fn chord_slots(runtime: &ChordRuntime) -> HashMap<ChordKey, trigger::Slot> {
    let mut live = HashMap::new();
    for (key, handle) in &runtime.firings {
        let slot = if handle.is_finished() {
            trigger::Slot::FiringFinished
        } else {
            trigger::Slot::FiringUnfinished
        };
        live.insert(key.clone(), slot);
    }
    for key in runtime.toggles.keys() {
        live.insert(key.clone(), trigger::Slot::Toggle);
    }
    live
}

/// The single-key liveness read `trigger::decide`'s overlap guard needs, over
/// a `(firings, toggles)` pair — `Input`-keyed on the individual path,
/// `ChordKey`-keyed on the Chord `FireChord` path. Replaces the old `fire` /
/// `execute_chord_fire` inline `firings.get(&key).is_finished()` checks, which
/// consulted *only* the firings map — so a firing entry wins here (the guard
/// `decide` makes is purely `Some(FiringUnfinished)`), and `Toggle` is only
/// reported as a fallback when no firing exists. This differs deliberately
/// from `chord_slots`' toggle-wins tie-break, which serves `chord::feed`'s
/// completion logic, not this guard.
fn slot_for<K: Eq + Hash>(
    firings: &HashMap<K, FiringHandle>,
    toggles: &HashMap<K, ActiveToggle>,
    key: &K,
) -> Option<trigger::Slot> {
    if let Some(handle) = firings.get(key) {
        return Some(if handle.is_finished() {
            trigger::Slot::FiringFinished
        } else {
            trigger::Slot::FiringUnfinished
        });
    }
    toggles.contains_key(key).then_some(trigger::Slot::Toggle)
}

/// The runtime state `perform_trigger` performs a `trigger::TriggerDecision`
/// against, generic over the slot key (`Input` for the individual path,
/// `ChordKey` for the Chord path). Built fresh per call site from a disjoint
/// `&mut self.<map>` borrow plus `&config` (ticket 05/08) — never held across
/// a `select!` poll. It survives the ticket-09 `DispatchState` reshape
/// precisely because a `&mut self` method can't be generic over which map
/// type `K` selects, so `perform_trigger<K>` stays a free function.
/// Dispatch-internal: never part of `trigger`'s interface.
struct TriggerCtx<'a, K> {
    injector: &'a Injector,
    firings: &'a mut HashMap<K, FiringHandle>,
    toggles: &'a mut HashMap<K, ActiveToggle>,
    macros: &'a HashMap<MacroId, MacroDef>,
    steppers: &'a HashMap<StepperId, StepperDef>,
    cursors: &'a mut stepper::Cursors,
    toggle_lap_target: Duration,
}

/// Performs `decision` against `ctx` — `compile_action` (behind the overlap
/// guard `trigger::decide` already cleared, so a dropped Fire-once /
/// Hold-to-repeat `Step` firing never advances the cursor) + `executor::
/// spawn_fire_once` / `ActiveToggle::spawn{,_held}` + map insert, or
/// `trigger::force_release_stuck`. Never produces an `edit::Edit`
/// (`ProfileSwitch` is handled before this is ever reached). The old `fire` /
/// `execute_chord_fire` executor halves, now one generic function.
async fn perform_trigger<K: Eq + Hash + Clone>(
    decision: trigger::TriggerDecision,
    key: K,
    binding: &Binding,
    ctx: &mut TriggerCtx<'_, K>,
) -> io::Result<()> {
    use trigger::TriggerDecision as D;
    match decision {
        D::Nothing => {}
        D::SpawnFireOnce => {
            let steps = compile_action(&binding.action, ctx.macros, ctx.steppers, ctx.cursors);
            let handle = executor::spawn_fire_once(ctx.injector.clone(), steps);
            ctx.firings.insert(key, handle);
        }
        D::HoldKeyDown(code) => {
            // A bare, unbalanced `KeyDown` mirroring the physical hold —
            // released by a `ForceReleaseStuck` (individual) or
            // `ChordEffect::ReleaseChordFiring` (Chord) later, reusing ticket
            // 33's force-release path rather than inventing new architecture.
            let handle =
                executor::spawn_fire_once(ctx.injector.clone(), vec![MacroStep::KeyDown(code)]);
            ctx.firings.insert(key, handle);
        }
        D::StartToggleLoop => {
            let steps = compile_action(&binding.action, ctx.macros, ctx.steppers, ctx.cursors);
            ctx.toggles.insert(
                key,
                ActiveToggle::spawn(ctx.injector.clone(), steps, ctx.toggle_lap_target),
            );
        }
        D::StartToggleHeld(code) => {
            ctx.toggles
                .insert(key, ActiveToggle::spawn_held(ctx.injector.clone(), code));
        }
        D::ForceReleaseStuck => {
            trigger::force_release_stuck(ctx.firings, &key, ctx.injector).await;
        }
    }
    Ok(())
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
    axis: &mut axis::Engine,
    analog_repeat: &mut analog_repeat::Engine,
    state: EventState,
) {
    let new_layer = match state {
        EventState::Down => Layer::Held,
        EventState::Up => Layer::Base,
        EventState::Repeat => return,
    };
    if new_layer == *active_layer {
        return;
    }
    *active_layer = new_layer;
    for w in axis.reset() {
        let _ = injector.set_axis_value(w.code, w.value).await;
    }
    analog_repeat.stop_all().await;
    if let Some(emitter) = signal_emitter {
        let _ = Daemon::active_layer_changed(emitter, new_layer.as_str()).await;
    }
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
/// the individual Trigger-mode path's own Hold-to-repeat-equivalent fallback
/// instead (`trigger::decide` treats Analog-repeat as Hold-to-repeat) — a
/// still-running task would otherwise double-fire alongside it.
async fn handle_capture_mode_change(
    capture_mode: &mut CaptureMode,
    signal_emitter: &Option<SignalEmitter<'static>>,
    analog_repeat: &mut analog_repeat::Engine,
    mode: CaptureMode,
) {
    if mode == *capture_mode {
        return;
    }
    *capture_mode = mode;
    if mode == CaptureMode::Digital {
        analog_repeat.stop_all().await;
    }
    if let Some(emitter) = signal_emitter {
        let _ = Daemon::capture_mode_changed(emitter, mode.as_str()).await;
    }
}

/// Compiles a Binding's `Action` into the flat step sequence
/// `perform_trigger` spawns — `executor::compile` for every ordinary Action,
/// or, for `Action::Step`, `stepper::Cursors::step` (which advances the
/// Daemon-owned per-list cursor `executor::compile` has no access to, ticket
/// 03/54) followed by `executor::compile_stepper_item`. A zero-item list
/// steps to nothing.
fn compile_action(
    action: &Action,
    macros: &HashMap<MacroId, MacroDef>,
    steppers: &HashMap<StepperId, StepperDef>,
    cursors: &mut stepper::Cursors,
) -> Vec<executor::MacroStep> {
    match action {
        Action::Step {
            stepper: id,
            direction,
        } => cursors
            .step(steppers, id, *direction)
            .map(executor::compile_stepper_item)
            .unwrap_or_default(),
        other => executor::compile(other, macros),
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
    use crate::capture::EventState;
    use crate::config::{
        Action, ActuationPoint, AxisTarget, DEFAULT_PROFILE_NAME, MacroStepDto, Modifiers, Profile,
        StepDirection, StepperItem,
    };
    use crate::edit::{CommandError, CreatedId};
    use crate::injector::testing::RecordingSink;
    use crate::injector::{self};
    use crate::input::{Direction, WheelEvent};
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::oneshot;

    /// Mirrors of `analog_repeat`'s documented per-fire hold dwells — its
    /// constants are private, and the values themselves are pinned by
    /// `analog_repeat::tests::pulse_hold_for_*`. The kept integration tests
    /// below only check that the engine's spawned task actually drives uinput
    /// on that cadence across the module boundary.
    const AR_PULSE_HOLD: Duration = Duration::from_millis(15);
    const AR_CONTROLLER_PULSE_HOLD: Duration = Duration::from_millis(35);

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

    /// The direct `DispatchState` seam (ticket 09): a `RecordingSink` injector
    /// plus an in-memory `Config`, no channels and no tempfile. Feed
    /// `PhysicalEvent`s (and `Command`s) straight into the handler methods and
    /// read the injected batches back — it replaces the old `run`-plus-
    /// `FakeCaptureSource` `run_scripted` helper for the per-handler-logic
    /// tests that never needed the `select!` plumbing. Tests that genuinely
    /// exercise channel-close, persist-failure rollback, `select!` arm
    /// interleaving, or the D-Bus round trip keep the full `CommandHarness`
    /// rig.
    struct Seam {
        state: DispatchState,
        config: Config,
        sink: RecordingSink,
        gamepad_sink: RecordingSink,
        inj: Injector,
        inj_handle: tokio::task::JoinHandle<io::Result<()>>,
    }

    impl Seam {
        fn new(config: Config) -> Self {
            let sink = RecordingSink::new();
            let gamepad_sink = RecordingSink::new();
            let (inj, inj_handle) = injector::spawn(sink.clone(), gamepad_sink.clone());
            let state = DispatchState::new(
                inj.clone(),
                None,
                actuation_channel(),
                capture_control_channel(),
                executor::MIN_TOGGLE_LAP,
            );
            Seam {
                state,
                config,
                sink,
                gamepad_sink,
                inj,
                inj_handle,
            }
        }

        fn with_bindings(bindings: HashMap<Input, Binding>) -> Self {
            Self::new(config_with_bindings(bindings))
        }

        /// Feeds one event through `handle_event`, then yields a handful of
        /// times so any firing it spawned gets to run — the same
        /// `yield_now` spacing the `CommandHarness` tests put between presses.
        async fn feed(&mut self, event: PhysicalEvent) -> Vec<edit::Edit> {
            let edits = self.state.handle_event(&self.config, event).await.unwrap();
            for _ in 0..5 {
                tokio::task::yield_now().await;
            }
            edits
        }

        async fn press(&mut self, input: Input) {
            let edits = self
                .feed(PhysicalEvent {
                    input,
                    state: EventState::Down,
                    depth: None,
                })
                .await;
            assert!(edits.is_empty(), "unexpected input-path Edits: {edits:?}");
        }

        async fn repeat(&mut self, input: Input) {
            let edits = self
                .feed(PhysicalEvent {
                    input,
                    state: EventState::Repeat,
                    depth: None,
                })
                .await;
            assert!(edits.is_empty(), "unexpected input-path Edits: {edits:?}");
        }

        async fn release(&mut self, input: Input) {
            let edits = self
                .feed(PhysicalEvent {
                    input,
                    state: EventState::Up,
                    depth: None,
                })
                .await;
            assert!(edits.is_empty(), "unexpected input-path Edits: {edits:?}");
        }

        async fn get_state(&mut self) -> State {
            let (reply, rx) = oneshot::channel();
            self.state
                .handle_command(
                    &mut self.config,
                    &unused_config_path(),
                    Command::GetState(reply),
                )
                .await;
            rx.await.unwrap()
        }

        fn gamepad_batches(&self) -> Vec<Vec<evdev::InputEvent>> {
            self.gamepad_sink.batches()
        }

        /// Stops every background task the state still owns, then drains the
        /// injector — mirrors `run` returning and the drop-and-join tail the
        /// full-rig helpers use.
        async fn finish(mut self) -> Vec<Vec<evdev::InputEvent>> {
            stop_all_toggles(&mut self.state.toggles).await;
            self.state.analog_repeat.stop_all().await;
            drop(self.state);
            drop(self.inj);
            self.inj_handle.await.unwrap().unwrap();
            self.sink.batches()
        }
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

        let mut seam = Seam::with_bindings(HashMap::new());
        for event in &scripted {
            seam.feed(*event).await;
        }
        let batches = seam.finish().await;
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

        let mut seam = Seam::with_bindings(bindings);
        seam.press(Input::Grid(1, 1)).await;
        let batches = seam.finish().await;

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

        let mut seam = Seam::new(config_with_bindings_and_macros(bindings, macros));

        // The `press` helper's own `yield_now` spacing lets the one-step
        // firing (no Delay) finish before the physical release lands — the
        // realistic case, since a physical press/release cycle vastly
        // outlasts an instant single-step Macro.
        seam.press(Input::Grid(1, 1)).await;
        seam.release(Input::Grid(1, 1)).await;
        let batches = seam.finish().await;

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
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action: Action::ControllerButton {
                    button: evdev::KeyCode::BTN_SOUTH,
                },
            },
        );
        let mut seam = Seam::with_bindings(bindings);

        seam.press(Input::Grid(1, 1)).await;
        for _ in 0..3 {
            seam.repeat(Input::Grid(1, 1)).await;
        }
        seam.release(Input::Grid(1, 1)).await;

        let batches = seam.gamepad_batches();
        seam.finish().await;

        // Exactly one KeyDown (the physical Down) and one KeyUp (the
        // physical Up) — the three Repeats produced nothing.
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
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::BTN_LEFT,
                },
            },
        );
        let mut seam = Seam::with_bindings(bindings);

        seam.press(Input::Grid(1, 1)).await;
        for _ in 0..3 {
            seam.repeat(Input::Grid(1, 1)).await;
        }
        seam.release(Input::Grid(1, 1)).await;

        let batches = seam.finish().await;

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
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_A,
                },
            },
        );
        let mut seam = Seam::with_bindings(bindings);

        seam.press(Input::Grid(1, 1)).await;
        seam.repeat(Input::Grid(1, 1)).await;
        seam.release(Input::Grid(1, 1)).await;

        let batches = seam.finish().await;

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
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::BTN_LEFT,
                },
            },
        );
        let mut seam = Seam::with_bindings(bindings);

        seam.press(Input::Grid(1, 1)).await;

        // Advance well past several ordinary Toggle laps' worth of time —
        // a looping Toggle would have re-pressed several times by now.
        for _ in 0..7 {
            tokio::time::advance(executor::MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }

        let state = seam.get_state().await;
        assert_eq!(state.active_toggles, vec![Input::Grid(1, 1)]);

        // Same physical key, still toggled on: stops it rather than
        // starting a second one.
        seam.press(Input::Grid(1, 1)).await;

        let batches = seam.finish().await;

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
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action: Action::ControllerButton {
                    button: evdev::KeyCode::BTN_SOUTH,
                },
            },
        );
        let mut seam = Seam::with_bindings(bindings);

        seam.press(Input::Grid(1, 1)).await;

        // Advance well past several ordinary Toggle laps' worth of time —
        // a looping Toggle would have re-pressed several times by now.
        for _ in 0..7 {
            tokio::time::advance(executor::MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }

        let state = seam.get_state().await;
        assert_eq!(state.active_toggles, vec![Input::Grid(1, 1)]);

        // Same physical key, still toggled on: stops it rather than
        // starting a second one.
        seam.press(Input::Grid(1, 1)).await;

        let batches = seam.gamepad_batches();
        seam.finish().await;

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
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_A,
                },
            },
        );
        let mut seam = Seam::with_bindings(bindings);

        seam.press(Input::Grid(1, 1)).await;
        for _ in 0..7 {
            tokio::time::advance(executor::MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }

        seam.press(Input::Grid(1, 1)).await;

        let batches = seam.finish().await;

        assert!(
            batches.len() > 2,
            "a keyboard-key Toggle must still loop (mash), unlike the mouse-button held variant: got {batches:?}"
        );
    }

    #[tokio::test]
    async fn analog_repeat_analog_sourced_events_are_swallowed() {
        // The opposite case from the test above: an Analog-*sourced* Down/
        // Repeat/Up (`event.depth: Some(_)`, synthesized from the key's
        // ordinary Actuation/Release points) must never reach the individual
        // Trigger-mode path at all for an Analog-repeat Binding — real firing
        // is `update_analog_repeats`'s own depth-driven background task,
        // exercised separately below. No depth-watch crossing is ever
        // published here (`depth_channel()`'s Sender is dropped
        // immediately), so if this Binding fell through to `trigger::decide`
        // instead of being swallowed, it would produce ordinary
        // Hold-to-repeat output — this asserts zero output instead.
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
        // hold-solid threshold — the rising edge spawns the task. The exact
        // rate curve is pinned by `analog_repeat::tests::rate_period_*`; this
        // test only checks the spawned task drives repeated pulses and then
        // genuinely stops.
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 100u8)]));
        tokio::task::yield_now().await;

        // Advance ~1s of paused time in small steps so the clock drives each
        // pulse's KeyDown / dwell / KeyUp / period sleep in turn. Depth 100
        // resolves to ≈ 9 Hz (pinned exactly by
        // `analog_repeat::tests::rate_period_*`), so ~1s is ≈ 9 pulses ≈ 18
        // batches. The bound is wide enough for scheduler jitter but still
        // catches a roughly-doubled rate, a halved pulse-hold, or an extra
        // Down/Up pair per tick — the assembled-loop cadence the pure tables
        // can't see.
        for _ in 0..40 {
            tokio::time::advance(Duration::from_millis(25)).await;
            tokio::task::yield_now().await;
        }
        let while_active = sink.batches().len();
        assert!(
            (12..=28).contains(&while_active),
            "expected ≈ 18 Down/Up batches over the 1s window, got {while_active}"
        );

        // Falling back below the deadzone stops the task — a no-op
        // force-release here, since every pulse above already self-released.
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 0u8)]));
        tokio::task::yield_now().await;
        let after_stop = sink.batches().len();

        // Advancing well past several ticks' worth of time produces nothing
        // further — the task is genuinely gone, not just paused between ticks.
        for _ in 0..40 {
            tokio::time::advance(Duration::from_millis(25)).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(sink.batches().len(), after_stop);

        drop(tx);
        drop(depth_tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert!(
            batches.len() >= 4 && batches.len().is_multiple_of(2),
            "expected an even run of Down/Up pulses, got {}",
            batches.len()
        );
        for pair in batches.chunks_exact(2) {
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

        tokio::time::advance(AR_PULSE_HOLD).await;
        tokio::task::yield_now().await;
        assert_eq!(
            sink.batches().len(),
            1,
            "the ordinary 15ms dwell must not release a ControllerButton pulse"
        );

        tokio::time::advance(AR_CONTROLLER_PULSE_HOLD - AR_PULSE_HOLD).await;
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
        let mut seam = Seam::new(config_with_bindings_and_macros(bindings, macros));

        seam.press(Input::Grid(1, 1)).await;
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;

        let state = seam.get_state().await;
        assert_eq!(state.active_toggles, vec![Input::Grid(1, 1)]);

        // Same physical key, still Down: stops the Toggle instead of
        // starting a second one — this press is consumed entirely by the
        // stop, no re-fire. `Seam::press` awaits `handle_event` to
        // completion, which includes the stop's own force-release, so
        // there's nothing racy left to synchronize on here.
        seam.press(Input::Grid(1, 1)).await;

        let batches = seam.finish().await;

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

        /// Sends an `Edit` through the `Command::Apply` channel and awaits the
        /// dispatch task's verdict — the round-trip every typed helper below
        /// shares (ticket 11). Signatures and return types are unchanged; only
        /// the message this builds internally is.
        async fn apply(&self, edit: edit::Edit) -> Result<Option<CreatedId>, CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::Apply { edit, reply })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn set_binding(
            &self,
            input: Input,
            layer: Layer,
            binding: Binding,
        ) -> Result<(), CommandError> {
            self.apply(edit::Edit::SetBinding {
                input,
                layer,
                binding,
            })
            .await
            .map(|_| ())
        }

        async fn clear_binding(&self, input: Input, layer: Layer) -> Result<(), CommandError> {
            self.apply(edit::Edit::ClearBinding { input, layer })
                .await
                .map(|_| ())
        }

        async fn set_chord_binding(
            &self,
            inputs: impl IntoIterator<Item = Input>,
            layer: Layer,
            binding: Binding,
        ) -> Result<(), CommandError> {
            self.apply(edit::Edit::SetChordBinding {
                inputs: inputs.into_iter().collect(),
                layer,
                binding,
            })
            .await
            .map(|_| ())
        }

        async fn set_mode_key_role(&self, role: ModeKeyRole) -> Result<(), CommandError> {
            self.apply(edit::Edit::SetModeKeyRole { role })
                .await
                .map(|_| ())
        }

        async fn create_stepper(
            &self,
            name: &str,
            items: Vec<crate::config::StepperItem>,
        ) -> Result<StepperId, CommandError> {
            match self
                .apply(edit::Edit::CreateStepper {
                    name: name.to_string(),
                    items,
                })
                .await?
            {
                Some(CreatedId::Stepper(id)) => Ok(id),
                other => unreachable!("CreateStepper must mint a Stepper id, got {other:?}"),
            }
        }

        async fn delete_stepper(&self, stepper_id: StepperId) -> Result<(), CommandError> {
            self.apply(edit::Edit::DeleteStepper { stepper_id })
                .await
                .map(|_| ())
        }

        async fn set_stepper_items(
            &self,
            stepper_id: StepperId,
            items: Vec<crate::config::StepperItem>,
        ) -> Result<(), CommandError> {
            self.apply(edit::Edit::SetStepperItems { stepper_id, items })
                .await
                .map(|_| ())
        }

        async fn switch_profile(&self, name: &str) -> Result<(), CommandError> {
            self.apply(edit::Edit::SwitchProfile {
                name: name.to_string(),
            })
            .await
            .map(|_| ())
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
            self.apply(edit::Edit::SetActuationPoint {
                input,
                actuation,
                release,
            })
            .await
            .map(|_| ())
        }

        async fn clear_actuation_point(&self, input: Input) -> Result<(), CommandError> {
            self.apply(edit::Edit::ClearActuationPoint { input })
                .await
                .map(|_| ())
        }

        async fn set_default_actuation(
            &self,
            actuation: u8,
            release: u8,
        ) -> Result<(), CommandError> {
            self.apply(edit::Edit::SetDefaultActuation { actuation, release })
                .await
                .map(|_| ())
        }

        async fn reset_actuation_points(&self) -> Result<(), CommandError> {
            self.apply(edit::Edit::ResetActuationPoints)
                .await
                .map(|_| ())
        }

        async fn set_force_digital(&self, force: bool) -> Result<(), CommandError> {
            self.apply(edit::Edit::SetForceDigital { force })
                .await
                .map(|_| ())
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

        async fn clear_axis_assignment(
            &self,
            input: Input,
            layer: Layer,
        ) -> Result<(), CommandError> {
            self.apply(edit::Edit::ClearAxisAssignment { input, layer })
                .await
                .map(|_| ())
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
        let mut seam = Seam::new(config_with_profile(profile_with_held_bindings(held)));

        // Base layer: Grid(1,1) is unbound there, so pressing it while the
        // Mode key is up must passthrough (KEY_1), never the Held binding.
        seam.press(Input::Grid(1, 1)).await;

        seam.feed(PhysicalEvent {
            input: Input::ModeKey,
            state: EventState::Down,
            depth: None,
        })
        .await;
        assert_eq!(seam.get_state().await.layer, "held");

        // Held layer active: the same physical key now fires the Held
        // Binding instead.
        seam.press(Input::Grid(1, 1)).await;

        let batches = seam.finish().await;

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
        let mut seam = Seam::new(config_with_profile(profile_with_held_bindings(held)));

        seam.feed(PhysicalEvent {
            input: Input::ModeKey,
            state: EventState::Down,
            depth: None,
        })
        .await;
        seam.feed(PhysicalEvent {
            input: Input::ModeKey,
            state: EventState::Up,
            depth: None,
        })
        .await;

        let state = seam.get_state().await;
        assert_eq!(state.layer, "base");

        // Base resumed: Grid(1,1) is unbound there, so this passes through.
        seam.press(Input::Grid(1, 1)).await;

        let batches = seam.finish().await;
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
        let mut seam = Seam::with_bindings(HashMap::new());

        seam.feed(PhysicalEvent {
            input: Input::ModeKey,
            state: EventState::Down,
            depth: None,
        })
        .await;
        seam.feed(PhysicalEvent {
            input: Input::ModeKey,
            state: EventState::Up,
            depth: None,
        })
        .await;

        let batches = seam.finish().await;
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

    // `StepperItem` → `Vec<MacroStep>` compilation (tickets 63 / 92) is
    // covered by `executor::tests::compile_stepper_item_*` since post-release
    // ticket 12 moved that match to `executor::compile_stepper_item`; the
    // cursor movement it feeds is covered by `stepper::tests`.

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
    /// leave the deleted Stepper's runtime cursor sitting in the cursor map —
    /// since `unique_stepper_id` can reassign a freed slug to a brand-new,
    /// unrelated `CreateStepper` call, a stale nonzero cursor would leak into
    /// that new entry's very first `GetState()`, violating "always resets to
    /// the list's first item." Now `edit::plan` emits
    /// `Effect::ReconcileStepperCursor` and `stepper::Cursors::reconcile`
    /// drops it.
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
    /// end, so `GetState()` reported an out-of-range index until the Stepper
    /// was next fired (only a subsequent `step` clamped). Now
    /// `stepper::Cursors::reconcile` clamps it at commit time.
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
        // Code-review finding (guarded here across the run_effects → engine →
        // uinput boundary): axis resolution used to only ever walk the codes
        // `axis_map` currently names — a code that drops out entirely (its
        // last remaining Input cleared) was never revisited, so its
        // last-written nonzero value stuck forever. `axis::Engine::recompute`
        // carries the stale-code sweep; `axis::tests` covers the decision.
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
    async fn a_digital_sourced_axis_event_routes_through_the_engine_to_uinput() {
        // Thin routing guard for `handle_event`'s Digital-mode branch
        // (`event.depth.is_none()` -> `axis::Engine::step_digital` -> the
        // emit loop). The ramp / saturate / reset *decision* is owned by
        // `axis::tests::step_digital_*`; this only checks the seam still
        // crosses the module boundary and reaches the gamepad device.
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
                (evdev::AbsoluteAxisCode::ABS_Z, 64),
                (evdev::AbsoluteAxisCode::ABS_Z, 128),
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
