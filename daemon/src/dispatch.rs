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

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use zbus::object_server::SignalEmitter;

use crate::capture::{CaptureMode, EventState, PhysicalEvent};
use crate::command::{Command, CommandError, State};
use crate::config::{
    self, Action, ActuationPoint, Binding, ChordKey, Config, Layer, MacroDef, MacroId, ModeKeyRole,
    Profile, StepDirection, StepperDef, StepperId, StepperItem, TriggerMode,
};
use crate::dbus::Daemon;
use crate::executor::{self, ActiveToggle, FiringHandle};
use crate::injector::Injector;
use crate::input::Input;

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
    let mut commands_open = true;
    let mut connection_open = true;
    let mut capture_mode_open = true;
    // Owns the ~50ms Chord simultaneity window plus every currently-active
    // Chord's Trigger-mode state (ticket 01/40) — reset fresh on every
    // dispatch task start, same as `toggles`/`active_layer`.
    let mut chord_state = ChordState::default();
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
                    event,
                )
                .await?;
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
                    Some(mode) => handle_capture_mode_change(&mut capture_mode, &signal_emitter, mode).await,
                    None => capture_mode_open = false,
                }
            }
            cmd = rx_commands.recv(), if commands_open => {
                match cmd {
                    Some(cmd) => handle_command(&mut config, &config_path, &mut toggles, &mut stepper_cursors, &active_layer, device_connected, capture_mode, &signal_emitter, &actuation_tx, &capture_control_tx, cmd).await,
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
    event: PhysicalEvent,
) -> io::Result<()> {
    let profile = config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile");

    if event.input == Input::ModeKey && profile.mode_key_role == ModeKeyRole::LayerSwitch {
        handle_layer_switch(active_layer, signal_emitter, event.state).await;
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
                    let succeeded =
                        switch_profile(config, config_path, toggles, actuation_tx, target.clone())
                            .await
                            .is_ok();
                    if succeeded && let Some(emitter) = signal_emitter {
                        let _ = Daemon::active_profile_changed(emitter, &target).await;
                    }
                }
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
async fn handle_layer_switch(
    active_layer: &mut Layer,
    signal_emitter: &Option<SignalEmitter<'static>>,
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
/// connection).
async fn handle_capture_mode_change(
    capture_mode: &mut CaptureMode,
    signal_emitter: &Option<SignalEmitter<'static>>,
    mode: CaptureMode,
) {
    if mode == *capture_mode {
        return;
    }
    *capture_mode = mode;
    if let Some(emitter) = signal_emitter {
        let _ = Daemon::capture_mode_changed(emitter, mode.as_str()).await;
    }
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
) -> io::Result<()> {
    match (binding.trigger, state) {
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
        (TriggerMode::Toggle, EventState::Down) => {
            let steps = compile_action(&binding.action, macros, steppers, stepper_cursors);
            chord_toggles.insert(key, ActiveToggle::spawn(injector.clone(), steps));
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
    signal_emitter: &Option<SignalEmitter<'static>>,
    layer: Layer,
    input: Input,
) -> io::Result<()> {
    let profile = config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile");
    let binding = profile.layer(layer).get(&input).cloned();
    match binding {
        Some(binding) => {
            if let Action::ProfileSwitch { target } = binding.action {
                if switch_profile(config, config_path, toggles, actuation_tx, target.clone())
                    .await
                    .is_ok()
                    && let Some(emitter) = signal_emitter
                {
                    let _ = Daemon::active_profile_changed(emitter, &target).await;
                }
                return Ok(());
            }
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
                    signal_emitter,
                    active_layer,
                    event.input,
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
            signal_emitter,
            active_layer,
            input,
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
/// newly-selected item through `executor::keypress_steps` — "one motion
/// moves the cursor and fires," ticket 03's Answer's firing semantics, now
/// carrying the item's own modifier combination if it has one (ticket 62's
/// Answer: a Stepper item's fire sequence reuses `Action::Keypress`'s
/// mods-down/key/mods-up compile path, atomically). A missing cursor entry
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
    let StepperItem::Key { key, modifiers } = def.items[next];
    executor::keypress_steps(modifiers, key)
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
) -> io::Result<()> {
    match (binding.trigger, state) {
        (TriggerMode::FireOnce, EventState::Down)
        | (TriggerMode::HoldToRepeat, EventState::Down | EventState::Repeat) => {
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
        (TriggerMode::Toggle, EventState::Down) => {
            let steps = compile_action(&binding.action, macros, steppers, stepper_cursors);
            toggles.insert(input, ActiveToggle::spawn(injector.clone(), steps));
            Ok(())
        }
        (TriggerMode::FireOnce | TriggerMode::HoldToRepeat, EventState::Up) => {
            if let Some(firing) = in_flight.get(&input) {
                firing.force_release_stuck(injector).await;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Shared by `SetActuationPoint`/`ClearActuationPoint` (ticket 17 §3): an
/// actuation point is a property of a physical Grid key, so setting or
/// clearing one on any other `Input` variant is rejected.
fn reject_non_grid_input(input: Input) -> Result<(), CommandError> {
    if matches!(input, Input::Grid(_, _)) {
        Ok(())
    } else {
        Err(CommandError::InvalidRequest(
            "actuation points can only be set on Grid Inputs".to_string(),
        ))
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
    if let Action::ControllerButton { button } = binding.action
        && !crate::input::is_gamepad_button(button)
    {
        return Err(CommandError::InvalidRequest(format!(
            "{button:?} is not a valid gamepad button"
        )));
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
async fn switch_profile(
    config: &mut Config,
    config_path: &Path,
    toggles: &mut HashMap<Input, ActiveToggle>,
    actuation_tx: &watch::Sender<HashMap<Input, ActuationPoint>>,
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
    config: &mut Config,
    config_path: &Path,
    toggles: &mut HashMap<Input, ActiveToggle>,
    stepper_cursors: &mut HashMap<StepperId, usize>,
    active_layer: &Layer,
    device_connected: bool,
    capture_mode: CaptureMode,
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
            let result =
                switch_profile(config, config_path, toggles, actuation_tx, name.clone()).await;
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
            if let Err(err) = reject_non_grid_input(input) {
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
            if let Err(err) = reject_non_grid_input(input) {
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
            if let Err(err) = validate_binding(&binding, config) {
                let _ = reply.send(Err(err));
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

    /// A fresh capture-control `Sender` for tests that don't exercise
    /// `SetForceDigital`'s live supervisor swap (ticket 23) — sends into it
    /// just fail silently once the paired `Receiver` (dropped here) is gone,
    /// matching `dispatch::run`'s own `let _ = capture_control_tx.send(...)`.
    fn capture_control_channel() -> mpsc::Sender<bool> {
        mpsc::channel(8).0
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
        sink: RecordingSink,
        dispatch_handle: tokio::task::JoinHandle<io::Result<()>>,
        inj_handle: tokio::task::JoinHandle<io::Result<()>>,
    }

    impl CommandHarness {
        fn spawn(config: Config) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let config_path = dir.path().join("config.toml");
            config::write(&config_path, &config).unwrap();

            let sink = RecordingSink::new();
            let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
            let (event_tx, event_rx) = mpsc::channel(8);
            let (conn_tx, conn_rx) = mpsc::channel(8);
            let (cmd_tx, cmd_rx) = mpsc::channel(8);
            let (actuation_tx, actuation_rx) = watch::channel(HashMap::new());
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
            ));

            CommandHarness {
                _dir: dir,
                config_path,
                cmd_tx,
                event_tx,
                actuation_rx,
                conn_tx,
                sink,
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

        /// Stands in for the `CaptureSource`'s poll loop reporting a
        /// device-connection transition (ticket 20) — there's no real
        /// evdev poll loop in these tests, so this is the seam that drives
        /// `device_connected`/`DeviceConnectionChanged`.
        async fn set_device_connected(&self, connected: bool) {
            self.conn_tx.send(connected).await.unwrap();
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
                    trigger: TriggerMode::FireOnce,
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
}
