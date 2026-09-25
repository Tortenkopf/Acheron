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
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use zbus::object_server::SignalEmitter;

use crate::analog_repeat;
use crate::axis;
use crate::capture::analog::{DeviceInfo, RepeatSchedule};
use crate::capture::{CaptureMode, EventState, PhysicalEvent};
use crate::chord;
use crate::command::{Command, State};
use crate::config::{
    self, Action, ActuationPoint, ChordKey, Config, Layer, LightingState, ModeKeyRole, StatusLeds,
    TriggerMode,
};
use crate::dbus::Daemon;
use crate::edit::{self, TeardownReason};
use crate::injector::Injector;
use crate::input::Input;
use crate::stage;
use crate::stepper;
use crate::trigger;

/// Every piece of ephemeral runtime state the dispatch task owns — built
/// fresh on every task start, the same lifetime as the loose `run` locals it
/// replaces. NOT `Config` (committed state; stays a `run` local so the input
/// path keeps only `&Config` — ticket 05). NOT the `rx_*` receivers or their
/// `*_open` liveness flags (pure `select!`-loop plumbing; no handler touches
/// them). No lifetime parameter — every handle below is owned and `'static`.
/// Dispatch-internal: never part of any module's interface. Adding a new
/// piece of dispatch runtime state means a field here, not a fresh `run`
/// local or a new `handle_*` parameter (CONTRIBUTING.md). Lifecycle teardown
/// of these fields — what gets released on a Layer switch / Profile switch /
/// disconnect / Digital-mode flip — lives in the single `tear_down(reason)`
/// match and nowhere else (ticket 19, ADR-0010).
struct DispatchState {
    /// The individual (`Input`-keyed) firing/toggle handle pair — was the
    /// loose `in_flight` + `toggles` maps (post-release ticket 15).
    individual: trigger::Slots<Input>,
    stepper: stepper::Cursors,
    active_layer: Layer,
    chord_machine: chord::ChordMachine,
    /// The Chord path's own `ChordKey`-keyed firing/toggle handle pair — the
    /// runtime *handles* the pure `chord` state machine (post-release ticket
    /// 07) never holds; `chord::feed` is handed a `trigger::Slot` snapshot
    /// (`chord_slots.snapshot()`) each call. Was the `ChordRuntime` struct
    /// (ticket 01/40), now the same `trigger::Slots<K>` the individual path
    /// uses, the way `axis::Engine` bundles its own two maps.
    chord_slots: trigger::Slots<ChordKey>,
    axis: axis::Engine,
    analog_repeat: analog_repeat::Engine,
    /// The third depth engine (`tartarus-dual-stage-keys` ticket 03),
    /// alongside `axis`/`analog_repeat` above — drives every dual-stage
    /// grid key's Handoff/No-Return/Quick-Skip state machine off the same
    /// live-Depth stream `handle_depth_update`/`update_analog_repeats`
    /// already read (`update_stages`, called from the same `rx_depth` arm).
    stage: stage::Engine,
    device_connected: bool,
    capture_mode: CaptureMode,
    device_info: Option<DeviceInfo>,
    injector: Injector,
    signal_emitter: Option<SignalEmitter<'static>>,
    actuation_tx: watch::Sender<HashMap<Input, ActuationPoint>>,
    capture_control_tx: mpsc::Sender<bool>,
    toggle_lap_target: Duration,
    /// The live kernel autorepeat envelope every self-driven `value=2` emitter
    /// paces from (`spec-kernel-shaped-repeat.md` §5.2 single-key Toggle,
    /// ticket 05; §5.3 Analog-repeat hold-solid, ticket 06) — resolved once at
    /// Daemon startup (`main.rs`) and threaded to every spawn site as a plain
    /// value, exactly like `toggle_lap_target`. Both emitters read the same
    /// envelope; Analog-repeat's hold-solid phase runs it through
    /// `RepeatSchedule::without_warmup` (no `delay_ms` gap before the first
    /// `value=2` — §5.3), the Toggle path keeps the full envelope.
    toggle_autorepeat_schedule: RepeatSchedule,
    /// The `led` task's `watch::Sender` (`tartarus-status-leds` ticket 02 /
    /// ADR-0006), handed in from `main.rs`. `push_status_leds` sends the
    /// active Profile's triple on it on device (re)connect and — from
    /// ticket 03 on — Profile switch. `Config` is the sole authoritative
    /// triple; there is no cached `led_state` here.
    led_tx: watch::Sender<Option<StatusLeds>>,
    /// The `led` task's second `watch::Sender` (`tartarus-backlight` ticket
    /// 02 / ADR-0012), alongside `led_tx` — same task, independent channel.
    /// `push_lighting` sends the active Profile's whole `LightingState` on
    /// it on device (re)connect, Daemon startup, Profile switch, and
    /// `SetLighting` (ticket 03). `Config` stays the sole authoritative
    /// source; there is no cached `lighting_state` here.
    lighting_tx: watch::Sender<Option<LightingState>>,
}

impl DispatchState {
    /// Builds the struct with every ephemeral field at its task-start value;
    /// the five owned collaborators come from `run`'s startup parameters (and
    /// from stub channels in the test seam). `run` calls this once and then
    /// only drives the `select!` loop.
    #[allow(clippy::too_many_arguments)]
    fn new(
        injector: Injector,
        signal_emitter: Option<SignalEmitter<'static>>,
        actuation_tx: watch::Sender<HashMap<Input, ActuationPoint>>,
        capture_control_tx: mpsc::Sender<bool>,
        toggle_lap_target: Duration,
        toggle_autorepeat_schedule: RepeatSchedule,
        led_tx: watch::Sender<Option<StatusLeds>>,
        lighting_tx: watch::Sender<Option<LightingState>>,
    ) -> Self {
        DispatchState {
            individual: trigger::Slots::default(),
            stepper: stepper::Cursors::default(),
            active_layer: Layer::Base,
            chord_machine: chord::ChordMachine::default(),
            chord_slots: trigger::Slots::default(),
            axis: axis::Engine::default(),
            analog_repeat: analog_repeat::Engine::default(),
            stage: stage::Engine::default(),
            device_connected: true,
            capture_mode: CaptureMode::Digital,
            device_info: None,
            injector,
            signal_emitter,
            actuation_tx,
            capture_control_tx,
            toggle_lap_target,
            toggle_autorepeat_schedule,
            led_tx,
            lighting_tx,
        }
    }

    /// Sends the active Profile's Status LED assignment on the `led` watch
    /// channel (`tartarus-status-leds` ticket 02 — CONTEXT.md: Status LED
    /// assignment; ADR-0006). Reads the triple straight from the
    /// just-committed `Config` — the sole authoritative source, no cached
    /// copy in `DispatchState`. `send_replace` (not `send`) mirrors this
    /// file's other `watch` publishers (`actuation_tx`, `depth_tx`) and
    /// never errors on a dropped receiver during teardown; `watch`
    /// coalescing collapses a burst to the final triple in the `led` task.
    /// Two call sites of this one helper: the `rx_connection` arm (connect)
    /// and — from ticket 03 — `run_effects` (`Effect::AssertStatusLeds`,
    /// Profile switch / `SetStatusLeds`).
    fn push_status_leds(&self, config: &Config) {
        let leds = config
            .active_profile()
            .expect("load_or_seed validates active_profile names a real profile")
            .status_leds;
        self.led_tx.send_replace(Some(leds));
    }

    /// Sends the active Profile's whole Lighting state on the `led` task's
    /// second watch channel (`tartarus-backlight` ticket 02 — CONTEXT.md:
    /// Lighting assignment; ADR-0012). Reads `lighting`/`brightness` straight
    /// from the just-committed `Config` — the sole authoritative source, no
    /// cached `LightingState` in `DispatchState` — the same discipline
    /// `push_status_leds` follows. Two call sites of this one helper: the
    /// `rx_connection` arm (connect/startup) and — from ticket 03 —
    /// `run_effects` (`Effect::AssertLighting`, Profile switch /
    /// `SetLighting`), mirroring `push_status_leds`'s own two call sites.
    fn push_lighting(&self, config: &Config) {
        let profile = config
            .active_profile()
            .expect("load_or_seed validates active_profile names a real profile");
        self.lighting_tx.send_replace(Some(LightingState {
            assignment: profile.lighting.clone(),
            brightness: profile.brightness,
        }));
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
            self.handle_layer_switch(event.state).await;
            return Ok(Vec::new());
        }

        // A Down on an Input with an active Toggle always stops that Toggle
        // first, regardless of what Binding the Input's current Layer nominally
        // assigns — this press is consumed entirely by the stop, per spec.md's
        // "Toggle behavior across Layer/Profile switches". Only a later press
        // resumes normal evaluation.
        if event.state == EventState::Down && self.individual.stop_toggle(&event.input).await {
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
        let live = self.chord_slots.snapshot();
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

        // The Staging-mode state machine (`tartarus-dual-stage-keys`,
        // `post-release-development` ticket 17) runs unconditionally here,
        // mirroring `chord::feed` just above — it owns the "is this a
        // dual-stage key on the active Layer?" predicate and interprets this
        // physical edge against the key's Quick-Skip / deep-repeat machine,
        // folding what used to be two hand-rolled blocks (the `quick_skip_key`
        // divert and the general deep-repeat swallow) plus the
        // `begin_windowed_press` / `is_late` / `primary_handed_off` / `deep_repeat`
        // reach-through into one call. `Handled` ⇒ the edge is consumed;
        // `NotMine { machine_sequenced }` ⇒ run the ordinary Binding path,
        // and — when `feed` tracks this key — build the following `perform`
        // with `PerformDeps::new_machine_sequenced` so a dual-stage primary's
        // press carries no Fire-once dwell (ticket 17 Addendum).
        let deps = stage::EngineDeps {
            config,
            active_layer: self.active_layer,
            individual: &mut self.individual,
            injector: &self.injector,
            cursors: &mut self.stepper,
            toggle_lap_target: self.toggle_lap_target,
            toggle_autorepeat_schedule: self.toggle_autorepeat_schedule,
        };
        let machine_sequenced = match self.stage.feed(deps, event).await? {
            stage::StageOutcome::Handled(edits) => return Ok(edits),
            stage::StageOutcome::NotMine { machine_sequenced } => machine_sequenced,
        };

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
                // The bound → `trigger::decide` + `Slots::perform` /
                // `ProfileSwitch` → `Edit` / unbound → passthrough tail, shared
                // verbatim with the Chord machine's `FireIndividual` executor so
                // the retroactive-fire logic exists once. `machine_sequenced`
                // (a dual-stage key's primary press, per `stage::Engine::feed`)
                // drops the Fire-once dwell — ticket 17 Addendum.
                self.dispatch_individual_down(config, event.input, machine_sequenced)
                    .await
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
                let slot = self.individual.slot(&event.input);
                let decision = trigger::decide(&binding, &config.macros, event.state, slot);
                // A dual-stage key's primary Repeat/Up is machine-sequenced
                // input too (`stage::Engine::feed` → `NotMine { machine_
                // sequenced: true }`); the Fire-once dwell never applies to a
                // Repeat/Up decision, but the constructor choice stays
                // consistent with the `Down` arm above.
                let deps = trigger::PerformDeps::for_input_press(
                    &self.injector,
                    config,
                    &mut self.stepper,
                    self.toggle_lap_target,
                    self.toggle_autorepeat_schedule,
                    machine_sequenced,
                );
                self.individual
                    .perform(decision, event.input, &binding, deps)
                    .await?;
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
                    // generic `Slots::perform`. A Chord's Action is never
                    // `AnalogRepeat` or `ProfileSwitch`, and `chord::feed` only
                    // ever emits `Down` / `Repeat`, so those `decide` arms are
                    // unreachable here.
                    let slot = self.chord_slots.slot(&key);
                    let decision = trigger::decide(&binding, &config.macros, state, slot);
                    let deps = trigger::PerformDeps::new(
                        &self.injector,
                        config,
                        &mut self.stepper,
                        self.toggle_lap_target,
                        self.toggle_autorepeat_schedule,
                    );
                    self.chord_slots
                        .perform(decision, key, &binding, deps)
                        .await?;
                }
                chord::ChordEffect::ReleaseChordFiring { key } => {
                    // Fire-once / Hold-to-repeat only — a Toggle Chord is
                    // deliberately not stopped by a member's `Up` (ticket 67).
                    self.chord_slots.force_release(&key, &self.injector).await;
                }
                chord::ChordEffect::StopChordToggle { key } => {
                    self.chord_slots.stop_toggle(&key).await;
                }
                chord::ChordEffect::FireIndividual { input } => {
                    // A Chord member can never be a dual-stage key
                    // (`ChordMemberDeepStageConflict`), so this retroactive
                    // Down is a genuine user one-shot — it keeps the Fire-once
                    // dwell (`machine_sequenced: false`), ticket 17 Addendum.
                    edits.extend(self.dispatch_individual_down(config, input, false).await?);
                }
                chord::ChordEffect::ForceReleaseIndividual { input } => {
                    self.individual.force_release(&input, &self.injector).await;
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
    /// `Slots::perform` / unbound → passthrough tail carved out of
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
    ///
    /// `machine_sequenced` selects the `PerformDeps` constructor (ticket 17
    /// Addendum): the ordinary input path passes it through from
    /// `stage::Engine::feed` (`true` for a dual-stage key's primary press, so
    /// it carries no Fire-once dwell); the Chord `FireIndividual` executor
    /// always passes `false` — a Chord member can never be a dual-stage key
    /// (`ChordMemberDeepStageConflict`), so that retroactive Down is a genuine
    /// user one-shot.
    async fn dispatch_individual_down(
        &mut self,
        config: &Config,
        input: Input,
        machine_sequenced: bool,
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
                let slot = self.individual.slot(&input);
                let decision = trigger::decide(&binding, &config.macros, EventState::Down, slot);
                let deps = trigger::PerformDeps::for_input_press(
                    &self.injector,
                    config,
                    &mut self.stepper,
                    self.toggle_lap_target,
                    self.toggle_autorepeat_schedule,
                    machine_sequenced,
                );
                self.individual
                    .perform(decision, input, &binding, deps)
                    .await?;
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
    /// the same "once per fire" precedent `trigger::Slots::perform` follows); a
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
            // Depth first crossed the deadzone (mirrors `trigger::Slots::
            // perform`'s own once-per-fire `compile_action` call) —
            // `trigger::compile_action` takes `Config` + the `stepper::Cursors`
            // so the engine needn't, and hands the task pre-compiled steps.
            let steps = trigger::compile_action(
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
                // The same startup-resolved kernel envelope the Toggle path
                // holds at — hold-solid's `value=2` stream paces from it
                // (spec-kernel-shaped-repeat.md §5.3), minus the `delay_ms`
                // warm-up (`RepeatSchedule::without_warmup`, applied in the
                // loop). Not re-read here: an inline blocking read would break
                // the `tokio::time::pause()` test harness (ticket 68).
                self.toggle_autorepeat_schedule,
                depth_rx.clone(),
            );
        }
    }

    /// Drives every dual-stage grid key's Staging-mode state machine off a
    /// fresh `rx_depth` snapshot (`tartarus-dual-stage-keys` ticket 03) — the
    /// third depth-driven step alongside `handle_depth_update`/
    /// `update_analog_repeats`, sharing the same snapshot value. `stage::
    /// Engine::update` performs every op itself (against `self.individual`
    /// for the primary keyspace, its own `Slots<StageKey>` for the deep one)
    /// and returns any `Edit::SwitchProfile` a re-press or a deep firing
    /// produced, for the `run` loop to commit — same shape as `handle_event`'s
    /// own return.
    async fn update_stages(
        &mut self,
        config: &Config,
        snapshot: &HashMap<Input, u8>,
    ) -> io::Result<Vec<edit::Edit>> {
        let deps = stage::EngineDeps {
            config,
            active_layer: self.active_layer,
            individual: &mut self.individual,
            injector: &self.injector,
            cursors: &mut self.stepper,
            toggle_lap_target: self.toggle_lap_target,
            toggle_autorepeat_schedule: self.toggle_autorepeat_schedule,
        };
        self.stage.update(deps, snapshot).await
    }

    /// Fires Quick-Skip's ~50ms window timeout (`tartarus-dual-stage-keys`
    /// ticket 04) — the `run` loop's fourth `select!` arm,
    /// `wait_for_stage_deadline`, mirroring `wait_for_chord_deadline`/
    /// `chord::tick`'s own shape. A no-op call before any key's deadline has
    /// actually elapsed (the same spurious-call tolerance `chord::tick`
    /// extends). On a genuine elapse with the deep band never reached,
    /// `stage::Engine::tick` fires the buffered primary retroactively
    /// (`dispatch_individual_down`'s exact logic, via `stage`'s own `fire`
    /// helper) and flips that key to Late — plain Handoff for the rest of
    /// the press.
    async fn tick_stages(&mut self, config: &Config, now: Instant) -> io::Result<Vec<edit::Edit>> {
        let deps = stage::EngineDeps {
            config,
            active_layer: self.active_layer,
            individual: &mut self.individual,
            injector: &self.injector,
            cursors: &mut self.stepper,
            toggle_lap_target: self.toggle_lap_target,
            toggle_autorepeat_schedule: self.toggle_autorepeat_schedule,
        };
        self.stage.tick(deps, now).await
    }

    /// Center every live axis output and clear the axis engine's state — the
    /// former `Effect::ResetAxisOutputs`, now a `tear_down` step the
    /// `LayerSwitch` and `ProfileSwitch` arms share. Every `ABS_*` write goes
    /// through the one dispatch-side emit loop with the injector error
    /// swallowed (`let _ =`), the ticket 10 unification.
    async fn reset_axis_outputs(&mut self) {
        for w in self.axis.reset() {
            let _ = self.injector.set_axis_value(w.code, w.value).await;
        }
    }

    /// The one place the lifecycle-teardown matrix lives
    /// (`post-release-development` ticket 19, ADR-0010): a single
    /// `match reason` naming every ephemeral participant — `axis`,
    /// `analog_repeat`, `stage`, `individual` firings, `individual` toggles,
    /// `chord_machine`, `chord_slots` — with an explicit `//` line for each
    /// one an arm deliberately leaves alone. Two entry points feed it: a
    /// direct call for the three momentary-state situations
    /// (`handle_layer_switch` / `handle_connection_change` /
    /// `handle_capture_mode_change`), and `Effect::TearDown` for the
    /// Config-commit path (a `SwitchProfile` mutates `Config`, so its
    /// teardown must run from `run_effects` — the sole commit point).
    ///
    /// Each arm's own operation order follows the individual → chord order the
    /// rest of dispatch uses (only `ProfileSwitch`'s toggles-before-firings
    /// has a further documented reason — edit.rs). `post-release-development`
    /// ticket 20 grilled every `//`-marked skip ticket 19 made visible;
    /// ticket 21 turned the six that graduated into real calls (`axis` +
    /// `analog_repeat` on disconnect, `axis` on the Digital flip,
    /// `chord_machine.reset()` + `chord_slots.drain_firings` in every arm,
    /// `chord_slots.stop_all_toggles` on a Profile switch), so Chord teardown
    /// now matches individual teardown exactly. The `//` lines that remain
    /// record the deliberate, spec-backed survivals (individual **and** Chord
    /// Toggles outlive a Layer switch / disconnect / Digital flip).
    ///
    /// Not a `DepthEngine` trait: the engines are radically heterogeneous
    /// (`axis` is sync/infallible, `analog_repeat` owns tokio tasks, `stage`
    /// is async/fallible with a 7-field `EngineDeps`, `chord`'s machine holds
    /// no handles and its teardown target is a sibling field), edition-2024
    /// `dyn` + `async-trait` is not this codebase's idiom, and the `rx_depth`
    /// arm is a sub-millisecond hot path — see ADR-0010.
    async fn tear_down(&mut self, reason: TeardownReason) {
        match reason {
            TeardownReason::LayerSwitch => {
                self.reset_axis_outputs().await;
                self.analog_repeat.stop_all().await;
                self.stage.stop_all(&self.injector).await;
                self.individual.drain_firings(&self.injector).await;
                self.chord_machine.reset();
                self.chord_slots.drain_firings(&self.injector).await;
                // individual + Chord toggles: survive a Layer switch
                //   (keybinder spec.md "Layer change never touches an active
                //   Toggle"; CONTEXT.md Toggle). Firings are not Toggles —
                //   they drain, matching the individual path.
            }
            TeardownReason::ProfileSwitch => {
                self.individual.stop_all_toggles().await;
                self.individual.drain_firings(&self.injector).await;
                self.reset_axis_outputs().await;
                self.analog_repeat.stop_all().await;
                self.stage.stop_all(&self.injector).await;
                self.chord_machine.reset();
                self.chord_slots.stop_all_toggles().await;
                self.chord_slots.drain_firings(&self.injector).await;
                // No `//` survivals: a Profile switch releases *every* active
                //   Toggle immediately (keybinder spec.md), individual and
                //   Chord alike (ticket 21, A6).
            }
            TeardownReason::Disconnect => {
                self.reset_axis_outputs().await;
                self.analog_repeat.stop_all().await;
                self.stage.stop_all(&self.injector).await;
                self.individual.drain_firings(&self.injector).await;
                self.chord_machine.reset();
                self.chord_slots.drain_firings(&self.injector).await;
                // individual + Chord toggles: left running — a disconnect is
                //   not a Layer/Profile change (ticket 21).
            }
            TeardownReason::CaptureModeToDigital => {
                self.reset_axis_outputs().await;
                self.analog_repeat.stop_all().await;
                self.stage.stop_all(&self.injector).await;
                self.individual.drain_firings(&self.injector).await;
                self.chord_machine.reset();
                self.chord_slots.drain_firings(&self.injector).await;
                // individual + Chord toggles: left running — the Digital flip
                //   is not a Layer/Profile change (ticket 21).
            }
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
                    self.individual.stop_toggle(&input).await;
                }
                edit::Effect::TearDown(reason) => self.tear_down(reason).await,
                edit::Effect::StopStage(input) => {
                    self.stage
                        .stop_stage(input, &self.individual, &self.injector)
                        .await;
                }
                edit::Effect::StopChord(key) => {
                    // The Chord-keyspace sibling of `StopStage` (ticket 22):
                    // an `edit` arm removed or changed the Chord backing this
                    // key, so force-stop a live Chord Toggle *and* force-
                    // release a live Chord Hold-to-repeat firing — neither can
                    // reach its own stop edge once the key has left
                    // `chords(layer)`. Both calls no-op when nothing is live.
                    self.chord_slots.stop_toggle(&key).await;
                    self.chord_slots.stop_firing(&key, &self.injector).await;
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
                edit::Effect::AssertStatusLeds => {
                    // A `SetStatusLeds` edit or a Profile switch re-asserts the
                    // active Profile's triple — same helper the `rx_connection`
                    // arm calls on connect, reading the just-committed `config`.
                    self.push_status_leds(config);
                }
                edit::Effect::AssertLighting => {
                    // A `SetLighting` edit or a Profile switch re-asserts the
                    // active Profile's whole Lighting state — same helper the
                    // `rx_connection` arm calls on connect, reading the
                    // just-committed `config` (`tartarus-backlight` ticket 03).
                    self.push_lighting(config);
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
                    active_toggles: self.individual.active_toggle_keys().copied().collect(),
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
                self.individual.stop_all_toggles().await;
                // `tartarus-dual-stage-keys` ticket 06: the manual "attention
                // just moved to the GUI" escape hatch also drains the deep
                // stage's own `Slots<StageKey>` toggles — deliberately more
                // aggressive than the Chord-toggle-survives-a-Profile-switch
                // precedent, since this is a manual, not automatic, teardown.
                self.stage.stop_all_toggles().await;
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
// The startup parameters: the `rx_*` receivers (plus `config` /
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
    // Ticket 05 / 06 / spec-kernel-shaped-repeat.md §5.2, §5.3: the live kernel
    // autorepeat envelope every self-driven `value=2` emitter paces from — a
    // single-key Toggle hold and Analog-repeat's hold-solid phase — resolved
    // once at Daemon startup (`main.rs`) beside `toggle_lap_target` and
    // threaded down the same way; an inline per-press blocking read would
    // break the `tokio::time::pause()` test harness (ticket 68's finding).
    toggle_autorepeat_schedule: RepeatSchedule,
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
    // `tartarus-status-leds` ticket 02 / ADR-0006: the `led` task's
    // `watch::Sender`, created in `main.rs`. Held on `DispatchState` and
    // written by `push_status_leds` — the active Profile's Status LED triple
    // pushed on every device (re)connect (and, from ticket 03, on Profile
    // switch). The `led` task drives it to the hardware; a write failure
    // there never reaches this task.
    led_tx: watch::Sender<Option<StatusLeds>>,
    // `tartarus-backlight` ticket 02 / ADR-0012: the same `led` task's
    // second `watch::Sender`, created in `main.rs` alongside `led_tx`. Held
    // on `DispatchState` and written by `push_lighting` — the active
    // Profile's whole `LightingState` pushed on every device (re)connect
    // (Profile switch and `SetLighting` follow in a later ticket).
    lighting_tx: watch::Sender<Option<LightingState>>,
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
        toggle_autorepeat_schedule,
        led_tx,
        lighting_tx,
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
                        let edits = state.update_stages(&config, &snapshot).await?;
                        if !edits.is_empty() {
                            state.commit_input_edits(edits, &mut config, &config_path).await;
                        }
                    }
                    Err(_) => depth_open = false,
                }
            }
            () = wait_for_deadline(chord::next_deadline(&state.chord_machine)) => {
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
            () = wait_for_deadline(state.stage.next_deadline()) => {
                let edits = state.tick_stages(&config, Instant::now()).await?;
                if !edits.is_empty() {
                    state.commit_input_edits(edits, &mut config, &config_path).await;
                }
            }
            connected = rx_connection.recv(), if connection_open => {
                match connected {
                    Some(connected) => {
                        state.handle_connection_change(connected).await;
                        // `tartarus-status-leds` ticket 02 / ADR-0006: the
                        // firmware reclaims the Status LEDs to its orange-only
                        // default on every USB enumeration, so re-assert the
                        // active Profile's assignment on *every* `connected ==
                        // true` — not on the connection *transition*.
                        // `device_connected` starts optimistically `true` and
                        // `handle_connection_change` early-returns on an
                        // unchanged bool, so a transition-gated assert would
                        // miss the present-at-startup case. Idempotent on the
                        // hardware; a redundant `true` costs one ioctl.
                        if connected {
                            state.push_status_leds(&config);
                            // `tartarus-backlight` ticket 02 / ADR-0012:
                            // same trigger and reasoning as Status LEDs —
                            // re-assert the active Profile's Lighting on
                            // every `connected == true`, not just the
                            // transition (covers present-at-startup too).
                            state.push_lighting(&config);
                        }
                    }
                    None => connection_open = false,
                }
            }
            mode = rx_capture_mode.recv(), if capture_mode_open => {
                match mode {
                    Some(mode) => state.handle_capture_mode_change(mode).await,
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

/// Awaits an `Instant` deadline, or never resolves if there is none — the
/// shared `select!` timeout-arm wrapper for both the Chord window
/// (`chord::next_deadline`, a single global window) and Quick-Skip
/// (`stage::Engine::next_deadline`, the earliest still-armed deadline across
/// every Quick-Skip key). The two arms keep their own next-deadline source —
/// different internals, same `Option<Instant>` — and only this wait wrapper
/// is shared (`post-release-development` ticket 19; the two were
/// byte-identical bar the name). The `select!` branch re-creates this future
/// every loop iteration, so a deadline opened, extended, cancelled, or
/// elapsed by a handler in between is always picked up on the very next
/// iteration (recreating a `sleep_until` against the same absolute `Instant`
/// doesn't lose progress).
async fn wait_for_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

impl DispatchState {
    /// The `LayerSwitch` interception itself: `Down` activates Held, `Up`
    /// reverts to Base, `Repeat` is a steady-state no-op (evdev autorepeat on
    /// a held modifier key carries no new information here). Emits
    /// `ActiveLayerChanged` only on an actual transition — a `signal_emitter`
    /// of `None` (unit tests with no live D-Bus connection) simply skips the
    /// push. On an actual transition the ephemeral teardown (axis reset,
    /// Analog-repeat stop, deep-stage release, held-firing drain) runs through
    /// `tear_down(TeardownReason::LayerSwitch)` — the one lifecycle-teardown
    /// matrix (ticket 19).
    async fn handle_layer_switch(&mut self, state: EventState) {
        let new_layer = match state {
            EventState::Down => Layer::Held,
            EventState::Up => Layer::Base,
            EventState::Repeat => return,
        };
        if new_layer == self.active_layer {
            return;
        }
        self.active_layer = new_layer;
        self.tear_down(TeardownReason::LayerSwitch).await;
        if let Some(emitter) = &self.signal_emitter {
            let _ = Daemon::active_layer_changed(emitter, new_layer.as_str()).await;
        }
    }

    /// Updates the dispatch task's view of device connectivity (ticket 20)
    /// and emits `DeviceConnectionChanged` only on an actual transition —
    /// mirrors `handle_layer_switch`'s pattern for `ActiveLayerChanged`,
    /// including skipping the push when `signal_emitter` is `None` (unit tests
    /// with no live D-Bus connection). A transition to `connected == false`
    /// runs `tear_down(TeardownReason::Disconnect)` — the deep-stage release +
    /// held-firing drain that `tartarus-dual-stage-keys` ticket 06 introduced
    /// here (no generic dropout hook exists — Analog-repeat has none at all,
    /// spec.md's "Out of Scope"), now one arm of the lifecycle-teardown
    /// matrix (ticket 19).
    async fn handle_connection_change(&mut self, connected: bool) {
        if connected == self.device_connected {
            return;
        }
        self.device_connected = connected;
        if !connected {
            self.tear_down(TeardownReason::Disconnect).await;
        }
        if let Some(emitter) = &self.signal_emitter {
            let _ = Daemon::device_connection_changed(emitter, connected).await;
        }
    }

    /// Updates the dispatch task's view of which capture path is running
    /// (ticket 23) and emits `CaptureModeChanged` only on an actual
    /// transition — mirrors `handle_connection_change` exactly, including
    /// skipping the push when `signal_emitter` is `None`. A transition to
    /// Digital runs `tear_down(TeardownReason::CaptureModeToDigital)`: the
    /// live-Depth stream every Analog-repeat task and the deep stage read
    /// goes stale the moment analog capture stops, and Digital-sourced
    /// Down/Repeat/Up events for the same Bindings are about to start
    /// reaching the individual Trigger-mode path — a still-running task or
    /// deep firing would double-fire alongside them (ticket 39 / ticket 03,
    /// now one arm of the lifecycle-teardown matrix, ticket 19).
    async fn handle_capture_mode_change(&mut self, mode: CaptureMode) {
        if mode == self.capture_mode {
            return;
        }
        self.capture_mode = mode;
        if mode == CaptureMode::Digital {
            self.tear_down(TeardownReason::CaptureModeToDigital).await;
        }
        if let Some(emitter) = &self.signal_emitter {
            let _ = Daemon::capture_mode_changed(emitter, mode.as_str()).await;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::EventState;
    use crate::config::{
        Action, ActuationPoint, AxisTarget, Binding, Colour, DEFAULT_PROFILE_NAME, DeepStageConfig,
        FixedEffect, LightingAssignment, MacroDef, MacroId, MacroStepDto, Modifiers, Profile,
        StagingMode, StatusLeds, StepDirection, StepperDef, StepperId, StepperItem,
    };
    use crate::edit::{CommandError, CreatedId};
    use crate::executor;
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

    /// A fresh Status-LED `Sender` for tests that don't assert on what
    /// dispatch pushes to the `led` task (`tartarus-status-leds` ticket 02) —
    /// `dispatch::run` / `DispatchState::new` require the `Sender` half
    /// unconditionally. Tests that *do* check the pushed triple
    /// (`CommandHarness`) keep their own paired `Receiver`.
    fn led_channel() -> watch::Sender<Option<StatusLeds>> {
        watch::channel(None).0
    }

    /// `led_channel`'s Lighting sibling (`tartarus-backlight` ticket 02) —
    /// a fresh `LightingState` `Sender` for tests that don't assert on what
    /// dispatch pushes. `CommandHarness` keeps its own paired `Receiver`.
    fn lighting_channel() -> watch::Sender<Option<LightingState>> {
        watch::channel(None).0
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
                RepeatSchedule::new(250, 33),
                led_channel(),
                lighting_channel(),
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
            self.state.individual.stop_all_toggles().await;
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
        // `hold_to_repeat_keyboard_key_emits_genuine_autorepeat_not_down_up_pairs`),
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
        // `hold_to_repeat_keyboard_key_emits_genuine_autorepeat_not_down_up_pairs`),
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
    async fn hold_to_repeat_keyboard_key_emits_genuine_autorepeat_not_down_up_pairs() {
        // Ticket 04 / spec-kernel-shaped-repeat.md §3.1: a single keyboard key
        // under Hold-to-repeat no longer emits a `[KeyDown, KeyUp]` pair per
        // kernel autorepeat tick. It presents as real Linux autorepeat —
        // `value=1` on the physical Down, one `value=2` per synthesized
        // `EventState::Repeat`, `value=0` (force-released) on the physical Up.
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
        seam.repeat(Input::Grid(1, 1)).await;
        seam.release(Input::Grid(1, 1)).await;

        let batches = seam.finish().await;

        // value=1, then exactly one value=2 per Repeat, then value=0 — no
        // intervening KeyUp/KeyDown, so the ~0ms dwell every pair implied is
        // gone.
        assert_eq!(
            batches
                .iter()
                .map(|b| key_and_value(b[0]))
                .collect::<Vec<_>>(),
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 2),
                (evdev::KeyCode::KEY_A, 2),
                (evdev::KeyCode::KEY_A, 0),
            ],
        );
    }

    #[tokio::test]
    async fn analog_synth_grid_hold_to_repeat_emits_the_same_autorepeat_shape() {
        // Spec §4 surface 2: an Analog-*sourced* Hold-to-repeat on a grid key
        // (`event.depth: Some(_)`, the stream `capture::analog`'s
        // `RepeatSchedule` synthesizes) rides the exact same `value=2` path as
        // the digital surface 1 above — one `value=2` per synthesized `Repeat`.
        // `RepeatSchedule` / `advance_fired` in `capture/analog.rs` are
        // untouched: they still decide *when* a `Repeat` arrives; only the
        // emitted event changed.
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

        // Analog-sourced edges (depth: Some) — what a grid key in Analog
        // capture produces once its Actuation point is crossed.
        for (state, depth) in [
            (EventState::Down, 150u8),
            (EventState::Repeat, 150),
            (EventState::Repeat, 150),
            (EventState::Up, 0),
        ] {
            seam.feed(PhysicalEvent {
                input: Input::Grid(1, 1),
                state,
                depth: Some(depth),
            })
            .await;
        }

        let batches = seam.finish().await;
        assert_eq!(
            batches
                .iter()
                .map(|b| key_and_value(b[0]))
                .collect::<Vec<_>>(),
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 2),
                (evdev::KeyCode::KEY_A, 2),
                (evdev::KeyCode::KEY_A, 0),
            ],
        );
    }

    #[tokio::test]
    async fn hold_to_repeat_modifier_wrapped_key_holds_the_modifier_and_autorepeats_only_the_base()
    {
        // Spec §3.1: `Ctrl`+key under Hold-to-repeat holds `Ctrl` `value=1`
        // alongside the base key `value=1`, then only the base key autorepeats
        // (`value=2`); the physical Up force-releases both. `Ctrl` never emits
        // a `value=2` — exactly what the kernel does for a physically held
        // `Ctrl+X`.
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action: Action::Keypress {
                    modifiers: Modifiers {
                        ctrl: true,
                        ..Modifiers::default()
                    },
                    key: evdev::KeyCode::KEY_X,
                },
            },
        );
        let mut seam = Seam::with_bindings(bindings);

        seam.press(Input::Grid(1, 1)).await;
        seam.repeat(Input::Grid(1, 1)).await;
        seam.release(Input::Grid(1, 1)).await;

        let events = seam
            .finish()
            .await
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect::<Vec<_>>();

        assert_eq!(
            &events[..3],
            &[
                (evdev::KeyCode::KEY_LEFTCTRL, 1),
                (evdev::KeyCode::KEY_X, 1),
                (evdev::KeyCode::KEY_X, 2),
            ],
            "Ctrl held value=1, base key down then one autorepeat"
        );
        // The trailing two events are the force-released Ctrl + X in either
        // order (a drained HashSet) — both value=0, and Ctrl never autorepeated.
        let tail = &events[3..];
        assert_eq!(tail.len(), 2);
        assert!(tail.contains(&(evdev::KeyCode::KEY_LEFTCTRL, 0)));
        assert!(tail.contains(&(evdev::KeyCode::KEY_X, 0)));
        assert!(
            !events.contains(&(evdev::KeyCode::KEY_LEFTCTRL, 2)),
            "the modifier must never autorepeat"
        );
    }

    #[tokio::test]
    async fn layer_switch_while_holding_a_hold_to_repeat_key_force_releases_it() {
        // Ticket 04 / spec §7: every single-key Hold-to-repeat now holds a
        // bare `value=1` for the life of the press. A Layer switch (the
        // default ModeKey action) while the key is held, then releasing it on
        // a Layer where it is unbound, must not strand `KEY_A` down — the
        // switch teardown (`Slots::drain_firings`) balances it. Pre-ticket-04
        // this path used balanced `[Down, Up]` pairs and nothing could stick.
        let mut base = HashMap::new();
        base.insert(
            Input::Grid(1, 1),
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
        );
        // Held layer left empty → Grid(1,1) is unbound there.
        let mut seam = Seam::new(config_with_profile(Profile {
            base,
            ..Default::default()
        }));

        seam.press(Input::Grid(1, 1)).await;
        seam.feed(PhysicalEvent {
            input: Input::ModeKey,
            state: EventState::Down,
            depth: None,
        })
        .await;
        assert_eq!(seam.get_state().await.layer, "held");
        seam.release(Input::Grid(1, 1)).await;

        let key_a: Vec<_> = seam
            .finish()
            .await
            .iter()
            .map(|b| key_and_value(b[0]))
            .filter(|(code, _)| *code == evdev::KeyCode::KEY_A)
            .collect();
        assert_eq!(
            key_a,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "the Layer switch teardown force-released the held KEY_A"
        );
    }

    #[tokio::test]
    async fn a_hold_to_repeat_key_held_across_a_layer_switch_re_presses_on_the_new_layer() {
        // The `decide` fallback that pairs with the teardown above: after
        // `drain_firings` clears the firing, a `Repeat` on the still-held key
        // sees `slot == None` and re-presses (`HoldKeyDown`) rather than
        // emitting a dangling `value=2` for the new Layer's binding.
        let mut base = HashMap::new();
        base.insert(
            Input::Grid(1, 1),
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
        );
        let mut held = HashMap::new();
        held.insert(
            Input::Grid(1, 1),
            hold_to_repeat_binding(evdev::KeyCode::KEY_B),
        );
        let mut seam = Seam::new(config_with_profile(Profile {
            base,
            held,
            ..Default::default()
        }));

        seam.press(Input::Grid(1, 1)).await; // KEY_A value=1
        seam.feed(PhysicalEvent {
            input: Input::ModeKey,
            state: EventState::Down,
            depth: None,
        })
        .await; // drain → KEY_A value=0
        seam.repeat(Input::Grid(1, 1)).await; // Held layer, no firing → re-press KEY_B value=1
        seam.repeat(Input::Grid(1, 1)).await; // firing established → KEY_B value=2
        seam.release(Input::Grid(1, 1)).await; // ForceReleaseStuck → KEY_B value=0

        let events: Vec<_> = seam
            .finish()
            .await
            .iter()
            .map(|b| key_and_value(b[0]))
            .filter(|(code, _)| *code == evdev::KeyCode::KEY_A || *code == evdev::KeyCode::KEY_B)
            .collect();
        assert_eq!(
            events,
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 0),
                (evdev::KeyCode::KEY_B, 1),
                (evdev::KeyCode::KEY_B, 2),
                (evdev::KeyCode::KEY_B, 0),
            ],
            "no dangling KEY_B value=2 — the held key re-presses first"
        );
    }

    #[tokio::test]
    async fn profile_switch_while_holding_a_hold_to_repeat_key_force_releases_it() {
        // The `Effect::ReleaseAllHolds` half of the spec §7 teardown, through
        // the full command harness — `SwitchProfile` drains every live
        // individual firing alongside `StopAllToggles` / `StopAllStages`.
        let mut base = HashMap::new();
        base.insert(
            Input::Grid(1, 1),
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
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
        let harness = CommandHarness::spawn(Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros: HashMap::new(),
            steppers: HashMap::new(),
        });

        harness.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.switch_profile("Gaming").await.unwrap();
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;
        assert_eq!(
            batches
                .iter()
                .map(|b| key_and_value(b[0]))
                .collect::<Vec<_>>(),
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "the Profile switch drained the held KEY_A firing",
        );
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

    /// Spec §4 surface 6 / §3.2 (ticket 05): a Toggle whose held target is a
    /// single keyboard key no longer loops `[Down, Up]` through
    /// `run_toggle_loop` at `target_lap` — it holds a genuine Linux
    /// autorepeat. `value=1` on the first press, the first `value=2` a full
    /// `REP_DELAY` later, `value=2` every `REP_PERIOD` after that, and
    /// `value=0` on the second press of the same key.
    #[tokio::test(start_paused = true)]
    async fn toggle_keyboard_key_holds_a_genuine_autorepeat_and_the_same_key_stops_it() {
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
        assert_eq!(
            seam.sink.batches().len(),
            1,
            "one value=1 on the first press, nothing else yet"
        );
        assert_eq!(
            key_and_value(seam.sink.batches()[0][0]),
            (evdev::KeyCode::KEY_A, 1)
        );

        // The Seam's default schedule is `RepeatSchedule::new(250, 33)` — the
        // full kernel envelope, exactly as a physically held key.
        tokio::time::advance(Duration::from_millis(249)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            seam.sink.batches().len(),
            1,
            "no autorepeat before the full REP_DELAY elapses"
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(33)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            seam.sink
                .batches()
                .iter()
                .map(|b| key_and_value(b[0]))
                .collect::<Vec<_>>(),
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 2),
                (evdev::KeyCode::KEY_A, 2),
            ],
            "value=1 then value=2 at the kernel envelope — never a [Down, Up] loop"
        );

        // Same physical key, still toggled on: stops it with value=0.
        seam.press(Input::Grid(1, 1)).await;
        let batches = seam.finish().await;

        let stream: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            stream,
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 2),
                (evdev::KeyCode::KEY_A, 2),
                (evdev::KeyCode::KEY_A, 0),
            ],
            "value=1, autorepeats, then a single value=0 — never a [Down, Up] pair stream"
        );
    }

    /// Spec §4 surface 6b (ticket 05): a Toggle wrapping a *single-key* Macro
    /// (identical compiled steps) produces the identical `value=1` … `value=2`
    /// envelope as the equivalent Keypress — not a `target_lap`-paced
    /// `[Down, Up]` loop.
    #[tokio::test(start_paused = true)]
    async fn toggle_single_key_macro_holds_the_same_autorepeat_as_the_equivalent_keypress() {
        let (action, macros) = macro_action(
            "hold-a",
            vec![
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                MacroStepDto::KeyUp(evdev::KeyCode::KEY_A),
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
        tokio::time::advance(Duration::from_millis(250)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(33)).await;
        tokio::task::yield_now().await;
        seam.press(Input::Grid(1, 1)).await;
        let batches = seam.finish().await;

        assert_eq!(
            batches
                .iter()
                .map(|b| key_and_value(b[0]))
                .collect::<Vec<_>>(),
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 2),
                (evdev::KeyCode::KEY_A, 2),
                (evdev::KeyCode::KEY_A, 0),
            ],
        );
    }

    /// Ticket 12: a Fire-once Keypress holds its key for `FIRE_ONCE_KEY_DWELL`
    /// between the edges. Held past the dwell the firing self-balances, so the
    /// physical `Up` force-releases nothing; and two presses spaced past the
    /// dwell both fire a full pair, with no stray events.
    #[tokio::test(start_paused = true)]
    async fn fire_once_keypress_holds_the_dwell_then_back_to_back_presses_both_fire() {
        let mut bindings = HashMap::new();
        bindings.insert(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_F1));
        let mut seam = Seam::with_bindings(bindings);

        seam.press(Input::Grid(1, 1)).await;
        // Mid-dwell: only the Down has gone out.
        assert_eq!(
            seam.sink.batches().len(),
            1,
            "the Up is still inside the spliced dwell"
        );
        tokio::time::advance(executor::FIRE_ONCE_KEY_DWELL).await;
        tokio::task::yield_now().await;
        seam.release(Input::Grid(1, 1)).await;

        // A second press, cleanly past the first firing's dwell.
        seam.press(Input::Grid(1, 1)).await;
        tokio::time::advance(executor::FIRE_ONCE_KEY_DWELL).await;
        tokio::task::yield_now().await;
        seam.release(Input::Grid(1, 1)).await;

        let batches = seam.finish().await;
        assert_eq!(
            batches
                .iter()
                .map(|b| key_and_value(b[0]))
                .collect::<Vec<_>>(),
            vec![
                (evdev::KeyCode::KEY_F1, 1),
                (evdev::KeyCode::KEY_F1, 0),
                (evdev::KeyCode::KEY_F1, 1),
                (evdev::KeyCode::KEY_F1, 0),
            ],
            "two full press/release pairs, no stray force-release events"
        );
    }

    /// Spec §4 surface 9 (ticket 05): a Toggle wrapping a *multi-step* Macro
    /// is the sole remaining `run_toggle_loop` user — it still loops the
    /// whole macro, paced at `target_lap`, unchanged.
    #[tokio::test(start_paused = true)]
    async fn toggle_multi_step_macro_still_loops_at_target_lap() {
        let (action, macros) = macro_action(
            "combo",
            vec![
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                MacroStepDto::KeyUp(evdev::KeyCode::KEY_A),
                MacroStepDto::KeyDown(evdev::KeyCode::KEY_B),
                MacroStepDto::KeyUp(evdev::KeyCode::KEY_B),
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
        for _ in 0..7 {
            tokio::time::advance(executor::MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }
        seam.press(Input::Grid(1, 1)).await;
        let batches = seam.finish().await;

        // Several full A-down/A-up/B-down/B-up laps ran — never a value=2.
        assert!(
            batches.len() > 4,
            "a multi-step Macro Toggle must still loop: got {} batches",
            batches.len()
        );
        assert!(
            batches.iter().all(|b| key_and_value(b[0]).1 != 2),
            "a multi-step Macro Toggle loops [Down, Up] pairs — no kernel autorepeat"
        );
    }

    /// Spec §3.2 / §7 (ticket 05): `StopAllToggles` (the GUI-focus stop, and
    /// the Profile-switch effect) releases a running single-key autorepeat
    /// Toggle with `value=0` — the loop-private `held` set is drained on
    /// cancel exactly like the `[Down, Up]` loop variant.
    #[tokio::test(start_paused = true)]
    async fn stop_all_toggles_releases_a_running_single_key_autorepeat_toggle() {
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
        tokio::time::advance(Duration::from_millis(300)).await;
        tokio::task::yield_now().await;
        assert!(
            seam.sink.batches().len() >= 2,
            "the Toggle is autorepeating before the stop"
        );

        let (reply, rx) = oneshot::channel();
        seam.state
            .handle_command(
                &mut seam.config,
                &unused_config_path(),
                Command::StopAllToggles { reply },
            )
            .await;
        rx.await.unwrap();

        let batches = seam.finish().await;
        assert_eq!(
            key_and_value(*batches.last().unwrap().last().unwrap()),
            (evdev::KeyCode::KEY_A, 0),
            "StopAllToggles force-releases the held key with value=0"
        );
    }

    /// Spec.md's "Toggle behavior across Layer/Profile switches": an
    /// *individual* Toggle deliberately survives a Layer switch —
    /// `handle_layer_switch` leaves `individual` toggles running (it only
    /// `drain_firings`). Ticket 05 does not change that: a running single-key
    /// autorepeat Toggle keeps emitting `value=2` across the switch. (The
    /// spec-kernel-shaped-repeat.md §7 "Layer / Profile switch" teardown row
    /// is the Profile-switch `Effect::StopAllToggles` path, covered above.)
    #[tokio::test(start_paused = true)]
    async fn single_key_autorepeat_toggle_survives_a_layer_switch() {
        let mut base = HashMap::new();
        base.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_A,
                },
            },
        );
        let mut seam = Seam::new(config_with_profile(Profile {
            base,
            ..Default::default()
        }));

        seam.press(Input::Grid(1, 1)).await;
        tokio::time::advance(Duration::from_millis(300)).await;
        tokio::task::yield_now().await;
        let before = seam.sink.batches().len();

        seam.feed(PhysicalEvent {
            input: Input::ModeKey,
            state: EventState::Down,
            depth: None,
        })
        .await;
        assert_eq!(seam.get_state().await.layer, "held");
        assert_eq!(
            seam.get_state().await.active_toggles,
            vec![Input::Grid(1, 1)],
            "the individual Toggle survives the Layer switch"
        );

        tokio::time::advance(Duration::from_millis(99)).await;
        tokio::task::yield_now().await;
        let after: Vec<_> = seam.sink.batches()[before..]
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert!(
            !after.is_empty()
                && after
                    .iter()
                    .all(|(c, v)| *c == evdev::KeyCode::KEY_A && *v == 2),
            "it keeps autorepeating across the switch: {after:?}"
        );

        let batches = seam.finish().await;
        assert_eq!(
            key_and_value(*batches.last().unwrap().last().unwrap()),
            (evdev::KeyCode::KEY_A, 0),
            "teardown still balances the held key"
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
            RepeatSchedule::new(250, 33),
            depth_channel(),
            device_info_channel(),
            led_channel(),
            lighting_channel(),
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
            RepeatSchedule::new(250, 33),
            depth_rx,
            device_info_channel(),
            led_channel(),
            lighting_channel(),
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
            RepeatSchedule::new(250, 33),
            depth_rx,
            device_info_channel(),
            led_channel(),
            lighting_channel(),
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
            RepeatSchedule::new(250, 33),
            depth_rx,
            device_info_channel(),
            led_channel(),
            lighting_channel(),
        ));

        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), u8::MAX)]));
        tokio::task::yield_now().await;

        // Crossing 235: a single value=1, nothing else yet — no [Down, Up]
        // taps, and no immediate value=2 (spec-kernel-shaped-repeat.md §5.3).
        assert_eq!(sink.batches().len(), 1, "one value=1 on crossing 235");
        assert_eq!(
            key_and_value(sink.batches()[0][0]),
            (evdev::KeyCode::KEY_F1, 1)
        );

        // Just before the kernel period: still no autorepeat.
        tokio::time::advance(Duration::from_millis(32)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            sink.batches().len(),
            1,
            "no value=2 before the first period_ms elapses"
        );

        // The first value=2 lands at period_ms (33) — not delay_ms (250):
        // this is the top of a hand-driven tapping ramp, not a fresh press.
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(sink.batches().len(), 2, "first value=2 at period_ms");
        assert_eq!(
            key_and_value(sink.batches()[1][0]),
            (evdev::KeyCode::KEY_F1, 2)
        );

        // Steady value=2 every period_ms thereafter.
        tokio::time::advance(Duration::from_millis(33)).await;
        tokio::task::yield_now().await;
        assert_eq!(sink.batches().len(), 3, "second value=2 one period later");
        assert_eq!(
            key_and_value(sink.batches()[2][0]),
            (evdev::KeyCode::KEY_F1, 2)
        );

        // Falling back below the deadzone force-releases the held key.
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 0u8)]));
        tokio::task::yield_now().await;

        drop(tx);
        drop(depth_tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(
            key_and_value(*batches.last().unwrap().last().unwrap()),
            (evdev::KeyCode::KEY_F1, 0),
            "a clean value=0 when Depth leaves hold-solid"
        );
        // Every value=1 for the key is balanced by a value=0 — nothing left down.
        let net_downs = batches
            .iter()
            .flatten()
            .filter(|e| key_and_value(**e).0 == evdev::KeyCode::KEY_F1)
            .filter_map(|e| match key_and_value(*e).1 {
                1 => Some(1i32),
                0 => Some(-1),
                _ => None,
            })
            .sum::<i32>();
        assert_eq!(net_downs, 0, "the key is not left logically down");
    }

    /// Ticket 06 / spec-kernel-shaped-repeat.md §5.3: the full
    /// tap → hold-solid → tap transition. Below 235: balanced [Down, Up]
    /// pulse pairs (ticket 20's deliberately-human shape, untouched). At/above
    /// 235: a value=1 + steady value=2 stream, first value=2 at period_ms.
    /// Dropping back below: a clean value=0, then pulsed pairs resume — and
    /// the key is never left down at any exit.
    #[tokio::test(start_paused = true)]
    async fn analog_repeat_tap_to_hold_solid_to_tap_transition() {
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
            RepeatSchedule::new(250, 33),
            depth_rx,
            device_info_channel(),
            led_channel(),
            lighting_channel(),
        ));

        // Advance ~`ms` of paused time in 5ms steps so the clock drives each
        // pulse edge / period sleep / injector round-trip in turn (the
        // one-big-`advance` shortcut skips intermediate wakeups). The tap
        // band's own `tap_pace_wait` sleep doesn't watch Depth, so a
        // band-crossing isn't noticed until the in-flight pulse's pace sleep
        // ends (~111ms worst case) — settle windows straddling a crossing are
        // sized well past that.
        async fn settle(ms: u64) {
            for _ in 0..ms / 5 {
                tokio::time::advance(Duration::from_millis(5)).await;
                tokio::task::yield_now().await;
            }
        }
        let values = |batches: &[Vec<evdev::InputEvent>]| -> Vec<i32> {
            batches.iter().map(|b| key_and_value(b[0]).1).collect()
        };

        // ── tap band (depth 100 ≈ 9 Hz, ~111ms period) ──────────────────
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 100u8)]));
        settle(300).await;

        let tap = values(&sink.batches());
        assert!(
            tap.len() >= 2 && tap.len().is_multiple_of(2),
            "balanced [Down, Up] pulse pairs below 235, got {tap:?}"
        );
        assert!(
            tap.iter().all(|v| matches!(v, 0 | 1)),
            "the tap band never emits value=2: {tap:?}"
        );

        // ── hold-solid (depth 250) ─────────────────────────────────────
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 250u8)]));
        settle(300).await;

        let solid = values(&sink.batches());
        // The hold-solid press is the last value=1 in the log; everything
        // after it is a bare value=2 stream — no [Down, Up] pulse pairs.
        let press_at = solid.iter().rposition(|&v| v == 1).expect("a value=1");
        assert!(
            solid[press_at + 1..].iter().all(|&v| v == 2),
            "hold-solid emits only value=2 after the press: {:?}",
            &solid[press_at..]
        );
        let v2_in_solid = solid[press_at + 1..].len();
        assert!(
            v2_in_solid >= 5,
            "a steady value=2 stream over 300ms at a 33ms period, got {v2_in_solid}"
        );
        let v2_total = solid.iter().filter(|&&v| v == 2).count();

        // ── back to the tap band (depth 100) ───────────────────────────
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 100u8)]));
        settle(300).await;

        let after = values(&sink.batches());
        // A clean value=0 closes the hold-solid stream…
        assert_eq!(
            after[solid.len()],
            0,
            "a clean value=0 on leaving hold-solid: {:?}",
            &after[solid.len().saturating_sub(1)..]
        );
        // …and no further value=2 is emitted once Depth is back below 235.
        assert_eq!(
            after.iter().filter(|&&v| v == 2).count(),
            v2_total,
            "no value=2 in the tap band"
        );
        assert!(
            after.len() > solid.len() + 1,
            "pulsed pairs resume below 235"
        );

        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 0u8)]));
        settle(40).await;
        drop(tx);
        drop(depth_tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        // Nothing left logically down at the end of the whole ramp.
        let net_downs = sink
            .batches()
            .iter()
            .flatten()
            .filter(|e| key_and_value(**e).0 == evdev::KeyCode::KEY_F1)
            .filter_map(|e| match key_and_value(*e).1 {
                1 => Some(1i32),
                0 => Some(-1),
                _ => None,
            })
            .sum::<i32>();
        assert_eq!(net_downs, 0, "no key left down after tap→solid→tap");
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
            RepeatSchedule::new(250, 33),
            depth_channel(),
            device_info_channel(),
            led_channel(),
            lighting_channel(),
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
        /// The `led`-task seam (`tartarus-status-leds` ticket 02): what
        /// dispatch pushes here on device connect / Profile switch — the
        /// real `led` task's `watch::Receiver`, read directly instead of
        /// running the hardware-touching task.
        led_rx: watch::Receiver<Option<StatusLeds>>,
        /// The `led`-task seam's Lighting sibling (`tartarus-backlight`
        /// ticket 02): what dispatch pushes on device connect/startup.
        lighting_rx: watch::Receiver<Option<LightingState>>,
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
            let (led_tx, led_rx) = watch::channel(None);
            let (lighting_tx, lighting_rx) = watch::channel(None);
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
                RepeatSchedule::new(250, 33),
                depth_rx,
                device_info_rx,
                led_tx,
                lighting_tx,
            ));

            CommandHarness {
                _dir: dir,
                config_path,
                cmd_tx,
                event_tx,
                actuation_rx,
                depth_tx,
                device_info_tx,
                led_rx,
                lighting_rx,
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

        async fn clear_deep_stage(&self, input: Input, layer: Layer) -> Result<(), CommandError> {
            self.apply(edit::Edit::ClearDeepStage { input, layer })
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

        async fn clear_chord_binding(
            &self,
            inputs: impl IntoIterator<Item = Input>,
            layer: Layer,
        ) -> Result<(), CommandError> {
            self.apply(edit::Edit::ClearChordBinding {
                inputs: inputs.into_iter().collect(),
                layer,
            })
            .await
            .map(|_| ())
        }

        async fn set_axis_assignment(
            &self,
            input: Input,
            layer: Layer,
            target: AxisTarget,
        ) -> Result<(), CommandError> {
            self.apply(edit::Edit::SetAxisAssignment {
                input,
                layer,
                target,
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

        async fn set_status_leds(
            &self,
            orange: bool,
            green: bool,
            blue: bool,
        ) -> Result<(), CommandError> {
            self.apply(edit::Edit::SetStatusLeds {
                orange,
                green,
                blue,
            })
            .await
            .map(|_| ())
        }

        async fn set_lighting(
            &self,
            assignment: LightingAssignment,
            brightness: u8,
        ) -> Result<(), CommandError> {
            self.apply(edit::Edit::SetLighting {
                assignment,
                brightness,
            })
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

        /// The latest Status-LED triple dispatch has pushed on the `led`
        /// watch channel (`tartarus-status-leds` ticket 02), marking it seen
        /// — `None` until the first connect edge asserts one. A burst
        /// coalesces here exactly as it would for the real `led` task.
        fn take_status_leds_pushed(&mut self) -> Option<StatusLeds> {
            *self.led_rx.borrow_and_update()
        }

        /// Whether dispatch has pushed a fresh value on the `led` channel
        /// since the last `take_status_leds_pushed` — true even for an
        /// unchanged triple, so a test can assert every `connected == true`
        /// re-asserts.
        fn status_leds_re_pushed(&self) -> bool {
            self.led_rx.has_changed().unwrap_or(false)
        }

        /// The latest `LightingState` dispatch has pushed on the `led`
        /// task's second watch channel (`tartarus-backlight` ticket 02),
        /// marking it seen — `None` until the first connect edge asserts
        /// one. A burst coalesces here exactly as it would for the real
        /// `led` task.
        fn take_lighting_pushed(&mut self) -> Option<LightingState> {
            self.lighting_rx.borrow_and_update().clone()
        }

        /// Whether dispatch has pushed a fresh value on the Lighting channel
        /// since the last `take_lighting_pushed` — true even for an
        /// unchanged state, so a test can assert every `connected == true`
        /// re-asserts.
        fn lighting_re_pushed(&self) -> bool {
            self.lighting_rx.has_changed().unwrap_or(false)
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

        /// `press_analog`'s `Up` mirror (`tartarus-dual-stage-keys` ticket
        /// 03) — stands in for `capture::analog`'s own primary-band `observe`
        /// firing a real Up when Depth crosses back down through the
        /// primary's Release point, the same way a real Analog capture
        /// session would alongside a `push_depth` call for the same report.
        async fn release_analog(&self, input: Input, depth: u8) {
            self.event_tx
                .send(PhysicalEvent {
                    input,
                    state: EventState::Up,
                    depth: Some(depth),
                })
                .await
                .unwrap();
        }

        /// `press_analog`'s `Repeat` mirror (`tartarus-dual-stage-keys`
        /// ticket 03) — stands in for `capture::analog`'s synthesized
        /// Hold-to-repeat `Repeat` pulses (`RepeatSchedule`) while a Grid key
        /// stays physically held in Analog capture.
        async fn repeat_analog(&self, input: Input, depth: u8) {
            self.event_tx
                .send(PhysicalEvent {
                    input,
                    state: EventState::Repeat,
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
    async fn single_key_chord_hold_to_repeat_emits_one_autorepeat_per_leader_repeat() {
        // Spec §3.3 + ticket 04: a Chord whose Action is a single key inherits
        // the `value=2` autorepeat path through the shared `decide` + `perform`
        // seam. `chord::feed_repeat` re-fires only the `BTreeSet`-first
        // "leader" member (`Grid(1,1)` here), so the Chord emits exactly one
        // `value=2` per leader `Repeat` — a non-leader member's `Repeat` is a
        // no-op — then a `value=0` when a member's `Up` dissolves it.
        let mut profile = Profile::default();
        profile.chords_base.insert(
            ChordKey::new(BTreeSet::from([Input::Grid(1, 1), Input::Grid(1, 2)])),
            hold_to_repeat_binding(evdev::KeyCode::KEY_C),
        );
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), profile);
        let harness = CommandHarness::spawn(Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros: HashMap::new(),
            steppers: HashMap::new(),
        });

        // Both members down within the window → the Chord fires: KEY_C value=1.
        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        // The non-leader's Repeat is a no-op; the leader's drives one value=2 each.
        harness.repeat(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        harness.repeat(Input::Grid(1, 1)).await;
        harness.repeat(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        // Releasing one member dissolves the Chord → force-release KEY_C value=0.
        harness.release(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = harness.shut_down().await;
        assert_eq!(
            batches
                .iter()
                .map(|b| key_and_value(b[0]))
                .collect::<Vec<_>>(),
            vec![
                (evdev::KeyCode::KEY_C, 1),
                (evdev::KeyCode::KEY_C, 2),
                (evdev::KeyCode::KEY_C, 2),
                (evdev::KeyCode::KEY_C, 0),
            ],
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

    // -- Status LEDs: dispatch pushes the active Profile's triple on the
    // `led` watch channel on every device connect (`tartarus-status-leds`
    // ticket 02). The `HIDIOCSFEATURE` write and the `led` task's own
    // consume/coalesce behaviour are covered in `capture::analog` /
    // `crate::led`. --------------------------------------------------------

    fn config_with_status_leds(leds: StatusLeds) -> Config {
        config_with_profile(Profile {
            status_leds: leds,
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn a_device_connect_pushes_the_active_profiles_status_leds() {
        let leds = StatusLeds {
            orange: true,
            green: false,
            blue: true,
        };
        let mut harness = CommandHarness::spawn(config_with_status_leds(leds));

        // No pre-loop assert — nothing on the channel until the connect edge.
        assert_eq!(harness.take_status_leds_pushed(), None);

        harness.set_device_connected(true).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert_eq!(harness.take_status_leds_pushed(), Some(leds));
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn every_connected_true_re_pushes_the_triple_even_without_a_transition() {
        let leds = StatusLeds {
            orange: true,
            ..Default::default()
        };
        let mut harness = CommandHarness::spawn(config_with_status_leds(leds));

        harness.set_device_connected(true).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(harness.take_status_leds_pushed(), Some(leds));

        // A second `true` with no intervening `false`:
        // `handle_connection_change` early-returns on the unchanged bool, but
        // the LED assert must still re-fire — the firmware reclaims the LEDs
        // to orange-only on every USB enumeration.
        harness.set_device_connected(true).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert!(harness.status_leds_re_pushed());
        assert_eq!(harness.take_status_leds_pushed(), Some(leds));

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn a_reported_disconnect_pushes_no_status_leds() {
        let leds = StatusLeds {
            green: true,
            ..Default::default()
        };
        let mut harness = CommandHarness::spawn(config_with_status_leds(leds));

        harness.set_device_connected(false).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert!(!harness.status_leds_re_pushed());
        assert_eq!(harness.take_status_leds_pushed(), None);
        harness.shut_down().await;
    }

    // -- Lighting: dispatch pushes the active Profile's whole `LightingState`
    // on the `led` task's second watch channel on every device connect and
    // on Daemon startup (`tartarus-backlight` ticket 02). The `HIDIOCSFEATURE`
    // write and the `led` task's own consume/coalesce behaviour are covered
    // in `capture::analog` / `crate::led`. ----------------------------------

    fn config_with_lighting(assignment: LightingAssignment, brightness: u8) -> Config {
        config_with_profile(Profile {
            lighting: assignment,
            brightness,
            ..Default::default()
        })
    }

    fn sample_fixed_effect() -> LightingAssignment {
        LightingAssignment::FixedEffect {
            effect: FixedEffect::Static {
                colour: Colour {
                    r: 0x10,
                    g: 0x20,
                    b: 0x30,
                },
            },
        }
    }

    #[tokio::test]
    async fn a_device_connect_pushes_the_active_profiles_lighting() {
        let assignment = sample_fixed_effect();
        let mut harness = CommandHarness::spawn(config_with_lighting(assignment.clone(), 0x80));

        // No pre-loop assert — nothing on the channel until the connect edge.
        assert_eq!(harness.take_lighting_pushed(), None);

        harness.set_device_connected(true).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert_eq!(
            harness.take_lighting_pushed(),
            Some(LightingState {
                assignment,
                brightness: 0x80,
            })
        );
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn every_connected_true_re_pushes_the_lighting_even_without_a_transition() {
        let assignment = LightingAssignment::Off;
        let mut harness = CommandHarness::spawn(config_with_lighting(assignment.clone(), 0x00));

        harness.set_device_connected(true).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            harness.take_lighting_pushed(),
            Some(LightingState {
                assignment: assignment.clone(),
                brightness: 0x00,
            })
        );

        // A second `true` with no intervening `false`: `handle_connection_
        // change` early-returns on the unchanged bool, but the Lighting
        // assert must still re-fire — the same reasoning Status LEDs'
        // equivalent test documents, applied here for consistency even
        // though VARSTORE means the firmware itself doesn't reset Lighting
        // on enumeration (ADR-0012's re-assertion-discipline argument).
        harness.set_device_connected(true).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert!(harness.lighting_re_pushed());
        assert_eq!(
            harness.take_lighting_pushed(),
            Some(LightingState {
                assignment,
                brightness: 0x00,
            })
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn a_reported_disconnect_pushes_no_lighting() {
        let mut harness = CommandHarness::spawn(config_with_lighting(sample_fixed_effect(), 0x40));

        harness.set_device_connected(false).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert!(!harness.lighting_re_pushed());
        assert_eq!(harness.take_lighting_pushed(), None);
        harness.shut_down().await;
    }

    // -- Status LEDs: a `SetStatusLeds` D-Bus edit and a Profile switch both
    // re-assert the active Profile's triple on the `led` channel
    // (`tartarus-status-leds` ticket 03, via `edit::Effect::AssertStatusLeds`
    // → `push_status_leds`). ----------------------------------------------

    #[tokio::test]
    async fn a_set_status_leds_edit_persists_the_triple_and_pushes_it() {
        let want = StatusLeds {
            orange: true,
            green: false,
            blue: true,
        };
        let mut harness = CommandHarness::spawn(config_with_status_leds(StatusLeds::default()));

        harness
            .set_status_leds(want.orange, want.green, want.blue)
            .await
            .expect("SetStatusLeds must succeed");
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert_eq!(harness.take_status_leds_pushed(), Some(want));
        assert_eq!(
            harness
                .get_config()
                .await
                .active_profile()
                .unwrap()
                .status_leds,
            want,
        );
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn switching_profile_re_asserts_the_newly_active_profiles_triple() {
        let gaming = StatusLeds {
            orange: false,
            green: true,
            blue: false,
        };
        let mut config = config_with_status_leds(StatusLeds {
            orange: true,
            ..Default::default()
        });
        config.profiles.insert(
            "Gaming".to_string(),
            Profile {
                status_leds: gaming,
                ..Default::default()
            },
        );
        let mut harness = CommandHarness::spawn(config);

        harness
            .switch_profile("Gaming")
            .await
            .expect("SwitchProfile must succeed");
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert_eq!(harness.take_status_leds_pushed(), Some(gaming));
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn a_burst_of_switches_coalesces_to_the_final_profiles_triple() {
        let a = StatusLeds {
            orange: true,
            ..Default::default()
        };
        let b = StatusLeds {
            green: true,
            ..Default::default()
        };
        let mut config = config_with_status_leds(StatusLeds::default());
        config.profiles.insert(
            "A".to_string(),
            Profile {
                status_leds: a,
                ..Default::default()
            },
        );
        config.profiles.insert(
            "B".to_string(),
            Profile {
                status_leds: b,
                ..Default::default()
            },
        );
        let mut harness = CommandHarness::spawn(config);

        harness.switch_profile("A").await.unwrap();
        harness.switch_profile("B").await.unwrap();
        harness.switch_profile("A").await.unwrap();
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert_eq!(harness.take_status_leds_pushed(), Some(a));
        harness.shut_down().await;
    }

    // -- Lighting: a `SetLighting` D-Bus edit and a Profile switch both
    // re-assert the active Profile's whole Lighting state on the `led`
    // task's second channel (`tartarus-backlight` ticket 03, via
    // `edit::Effect::AssertLighting` → `push_lighting`), mirroring Status
    // LEDs' own trio of tests above exactly. ------------------------------

    #[tokio::test]
    async fn a_set_lighting_edit_persists_the_assignment_and_pushes_it() {
        let want = sample_fixed_effect();
        let mut harness = CommandHarness::spawn(config_with_lighting(LightingAssignment::Off, 0));

        harness
            .set_lighting(want.clone(), 0x55)
            .await
            .expect("SetLighting must succeed");
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert_eq!(
            harness.take_lighting_pushed(),
            Some(LightingState {
                assignment: want.clone(),
                brightness: 0x55,
            })
        );
        let config = harness.get_config().await;
        let profile = config.active_profile().unwrap();
        assert_eq!(profile.lighting, want);
        assert_eq!(profile.brightness, 0x55);
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn switching_profile_re_asserts_the_newly_active_profiles_lighting() {
        let gaming = sample_fixed_effect();
        let mut config = config_with_lighting(LightingAssignment::Off, 0);
        config.profiles.insert(
            "Gaming".to_string(),
            Profile {
                lighting: gaming.clone(),
                brightness: 0xAA,
                ..Default::default()
            },
        );
        let mut harness = CommandHarness::spawn(config);

        harness
            .switch_profile("Gaming")
            .await
            .expect("SwitchProfile must succeed");
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert_eq!(
            harness.take_lighting_pushed(),
            Some(LightingState {
                assignment: gaming,
                brightness: 0xAA,
            })
        );
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn a_burst_of_switches_coalesces_to_the_final_profiles_lighting() {
        let a = LightingAssignment::Off;
        let b = sample_fixed_effect();
        let mut config = config_with_lighting(LightingAssignment::Off, 0);
        config.profiles.insert(
            "A".to_string(),
            Profile {
                lighting: a.clone(),
                brightness: 0x11,
                ..Default::default()
            },
        );
        config.profiles.insert(
            "B".to_string(),
            Profile {
                lighting: b,
                brightness: 0x22,
                ..Default::default()
            },
        );
        let mut harness = CommandHarness::spawn(config);

        harness.switch_profile("A").await.unwrap();
        harness.switch_profile("B").await.unwrap();
        harness.switch_profile("A").await.unwrap();
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        assert_eq!(
            harness.take_lighting_pushed(),
            Some(LightingState {
                assignment: a,
                brightness: 0x11,
            })
        );
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

        // Ticket 04: a single keyboard key under Hold-to-repeat now presents
        // as genuine kernel autorepeat — the bound Keypress's `KEY_F1`
        // `value=1` on Down, then one `value=2` per Repeat — not a stream of
        // `[Down, Up]` pairs, not `KEY_LEFTALT`, and not a Layer switch.
        assert_eq!(batches.len(), 2);
        assert_eq!(key_and_value(batches[0][0]), (evdev::KeyCode::KEY_F1, 1));
        assert_eq!(key_and_value(batches[1][0]), (evdev::KeyCode::KEY_F1, 2));
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

        // Post-release ticket 12 follow-up: a single-key Stepper `Key` step
        // under Hold-to-repeat now also splices `FIRE_ONCE_KEY_DWELL` between
        // its edges (it's the one Hold-to-repeat shape that reaches
        // `D::SpawnFireOnce` rather than the kernel-autorepeat arms), so the
        // Down and the Repeat must be spaced past the dwell or the second one
        // is dropped by `decide`'s ordinary same-key overlap guard.
        harness.press(Input::Grid(1, 1)).await;
        tokio::time::sleep(executor::FIRE_ONCE_KEY_DWELL + Duration::from_millis(20)).await;
        harness.repeat(Input::Grid(1, 1)).await;
        tokio::time::sleep(executor::FIRE_ONCE_KEY_DWELL + Duration::from_millis(20)).await;

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
        // Advance to index 2 (the last item of the 3-item list). The two
        // presses are spaced past `FIRE_ONCE_KEY_DWELL` (ticket 12): a
        // single-key Step under Fire-once now holds its key for the spliced
        // dwell, so a second press landing inside that window would be
        // dropped by `decide`'s ordinary same-key overlap guard.
        harness.press(Input::Grid(1, 1)).await;
        tokio::time::sleep(executor::FIRE_ONCE_KEY_DWELL + Duration::from_millis(20)).await;
        harness.release(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 1)).await;
        tokio::time::sleep(executor::FIRE_ONCE_KEY_DWELL + Duration::from_millis(20)).await;
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
        // A single-key Stepper `Key` step under Fire-once compiles to a
        // single held key, so ticket 12 splices `FIRE_ONCE_KEY_DWELL` between
        // its edges. Held past the dwell (the ordinary case — a real press is
        // tens of ms), the firing self-balances its KeyDown/KeyUp before the
        // physical `Up`'s force-release check (ticket 33) runs, so that check
        // finds nothing held and no extra output lands.
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
        // Hold past the spliced dwell so the firing balances itself.
        tokio::time::sleep(executor::FIRE_ONCE_KEY_DWELL + Duration::from_millis(20)).await;
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

    // ── `tartarus-dual-stage-keys` ticket 03: `stage::Engine` end-to-end ────

    /// Builds a `Config` with `Input::Grid(1, 1)` carrying a primary
    /// Binding, a deep Binding, and a `DeepStageConfig` in the given Staging
    /// `mode`. Primary's Actuation/Release stays the Profile default
    /// (128/112); deep's own band (220/200) is spec.md's own sample
    /// fragment — `deep.release` 200 > `primary.actuation` 128 (disjoint and
    /// stacked), `deep.release` 200 < `deep.actuation` 220 (the deep pair's
    /// own hysteresis).
    fn dual_stage_config(mode: StagingMode, primary: Binding, deep: Binding) -> Config {
        config_with_profile(Profile {
            base: HashMap::from([(Input::Grid(1, 1), primary)]),
            deep_base: HashMap::from([(Input::Grid(1, 1), deep)]),
            deep_stages: HashMap::from([(
                Input::Grid(1, 1),
                DeepStageConfig {
                    actuation: ActuationPoint {
                        actuation: 220,
                        release: 200,
                    },
                    mode,
                },
            )]),
            ..Default::default()
        })
    }

    fn toggle_binding(key: evdev::KeyCode) -> Binding {
        Binding {
            trigger: TriggerMode::Toggle,
            action: Action::Keypress {
                modifiers: Modifiers::default(),
                key,
            },
        }
    }

    fn hold_to_repeat_binding(key: evdev::KeyCode) -> Binding {
        Binding {
            trigger: TriggerMode::HoldToRepeat,
            action: Action::Keypress {
                modifiers: Modifiers::default(),
                key,
            },
        }
    }

    /// A generous settle for a spawned Fire-once/Toggle task to actually run
    /// and land its writes in the `RecordingSink` — mirrors `Seam::feed`'s
    /// own five-yield spacing.
    async fn settle() {
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
    }

    // ── ticket 19: `DispatchState::tear_down` — the lifecycle-teardown matrix ──

    /// A Profile that carries one live-able participant of every kind
    /// `tear_down` names, so a single `state.tear_down(reason)` call can be
    /// asserted against the whole matrix at once (ticket 19):
    ///
    /// | Input        | participant                                  | key   |
    /// |--------------|----------------------------------------------|-------|
    /// | `Grid(1,1)`  | dual-stage key (Handoff): primary Hold-to-repeat | KEY_A |
    /// | `Grid(1,1)`  | …its deep stage = Toggle                     | KEY_B |
    /// | `Grid(2,1)`  | individual Toggle                            | KEY_C |
    /// | `Grid(3,1)`  | individual Hold-to-repeat (bare held value=1) | KEY_D |
    /// | `Grid(4,1)`  | axis assignment → `ABS_Z`                    | —     |
    /// | `Grid(2,3)`  | Analog-repeat task                           | KEY_G |
    /// | `Grid(1,3)+Grid(1,4)` | Chord Toggle                       | KEY_E |
    /// | `Grid(3,3)+Grid(3,4)` | Chord Hold-to-repeat firing        | KEY_F |
    /// | `Grid(4,3)+Grid(4,4)+Grid(4,5)` | 3-member Chord, one member down: an open simultaneity window | KEY_H |
    ///
    /// Ticket 21 flips the Chord-path cells: `chord_machine` resets, Chord
    /// firings drain, and Chord Toggles drain on a Profile switch.
    fn matrix_config() -> Config {
        config_with_profile(Profile {
            base: HashMap::from([
                (
                    Input::Grid(1, 1),
                    hold_to_repeat_binding(evdev::KeyCode::KEY_A),
                ),
                (Input::Grid(2, 1), toggle_binding(evdev::KeyCode::KEY_C)),
                (
                    Input::Grid(3, 1),
                    hold_to_repeat_binding(evdev::KeyCode::KEY_D),
                ),
                (
                    Input::Grid(2, 3),
                    Binding {
                        trigger: TriggerMode::AnalogRepeat,
                        action: Action::Keypress {
                            modifiers: Modifiers::default(),
                            key: evdev::KeyCode::KEY_G,
                        },
                    },
                ),
            ]),
            deep_base: HashMap::from([(Input::Grid(1, 1), toggle_binding(evdev::KeyCode::KEY_B))]),
            deep_stages: HashMap::from([(
                Input::Grid(1, 1),
                DeepStageConfig {
                    actuation: ActuationPoint {
                        actuation: 220,
                        release: 200,
                    },
                    mode: StagingMode::Handoff,
                },
            )]),
            axis_base: HashMap::from([(Input::Grid(4, 1), AxisTarget::LeftTrigger)]),
            chords_base: HashMap::from([
                (
                    ChordKey::new(BTreeSet::from([Input::Grid(1, 3), Input::Grid(1, 4)])),
                    toggle_binding(evdev::KeyCode::KEY_E),
                ),
                (
                    ChordKey::new(BTreeSet::from([Input::Grid(3, 3), Input::Grid(3, 4)])),
                    hold_to_repeat_binding(evdev::KeyCode::KEY_F),
                ),
                (
                    ChordKey::new(BTreeSet::from([
                        Input::Grid(4, 3),
                        Input::Grid(4, 4),
                        Input::Grid(4, 5),
                    ])),
                    hold_to_repeat_binding(evdev::KeyCode::KEY_H),
                ),
            ]),
            ..Default::default()
        })
    }

    /// Drives every participant of `matrix_config` into a live state on a
    /// fresh `Seam`: KEY_D held as a bare individual firing, KEY_C an
    /// individual Toggle, KEY_B a live deep-stage Toggle, `ABS_Z` a live axis
    /// output, KEY_G a spawned Analog-repeat task, KEY_E a Chord Toggle, KEY_F
    /// a live Chord firing, and one member of the KEY_H 3-member Chord down
    /// to leave a `chord_machine` simultaneity window open. The caller keeps
    /// the `depth_rx`'s `Sender` alive for the life of the test (the
    /// Analog-repeat task holds a clone).
    async fn seed_every_teardown_participant(
        seam: &mut Seam,
        depth_rx: &watch::Receiver<HashMap<Input, u8>>,
    ) {
        seam.press(Input::Grid(3, 1)).await;
        seam.press(Input::Grid(2, 1)).await;

        // Dual-stage key: baseline tick, primary press, then a deep-band tick
        // that hands the primary off and fires the deep Toggle (KEY_B).
        seam.state
            .update_stages(&seam.config, &HashMap::from([(Input::Grid(1, 1), 0)]))
            .await
            .unwrap();
        seam.feed(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Down,
            depth: Some(150),
        })
        .await;
        seam.state
            .update_stages(&seam.config, &HashMap::from([(Input::Grid(1, 1), 150)]))
            .await
            .unwrap();
        seam.state
            .update_stages(&seam.config, &HashMap::from([(Input::Grid(1, 1), 250)]))
            .await
            .unwrap();

        // Live axis output.
        seam.state
            .handle_depth_update(&seam.config, &HashMap::from([(Input::Grid(4, 1), 200)]))
            .await;

        // Spawned Analog-repeat task.
        seam.state
            .update_analog_repeats(
                &seam.config,
                depth_rx,
                &HashMap::from([(Input::Grid(2, 3), 200)]),
            )
            .await;

        // Chord Toggle (KEY_E) and Chord Hold-to-repeat firing (KEY_F) — both
        // members of each down inside the window.
        for input in [
            Input::Grid(1, 3),
            Input::Grid(1, 4),
            Input::Grid(3, 3),
            Input::Grid(3, 4),
        ] {
            seam.feed(PhysicalEvent {
                input,
                state: EventState::Down,
                depth: None,
            })
            .await;
        }

        // One member of the 3-member KEY_H Chord: opens a simultaneity
        // window that never completes — the state a lifecycle switch can
        // catch mid-window (ticket 21, A4).
        seam.feed(PhysicalEvent {
            input: Input::Grid(4, 3),
            state: EventState::Down,
            depth: None,
        })
        .await;
        assert!(
            chord::next_deadline(&seam.state.chord_machine).is_some(),
            "seed leaves a chord_machine window open"
        );
        settle().await;
    }

    /// Stops every background task `seed_every_teardown_participant` may have
    /// left running that `Seam::finish` doesn't (the two Chord-path handles),
    /// then hands off to `finish`.
    async fn finish_matrix(mut seam: Seam) {
        seam.state.chord_slots.stop_all_toggles().await;
        seam.state.chord_slots.drain_firings(&seam.inj).await;
        seam.finish().await;
    }

    /// `(KeyCode, value)` of every batch the recording sink took at or after
    /// `from` — the release edges a `tear_down` produced.
    fn released_keys_since(seam: &Seam, from: usize) -> Vec<(evdev::KeyCode, i32)> {
        seam.sink.batches()[from..]
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect()
    }

    /// True once the Analog-repeat task seeded on `Grid(2,3)` is gone —
    /// `reconcile` only asks for a fresh `Spawn` when no task is active.
    async fn analog_repeat_task_gone(seam: &mut Seam) -> bool {
        !seam
            .state
            .analog_repeat
            .update(
                &HashSet::from([Input::Grid(2, 3)]),
                &HashMap::from([(Input::Grid(2, 3), 200u8)]),
            )
            .await
            .is_empty()
    }

    #[tokio::test]
    async fn tear_down_layer_switch_drains_firings_axis_analog_stage_and_keeps_every_toggle() {
        let (_depth_tx, depth_rx) = watch::channel(HashMap::new());
        let mut seam = Seam::new(matrix_config());
        seed_every_teardown_participant(&mut seam, &depth_rx).await;

        let mark = seam.sink.batches().len();
        let gamepad_mark = seam.gamepad_sink.batches().len();
        seam.state.tear_down(TeardownReason::LayerSwitch).await;
        settle().await;

        let released = released_keys_since(&seam, mark);
        assert!(
            released.contains(&(evdev::KeyCode::KEY_D, 0)),
            "the individual Hold-to-repeat's bare value=1 is drained: {released:?}"
        );
        assert!(
            released.contains(&(evdev::KeyCode::KEY_B, 0)),
            "the live deep-stage Toggle is released: {released:?}"
        );
        assert!(
            !released.iter().any(|&(c, _)| c == evdev::KeyCode::KEY_C),
            "the individual Toggle survives a Layer switch (CONTEXT.md): {released:?}"
        );
        assert!(
            released.contains(&(evdev::KeyCode::KEY_F, 0)),
            "the Chord Hold-to-repeat firing is drained on a Layer switch \
             (ticket 21, A5): {released:?}"
        );
        assert!(
            !released.iter().any(|&(c, _)| c == evdev::KeyCode::KEY_E),
            "the Chord Toggle survives a Layer switch (keybinder spec.md — \
             Layer change never touches an active Toggle): {released:?}"
        );
        assert_eq!(
            seam.state
                .individual
                .active_toggle_keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![Input::Grid(2, 1)]
        );
        assert_eq!(
            seam.state.chord_slots.active_toggle_keys().count(),
            1,
            "the Chord Toggle is still live after a Layer switch"
        );
        assert_eq!(
            chord::next_deadline(&seam.state.chord_machine),
            None,
            "the chord_machine window is reset on a Layer switch (ticket 21, A4)"
        );

        let axis_writes = flat_axis_writes(seam.gamepad_sink.batches()[gamepad_mark..].to_vec());
        assert_eq!(
            axis_writes,
            vec![(evdev::AbsoluteAxisCode::ABS_Z, 0)],
            "every live axis output is centered on a Layer switch"
        );
        assert!(
            analog_repeat_task_gone(&mut seam).await,
            "the Analog-repeat task is stopped on a Layer switch"
        );

        finish_matrix(seam).await;
    }

    #[tokio::test]
    async fn tear_down_profile_switch_is_the_strongest_sweep_and_drains_every_toggle() {
        let (_depth_tx, depth_rx) = watch::channel(HashMap::new());
        let mut seam = Seam::new(matrix_config());
        seed_every_teardown_participant(&mut seam, &depth_rx).await;

        let mark = seam.sink.batches().len();
        let gamepad_mark = seam.gamepad_sink.batches().len();
        seam.state.tear_down(TeardownReason::ProfileSwitch).await;
        settle().await;

        let released = released_keys_since(&seam, mark);
        assert!(
            released.contains(&(evdev::KeyCode::KEY_C, 0)),
            "the individual Toggle drains on a Profile switch: {released:?}"
        );
        assert!(
            released.contains(&(evdev::KeyCode::KEY_D, 0)),
            "the individual firing is drained: {released:?}"
        );
        assert!(
            released.contains(&(evdev::KeyCode::KEY_B, 0)),
            "the live deep-stage Toggle is released: {released:?}"
        );
        assert!(
            released.contains(&(evdev::KeyCode::KEY_F, 0)),
            "the Chord Hold-to-repeat firing is drained on a Profile switch \
             (ticket 21, A5): {released:?}"
        );
        assert!(
            released.contains(&(evdev::KeyCode::KEY_E, 0)),
            "the Chord Toggle drains on a Profile switch too (ticket 21, A6 — \
             keybinder spec.md: every active Toggle): {released:?}"
        );
        assert!(
            seam.state.individual.active_toggle_keys().next().is_none(),
            "no individual Toggle survives a Profile switch"
        );
        assert_eq!(
            seam.state.chord_slots.active_toggle_keys().count(),
            0,
            "no Chord Toggle survives a Profile switch either"
        );
        assert_eq!(
            chord::next_deadline(&seam.state.chord_machine),
            None,
            "the chord_machine window is reset on a Profile switch (ticket 21, A4)"
        );

        let axis_writes = flat_axis_writes(seam.gamepad_sink.batches()[gamepad_mark..].to_vec());
        assert_eq!(axis_writes, vec![(evdev::AbsoluteAxisCode::ABS_Z, 0)]);
        assert!(
            analog_repeat_task_gone(&mut seam).await,
            "the Analog-repeat task is stopped on a Profile switch"
        );

        finish_matrix(seam).await;
    }

    #[tokio::test]
    async fn tear_down_disconnect_centres_axis_stops_analog_drains_every_firing_and_keeps_toggles()
    {
        let (_depth_tx, depth_rx) = watch::channel(HashMap::new());
        let mut seam = Seam::new(matrix_config());
        seed_every_teardown_participant(&mut seam, &depth_rx).await;

        let mark = seam.sink.batches().len();
        let gamepad_mark = seam.gamepad_sink.batches().len();
        seam.state.tear_down(TeardownReason::Disconnect).await;
        settle().await;

        let released = released_keys_since(&seam, mark);
        assert!(
            released.contains(&(evdev::KeyCode::KEY_B, 0)),
            "the deep stage is released on a disconnect: {released:?}"
        );
        assert!(
            released.contains(&(evdev::KeyCode::KEY_D, 0)),
            "individual firings are drained on a disconnect: {released:?}"
        );
        assert!(
            released.contains(&(evdev::KeyCode::KEY_F, 0)),
            "the Chord Hold-to-repeat firing is drained on a disconnect \
             (ticket 21, A5): {released:?}"
        );
        assert!(
            !released.iter().any(|&(c, _)| c == evdev::KeyCode::KEY_C),
            "individual Toggles survive a disconnect: {released:?}"
        );
        assert!(
            !released.iter().any(|&(c, _)| c == evdev::KeyCode::KEY_E),
            "the Chord Toggle survives a disconnect (not a Layer/Profile \
             change — ticket 21): {released:?}"
        );
        assert_eq!(
            seam.state
                .individual
                .active_toggle_keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![Input::Grid(2, 1)]
        );
        assert_eq!(
            seam.state.chord_slots.active_toggle_keys().count(),
            1,
            "the Chord Toggle is still live after a disconnect"
        );
        assert_eq!(
            chord::next_deadline(&seam.state.chord_machine),
            None,
            "the chord_machine window is reset on a disconnect (ticket 21, A4)"
        );

        let axis_writes = flat_axis_writes(seam.gamepad_sink.batches()[gamepad_mark..].to_vec());
        assert_eq!(
            axis_writes,
            vec![(evdev::AbsoluteAxisCode::ABS_Z, 0)],
            "every live axis output is centered on a disconnect (ticket 21, A1)"
        );
        assert!(
            analog_repeat_task_gone(&mut seam).await,
            "the Analog-repeat task is stopped on a disconnect (ticket 21, A2)"
        );

        finish_matrix(seam).await;
    }

    #[tokio::test]
    async fn tear_down_capture_mode_to_digital_centres_axis_stops_analog_and_drains_every_firing() {
        let (_depth_tx, depth_rx) = watch::channel(HashMap::new());
        let mut seam = Seam::new(matrix_config());
        seed_every_teardown_participant(&mut seam, &depth_rx).await;

        let mark = seam.sink.batches().len();
        let gamepad_mark = seam.gamepad_sink.batches().len();
        seam.state
            .tear_down(TeardownReason::CaptureModeToDigital)
            .await;
        settle().await;

        let released = released_keys_since(&seam, mark);
        assert!(
            released.contains(&(evdev::KeyCode::KEY_B, 0)),
            "the deep stage is released on the Digital flip: {released:?}"
        );
        assert!(
            released.contains(&(evdev::KeyCode::KEY_D, 0)),
            "individual firings are drained on the Digital flip: {released:?}"
        );
        assert!(
            released.contains(&(evdev::KeyCode::KEY_F, 0)),
            "the Chord Hold-to-repeat firing is drained on the Digital flip \
             (ticket 21, A5): {released:?}"
        );
        assert!(
            !released.iter().any(|&(c, _)| c == evdev::KeyCode::KEY_C),
            "individual Toggles survive the Digital flip: {released:?}"
        );
        assert!(
            !released.iter().any(|&(c, _)| c == evdev::KeyCode::KEY_E),
            "the Chord Toggle survives the Digital flip (not a Layer/Profile \
             change — ticket 21): {released:?}"
        );
        assert_eq!(
            seam.state.chord_slots.active_toggle_keys().count(),
            1,
            "the Chord Toggle is still live after the Digital flip"
        );
        assert_eq!(
            chord::next_deadline(&seam.state.chord_machine),
            None,
            "the chord_machine window is reset on the Digital flip (ticket 21, A4)"
        );

        let axis_writes = flat_axis_writes(seam.gamepad_sink.batches()[gamepad_mark..].to_vec());
        assert_eq!(
            axis_writes,
            vec![(evdev::AbsoluteAxisCode::ABS_Z, 0)],
            "every live axis output is centered on the Digital flip (ticket 21, A3)"
        );
        assert!(
            analog_repeat_task_gone(&mut seam).await,
            "the Analog-repeat task is stopped on the Digital flip"
        );

        finish_matrix(seam).await;
    }

    // ── ticket 21: the integration net — one pipeline test per genuinely
    //    new stuck-output symptom the matrix additions close ──────────────

    #[tokio::test]
    async fn a_disconnect_centers_any_live_axis_output() {
        // A1: a grid key Axis-assigned and pushed to full travel when the
        // device drops leaves its last `ABS_*` value asserted with no key
        // left to release it. `handle_connection_change` now runs
        // `reset_axis_outputs` in its `tear_down` arm, exactly as a Layer
        // switch does.
        let mut config = config_with_bindings(HashMap::new());
        config
            .active_profile_mut()
            .unwrap()
            .axis_base
            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);
        let harness = CommandHarness::spawn(config);

        harness.push_depth([(Input::Grid(1, 1), 255)]);
        tokio::task::yield_now().await;
        harness.set_device_connected(false).await;
        tokio::task::yield_now().await;

        let writes = flat_axis_writes(harness.gamepad_batches());
        harness.shut_down().await;

        assert_eq!(
            writes,
            vec![
                (evdev::AbsoluteAxisCode::ABS_Z, 255),
                (evdev::AbsoluteAxisCode::ABS_Z, 0),
            ],
            "the disconnect centered the frozen axis output"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_disconnect_stops_a_live_analog_repeat_task_and_releases_its_held_key() {
        // A2: the dual-stage `spec.md` listed Analog-repeat's lack of
        // dropout handling Out of Scope as "a separate effort"; ticket 20
        // decided ticket 21 is that effort. A hold-solid Analog-repeat task
        // holds `KEY_F1` down forever against a frozen Depth snapshot after
        // the device drops — `analog_repeat.stop_all()` in the `Disconnect`
        // arm cancels the task and force-releases the key.
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
        let harness = CommandHarness::spawn(config_with_bindings(bindings));

        harness.push_depth([(Input::Grid(1, 1), u8::MAX)]);
        tokio::task::yield_now().await;
        assert_eq!(
            key_and_value(harness.sink.batches()[0][0]),
            (evdev::KeyCode::KEY_F1, 1),
            "the hold-solid task has KEY_F1 held down"
        );

        harness.set_device_connected(false).await;
        tokio::task::yield_now().await;
        let after_disconnect = harness.sink.batches().len();
        assert_eq!(
            key_and_value(harness.sink.batches()[after_disconnect - 1][0]),
            (evdev::KeyCode::KEY_F1, 0),
            "the disconnect force-released the held key"
        );

        // Advancing well past several kernel periods produces nothing more —
        // the task is genuinely cancelled, not paused against a stale watch.
        tokio::time::advance(Duration::from_millis(500)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            harness.sink.batches().len(),
            after_disconnect,
            "no further output after the disconnect stopped the task"
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn a_layer_switch_drains_a_live_chord_hold_to_repeat_firing() {
        // A5: a live Chord Hold-to-repeat firing (its bare `value=1`
        // `HoldKeyDown`) survives a Layer switch with no release edge — the
        // member `Up` that would end it is on the old Layer's suppression
        // path. `chord_slots.drain_firings` in the `LayerSwitch` arm is the
        // stuck-key balance, matching `individual.drain_firings`.
        let mut profile = Profile::default();
        profile.chords_base.insert(
            ChordKey::new(BTreeSet::from([Input::Grid(1, 1), Input::Grid(1, 2)])),
            hold_to_repeat_binding(evdev::KeyCode::KEY_C),
        );
        let mut seam = Seam::new(config_with_profile(profile));

        seam.press(Input::Grid(1, 1)).await;
        seam.press(Input::Grid(1, 2)).await;
        settle().await;
        let mark = seam.sink.batches().len();
        assert!(
            (0..mark)
                .map(|i| key_and_value(seam.sink.batches()[i][0]))
                .any(|kv| kv == (evdev::KeyCode::KEY_C, 1)),
            "the Chord Hold-to-repeat is holding KEY_C down"
        );

        seam.feed(PhysicalEvent {
            input: Input::ModeKey,
            state: EventState::Down,
            depth: None,
        })
        .await;
        settle().await;

        let released: Vec<_> = seam.sink.batches()[mark..]
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert!(
            released.contains(&(evdev::KeyCode::KEY_C, 0)),
            "the Layer switch drained the Chord firing, releasing KEY_C: {released:?}"
        );

        seam.state.chord_slots.stop_all_toggles().await;
        seam.state.chord_slots.drain_firings(&seam.inj).await;
        seam.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_profile_switch_drains_a_live_chord_toggle() {
        // A6: `edit.rs` documented "an active Chord Toggle survives a
        // Profile switch today" as current behaviour, never as a decision.
        // Keybinder `spec.md`: "Profile switch releases every active Toggle
        // immediately" — unqualified. `chord_slots.stop_all_toggles()` now
        // runs in the `ProfileSwitch` arm alongside the individual path's.
        let mut profile = Profile::default();
        profile.chords_base.insert(
            ChordKey::new(BTreeSet::from([Input::Grid(1, 1), Input::Grid(1, 2)])),
            toggle_binding(evdev::KeyCode::KEY_C),
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

        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        settle().await;
        let mark = harness.sink.batches().len();
        assert!(
            (0..mark)
                .map(|i| key_and_value(harness.sink.batches()[i][0]))
                .any(|kv| kv == (evdev::KeyCode::KEY_C, 1)),
            "the Chord Toggle turned KEY_C on"
        );

        harness
            .switch_profile("Gaming")
            .await
            .expect("SwitchProfile must succeed");
        settle().await;

        let released: Vec<_> = harness.sink.batches()[mark..]
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert!(
            released.contains(&(evdev::KeyCode::KEY_C, 0)),
            "the Profile switch drained the Chord Toggle, releasing KEY_C: {released:?}"
        );

        harness.shut_down().await;
    }

    // ── ticket 22: the integration net — a config edit that orphans a live
    //    individual Toggle or Chord firing/Toggle now pushes the teardown ──

    #[tokio::test(start_paused = true)]
    async fn clearing_a_chord_binding_force_releases_its_live_chord_toggle() {
        // B11: a `ClearChordBinding` while a Chord Toggle is live leaves the
        // Toggle with nothing to stop it — its key is gone from
        // `chords(layer)`, so no fresh full-member completion can route a
        // stop through `chord::feed`. `Effect::StopChord` force-releases it
        // on commit.
        let mut profile = Profile::default();
        let members = [Input::Grid(1, 1), Input::Grid(1, 2)];
        profile.chords_base.insert(
            ChordKey::new(BTreeSet::from(members)),
            toggle_binding(evdev::KeyCode::KEY_C),
        );
        let harness = CommandHarness::spawn(config_with_profile(profile));

        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        settle().await;
        let mark = harness.sink.batches().len();
        assert!(
            (0..mark)
                .map(|i| key_and_value(harness.sink.batches()[i][0]))
                .any(|kv| kv == (evdev::KeyCode::KEY_C, 1)),
            "the Chord Toggle turned KEY_C on"
        );

        harness
            .clear_chord_binding(members, Layer::Base)
            .await
            .expect("ClearChordBinding must succeed");
        settle().await;

        let released: Vec<_> = harness.sink.batches()[mark..]
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert!(
            released.contains(&(evdev::KeyCode::KEY_C, 0)),
            "the clear released the Chord Toggle's key: {released:?}"
        );

        let after = harness.sink.batches().len();
        tokio::time::advance(Duration::from_millis(500)).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            after,
            "no further output — the Chord Toggle is genuinely stopped, not paused"
        );

        harness.shut_down().await;
    }

    #[tokio::test(start_paused = true)]
    async fn replacing_a_live_individual_toggles_binding_releases_the_old_action_immediately() {
        // B10: overwriting an individual Toggle's Binding with a different
        // Action used to leave the old Action running until the key's next
        // press (`dispatch.rs`'s stop-toggle-on-`Down`). `Effect::StopToggle`
        // now releases it on commit.
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::from([(
            Input::Grid(1, 1),
            toggle_binding(evdev::KeyCode::KEY_C),
        )])));

        harness.press(Input::Grid(1, 1)).await;
        settle().await;
        let mark = harness.sink.batches().len();
        assert!(
            (0..mark)
                .map(|i| key_and_value(harness.sink.batches()[i][0]))
                .any(|kv| kv == (evdev::KeyCode::KEY_C, 1)),
            "the individual Toggle turned KEY_C on"
        );

        harness
            .set_binding(
                Input::Grid(1, 1),
                Layer::Base,
                toggle_binding(evdev::KeyCode::KEY_D),
            )
            .await
            .expect("SetBinding must succeed");
        settle().await;

        let released: Vec<_> = harness.sink.batches()[mark..]
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert!(
            released.contains(&(evdev::KeyCode::KEY_C, 0)),
            "the replace released the stale Toggle's KEY_C without a second press: {released:?}"
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn assigning_an_axis_to_a_chord_member_drains_the_live_chord_firing() {
        // B9: `SetAxisAssignment` atomically removes the Chord membership for
        // the key; a live Chord Hold-to-repeat firing on that Chord then has
        // no member `Up` left to end it. `Effect::StopChord` (one per removed
        // membership) force-releases it.
        let mut profile = Profile::default();
        let members = [Input::Grid(1, 1), Input::Grid(1, 2)];
        profile.chords_base.insert(
            ChordKey::new(BTreeSet::from(members)),
            hold_to_repeat_binding(evdev::KeyCode::KEY_C),
        );
        let harness = CommandHarness::spawn(config_with_profile(profile));

        harness.press(Input::Grid(1, 1)).await;
        harness.press(Input::Grid(1, 2)).await;
        settle().await;
        let mark = harness.sink.batches().len();
        assert!(
            (0..mark)
                .map(|i| key_and_value(harness.sink.batches()[i][0]))
                .any(|kv| kv == (evdev::KeyCode::KEY_C, 1)),
            "the Chord Hold-to-repeat is holding KEY_C down"
        );

        harness
            .set_axis_assignment(Input::Grid(1, 1), Layer::Base, AxisTarget::LeftTrigger)
            .await
            .expect("SetAxisAssignment must succeed");
        settle().await;

        let released: Vec<_> = harness.sink.batches()[mark..]
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert!(
            released.contains(&(evdev::KeyCode::KEY_C, 0)),
            "the axis assignment drained the Chord firing, releasing KEY_C: {released:?}"
        );

        harness.shut_down().await;
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_deadline_never_resolves_without_one_and_resolves_promptly_for_a_past_instant()
    {
        // `None` never resolves — a 1s race against it must time out.
        let raced = tokio::time::timeout(Duration::from_secs(1), wait_for_deadline(None)).await;
        assert!(raced.is_err(), "wait_for_deadline(None) must never resolve");

        // `Some(past)` resolves right away.
        let past = Instant::now() - Duration::from_millis(1);
        tokio::time::timeout(Duration::from_secs(1), wait_for_deadline(Some(past)))
            .await
            .expect("wait_for_deadline(Some(past)) must resolve promptly");
    }

    /// A multiset of decoded `(KeyCode, value)` events — for asserting two
    /// concurrently in-flight Fire-once firings both completed a full
    /// Down/Up pair without depending on how their individual steps
    /// interleaved (`Slots::perform` spawns and returns without awaiting).
    fn event_counts(events: &[(evdev::KeyCode, i32)]) -> HashMap<(evdev::KeyCode, i32), usize> {
        let mut counts = HashMap::new();
        for &event in events {
            *counts.entry(event).or_insert(0) += 1;
        }
        counts
    }

    #[tokio::test]
    async fn dual_stage_handoff_walks_primary_then_deep_then_back_across_separate_reports() {
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        // The primary crossing: the real Analog-sourced Down `capture::
        // analog` would emit for any single-stage key's own Actuation point
        // — `stage::Engine` leaves this lone `FirePrimary` row alone
        // entirely, relying on this unmodified path.
        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        // A dual-stage key's primary press is machine-sequenced input — it
        // carries no Fire-once dwell (ticket 17 Addendum), so the primary
        // Down/Up pair lands back to back and the overlap guard is clear for
        // the RepressPrimary below with a plain `settle()`.
        settle().await;

        // The deep crossing, a separate report: primary's own hysteresis
        // never re-crosses its own Actuation/Release going from 150 to 250,
        // so capture emits nothing here — only `stage::Engine`, off the
        // depth watch, reacts (`ReleasePrimary` -> `FireDeep`).
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;

        // Back out of the deep band, a separate report again (`ReleaseDeep`
        // -> `RepressPrimary`, a *fresh* Fire-once).
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;

        // The full release: the real Analog-sourced Up — a no-op against
        // the already self-released `RepressPrimary` firing.
        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;

        let batches = harness.shut_down().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 0),
                (evdev::KeyCode::KEY_B, 1),
                (evdev::KeyCode::KEY_B, 0),
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 0),
            ],
            "primary fires, hands off to deep, hands back to a fresh primary press, \
             and the final real Up is a no-op against an already self-released Fire-once"
        );
    }

    #[tokio::test]
    async fn dual_stage_handoff_same_report_double_crossing_resolves_synchronously() {
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        // A single hidraw report jumping straight from fully released past
        // the deep Actuation point, with **no** real primary event ever
        // sent — the adversarial case ADR-0007's ordering-race fix must
        // survive: mechanical replay walks the full `[FirePrimary,
        // ReleasePrimary, FireDeep]` sequence synchronously, off `rx_depth`
        // alone, in one `stage::Engine::update` call. `FirePrimary` is
        // *initiated* strictly before `FireDeep` (checked below via each
        // fire's own leading `Down`), but `Slots::perform` spawns each
        // Fire-once and returns without awaiting it — the same fire-and-
        // forget shape any two ordinary Fire-once presses have — so once
        // both are concurrently in flight their *own* Down/Up pairs can
        // interleave with each other; this asserts the multiset each jump
        // produces, plus each jump's leading event, rather than one single
        // fully-interleaved sequence.
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        let first_jump: Vec<_> = harness
            .sink
            .batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert_eq!(
            first_jump[0],
            (evdev::KeyCode::KEY_A, 1),
            "FirePrimary must be initiated before FireDeep, even with no real \
             primary event ever sent"
        );
        assert_eq!(
            event_counts(&first_jump),
            event_counts(&[
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 0),
                (evdev::KeyCode::KEY_B, 1),
                (evdev::KeyCode::KEY_B, 0),
            ]),
            "FirePrimary and FireDeep must each complete a full Down/Up pair"
        );

        // The mirror on the way back out, in one report too:
        // `[ReleaseDeep, RepressPrimary, ReleasePrimary]` — `ReleaseDeep`
        // and the trailing `ReleasePrimary` are no-ops against an
        // already-self-released Fire-once, so only `RepressPrimary`'s fresh
        // fire is visible.
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;
        let batches = harness.shut_down().await;
        let second_jump: Vec<_> = batches[first_jump.len()..]
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert_eq!(
            second_jump,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "RepressPrimary must fire fresh; ReleaseDeep/the trailing ReleasePrimary \
             stay no-ops"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_handoff_toggle_primary_gets_a_fresh_loop_on_repress_not_a_resume() {
        let config = dual_stage_config(
            StagingMode::Handoff,
            toggle_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        // The primary crossing starts the Toggle loop — a lone `FirePrimary`
        // row, unaffected by `stage::Engine`.
        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        let running_count = harness.sink.batches().len();
        assert!(running_count > 0, "the Toggle loop must already be tapping");

        // Crossing into the deep band: `ReleasePrimary` force-stops the
        // Toggle unconditionally (unlike an ordinary bare Up, which leaves a
        // Toggle running) before `FireDeep` fires.
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        let stopped_count = harness.sink.batches().len();

        // Advancing well past several laps produces nothing further — the
        // Toggle is genuinely gone, not merely paused mid-loop.
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 5).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            stopped_count,
            "the Toggle must be fully stopped, not paused, once the deep band engages"
        );

        // Crossing back out: `RepressPrimary` starts a *fresh* Toggle loop
        // (`decide(Toggle, Down, None)` has no "resume") — new taps resume.
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        assert!(
            harness.sink.batches().len() > stopped_count,
            "a fresh Toggle loop must be tapping again after RepressPrimary"
        );

        harness.stop_all_toggles().await;
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn dual_stage_no_return_never_represses_the_primary_on_the_way_back_out() {
        let config = dual_stage_config(
            StagingMode::NoReturn,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        // A dual-stage primary carries no Fire-once dwell (ticket 17
        // Addendum) — its self-balanced KeyUp lands right here, not as a
        // redundant trailing release after the handoff, so a plain
        // `settle()` suffices.
        settle().await;

        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;

        // Back out of the deep band: No-Return releases the deep stage
        // only — no `RepressPrimary`, unlike Handoff's own equivalent row.
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;

        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;

        let batches = harness.shut_down().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 0),
                (evdev::KeyCode::KEY_B, 1),
                (evdev::KeyCode::KEY_B, 0),
            ],
            "No-Return must never repress the primary on the way back out — \
             the key stays quiet until a fresh press"
        );
    }

    /// Ticket 17 Addendum: a dual-stage key's primary press is machine-
    /// sequenced input, so a Fire-once single-key primary carries **no**
    /// `FIRE_ONCE_KEY_DWELL`. Pressing the primary and crossing into the deep
    /// band on the very next report — inside what used to be the 40 ms dwell
    /// window — leaves exactly one balanced `[Down, Up]` for the primary,
    /// with no redundant trailing `value=0` from a dwell task landing after
    /// the deep excursion already released the key, and the primary's `Up` is
    /// not deferred by the dwell.
    #[tokio::test(start_paused = true)]
    async fn dual_stage_primary_press_carries_no_fire_once_dwell() {
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        // The real Analog-sourced primary Down, then the deep crossing on the
        // next report with no time advanced — the deep excursion lands well
        // inside the old dwell window.
        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        let after_primary: Vec<_> = harness
            .sink
            .batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert_eq!(
            after_primary,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "the primary's Fire-once pair lands back to back — its Up is not \
             held for a 40 ms dwell"
        );

        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        // A stray dwell task, had one been spawned, would land its redundant
        // KEY_A `value=0` somewhere in this window.
        tokio::time::advance(executor::FIRE_ONCE_KEY_DWELL * 2).await;
        settle().await;

        let batches = harness.shut_down().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 0),
                (evdev::KeyCode::KEY_B, 1),
                (evdev::KeyCode::KEY_B, 0),
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 0),
            ],
            "every KEY_A press is a balanced Down/Up pair — no redundant \
             trailing value=0 from a dwell task"
        );
    }

    /// Ticket 17 Addendum: a dual-stage primary tapped as a plain key — never
    /// crossing into the deep band — still fires its Fire-once pair exactly
    /// once; it just no longer holds the ~40 ms plausibility dwell on that
    /// press.
    #[tokio::test(start_paused = true)]
    async fn dual_stage_primary_tapped_without_crossing_deep_fires_dwell_free() {
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        // No time advanced: if the dwell were still on, only KEY_A `value=1`
        // would be visible until 40 ms elapse.
        settle().await;
        let after_press: Vec<_> = harness
            .sink
            .batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert_eq!(
            after_press,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "the Fire-once pair lands immediately, dwell-free"
        );

        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        tokio::time::advance(executor::FIRE_ONCE_KEY_DWELL * 2).await;
        settle().await;

        let batches = harness.shut_down().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "a dual-stage primary that never crosses deep just fires its \
             Fire-once pair once"
        );
    }

    #[tokio::test]
    async fn dual_stage_handoff_deep_stage_hold_to_repeat_actually_repeats() {
        // A Hold-to-repeat deep Binding must keep tapping while the deep
        // band stays engaged — `FireDeep` only fires on the crossing, so the
        // repeat cadence rides `capture::analog`'s synthesized primary
        // `Repeat` pulses (the deep band is strictly above the primary's, so
        // every pulse that holds the primary holds the deep band too).
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            hold_to_repeat_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        // Ticket 04: the deep single-key Hold-to-repeat now presents as
        // genuine kernel autorepeat through the deep `Slots<StageKey>` —
        // `value=1` on the `FireDeep` crossing (after the FireOnce primary's
        // own `[Down, Up]` on its crossing).
        assert_eq!(
            key_and_value(harness.sink.batches().last().unwrap()[0]),
            (evdev::KeyCode::KEY_B, 1),
            "FireDeep taps KEY_B value=1 on the crossing"
        );
        let after_crossing = harness.sink.batches().len();

        // Synthesized primary Repeat pulses while both bands stay engaged:
        // the deep Hold-to-repeat emits one `value=2` per pulse, not a
        // `[Down, Up]` pair and not a single Fire-once.
        for _ in 0..3 {
            harness.repeat_analog(Input::Grid(1, 1), 250).await;
            settle().await;
        }
        assert_eq!(
            harness.sink.batches()[after_crossing..]
                .iter()
                .map(|b| key_and_value(b[0]))
                .collect::<Vec<_>>(),
            vec![
                (evdev::KeyCode::KEY_B, 2),
                (evdev::KeyCode::KEY_B, 2),
                (evdev::KeyCode::KEY_B, 2),
            ],
            "the deep Hold-to-repeat must autorepeat on every pulse"
        );
        let after_repeats = harness.sink.batches().len();

        // Back out of the deep band: `ReleaseDeep` force-releases the held
        // deep `value=1` (`RepressPrimary` also re-fires the FireOnce primary).
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        assert!(
            harness.sink.batches()[after_repeats..]
                .iter()
                .any(|b| key_and_value(b[0]) == (evdev::KeyCode::KEY_B, 0)),
            "ReleaseDeep must balance the deep hold's value=1"
        );
        let after_release = harness.sink.batches().len();
        harness.repeat_analog(Input::Grid(1, 1), 150).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            after_release,
            "no more deep autorepeat once the deep band releases"
        );

        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn dual_stage_handoff_suppresses_the_hold_to_repeat_primary_under_the_deep_stage() {
        // The primary band stays physically Down through a Handoff hand-off,
        // so `capture::analog` keeps synthesizing the primary's Hold-to-
        // repeat `Repeat` pulses — every one must be swallowed while the deep
        // stage holds the press, then resume the instant `RepressPrimary`
        // hands it back.
        let config = dual_stage_config(
            StagingMode::Handoff,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.repeat_analog(Input::Grid(1, 1), 150).await;
        settle().await;
        assert!(
            !harness.sink.batches().is_empty(),
            "the primary taps normally on Down + Repeat before any hand-off"
        );

        // Into the deep band: `[ReleasePrimary, FireDeep]`.
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        let after_handoff = harness.sink.batches().len();

        for _ in 0..3 {
            harness.repeat_analog(Input::Grid(1, 1), 250).await;
            settle().await;
        }
        assert_eq!(
            harness.sink.batches().len(),
            after_handoff,
            "the handed-off Hold-to-repeat primary must stop tapping under the deep stage"
        );

        // Back out: `RepressPrimary` re-fires it, and its Repeats resume.
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.repeat_analog(Input::Grid(1, 1), 150).await;
        settle().await;
        assert!(
            harness.sink.batches().len() > after_handoff,
            "the primary must tap again once the deep band releases"
        );

        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn dual_stage_no_return_keeps_the_hold_to_repeat_primary_suppressed_past_the_deep_band() {
        // No-Return doesn't repress on the way out, so its primary stays
        // suppressed even after the deep band releases — until a full
        // physical release and a fresh press.
        let config = dual_stage_config(
            StagingMode::NoReturn,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        let after_handoff = harness.sink.batches().len();

        // Out of the deep band (No-Return: `[ReleaseDeep]`, no repress), then
        // several synthesized primary Repeats — all still swallowed.
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        for _ in 0..3 {
            harness.repeat_analog(Input::Grid(1, 1), 150).await;
            settle().await;
        }
        assert_eq!(
            harness.sink.batches().len(),
            after_handoff,
            "No-Return keeps the primary silent after the deep band releases, until a fresh press"
        );

        // Full release, then a fresh press — the primary taps again.
        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;
        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        assert!(
            harness.sink.batches().len() > after_handoff,
            "a fresh press after full release taps the primary again"
        );

        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn dual_stage_key_in_digital_mode_fires_only_its_primary() {
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        // Digital-sourced events only (`depth: None`) — `rx_depth` never
        // moves, so `stage::Engine::update` never runs at all; the deep
        // stage is inert for free (spec.md: "In Digital Capture mode ...
        // only the primary fires").
        harness.press(Input::Grid(1, 1)).await;
        harness.release(Input::Grid(1, 1)).await;
        settle().await;

        let batches = harness.shut_down().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)]
        );
    }

    // ── `tartarus-dual-stage-keys` ticket 04: Quick-Skip's dispatch-side
    // primary-suppression buffer ──────────────────────────────────────────

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_fast_full_press_skips_the_primary_entirely() {
        // A single hidraw report jumping straight from released past the
        // deep Actuation point — the real primary `Down` PhysicalEvent
        // carries `depth: Some(250)`, already past the deep band's own
        // threshold (220/200). `begin_windowed_press` resolves this
        // synchronously from that event's own depth field: `KEY_A` (primary)
        // must never appear at all, only `KEY_B` (deep).
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 250).await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;

        // Advancing well past the window produces nothing further — this
        // press already resolved to Skipped, not merely still Armed.
        tokio::time::advance(Duration::from_millis(100)).await;
        settle().await;

        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;

        let batches = harness.shut_down().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_B, 1), (evdev::KeyCode::KEY_B, 0)],
            "the primary's Down (and its eventual Up) must be skipped entirely"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_deep_reached_within_the_window_becomes_skipped() {
        // A slower press: the primary crosses first (a separate report),
        // arming the ~50ms window; the deep band is reached shortly after,
        // still inside the window — resolved to Skipped via `stage::
        // Engine::update`'s own `rx_depth` path this time, not the
        // synchronous same-report check.
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;

        tokio::time::advance(Duration::from_millis(10)).await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;

        // The window's own deadline elapsing afterward changes nothing —
        // this key already resolved to Skipped, not Armed.
        tokio::time::advance(Duration::from_millis(100)).await;
        settle().await;

        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;

        let batches = harness.shut_down().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_B, 1), (evdev::KeyCode::KEY_B, 0)],
            "the primary must never fire once the deep band is reached inside the window"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_deadline_elapses_fires_primary_late_then_runs_as_handoff() {
        // The primary crosses, but the deep band is never reached inside the
        // window — the buffered primary fires retroactively once the
        // deadline elapses, and the rest of the press runs as ordinary
        // Handoff from there.
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        assert!(
            harness.sink.batches().is_empty(),
            "the primary must stay buffered, not fire immediately"
        );

        tokio::time::advance(Duration::from_millis(60)).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            2,
            "the deadline elapsing must fire the primary retroactively (a Fire-once \
             keypress self-completes as its own Down+Up pair, two batches)"
        );

        // From here on, plain Handoff: deep engages (releases the now-Late
        // primary, fires deep), then disengages (releases deep, represses a
        // *fresh* primary).
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;

        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;

        let batches = harness.shut_down().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 0),
                (evdev::KeyCode::KEY_B, 1),
                (evdev::KeyCode::KEY_B, 0),
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 0),
            ],
            "Late fires the buffered primary retroactively, then the rest of the \
             press runs as ordinary Handoff — including the final real Up landing \
             as a no-op against an already self-released Fire-once"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_quick_shallow_tap_fires_the_primary_as_a_tap() {
        // `either-or-staging-mode` §"Early-Up flush": the primary crosses and
        // arms the window, but the key is released again before the deadline
        // and before the deep band is ever reached. The buffered primary is
        // flushed as a tap — its Down on the release, its Up one canned-tap
        // dwell (40ms) later — through the real `run` loop's stage-deadline
        // arm.
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        assert!(harness.sink.batches().is_empty(), "still buffered");

        tokio::time::advance(Duration::from_millis(10)).await;
        harness.release_analog(Input::Grid(1, 1), 50).await;
        harness.push_depth([(Input::Grid(1, 1), 50)]);
        settle().await;
        let events = |h: &CommandHarness| -> Vec<_> {
            let batches = h.sink.batches();
            batches.iter().map(|b| key_and_value(b[0])).collect()
        };
        assert_eq!(
            events(&harness),
            vec![(evdev::KeyCode::KEY_A, 1)],
            "the buffered primary Down is performed on the release"
        );

        tokio::time::advance(executor::FIRE_ONCE_KEY_DWELL - Duration::from_millis(1)).await;
        settle().await;
        assert_eq!(events(&harness), vec![(evdev::KeyCode::KEY_A, 1)]);

        tokio::time::advance(Duration::from_millis(1)).await;
        settle().await;
        assert_eq!(
            events(&harness),
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "its Up follows one canned-tap dwell later"
        );

        // Well past the original window: no Late re-fire, nothing further.
        tokio::time::advance(Duration::from_millis(100)).await;
        settle().await;

        let batches = harness.shut_down().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)]
        );
    }

    /// A quick shallow tap on `Grid(1, 1)` through the `Seam`: the primary
    /// crosses (arming the Quick-Skip window) and releases 10ms later
    /// without reaching the deep band. Returns the `Up` edge's `Edit`s.
    async fn quick_skip_shallow_tap(seam: &mut Seam) -> Vec<edit::Edit> {
        let edits = seam
            .feed(PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Down,
                depth: Some(150),
            })
            .await;
        assert!(edits.is_empty());
        tokio::time::advance(Duration::from_millis(10)).await;
        seam.feed(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Up,
            depth: Some(50),
        })
        .await
    }

    /// Advances paused time by `by`, then runs the stage-deadline arm the
    /// way `run`'s `select!` would once it elapsed.
    async fn advance_stages(seam: &mut Seam, by: Duration) -> Vec<edit::Edit> {
        tokio::time::advance(by).await;
        let edits = seam
            .state
            .tick_stages(&seam.config, Instant::now())
            .await
            .unwrap();
        settle().await;
        edits
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_flushed_fire_once_primary_fires_once() {
        let mut seam = Seam::new(dual_stage_config(
            StagingMode::QuickSkip,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        ));

        assert!(quick_skip_shallow_tap(&mut seam).await.is_empty());
        advance_stages(&mut seam, executor::FIRE_ONCE_KEY_DWELL).await;
        advance_stages(&mut seam, Duration::from_millis(100)).await;

        let batches = seam.finish().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "a Fire-once primary fires exactly once"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_flushed_toggle_primary_keeps_running_after_the_tap() {
        let mut seam = Seam::new(dual_stage_config(
            StagingMode::QuickSkip,
            toggle_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        ));

        quick_skip_shallow_tap(&mut seam).await;
        advance_stages(&mut seam, Duration::from_millis(100)).await;

        let batches = seam.sink.batches();
        assert!(
            !batches.is_empty() && key_and_value(batches[0][0]) == (evdev::KeyCode::KEY_A, 1),
            "the Toggle started on the flush: {batches:?}"
        );
        assert!(
            !batches
                .iter()
                .any(|b| key_and_value(b[0]) == (evdev::KeyCode::KEY_A, 0)),
            "a Toggle outlives its release — the deferred Up must not stop it: {batches:?}"
        );
        seam.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_flushed_mouse_button_primary_is_held_for_the_dwell() {
        let button = evdev::KeyCode::BTN_LEFT;
        let mut seam = Seam::new(dual_stage_config(
            StagingMode::QuickSkip,
            hold_to_repeat_binding(button),
            keypress_binding(evdev::KeyCode::KEY_B),
        ));

        quick_skip_shallow_tap(&mut seam).await;
        advance_stages(
            &mut seam,
            executor::FIRE_ONCE_KEY_DWELL - Duration::from_millis(1),
        )
        .await;
        let batches = seam.sink.batches();
        let held: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(held, vec![(button, 1)]);

        advance_stages(&mut seam, Duration::from_millis(1)).await;
        let batches = seam.finish().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(button, 1), (button, 0)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_flushed_controller_button_primary_taps_the_gamepad() {
        let button = evdev::KeyCode::BTN_SOUTH;
        let mut seam = Seam::new(dual_stage_config(
            StagingMode::QuickSkip,
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action: Action::ControllerButton { button },
            },
            keypress_binding(evdev::KeyCode::KEY_B),
        ));

        quick_skip_shallow_tap(&mut seam).await;
        advance_stages(
            &mut seam,
            executor::FIRE_ONCE_KEY_DWELL - Duration::from_millis(1),
        )
        .await;
        let held: Vec<_> = seam
            .gamepad_batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert_eq!(held, vec![(button, 1)]);

        advance_stages(&mut seam, Duration::from_millis(1)).await;
        let gamepad: Vec<_> = seam
            .gamepad_batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert_eq!(
            gamepad,
            vec![(button, 1), (button, 0)],
            "Down, 40ms, Up — on the gamepad device"
        );
        seam.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_flushed_profile_switch_primary_switches_profile() {
        let mut config = dual_stage_config(
            StagingMode::QuickSkip,
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::ProfileSwitch {
                    target: "Gaming".to_string(),
                },
            },
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        config
            .profiles
            .insert("Gaming".to_string(), Profile::default());
        let mut seam = Seam::new(config);

        let edits = quick_skip_shallow_tap(&mut seam).await;
        assert_eq!(
            edits,
            vec![edit::Edit::SwitchProfile {
                name: "Gaming".to_string()
            }]
        );
        assert_eq!(
            seam.state.stage.next_deadline(),
            None,
            "a ProfileSwitch has no deferred Up to perform"
        );
        assert!(seam.finish().await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_re_press_before_the_flushed_up_performs_it_first() {
        let mut seam = Seam::new(dual_stage_config(
            StagingMode::QuickSkip,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        ));

        quick_skip_shallow_tap(&mut seam).await;
        tokio::time::advance(Duration::from_millis(10)).await;
        seam.feed(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Down,
            depth: Some(150),
        })
        .await;
        let batches = seam.sink.batches();
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "the pending Up is performed immediately on the re-press"
        );
        // …and the new press begins normally: Armed, with a fresh window.
        assert_eq!(
            seam.state.stage.next_deadline(),
            Some(Instant::now() + Duration::from_millis(50))
        );

        // It resolves Late like any other press, and the old pending Up
        // never lands on top of it.
        advance_stages(&mut seam, Duration::from_millis(50)).await;
        let batches = seam.finish().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![
                (evdev::KeyCode::KEY_A, 1),
                (evdev::KeyCode::KEY_A, 0),
                (evdev::KeyCode::KEY_A, 1),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_layer_switch_with_a_flushed_up_pending_releases_the_primary() {
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        tokio::time::advance(Duration::from_millis(10)).await;
        harness.release_analog(Input::Grid(1, 1), 50).await;
        harness.push_depth([(Input::Grid(1, 1), 50)]);
        settle().await;

        harness.press(Input::ModeKey).await;
        settle().await;
        let events: Vec<_> = harness
            .sink
            .batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "the switch's teardown releases the flushed primary immediately"
        );

        tokio::time::advance(Duration::from_millis(100)).await;
        settle().await;
        harness.release(Input::ModeKey).await;
        let batches = harness.shut_down().await;
        assert_eq!(batches.len(), 2, "the cancelled pending Up fires nothing");
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_profile_switch_with_a_flushed_up_pending_releases_primary() {
        let mut config = dual_stage_config(
            StagingMode::QuickSkip,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        config
            .profiles
            .insert("Gaming".to_string(), Profile::default());
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        tokio::time::advance(Duration::from_millis(10)).await;
        harness.release_analog(Input::Grid(1, 1), 50).await;
        harness.push_depth([(Input::Grid(1, 1), 50)]);
        settle().await;

        harness.switch_profile("Gaming").await.unwrap();
        settle().await;
        tokio::time::advance(Duration::from_millis(100)).await;
        settle().await;

        let batches = harness.shut_down().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "released by the switch's teardown, nothing stuck and nothing doubled"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_capture_flip_with_a_flushed_up_pending_releases_the_primary() {
        let mut seam = Seam::new(dual_stage_config(
            StagingMode::QuickSkip,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        ));
        seam.state
            .handle_capture_mode_change(CaptureMode::Analog)
            .await;

        quick_skip_shallow_tap(&mut seam).await;
        seam.state
            .handle_capture_mode_change(CaptureMode::Digital)
            .await;
        settle().await;
        assert_eq!(
            seam.state.stage.next_deadline(),
            None,
            "the pending Up is cancelled"
        );

        let batches = seam.finish().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "the flip's teardown releases the flushed primary"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_quick_shallow_tap_disarms_the_window_on_the_up_edge() {
        // Ticket 13's stuck-key repro, made deterministic on the `Seam` seam:
        // a coalescing `rx_depth` `{KEY: 0}` tick lands *first* (fresh runtime,
        // a no-op), then the queued Down/Up drain. `feed` must disarm the
        // window off the real `Up` edge itself — the depth path never gets
        // another tick to run the cancel row, so if `feed` swallows the `Up`
        // without disarming, the deadline fires `RepressPrimary` into a press
        // with no release edge left and the Hold-to-repeat primary latches.
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            hold_to_repeat_binding(evdev::KeyCode::KEY_B),
        );
        let mut seam = Seam::new(config);

        seam.state
            .update_stages(&seam.config, &HashMap::from([(Input::Grid(1, 1), 0)]))
            .await
            .unwrap();

        seam.feed(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Down,
            depth: Some(150),
        })
        .await;
        let deadline = seam
            .state
            .stage
            .next_deadline()
            .expect("the ~50ms window is armed");

        seam.feed(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Up,
            depth: Some(0),
        })
        .await;

        // The window is disarmed off the edge itself; the only deadline left
        // is the flushed tap's own deferred Up, well inside the window.
        let release_due = seam
            .state
            .stage
            .next_deadline()
            .expect("the flushed tap's Up is pending");
        assert!(
            release_due < deadline,
            "the outer Up must disarm the window off the edge itself"
        );

        // Fire the deadline arm past the old window — the tap's Up lands and
        // the window's `RepressPrimary` stays inert.
        tokio::time::advance(Duration::from_millis(100)).await;
        let edits = seam
            .state
            .tick_stages(&seam.config, deadline + Duration::from_millis(1))
            .await
            .unwrap();
        assert!(edits.is_empty());
        assert!(seam.state.stage.next_deadline().is_none());
        settle().await;

        let batches = seam.finish().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "the primary is flushed as one tap — never a stuck value=1"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_late_primary_is_released_when_a_depth_tick_beats_the_real_up() {
        // Ticket 13, second hole: the deadline elapses first, so the buffered
        // Hold-to-repeat primary fires retroactively (`value=1`, `Late`). Then
        // the coalescing `rx_depth` `{KEY: 0}` tick is serviced *before* the
        // queued real primary `Up`: `update` runs the `Late -> None` row, whose
        // lone `ReleasePrimary` it `continue`s past (the ordinary `rx_events`
        // edge is meant to release it). But for a Quick-Skip key `feed` swallows
        // that ordinary edge — so `end_windowed_press` must force-release the
        // primary itself, or `value=1` latches forever (kernel autorepeat).
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let mut seam = Seam::new(config);

        seam.feed(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Down,
            depth: Some(150),
        })
        .await;
        let deadline = seam.state.stage.next_deadline().expect("armed");

        // Deadline elapses -> retroactive primary `value=1`, phase `Late`.
        tokio::time::advance(Duration::from_millis(60)).await;
        seam.state
            .tick_stages(&seam.config, deadline + Duration::from_millis(1))
            .await
            .unwrap();
        settle().await;
        assert_eq!(
            seam.sink
                .batches()
                .iter()
                .map(|b| key_and_value(b[0]))
                .collect::<Vec<_>>(),
            vec![(evdev::KeyCode::KEY_A, 1)],
            "the buffered Hold-to-repeat primary fired retroactively",
        );

        // The `{KEY: 0}` depth tick wins the race against the real `Up`.
        seam.state
            .update_stages(&seam.config, &HashMap::from([(Input::Grid(1, 1), 0)]))
            .await
            .unwrap();

        // Now the real `Up` finally drains.
        seam.feed(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Up,
            depth: Some(0),
        })
        .await;

        let batches = seam.finish().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_A, 1), (evdev::KeyCode::KEY_A, 0)],
            "the retroactively-fired primary must be released on the real Up, \
             not left latched: {events:?}",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_skipped_then_outer_up_releases_the_deep_stage() {
        // Ticket 13's `Skipped` case: a fast full press skips the primary and
        // fires the deep stage, then the key is released in one report straight
        // from the deep band. `feed` must run the No-Return-shaped release off
        // the `Up` edge — deep stage released, primary never touched, window
        // never armed.
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let mut seam = Seam::new(config);

        seam.feed(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Down,
            depth: Some(250),
        })
        .await;
        seam.feed(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Up,
            depth: Some(0),
        })
        .await;

        assert!(seam.state.stage.next_deadline().is_none());

        let batches = seam.finish().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_B, 1), (evdev::KeyCode::KEY_B, 0)],
            "the deep stage fires and releases; the primary KEY_A is never touched",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_layer_switch_while_armed_cancels_the_buffered_primary() {
        // A Layer/Profile switch or capture-mode flip to Digital while Armed
        // cancels outright via `stage::Engine::stop_all()` (already wired at
        // those call sites from ticket 03) — this exercises the Layer-switch
        // call site specifically (the capture-mode-flip call site gets its
        // own sibling test right below); `dual_stage_layer_switch_mid_
        // press_...`/`dual_stage_digital_mode_flip_mid_press_...` above
        // already cover `stop_all()` itself for Handoff, so these only need
        // to confirm the Quick-Skip buffer specifically is included in what
        // it clears.
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;

        harness.press(Input::ModeKey).await;
        settle().await;

        // Advancing well past the original window fires nothing — the
        // buffered primary was dropped by `stop_all()`, not merely deferred.
        tokio::time::advance(Duration::from_millis(100)).await;
        settle().await;

        harness.release(Input::ModeKey).await;
        let batches = harness.shut_down().await;
        assert!(
            batches.is_empty(),
            "a Layer switch while Armed must cancel the buffered primary outright"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_capture_mode_flip_while_armed_cancels_the_buffered_primary() {
        // The capture-mode-flip-to-Digital call site's own share of the
        // Layer-switch test above — `handle_capture_mode_change`'s existing
        // Digital-transition branch (ticket 03) already calls `stage::
        // Engine::stop_all()`; this confirms the Quick-Skip buffer
        // specifically is included in what that clears too. Raw `run()`
        // setup, mirroring `dual_stage_digital_mode_flip_mid_press_...`
        // above, since `DispatchState::new` starts `capture_mode` at
        // `Digital` and must flip to `Analog` first.
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (event_tx, event_rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let (depth_tx, depth_rx) = watch::channel(HashMap::new());
        let (capture_mode_tx, capture_mode_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            event_rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config,
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_rx,
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            RepeatSchedule::new(250, 33),
            depth_rx,
            device_info_channel(),
            led_channel(),
            lighting_channel(),
        ));

        capture_mode_tx.send(CaptureMode::Analog).await.unwrap();
        settle().await;

        event_tx
            .send(PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Down,
                depth: Some(150),
            })
            .await
            .unwrap();
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 150)]));
        settle().await;

        // The Analog->Digital transition while Armed cancels the buffered
        // primary outright via `stage::Engine::stop_all()`.
        capture_mode_tx.send(CaptureMode::Digital).await.unwrap();
        settle().await;

        // Advancing well past the original window fires nothing — the
        // buffered primary was dropped by `stop_all()`, not merely deferred.
        tokio::time::advance(Duration::from_millis(100)).await;
        settle().await;

        drop(event_tx);
        drop(depth_tx);
        drop(capture_mode_tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        assert!(
            sink.batches().is_empty(),
            "a capture-mode flip to Digital while Armed must cancel the buffered \
             primary outright"
        );
    }

    #[tokio::test]
    async fn dual_stage_quick_skip_reordered_depth_tick_still_resolves_skipped_not_late() {
        // Code-review finding on this ticket: `rx_depth` is a coalescing
        // `watch` while `rx_events` is a non-lossy `mpsc` — under load the
        // two can reorder, so `update`'s own bypassed shadow-band tracking
        // can observe a *later*, larger depth sample before this key's
        // still-queued primary `Down` event (carrying an earlier, smaller
        // depth) is ever drained. Drives `DispatchState` directly (the
        // `Seam` seam, ticket 09) so this specific ordering — `update_stages`
        // before `handle_event` — is deterministic rather than left to
        // `tokio::select!`'s fairness draw. `begin_windowed_press` must defer to
        // `update`'s already-tracked `rt.deep` in that case, not the event's
        // own stale depth field, or a fast full press would wrongly Arm (and
        // later fire Late) instead of resolving Skipped.
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let mut seam = Seam::new(config);

        let edits = seam
            .state
            .update_stages(&seam.config, &HashMap::from([(Input::Grid(1, 1), 250)]))
            .await
            .unwrap();
        assert!(
            edits.is_empty(),
            "the bypassed tick must decide no op itself"
        );

        let edits = seam
            .feed(PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Down,
                depth: Some(150),
            })
            .await;
        assert!(edits.is_empty());

        let batches = seam.finish().await;
        let events: Vec<_> = batches.iter().map(|b| key_and_value(b[0])).collect();
        assert_eq!(
            events,
            vec![(evdev::KeyCode::KEY_B, 1), (evdev::KeyCode::KEY_B, 0)],
            "a reordered depth tick must not roll a genuine deep-band crossing back \
             to Armed — the primary must still be skipped, not fired Late"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_layer_switch_mid_press_force_releases_a_live_deep_toggle() {
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            toggle_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        // Hand off into the deep Toggle.
        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        let running_count = harness.sink.batches().len();
        assert!(
            running_count > 0,
            "the deep Toggle loop must already be tapping"
        );

        // A Layer switch mid-press: `handle_layer_switch` wires `stage::
        // Engine::stop_all()` alongside `analog_repeat.stop_all()`, force-
        // releasing the live deep Toggle — the incoming Held Layer's own
        // `deep_held` (empty here) never picks it up mid-Depth.
        harness.press(Input::ModeKey).await;
        settle().await;
        let stopped_count = harness.sink.batches().len();

        tokio::time::advance(executor::MIN_TOGGLE_LAP * 5).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            stopped_count,
            "the deep Toggle must be genuinely stopped by the Layer switch, not paused"
        );

        harness.release(Input::ModeKey).await;
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn dual_stage_layer_switch_mid_press_force_releases_a_live_deep_hold_to_repeat() {
        // Ticket 04 / spec §7: a single-key deep Hold-to-repeat now holds a
        // bare `value=1` while the deep band is engaged (the `value=2` stream
        // is stateless). `stage::Engine::stop_all()` on a Layer switch must
        // still balance that `value=1` with a force-released `value=0`, exactly
        // as it does for a deep Toggle.
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            hold_to_repeat_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        harness.repeat_analog(Input::Grid(1, 1), 250).await;
        settle().await;
        // After the FireOnce primary's own `[Down, Up]`, the deep hold's
        // `value=1` on the crossing then one `value=2` on the pulse.
        let deep: Vec<_> = harness
            .sink
            .batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .filter(|(code, _)| *code == evdev::KeyCode::KEY_B)
            .collect();
        assert_eq!(
            deep,
            vec![(evdev::KeyCode::KEY_B, 1), (evdev::KeyCode::KEY_B, 2)]
        );

        harness.press(Input::ModeKey).await;
        settle().await;
        assert_eq!(
            key_and_value(harness.sink.batches().last().unwrap()[0]),
            (evdev::KeyCode::KEY_B, 0),
            "the Layer switch's stop_all must force-release the held deep value=1"
        );

        let events: Vec<_> = harness
            .sink
            .batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        let downs = events.iter().filter(|(_, v)| *v == 1).count();
        let ups = events.iter().filter(|(_, v)| *v == 0).count();
        assert_eq!(downs, ups, "no key left logically down: {events:?}");

        harness.release(Input::ModeKey).await;
        harness.shut_down().await;
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_digital_mode_flip_mid_press_force_releases_a_live_deep_toggle() {
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            toggle_binding(evdev::KeyCode::KEY_B),
        );
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (event_tx, event_rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let (depth_tx, depth_rx) = watch::channel(HashMap::new());
        let (capture_mode_tx, capture_mode_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            event_rx,
            conn_rx,
            cmd_rx,
            inj.clone(),
            config,
            unused_config_path(),
            None,
            actuation_channel(),
            capture_mode_rx,
            capture_control_channel(),
            executor::MIN_TOGGLE_LAP,
            RepeatSchedule::new(250, 33),
            depth_rx,
            device_info_channel(),
            led_channel(),
            lighting_channel(),
        ));

        // `DispatchState::new` starts `capture_mode` at `Digital` — flip to
        // `Analog` first so the later flip back to `Digital` is a genuine
        // transition `handle_capture_mode_change` actually acts on.
        capture_mode_tx.send(CaptureMode::Analog).await.unwrap();
        settle().await;

        event_tx
            .send(PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Down,
                depth: Some(150),
            })
            .await
            .unwrap();
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 150)]));
        settle().await;
        depth_tx.send_replace(HashMap::from([(Input::Grid(1, 1), 250)]));
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        assert!(
            !sink.batches().is_empty(),
            "the deep Toggle loop must already be tapping"
        );

        // The Analog→Digital transition: `handle_capture_mode_change` wires
        // `stage::Engine::stop_all()` alongside `analog_repeat.stop_all()`
        // in its existing Digital-transition branch, force-releasing the
        // live deep Toggle — `stage::Engine::update` never runs off
        // Digital-sourced events, so nothing would otherwise release it.
        capture_mode_tx.send(CaptureMode::Digital).await.unwrap();
        settle().await;
        let stopped_count = sink.batches().len();

        tokio::time::advance(executor::MIN_TOGGLE_LAP * 5).await;
        settle().await;
        assert_eq!(
            sink.batches().len(),
            stopped_count,
            "the deep Toggle must be genuinely stopped by the capture-mode flip, not paused"
        );

        drop(event_tx);
        drop(depth_tx);
        drop(capture_mode_tx);
        dispatch_handle.await.unwrap().unwrap();
        drop(inj);
        inj_handle.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_deep_toggle_stops_on_release_deep_and_gets_a_fresh_loop_on_refire() {
        // Code-review finding on this ticket: `ReleaseDeep` used to run the
        // ordinary `decide(Up)` + `perform` path, a no-op for a Toggle-mode
        // deep Binding — the deep Toggle never stopped, and the next
        // `FireDeep` unconditionally started a second, orphaned loop over
        // it. `ReleaseDeep` now mirrors `ReleasePrimary`'s own unconditional
        // stop, exactly like this test exercises: two full in/out
        // oscillations through the deep band must never leave more than one
        // Toggle loop alive at a time.
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            toggle_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;

        // First hand-off into the deep Toggle.
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        let first_loop_count = harness.sink.batches().len();
        assert!(
            first_loop_count > 0,
            "the first deep Toggle loop must be tapping"
        );

        // Back out: `ReleaseDeep` must fully stop it.
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        let stopped_count = harness.sink.batches().len();
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 5).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            stopped_count,
            "the first deep Toggle must be genuinely stopped, not left running"
        );

        // Back in: `FireDeep` must start one *fresh* loop, not stack a
        // second one atop a leaked first.
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        let second_loop_count = harness.sink.batches().len() - stopped_count;
        assert!(
            second_loop_count > 0 && second_loop_count <= first_loop_count + 2,
            "exactly one fresh Toggle loop must be tapping, not two overlapping ones \
             (first window: {first_loop_count}, second window: {second_loop_count})"
        );

        // Clean up via the staging mechanism itself (`ReleaseDeep` on the
        // way back out), not `Command::StopAllToggles` — draining
        // `stage::Engine`'s own `Slots<StageKey>` toggles through that
        // command is ticket 06's job (spec.md's "GUI-focus StopAllToggles"
        // extension), not part of this ticket; leaving the second loop
        // running here would hang `shut_down()` forever on the still-open
        // injector clone it holds.
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn dual_stage_layer_switch_mid_press_while_still_deep_does_not_spuriously_refire() {
        // Code-review finding on this ticket: `stop_all()` resetting a held
        // key's `KeyRuntime` to `(Up, Up)` must not let the very next
        // `Engine::update` tick see a stale-`(Up,Up)`-to-still-deep
        // transition and mechanically replay a full fresh press through
        // both bands the instant the new Layer becomes active, even though
        // the user's finger never moved.
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        let engaged_count = harness.sink.batches().len();
        assert!(engaged_count > 0, "primary and deep must have fired by now");

        // A Layer switch while the key is still held deep — `stop_all()`
        // runs, but the key's own Depth never moved.
        harness.press(Input::ModeKey).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            engaged_count,
            "a Layer switch must not spuriously replay a fresh press just because \
             tracking reset under an unmoved Depth"
        );

        // A later depth tick under the new Layer (no deep stage there)
        // still produces nothing further — confirming the reset genuinely
        // resynced rather than merely deferring the spurious replay.
        harness.push_depth([(Input::Grid(1, 1), 200)]);
        settle().await;
        assert_eq!(harness.sink.batches().len(), engaged_count);

        harness.release(Input::ModeKey).await;
        harness.shut_down().await;
    }

    // ── `tartarus-dual-stage-keys` ticket 06: runtime teardown ─────────────

    #[tokio::test(start_paused = true)]
    async fn dual_stage_profile_switch_mid_press_force_releases_a_live_deep_toggle() {
        let mut config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            toggle_binding(evdev::KeyCode::KEY_B),
        );
        config
            .profiles
            .insert("Gaming".to_string(), Profile::default());
        let harness = CommandHarness::spawn(config);

        // Hand off into the deep Toggle.
        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        let running_count = harness.sink.batches().len();
        assert!(
            running_count > 0,
            "the deep Toggle loop must already be tapping"
        );

        // A Profile switch mid-press: `Effect::StopAllStages` joins
        // `SwitchProfile`'s effect list alongside `StopAllToggles`/
        // `StopAllAnalogRepeats`, force-releasing the live deep Toggle.
        harness.switch_profile("Gaming").await.unwrap();
        settle().await;
        let stopped_count = harness.sink.batches().len();

        tokio::time::advance(executor::MIN_TOGGLE_LAP * 5).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            stopped_count,
            "the deep Toggle must be genuinely stopped by the Profile switch, not paused"
        );

        harness.shut_down().await;
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_profile_switch_while_armed_cancels_the_buffered_primary() {
        // The Profile-switch call site's own share of the Layer-switch/
        // capture-mode-flip Quick-Skip-cancellation tests above.
        let mut config = dual_stage_config(
            StagingMode::QuickSkip,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        config
            .profiles
            .insert("Gaming".to_string(), Profile::default());
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;

        harness.switch_profile("Gaming").await.unwrap();
        settle().await;

        // Advancing well past the original window fires nothing — the
        // buffered primary was dropped by `stop_all()`, not merely deferred.
        tokio::time::advance(Duration::from_millis(100)).await;
        settle().await;

        let batches = harness.shut_down().await;
        assert!(
            batches.is_empty(),
            "a Profile switch while Armed must cancel the buffered primary outright"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_deep_actions_own_profile_switch_completes_before_its_teardown() {
        // spec.md: "A Profile switch fired by the deep stage's own Action
        // completes that firing ... before the resulting Edit::SwitchProfile
        // applies and tears the stage state down" — the triggering firing is
        // never itself interrupted by its own consequence.
        let mut config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::ProfileSwitch {
                    target: "Gaming".to_string(),
                },
            },
        );
        config
            .profiles
            .insert("Gaming".to_string(), Profile::default());
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        // Hand off into the deep stage, whose own Action is a Profile
        // switch — this firing must resolve against the pre-switch Profile
        // and complete (`active_profile` actually flips) rather than being
        // torn down by the `Effect::StopAllStages` it itself schedules.
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;

        let state = harness.get_state().await;
        assert_eq!(state.profile, "Gaming");

        harness.shut_down().await;
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_clearing_the_primary_binding_force_releases_a_live_deep_toggle_immediately()
    {
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            toggle_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        let running_count = harness.sink.batches().len();
        assert!(
            running_count > 0,
            "the deep Toggle loop must already be tapping"
        );

        // Cascade-delete: clearing the primary Binding on this Layer orphans
        // the live `deep_base` entry — `edit::plan` cascades it away and
        // pushes `Effect::StopStage`, force-releasing the deep Toggle
        // immediately, not on the key's next Up (which may never come once
        // the deep Binding backing it is gone).
        harness
            .clear_binding(Input::Grid(1, 1), Layer::Base)
            .await
            .unwrap();
        settle().await;
        let stopped_count = harness.sink.batches().len();

        tokio::time::advance(executor::MIN_TOGGLE_LAP * 5).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            stopped_count,
            "the deep Toggle must be genuinely stopped by the cascade-delete, not paused"
        );

        harness.shut_down().await;
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_clearing_the_deep_stage_force_releases_a_live_deep_toggle_immediately() {
        // Ticket 18: the GUI's "Clear deep stage" button drives
        // `Edit::ClearDeepStage` directly. Removing the deep Binding while it
        // holds a live deep Toggle orphans that Toggle exactly as the
        // primary-removal cascade would — `edit::plan` now pushes
        // `Effect::StopStage`, force-releasing it immediately rather than
        // leaving it stuck until the GUI's next focus-`StopAllToggles`.
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            toggle_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        let running_count = harness.sink.batches().len();
        assert!(
            running_count > 0,
            "the deep Toggle loop must already be tapping"
        );

        harness
            .clear_deep_stage(Input::Grid(1, 1), Layer::Base)
            .await
            .unwrap();
        settle().await;
        let stopped_count = harness.sink.batches().len();

        tokio::time::advance(executor::MIN_TOGGLE_LAP * 5).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            stopped_count,
            "the deep Toggle must be genuinely stopped by the clear, not paused"
        );

        let events: Vec<_> = harness
            .sink
            .batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .filter(|(code, _)| *code == evdev::KeyCode::KEY_B)
            .collect();
        let downs = events.iter().filter(|(_, v)| *v == 1).count();
        let ups = events.iter().filter(|(_, v)| *v == 0).count();
        assert_eq!(
            downs, ups,
            "the deep Toggle key is left logically released: {events:?}"
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn dual_stage_clearing_the_deep_stage_force_releases_a_live_deep_hold_to_repeat() {
        // Ticket 18, single-key path: a live deep Hold-to-repeat holds a bare
        // `value=1` while the deep band is engaged. Without a teardown effect
        // it stays stuck until a Layer or Profile switch — `ClearDeepStage`
        // must balance that `value=1` with a force-released `value=0` now.
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            hold_to_repeat_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        harness.repeat_analog(Input::Grid(1, 1), 250).await;
        settle().await;
        let deep: Vec<_> = harness
            .sink
            .batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .filter(|(code, _)| *code == evdev::KeyCode::KEY_B)
            .collect();
        assert_eq!(
            deep,
            vec![(evdev::KeyCode::KEY_B, 1), (evdev::KeyCode::KEY_B, 2)]
        );

        harness
            .clear_deep_stage(Input::Grid(1, 1), Layer::Base)
            .await
            .unwrap();
        settle().await;
        assert_eq!(
            key_and_value(harness.sink.batches().last().unwrap()[0]),
            (evdev::KeyCode::KEY_B, 0),
            "clearing the deep stage must force-release the held deep value=1"
        );

        let events: Vec<_> = harness
            .sink
            .batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        let downs = events.iter().filter(|(_, v)| *v == 1).count();
        let ups = events.iter().filter(|(_, v)| *v == 0).count();
        assert_eq!(downs, ups, "no key left logically down: {events:?}");

        harness.shut_down().await;
    }

    // ── `post-release-development` ticket 23: cross-Layer clear + mid-press mode flip ──

    #[tokio::test]
    async fn dual_stage_cross_layer_clear_does_not_re_press_the_still_held_deep_stage() {
        // Ticket 23 B12: a grid key carries a deep stage on both Base and
        // Held. Physically held into the deep band on Held (active),
        // `ClearDeepStage` for **Base** must not release-then-re-press the
        // live Held deep key — `stop_stage`'s reset-and-keep lets the next
        // `Engine::update` tick silently re-adopt the unmoved Depth. (One
        // brief `release_deep_slot` drop is the accepted residual; a full
        // `value=0` then `value=1` re-press is the regression.)
        let mut config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            hold_to_repeat_binding(evdev::KeyCode::KEY_C),
        );
        {
            let profile = config.active_profile_mut().expect("seed profile");
            profile
                .held
                .insert(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_A));
            profile.deep_held.insert(
                Input::Grid(1, 1),
                hold_to_repeat_binding(evdev::KeyCode::KEY_B),
            );
        }
        let harness = CommandHarness::spawn(config);

        // Switch to the Held Layer, then hand off into its deep Hold-to-repeat.
        harness.press(Input::ModeKey).await;
        settle().await;
        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;

        let deep_b_downs = |h: &CommandHarness| {
            h.sink
                .batches()
                .iter()
                .map(|b| key_and_value(b[0]))
                .filter(|(c, v)| *c == evdev::KeyCode::KEY_B && *v == 1)
                .count()
        };
        assert_eq!(deep_b_downs(&harness), 1, "the Held deep stage fired once");

        // Clear the *Base* deep Binding while the key is still held deep on
        // Held — the active-Layer (`Held`) `deep_layer` guard stays true.
        harness
            .clear_deep_stage(Input::Grid(1, 1), Layer::Base)
            .await
            .unwrap();
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;

        assert_eq!(
            deep_b_downs(&harness),
            1,
            "a Base clear must not re-press the still-valid Held deep stage"
        );

        harness.release(Input::ModeKey).await;
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn dual_stage_set_staging_mode_mid_press_releases_the_live_deep_stage() {
        // Ticket 23 B7: `SetStagingMode` under a live deep firing pushes
        // `Effect::StopStage` — the deep key is force-released and nothing
        // more is emitted until a fresh physical deep crossing (under the new
        // mode).
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            hold_to_repeat_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        harness.repeat_analog(Input::Grid(1, 1), 250).await;
        settle().await;
        let deep: Vec<_> = harness
            .sink
            .batches()
            .iter()
            .map(|b| key_and_value(b[0]))
            .filter(|(c, _)| *c == evdev::KeyCode::KEY_B)
            .collect();
        assert_eq!(
            deep,
            vec![(evdev::KeyCode::KEY_B, 1), (evdev::KeyCode::KEY_B, 2)]
        );

        harness
            .apply(edit::Edit::SetStagingMode {
                input: Input::Grid(1, 1),
                mode: StagingMode::NoReturn,
            })
            .await
            .unwrap();
        settle().await;
        assert_eq!(
            key_and_value(harness.sink.batches().last().unwrap()[0]),
            (evdev::KeyCode::KEY_B, 0),
            "SetStagingMode must force-release the held deep value=1"
        );
        let after = harness.sink.batches().len();

        // A further repeat pulse at the same held Depth: no phantom re-press
        // (the runtime was reset-and-kept, re-adopting the current band
        // silently).
        harness.repeat_analog(Input::Grid(1, 1), 250).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            after,
            "no output until the deep band is physically re-crossed"
        );

        // Dip out of and back into the deep band — a fresh physical crossing —
        // and the deep stage fires again under the new mode.
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        assert_eq!(
            key_and_value(harness.sink.batches().last().unwrap()[0]),
            (evdev::KeyCode::KEY_B, 1),
            "a fresh deep crossing re-fires the deep stage under the new mode"
        );

        harness.shut_down().await;
    }

    // ── `post-release-development` ticket 24: `stop_stage` + the `feed` path ──

    #[tokio::test]
    async fn dual_stage_set_staging_mode_mid_press_stays_silent_after_a_held_depth_report() {
        // Ticket 24: ticket 23's B7 test drove `repeat_analog` straight after
        // `SetStagingMode` with no interleaved `push_depth`, so `just_reset`
        // was never consumed and `rt.deep` stayed `Up` — `deep_repeat`'s
        // `rt.deep == Down` guard masked the bug. Production's `capture::
        // analog` sends a fresh Depth snapshot *and* `Repeat` pulses per
        // report: the snapshot re-adopts `rt.deep = Down`, and the next pulse
        // would reach `deep_repeat` → `RepeatKey` off the `FiringFinished`
        // slot `release_deep_slot` left behind.
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            hold_to_repeat_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        harness.repeat_analog(Input::Grid(1, 1), 250).await;
        settle().await;
        let deep_b = |h: &CommandHarness| {
            h.sink
                .batches()
                .iter()
                .map(|b| key_and_value(b[0]))
                .filter(|(c, _)| *c == evdev::KeyCode::KEY_B)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            deep_b(&harness),
            vec![(evdev::KeyCode::KEY_B, 1), (evdev::KeyCode::KEY_B, 2)],
            "the deep Hold-to-repeat fired and repeated once before the mode flip"
        );

        harness
            .apply(edit::Edit::SetStagingMode {
                input: Input::Grid(1, 1),
                mode: StagingMode::NoReturn,
            })
            .await
            .unwrap();
        settle().await;
        assert_eq!(
            deep_b(&harness).last(),
            Some(&(evdev::KeyCode::KEY_B, 0)),
            "SetStagingMode force-releases the held deep value=1"
        );
        let quiesced = deep_b(&harness);

        // The full production ordering: a fresh held-Depth snapshot (consumes
        // `just_reset`, re-adopts `rt.deep = Down`) then a run of `Repeat`s.
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        for _ in 0..4 {
            harness.repeat_analog(Input::Grid(1, 1), 250).await;
            settle().await;
        }
        assert_eq!(
            deep_b(&harness),
            quiesced,
            "a released deep Hold-to-repeat must not resume off its leftover slot"
        );

        // A genuine re-crossing re-fires it under the new mode.
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        assert_eq!(
            deep_b(&harness).last(),
            Some(&(evdev::KeyCode::KEY_B, 1)),
            "a fresh deep crossing re-fires the deep stage"
        );

        harness.shut_down().await;
    }

    #[tokio::test]
    async fn dual_stage_set_staging_mode_mid_press_keeps_a_handed_off_primary_silent() {
        // Ticket 24 failure 2: a handed-off Hold-to-repeat *primary*. `update`
        // did `ReleasePrimary` + set `primary_handed_off`; `stop_stage`
        // carries that flag across its reset, so `feed` keeps swallowing the
        // primary's `Repeat`s — with *or without* an interleaved `push_depth`
        // (the `rx_depth` snapshot can be reordered behind the `rx_events`
        // pulse, or a steady hold can stop producing depth reports entirely,
        // which `capture::analog`'s wall-clock `Repeat` synthesis tolerates).
        let config = dual_stage_config(
            StagingMode::Handoff,
            hold_to_repeat_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.repeat_analog(Input::Grid(1, 1), 150).await;
        settle().await;
        let key_a_taps = |h: &CommandHarness| {
            h.sink
                .batches()
                .iter()
                .map(|b| key_and_value(b[0]))
                .filter(|(c, _)| *c == evdev::KeyCode::KEY_A)
                .count()
        };
        assert!(
            key_a_taps(&harness) > 0,
            "the primary taps before the hand-off"
        );

        // Hand off into the deep band.
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;

        harness
            .apply(edit::Edit::SetStagingMode {
                input: Input::Grid(1, 1),
                mode: StagingMode::NoReturn,
            })
            .await
            .unwrap();
        settle().await;

        // No interleaved `push_depth` — the primary `Repeat`s arrive with
        // `Engine::update` never having ticked since the reset.
        let taps_after_reset = key_a_taps(&harness);
        for _ in 0..3 {
            harness.repeat_analog(Input::Grid(1, 1), 250).await;
            settle().await;
        }
        assert_eq!(
            key_a_taps(&harness),
            taps_after_reset,
            "swallowed with no `update` tick between the mode flip and the pulses"
        );

        // Now the held-Depth snapshot lands and more pulses follow — still
        // swallowed (re-confirmed by the re-adoption).
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        for _ in 0..3 {
            harness.repeat_analog(Input::Grid(1, 1), 250).await;
            settle().await;
        }
        assert_eq!(
            key_a_taps(&harness),
            taps_after_reset,
            "the handed-off Hold-to-repeat primary must stay silent under the deep stage after a mode flip"
        );

        harness.release_analog(Input::Grid(1, 1), 0).await;
        harness.push_depth([(Input::Grid(1, 1), 0)]);
        settle().await;
        harness.shut_down().await;
    }

    #[tokio::test]
    async fn dual_stage_cross_layer_clear_leaves_the_held_deep_hold_to_repeat_silent_on_repeat() {
        // Ticket 24 failure 1 at the pipeline level: ticket 23's B12 test
        // stopped at a `push_depth` after the cross-Layer clear and never
        // sent a `Repeat`. With the deep band re-adopted `Down` and the deep
        // slot left `FiringFinished` by `release_deep_slot`, the next
        // synthesized primary `Repeat` would drive `deep_repeat` →
        // `RepeatKey` — phantom `value=2` for the whole remaining hold.
        let mut config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            hold_to_repeat_binding(evdev::KeyCode::KEY_C),
        );
        {
            let profile = config.active_profile_mut().expect("seed profile");
            profile
                .held
                .insert(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_A));
            profile.deep_held.insert(
                Input::Grid(1, 1),
                hold_to_repeat_binding(evdev::KeyCode::KEY_B),
            );
        }
        let harness = CommandHarness::spawn(config);

        harness.press(Input::ModeKey).await;
        settle().await;
        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        harness.repeat_analog(Input::Grid(1, 1), 250).await;
        settle().await;

        let deep_b = |h: &CommandHarness| {
            h.sink
                .batches()
                .iter()
                .map(|b| key_and_value(b[0]))
                .filter(|(c, _)| *c == evdev::KeyCode::KEY_B)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            deep_b(&harness),
            vec![(evdev::KeyCode::KEY_B, 1), (evdev::KeyCode::KEY_B, 2)],
            "the Held deep Hold-to-repeat fired and repeated once before the clear"
        );

        // Clear the *Base* deep Binding while the key is held deep on Held —
        // the active-Layer (`Held`) `deep_layer` guard stays true.
        harness
            .clear_deep_stage(Input::Grid(1, 1), Layer::Base)
            .await
            .unwrap();
        settle().await;
        assert_eq!(
            deep_b(&harness).last(),
            Some(&(evdev::KeyCode::KEY_B, 0)),
            "the accepted one-frame `release_deep_slot` residual"
        );
        let quiesced = deep_b(&harness);

        // A further held-Depth report re-adopts `rt.deep = Down`; the pulses
        // that follow must not resurrect the released deep stage.
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        for _ in 0..4 {
            harness.repeat_analog(Input::Grid(1, 1), 250).await;
            settle().await;
        }
        assert_eq!(
            deep_b(&harness),
            quiesced,
            "no phantom deep output after a cross-Layer clear of the other Layer"
        );

        harness.release(Input::ModeKey).await;
        harness.shut_down().await;
    }

    // Overwriting (not removing) a primary Binding no longer tears its deep
    // stage down — covered as a `plan` unit test
    // (`edit::tests::set_binding_overwriting_a_primary_keeps_its_live_deep_binding`),
    // not here: a dispatch-harness test would have to run the deep Toggle
    // live *through* `shut_down`, and the paused-time harness starves the
    // dispatch loop's channel-close check against a perpetually-ready Toggle
    // tick.

    #[tokio::test(start_paused = true)]
    async fn dual_stage_stop_all_toggles_command_drains_a_live_deep_toggle_too() {
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            toggle_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        let running_count = harness.sink.batches().len();
        assert!(
            running_count > 0,
            "the deep Toggle loop must already be tapping"
        );

        // The GUI-focus escape hatch — deliberately more aggressive than
        // the Chord-toggle-survives-a-Profile-switch precedent — also drains
        // `stage::Engine`'s own `Slots<StageKey>` toggles now, alongside the
        // individual path's.
        harness.stop_all_toggles().await;
        settle().await;
        let stopped_count = harness.sink.batches().len();

        tokio::time::advance(executor::MIN_TOGGLE_LAP * 5).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            stopped_count,
            "Command::StopAllToggles must also drain the deep stage's own Slots<StageKey> toggles"
        );

        harness.shut_down().await;
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_disconnect_force_releases_a_live_deep_toggle_and_resets_runtime_state() {
        let config = dual_stage_config(
            StagingMode::Handoff,
            keypress_binding(evdev::KeyCode::KEY_A),
            toggle_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;
        harness.push_depth([(Input::Grid(1, 1), 250)]);
        settle().await;
        tokio::time::advance(executor::MIN_TOGGLE_LAP * 3).await;
        settle().await;
        let running_count = harness.sink.batches().len();
        assert!(
            running_count > 0,
            "the deep Toggle loop must already be tapping"
        );

        // The device drops out mid-press: ticket 06's new, explicit
        // disconnect hook force-releases every live deep slot and resets
        // `stage::Engine`'s per-key runtime state — independent of capture's
        // own synthetic-Up trick for the primary band, which only ever
        // covers the primary.
        harness.set_device_connected(false).await;
        settle().await;
        let stopped_count = harness.sink.batches().len();

        tokio::time::advance(executor::MIN_TOGGLE_LAP * 5).await;
        settle().await;
        assert_eq!(
            harness.sink.batches().len(),
            stopped_count,
            "the deep Toggle must be genuinely stopped by the disconnect, not paused"
        );

        harness.shut_down().await;
    }

    #[tokio::test(start_paused = true)]
    async fn dual_stage_quick_skip_disconnect_while_armed_cancels_the_buffered_primary() {
        // The disconnect hook's own share of the Layer-switch/capture-mode-
        // flip/Profile-switch Quick-Skip-cancellation tests above.
        let config = dual_stage_config(
            StagingMode::QuickSkip,
            keypress_binding(evdev::KeyCode::KEY_A),
            keypress_binding(evdev::KeyCode::KEY_B),
        );
        let harness = CommandHarness::spawn(config);

        harness.press_analog(Input::Grid(1, 1), 150).await;
        harness.push_depth([(Input::Grid(1, 1), 150)]);
        settle().await;

        harness.set_device_connected(false).await;
        settle().await;

        // Advancing well past the original window fires nothing — the
        // buffered primary was dropped by `stop_all()`, not merely deferred.
        tokio::time::advance(Duration::from_millis(100)).await;
        settle().await;

        let batches = harness.shut_down().await;
        assert!(
            batches.is_empty(),
            "a disconnect while Armed must cancel the buffered primary outright"
        );
    }
}
