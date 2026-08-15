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

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use zbus::object_server::SignalEmitter;

use crate::capture::{EventState, PhysicalEvent};
use crate::command::{Command, CommandError, State};
use crate::config::{self, Binding, Config, Layer, ModeKeyRole, Profile, TriggerMode};
use crate::dbus::Daemon;
use crate::executor::{self, ActiveToggle};
use crate::injector::Injector;
use crate::input::Input;

/// Returns an error once the injector channel closes, or the capture
/// channel closes (meaning the capture task has died) — per issue 07, a
/// genuine, fatal capture-pipeline error rather than something to swallow
/// silently. The command channel closing is not fatal: it only means the
/// D-Bus server side has gone away, and this task's other job (capture ->
/// injector passthrough/remapping) still has work to do.
pub async fn run(
    mut rx_events: mpsc::Receiver<PhysicalEvent>,
    mut rx_commands: mpsc::Receiver<Command>,
    injector: Injector,
    mut config: Config,
    config_path: PathBuf,
    signal_emitter: Option<SignalEmitter<'static>>,
) -> io::Result<()> {
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
    let mut in_flight: HashMap<Input, JoinHandle<()>> = HashMap::new();
    // The one piece of momentary Layer runtime state (ticket 18) — not part
    // of `Config`, reset to `Base` whenever this task starts.
    let mut active_layer = Layer::Base;
    let mut commands_open = true;
    loop {
        tokio::select! {
            event = rx_events.recv() => {
                let Some(event) = event else { break };
                handle_event(
                    &injector,
                    &config,
                    &mut toggles,
                    &mut in_flight,
                    &mut active_layer,
                    &signal_emitter,
                    event,
                )
                .await?;
            }
            cmd = rx_commands.recv(), if commands_open => {
                match cmd {
                    Some(cmd) => handle_command(&mut config, &config_path, &mut toggles, &active_layer, &signal_emitter, cmd).await,
                    None => commands_open = false,
                }
            }
        }
    }
    Ok(())
}

async fn handle_event(
    injector: &Injector,
    config: &Config,
    toggles: &mut HashMap<Input, ActiveToggle>,
    in_flight: &mut HashMap<Input, JoinHandle<()>>,
    active_layer: &mut Layer,
    signal_emitter: &Option<SignalEmitter<'static>>,
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

    let bindings = profile.layer(*active_layer);
    match bindings.get(&event.input) {
        Some(binding) => {
            fire(
                injector,
                toggles,
                in_flight,
                event.input,
                binding,
                event.state,
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

/// Branches on `TriggerMode` x event state, per ticket 17: Fire-once fires
/// only on `Down`; Hold-to-repeat fires on `Down` and every subsequent
/// `Repeat` (the device's own evdev autorepeat, no separate repeat-interval
/// config); Toggle starts its own looping task on `Down` (stopping is
/// handled earlier, in `handle_event`, before a Binding is even looked up).
/// Every other combination (a bound Input's `Up`, or `Repeat` for
/// Fire-once/Toggle) is ignored outright — no passthrough of the original
/// key for a bound Input, matching the pre-ticket-17 Fire-once behavior.
async fn fire(
    injector: &Injector,
    toggles: &mut HashMap<Input, ActiveToggle>,
    in_flight: &mut HashMap<Input, JoinHandle<()>>,
    input: Input,
    binding: &Binding,
    state: EventState,
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
            // while the key stays held.
            if let Some(handle) = in_flight.get(&input)
                && !handle.is_finished()
            {
                return Ok(());
            }
            let steps = executor::compile(&binding.action);
            let handle = executor::spawn_fire_once(injector.clone(), steps);
            in_flight.insert(input, handle);
            Ok(())
        }
        (TriggerMode::Toggle, EventState::Down) => {
            let steps = executor::compile(&binding.action);
            toggles.insert(input, ActiveToggle::spawn(injector.clone(), steps));
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The `Default` Profile always exists — `load_or_seed` (issue 11) refuses
/// to start a `Config` whose `active_profile` doesn't name a real Profile.
fn active_profile_mut(config: &mut Config) -> &mut Profile {
    config
        .active_profile_mut()
        .expect("load_or_seed validates active_profile names a real profile")
}

async fn handle_command(
    config: &mut Config,
    config_path: &Path,
    toggles: &mut HashMap<Input, ActiveToggle>,
    active_layer: &Layer,
    signal_emitter: &Option<SignalEmitter<'static>>,
    cmd: Command,
) {
    match cmd {
        Command::GetConfig(reply) => {
            let _ = reply.send(config.clone());
        }
        Command::GetState(reply) => {
            let _ = reply.send(State {
                profile: config.active_profile.clone(),
                layer: active_layer.as_str(),
                active_toggles: toggles.keys().copied().collect(),
                device_connected: true,
            });
        }
        Command::SetBinding {
            input,
            layer,
            binding,
            reply,
        } => {
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
            let profile = config
                .profiles
                .remove(&old_name)
                .expect("just checked old_name exists");
            config.profiles.insert(new_name.clone(), profile.clone());
            let was_active = config.active_profile == old_name;
            if was_active {
                config.active_profile = new_name.clone();
            }
            let result = persist(config, config_path).await;
            if result.is_err() {
                config.profiles.remove(&new_name);
                config.profiles.insert(old_name.clone(), profile);
                if was_active {
                    config.active_profile = old_name;
                }
            }
            let _ = reply.send(result);
        }
        Command::SwitchProfile { name, reply } => {
            if !config.profiles.contains_key(&name) {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            }
            let previous = std::mem::replace(&mut config.active_profile, name.clone());
            let result = persist(config, config_path).await;
            if result.is_err() {
                config.active_profile = previous;
            } else {
                // Force-stop every active Toggle before the new Profile's
                // state is considered live, per spec.md's "Toggle behavior
                // across Layer/Profile switches" — no Toggle survives a
                // Profile switch, regardless of which Profile started it.
                for (_, toggle) in toggles.drain() {
                    toggle.stop().await;
                }
                if let Some(emitter) = signal_emitter {
                    let _ = Daemon::active_profile_changed(emitter, &name).await;
                }
            }
            let _ = reply.send(result);
        }
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
    use crate::config::{Action, DEFAULT_PROFILE_NAME, MacroStepDto, Modifiers};
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
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), profile);
        Config {
            schema_version: config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
        }
    }

    /// A `config_path` no test in this module ever writes to (persistence
    /// via `Command`s is covered separately, with a real `tempfile` path).
    fn unused_config_path() -> PathBuf {
        PathBuf::from("/nonexistent/acheron-dispatch-test/config.toml")
    }

    async fn run_scripted(
        scripted: Vec<PhysicalEvent>,
        bindings: HashMap<Input, Binding>,
    ) -> Vec<Vec<evdev::InputEvent>> {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone());

        let (tx, rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings(bindings),
            unused_config_path(),
            None,
        ));

        FakeCaptureSource::new(scripted).run(tx).await.unwrap();

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
            },
            PhysicalEvent {
                input: Input::Grid(2, 3),
                state: EventState::Repeat,
            },
            PhysicalEvent {
                input: Input::Thumbstick(Direction::Up),
                state: EventState::Up,
            },
            PhysicalEvent {
                input: Input::Wheel(WheelEvent::ScrollDown),
                state: EventState::Down,
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
            },
            PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Repeat,
            },
            PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Up,
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
        let (inj, inj_handle) = injector::spawn(sink.clone());
        let (tx, rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings(bindings),
            unused_config_path(),
            None,
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

    #[tokio::test(start_paused = true)]
    async fn overlapping_same_input_firings_are_dropped_not_queued() {
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::HoldToRepeat,
                action: Action::Macro {
                    steps: vec![
                        MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                        MacroStepDto::Delay(20),
                        MacroStepDto::KeyUp(evdev::KeyCode::KEY_A),
                    ],
                },
            },
        );

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone());
        let (tx, rx) = mpsc::channel(8);
        let (_cmd_tx, cmd_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(
            rx,
            cmd_rx,
            inj.clone(),
            config_with_bindings(bindings),
            unused_config_path(),
            None,
        ));

        // Down starts a firing that immediately sends KeyDown, then sleeps
        // 20ms. A Repeat that lands before that firing finishes must be
        // dropped, not spawn a second overlapping firing.
        tx.send(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Down,
        })
        .await
        .unwrap();
        tokio::task::yield_now().await;
        tx.send(PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Repeat,
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
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::Macro {
                    steps: vec![
                        MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                        MacroStepDto::Delay(20),
                        MacroStepDto::KeyUp(evdev::KeyCode::KEY_A),
                    ],
                },
            },
        );
        let harness = CommandHarness::spawn(config_with_bindings(bindings));

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
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action: Action::Macro {
                    // Deliberately unbalanced within the window we stop in:
                    // KeyDown fires, then a long Delay, so KEY_A is still
                    // held when the second press stops the Toggle.
                    steps: vec![
                        MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                        MacroStepDto::Delay(50),
                    ],
                },
            },
        );
        let harness = CommandHarness::spawn(config_with_bindings(bindings));

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
            let (inj, inj_handle) = injector::spawn(sink.clone());
            let (event_tx, event_rx) = mpsc::channel(8);
            let (cmd_tx, cmd_rx) = mpsc::channel(8);
            let dispatch_handle = tokio::spawn(run(
                event_rx,
                cmd_rx,
                inj,
                config,
                config_path.clone(),
                None,
            ));

            CommandHarness {
                _dir: dir,
                config_path,
                cmd_tx,
                event_tx,
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

        async fn press(&self, input: Input) {
            self.event_tx
                .send(PhysicalEvent {
                    input,
                    state: EventState::Down,
                })
                .await
                .unwrap();
        }

        async fn shut_down(self) -> Vec<Vec<evdev::InputEvent>> {
            drop(self.cmd_tx);
            drop(self.event_tx);
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
    async fn get_state_command_returns_this_tickets_fixed_stub_values() {
        let harness = CommandHarness::spawn(config_with_bindings(HashMap::new()));

        let state = harness.get_state().await;
        harness.shut_down().await;

        // Device detection is ticket 20's scope, so that field stays
        // fixed/stubbed. `active_toggles` is real as of ticket 17 (see the
        // Toggle tests above); with no Toggle running it's correctly empty
        // here. `layer` is real as of ticket 18 — with no ModeKey press this
        // task's dispatch loop starts and stays at Base.
        assert_eq!(state.profile, DEFAULT_PROFILE_NAME);
        assert_eq!(state.layer, "base");
        assert!(state.active_toggles.is_empty());
        assert!(state.device_connected);
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
            })
            .await
            .unwrap();
        harness
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Up,
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
            })
            .await
            .unwrap();
        harness
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Up,
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
    async fn leaving_bound_role_stops_an_active_toggle_on_the_mode_key() {
        // A code-review-caught edge case: a Toggle can only ever have been
        // started on the Mode key while `Bound`. Once `LayerSwitch` takes
        // over, every `Input::ModeKey` press is intercepted for Layer
        // switching before it ever reaches the stop-toggle check — so
        // without an explicit stop here, this Toggle would run forever with
        // no physical key able to stop it.
        let mut base = HashMap::new();
        base.insert(
            Input::ModeKey,
            Binding {
                trigger: TriggerMode::Toggle,
                action: Action::Macro {
                    steps: vec![
                        MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                        MacroStepDto::Delay(50),
                    ],
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
        let mut base = HashMap::new();
        base.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::Toggle,
                action: Action::Macro {
                    // Deliberately unbalanced within the window we switch
                    // in: KeyDown fires, then a long Delay, so KEY_A is
                    // still held when the Profile switch stops it.
                    steps: vec![
                        MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                        MacroStepDto::Delay(50),
                    ],
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
}
