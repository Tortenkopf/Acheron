//! The dispatch task: single consumer of both the capture channel and the
//! D-Bus command channel (issue 07's "D-Bus interleaving" — GUI-originated
//! calls push a `Command` alongside `PhysicalEvent`s, so one task remains
//! the sole owner of `Config`, no lock or second copy of state). Resolves
//! each `PhysicalEvent`'s `Input` against the active Profile's Base Layer
//! (ticket 14) and, per ticket 17, branches on `TriggerMode` — Fire-once
//! fires once on `Down`, Hold-to-repeat fires on `Down` and every `Repeat`,
//! Toggle starts/stops only on `Down`. Applies `Command`s (ticket 15) by
//! mutating `Config` in place and rewriting `config.toml` immediately,
//! atomically per call. Held Layer remains future work (ticket 18).

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::capture::{EventState, PhysicalEvent};
use crate::command::{Command, CommandError, State};
use crate::config::{self, Binding, Config, Profile, TriggerMode};
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
    let mut commands_open = true;
    loop {
        tokio::select! {
            event = rx_events.recv() => {
                let Some(event) = event else { break };
                handle_event(&injector, &config, &mut toggles, &mut in_flight, event).await?;
            }
            cmd = rx_commands.recv(), if commands_open => {
                match cmd {
                    Some(cmd) => handle_command(&mut config, &config_path, &toggles, cmd).await,
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
    event: PhysicalEvent,
) -> io::Result<()> {
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

    let bindings = &config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile")
        .base;
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
    toggles: &HashMap<Input, ActiveToggle>,
    cmd: Command,
) {
    match cmd {
        Command::GetConfig(reply) => {
            let _ = reply.send(config.clone());
        }
        Command::GetState(reply) => {
            let _ = reply.send(State {
                profile: config.active_profile.clone(),
                layer: "base",
                active_toggles: toggles.keys().copied().collect(),
                device_connected: true,
            });
        }
        Command::SetBinding {
            input,
            binding,
            reply,
        } => {
            let previous = active_profile_mut(config).base.insert(input, binding);
            let result = persist(config, config_path).await;
            if result.is_err() {
                // config.toml on disk must always match in-memory state
                // (spec.md's config lifecycle) — roll the in-memory edit
                // back rather than let GetConfig lie about what's saved.
                let profile = active_profile_mut(config);
                match previous {
                    Some(prev) => {
                        profile.base.insert(input, prev);
                    }
                    None => {
                        profile.base.remove(&input);
                    }
                }
            }
            let _ = reply.send(result);
        }
        Command::ClearBinding { input, reply } => {
            let Some(previous) = active_profile_mut(config).base.remove(&input) else {
                let _ = reply.send(Err(CommandError::NotFound));
                return;
            };
            let result = persist(config, config_path).await;
            if result.is_err() {
                active_profile_mut(config).base.insert(input, previous);
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
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), Profile { base: bindings });
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
        ));

        FakeCaptureSource::new(scripted).run(tx).await.unwrap();

        drop(inj);
        dispatch_handle.await.unwrap().unwrap();
        inj_handle.await.unwrap().unwrap();

        sink.batches()
    }

    #[tokio::test]
    async fn passthrough_reinjects_every_captured_event_unchanged_when_unbound() {
        let scripted = vec![
            PhysicalEvent {
                input: Input::ModeKey,
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
            let dispatch_handle =
                tokio::spawn(run(event_rx, cmd_rx, inj, config, config_path.clone()));

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

        async fn set_binding(&self, input: Input, binding: Binding) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::SetBinding {
                    input,
                    binding,
                    reply,
                })
                .await
                .unwrap();
            rx.await.unwrap()
        }

        async fn clear_binding(&self, input: Input) -> Result<(), CommandError> {
            let (reply, rx) = oneshot::channel();
            self.cmd_tx
                .send(Command::ClearBinding { input, reply })
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
            .set_binding(Input::Grid(1, 1), keypress_binding(evdev::KeyCode::KEY_F1))
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
            .clear_binding(Input::Grid(1, 1))
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
            .clear_binding(Input::Grid(1, 1))
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

        // Layers don't exist yet (ticket 18) and device detection is ticket
        // 20's scope — those two fields stay fixed/stubbed. `active_toggles`
        // is real as of ticket 17 (see the Toggle tests above); with no
        // Toggle running it's correctly empty here.
        assert_eq!(state.profile, DEFAULT_PROFILE_NAME);
        assert_eq!(state.layer, "base");
        assert!(state.active_toggles.is_empty());
        assert!(state.device_connected);
    }
}
