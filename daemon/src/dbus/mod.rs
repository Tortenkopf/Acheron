//! The D-Bus surface (ticket 15 / issue 08): one flat object,
//! `/com/acheron/Daemon`, on bus name `com.acheron.Daemon`, one combined
//! interface (also `com.acheron.Daemon`) — no `ObjectManager` hierarchy.
//! `Daemon` itself holds only a `Command` sender: every read/mutation is
//! forwarded to the dispatch task (the sole owner of `Config`) and awaited
//! over a `oneshot` reply, so this type never touches `Config` directly.

pub mod wire;

use std::collections::HashMap;

use tokio::sync::{mpsc, oneshot};
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

use crate::command::{Command, CommandError};
use crate::input::Input;

/// `com.acheron.Daemon.Error.*` — a small named set (issue 08's grilling
/// answer), not one generic error or one per validation rule, so a client
/// can respond differently without string-matching a message body.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "com.acheron.Daemon.Error")]
pub enum DaemonError {
    NotFound(String),
    AlreadyExists(String),
    InvalidBinding(String),
    IoError(String),
}

impl From<CommandError> for DaemonError {
    fn from(err: CommandError) -> Self {
        match err {
            CommandError::NotFound => {
                DaemonError::NotFound("the requested entity was not found".to_string())
            }
            CommandError::AlreadyExists => {
                DaemonError::AlreadyExists("that name is already taken".to_string())
            }
            CommandError::InvalidRequest(message) => DaemonError::InvalidBinding(message),
            CommandError::IoError(message) => DaemonError::IoError(message),
        }
    }
}

/// The dispatch task has stopped responding to commands — only possible if
/// it panicked or exited, both genuine Daemon-internal failures.
fn dispatch_gone<T>(_: T) -> DaemonError {
    DaemonError::IoError("the dispatch task is not responding".to_string())
}

type DaemonResult<T> = Result<T, DaemonError>;

pub struct Daemon {
    commands: mpsc::Sender<Command>,
}

impl Daemon {
    pub fn new(commands: mpsc::Sender<Command>) -> Self {
        Daemon { commands }
    }

    fn parse_input(input: &str) -> DaemonResult<Input> {
        input
            .parse()
            .map_err(|_| DaemonError::InvalidBinding(format!("{input:?} is not a valid Input")))
    }
}

#[interface(name = "com.acheron.Daemon")]
impl Daemon {
    /// The entire config document — every Profile's Base- and Held-layer
    /// Bindings plus its `mode_key_role` (issue 08, ticket 18).
    async fn get_config(&self) -> Result<HashMap<String, OwnedValue>, DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::GetConfig(reply))
            .await
            .map_err(dispatch_gone)?;
        let config = rx.await.map_err(dispatch_gone)?;
        Ok(wire::config_to_dict(&config))
    }

    /// The live runtime snapshot. `active_toggles` reflects the dispatch
    /// task's real `HashMap<Input, ActiveToggle>` as of ticket 17. `layer`
    /// reflects the dispatch task's real active Layer as of ticket 18.
    /// `device_connected` is hardcoded `true` (real detection is ticket 20's
    /// scope).
    async fn get_state(&self) -> Result<(String, String, Vec<String>, bool), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::GetState(reply))
            .await
            .map_err(dispatch_gone)?;
        let state = rx.await.map_err(dispatch_gone)?;
        Ok((
            state.profile,
            state.layer.to_string(),
            state
                .active_toggles
                .iter()
                .map(ToString::to_string)
                .collect(),
            state.device_connected,
        ))
    }

    /// Atomic: validates, applies in-memory, and rewrites `config.toml`
    /// immediately — no draft/save step (issue 08). `layer` (`"base"`/
    /// `"held"`) lets the GUI edit each Layer's Bindings independently
    /// (ticket 18) — always the active Profile's; a Profile argument is
    /// ticket 19's job.
    async fn set_binding(
        &self,
        input: String,
        layer: String,
        binding: HashMap<String, OwnedValue>,
    ) -> Result<(), DaemonError> {
        let input = Self::parse_input(&input)?;
        let layer = wire::layer_from_str(&layer).map_err(DaemonError::InvalidBinding)?;
        let binding = wire::binding_from_dict(&binding).map_err(DaemonError::InvalidBinding)?;

        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SetBinding {
                input,
                layer,
                binding,
                reply,
            })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Atomic: removes the Binding (passthrough resumes) and rewrites
    /// `config.toml` immediately. Errors `NotFound` if `input` has no
    /// Binding to clear on `layer`.
    async fn clear_binding(&self, input: String, layer: String) -> Result<(), DaemonError> {
        let input = Self::parse_input(&input)?;
        let layer = wire::layer_from_str(&layer).map_err(DaemonError::InvalidBinding)?;

        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::ClearBinding {
                input,
                layer,
                reply,
            })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Flips the active Profile's `mode_key_role` (ticket 18) —
    /// `"layer_switch"`/`"bound"`. Held-layer Bindings are retained either
    /// way; only which role governs the Mode key's dispatch changes.
    async fn set_mode_key_role(&self, role: String) -> Result<(), DaemonError> {
        let role = wire::mode_key_role_from_str(&role).map_err(DaemonError::InvalidBinding)?;

        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SetModeKeyRole { role, reply })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Creates a new, empty Profile (ticket 19) — atomic/immediately-
    /// applied/immediately-persisted, per the conventions `SetBinding`/
    /// `SetModeKeyRole` already established. Errors `AlreadyExists` if
    /// `name` is already taken.
    async fn create_profile(&self, name: String) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::CreateProfile { name, reply })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Deletes a Profile by name. Errors `NotFound` if it doesn't exist, or
    /// `InvalidBinding` if it's the currently active Profile — switch away
    /// from it first via `SwitchProfile`.
    async fn delete_profile(&self, name: String) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::DeleteProfile { name, reply })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Renames a Profile, updating `active_profile` too if it's the active
    /// one. Errors `NotFound` if `old_name` doesn't exist, or
    /// `AlreadyExists` if `new_name` is already taken.
    async fn rename_profile(&self, old_name: String, new_name: String) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::RenameProfile {
                old_name,
                new_name,
                reply,
            })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Switches the active Profile, force-stopping every currently running
    /// Toggle as part of the switch before the new Profile's state becomes
    /// active (ticket 19). Errors `NotFound` if `name` doesn't name a real
    /// Profile.
    async fn switch_profile(&self, name: String) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SwitchProfile { name, reply })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Fires on active-Profile changes — every `SwitchProfile` call, as of
    /// ticket 19.
    #[zbus(signal)]
    pub async fn active_profile_changed(
        signal_emitter: &SignalEmitter<'_>,
        name: &str,
    ) -> zbus::Result<()>;

    /// Fires on Layer transitions (Mode key pressed/released while the
    /// active Profile's `mode_key_role` is `LayerSwitch`) — `"base"` /
    /// `"held"` (ticket 18). The dispatch task calls this directly (it
    /// holds the `SignalEmitter` `main.rs` builds alongside the D-Bus
    /// connection), not through a `Command` — nothing needs a reply.
    #[zbus(signal)]
    pub async fn active_layer_changed(
        signal_emitter: &SignalEmitter<'_>,
        layer: &str,
    ) -> zbus::Result<()>;

    /// Fires with the full current snapshot (not a delta — D-Bus signals
    /// aren't guaranteed-delivery) on every Toggle start/stop. Toggles are
    /// real as of ticket 17 (`GetState`'s `active_toggles` reflects them
    /// live), but nothing calls this signal yet — the dispatch task, the
    /// sole owner of the `ActiveToggle` map, has no `SignalEmitter` handle
    /// to push through, and the GUI doesn't poll/consume Toggle state yet
    /// either. Wiring live push notification here is left for whichever
    /// ticket first needs it.
    #[zbus(signal)]
    pub async fn active_toggles_changed(
        signal_emitter: &SignalEmitter<'_>,
        active_inputs: Vec<String>,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{EventState, PhysicalEvent};
    use crate::config::{Config, DEFAULT_PROFILE_NAME, Profile};
    use crate::injector::testing::RecordingSink;
    use crate::injector::{self};
    use futures_util::StreamExt;
    use zbus::Connection;
    use zbus::proxy;

    #[proxy(
        interface = "com.acheron.Daemon",
        default_service = "com.acheron.Daemon",
        default_path = "/com/acheron/Daemon"
    )]
    trait DaemonProxy {
        fn get_config(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
        fn get_state(&self) -> zbus::Result<(String, String, Vec<String>, bool)>;
        fn set_binding(
            &self,
            input: &str,
            layer: &str,
            binding: HashMap<String, OwnedValue>,
        ) -> zbus::Result<()>;
        fn clear_binding(&self, input: &str, layer: &str) -> zbus::Result<()>;
        fn set_mode_key_role(&self, role: &str) -> zbus::Result<()>;
        fn create_profile(&self, name: &str) -> zbus::Result<()>;
        fn delete_profile(&self, name: &str) -> zbus::Result<()>;
        fn rename_profile(&self, old_name: &str, new_name: &str) -> zbus::Result<()>;
        fn switch_profile(&self, name: &str) -> zbus::Result<()>;

        #[zbus(signal)]
        fn active_profile_changed(&self, name: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn active_layer_changed(&self, layer: String) -> zbus::Result<()>;
    }

    /// Spins up a real, in-process `zbus` peer-to-peer connection (no
    /// session bus / broker needed) serving a real `Daemon` backed by a
    /// real dispatch task and ticket 13's fake `CaptureSource`, per this
    /// ticket's testing requirement: exercise the full path D-Bus mutation
    /// -> dispatch state -> injected output.
    struct TestServer {
        _dir: tempfile::TempDir,
        config_path: std::path::PathBuf,
        proxy: DaemonProxyProxy<'static>,
        event_tx: mpsc::Sender<PhysicalEvent>,
        sink: RecordingSink,
        server_connection: Connection,
        dispatch_handle: tokio::task::JoinHandle<std::io::Result<()>>,
        inj_handle: tokio::task::JoinHandle<std::io::Result<()>>,
    }

    impl TestServer {
        async fn start() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let config_path = dir.path().join("config.toml");
            let mut profiles = HashMap::new();
            profiles.insert(DEFAULT_PROFILE_NAME.to_string(), Profile::default());
            let config = Config {
                schema_version: crate::config::SCHEMA_VERSION,
                active_profile: DEFAULT_PROFILE_NAME.to_string(),
                profiles,
            };
            crate::config::write(&config_path, &config).unwrap();

            let sink = RecordingSink::new();
            let (inj, inj_handle) = injector::spawn(sink.clone());
            let (event_tx, event_rx) = mpsc::channel(8);
            let (cmd_tx, cmd_rx) = mpsc::channel(8);

            let daemon = Daemon::new(cmd_tx);
            let guid = zbus::Guid::generate();
            let (server_transport, client_transport) = tokio::net::UnixStream::pair().unwrap();

            // The SASL handshake is a back-and-forth over the socket, so
            // both `build()`s must run concurrently — awaiting one to
            // completion before starting the other deadlocks, since neither
            // side has anyone to talk to yet.
            let server_builder = zbus::connection::Builder::unix_stream(server_transport)
                .server(guid)
                .unwrap()
                .p2p()
                .serve_at("/com/acheron/Daemon", daemon)
                .unwrap();
            let client_builder = zbus::connection::Builder::unix_stream(client_transport).p2p();
            let (server_connection, client_connection) =
                tokio::join!(server_builder.build(), client_builder.build());
            let server_connection = server_connection.unwrap();
            let client_connection = client_connection.unwrap();
            let proxy = DaemonProxyProxy::new(&client_connection).await.unwrap();

            // Real signal emission (ticket 18's `ActiveLayerChanged`) needs a
            // live connection, so this test harness wires one through —
            // unlike `dispatch.rs`'s own unit tests, which pass `None` and
            // never assert on the signal itself.
            let signal_emitter = SignalEmitter::new(&server_connection, "/com/acheron/Daemon")
                .unwrap()
                .into_owned();
            let dispatch_handle = tokio::spawn(crate::dispatch::run(
                event_rx,
                cmd_rx,
                inj,
                config,
                config_path.clone(),
                Some(signal_emitter),
            ));

            TestServer {
                _dir: dir,
                config_path,
                proxy,
                event_tx,
                sink,
                server_connection,
                dispatch_handle,
                inj_handle,
            }
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
            drop(self.proxy);
            drop(self.server_connection);
            drop(self.event_tx);
            self.dispatch_handle.await.unwrap().unwrap();
            self.inj_handle.await.unwrap().unwrap();
            self.sink.batches()
        }
    }

    #[tokio::test]
    async fn set_binding_over_real_dbus_changes_live_output_and_config_toml() {
        let server = TestServer::start().await;

        let mut binding = wire::action_to_dict(&crate::config::Action::Keypress {
            modifiers: crate::config::Modifiers::default(),
            key: evdev::KeyCode::KEY_F1,
        });
        binding.insert(
            "trigger".to_string(),
            OwnedValue::try_from(zbus::zvariant::Value::new("fire_once".to_string())).unwrap(),
        );

        server
            .proxy
            .set_binding("grid_r1c1", "base", binding)
            .await
            .expect("SetBinding over D-Bus must succeed");

        let config = server.proxy.get_config().await.unwrap();
        let profiles: wire::Dict = config.get("profiles").unwrap().clone().try_into().unwrap();
        let default_profile: wire::Dict = profiles
            .get(DEFAULT_PROFILE_NAME)
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let base: wire::Dict = default_profile
            .get("base")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert!(base.contains_key("grid_r1c1"));

        server.press(Input::Grid(1, 1)).await;

        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        let batches = server.shut_down().await;

        assert_eq!(batches.len(), 2, "one press batch + one release batch");
        let evdev::EventSummary::Key(_, code, _) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_F1);
        assert!(on_disk.contains("grid_r1c1"));
        assert!(on_disk.contains("KEY_F1"));
    }

    /// Ticket 17: `SetBinding`'s `a{sv}` encoding, already settled in ticket
    /// 15, is exercised for real with a `Macro`/`Toggle` payload — no
    /// wire-format changes needed, but the Daemon must actually run it: a
    /// first press starts the Toggle, a second press on the same physical
    /// key stops it and force-releases exactly the key it left held.
    #[tokio::test]
    async fn set_binding_over_real_dbus_with_a_macro_toggle_payload_starts_and_stops_it() {
        let server = TestServer::start().await;

        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::Toggle,
            action: crate::config::Action::Macro {
                steps: vec![
                    crate::config::MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                    crate::config::MacroStepDto::Delay(50),
                ],
            },
        });

        server
            .proxy
            .set_binding("grid_r1c1", "base", binding)
            .await
            .expect("SetBinding with a Macro/Toggle payload must succeed");

        server.press(Input::Grid(1, 1)).await;
        // Let the Toggle's own task actually run its first KeyDown and
        // register its Delay sleep before stopping it — a real (unpaused)
        // 50ms Delay comfortably outlasts a few scheduler yields, so the
        // second press below cancels it well before the Delay would elapse
        // on its own.
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        // Same physical key stops it — this press is consumed by the stop.
        server.press(Input::Grid(1, 1)).await;

        let batches = server.shut_down().await;

        assert_eq!(batches.len(), 2, "one KeyDown lap, then the force-release");
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
    async fn clear_binding_over_real_dbus_on_an_unbound_input_returns_not_found() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .clear_binding("grid_r1c1", "base")
            .await
            .expect_err("clearing an unbound Input must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.NotFound")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn set_binding_over_real_dbus_with_an_invalid_input_string_is_rejected() {
        let server = TestServer::start().await;

        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::FireOnce,
            action: crate::config::Action::Keypress {
                modifiers: crate::config::Modifiers::default(),
                key: evdev::KeyCode::KEY_F1,
            },
        });

        let err = server
            .proxy
            .set_binding("not_a_real_input", "base", binding)
            .await
            .expect_err("an unparseable Input must be rejected");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn set_binding_over_real_dbus_with_an_invalid_layer_string_is_rejected() {
        let server = TestServer::start().await;

        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::FireOnce,
            action: crate::config::Action::Keypress {
                modifiers: crate::config::Modifiers::default(),
                key: evdev::KeyCode::KEY_F1,
            },
        });

        let err = server
            .proxy
            .set_binding("grid_r1c1", "bogus", binding)
            .await
            .expect_err("an unparseable Layer must be rejected");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn get_state_over_real_dbus_returns_this_tickets_fixed_stub_values() {
        let server = TestServer::start().await;

        let (profile, layer, active_toggles, device_connected) =
            server.proxy.get_state().await.unwrap();

        server.shut_down().await;

        assert_eq!(profile, DEFAULT_PROFILE_NAME);
        assert_eq!(layer, "base");
        assert!(active_toggles.is_empty());
        assert!(device_connected);
    }

    /// Proves the signal plumbing itself works end to end over a real
    /// connection, directly (not through a real `SwitchProfile` call, which
    /// has its own dedicated test below) — kept from ticket 15/18's original
    /// version of this test now that ticket 19 actually wires a trigger.
    #[tokio::test]
    async fn active_profile_changed_signal_is_delivered_to_a_subscribed_client() {
        let server = TestServer::start().await;

        let mut signals = server.proxy.receive_active_profile_changed().await.unwrap();

        let emitter = SignalEmitter::new(&server.server_connection, "/com/acheron/Daemon").unwrap();
        Daemon::active_profile_changed(&emitter, "Gaming")
            .await
            .unwrap();

        let signal = signals.next().await.expect("signal must be delivered");
        let args = signal.args().unwrap();
        assert_eq!(args.name, "Gaming");

        drop(signals);
        server.shut_down().await;
    }

    /// Ticket 18's end-to-end live demo, exercised without real hardware:
    /// a real `PhysicalEvent` Down/Up on `Input::ModeKey`, under the default
    /// `LayerSwitch` role, must push a real `ActiveLayerChanged` signal to a
    /// subscribed client for each transition.
    #[tokio::test]
    async fn mode_key_press_and_release_pushes_active_layer_changed_over_real_dbus() {
        let server = TestServer::start().await;

        let mut signals = server.proxy.receive_active_layer_changed().await.unwrap();

        server.press(Input::ModeKey).await;
        let signal = signals.next().await.expect("Held signal must be delivered");
        assert_eq!(signal.args().unwrap().layer, "held");

        server
            .event_tx
            .send(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Up,
            })
            .await
            .unwrap();
        let signal = signals.next().await.expect("Base signal must be delivered");
        assert_eq!(signal.args().unwrap().layer, "base");

        drop(signals);
        server.shut_down().await;
    }

    /// Ticket 18: `SetBinding`/`ClearBinding` target the Held Layer
    /// independently of Base when told to, over the real D-Bus surface.
    #[tokio::test]
    async fn set_binding_over_real_dbus_targets_the_held_layer_independently() {
        let server = TestServer::start().await;

        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::FireOnce,
            action: crate::config::Action::Keypress {
                modifiers: crate::config::Modifiers::default(),
                key: evdev::KeyCode::KEY_F1,
            },
        });

        server
            .proxy
            .set_binding("grid_r1c1", "held", binding)
            .await
            .expect("SetBinding on the Held layer must succeed");

        let config = server.proxy.get_config().await.unwrap();
        server.shut_down().await;

        let profiles: wire::Dict = config.get("profiles").unwrap().clone().try_into().unwrap();
        let default_profile: wire::Dict = profiles
            .get(DEFAULT_PROFILE_NAME)
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let base: wire::Dict = default_profile
            .get("base")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let held: wire::Dict = default_profile
            .get("held")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert!(!base.contains_key("grid_r1c1"), "Base must be untouched");
        assert!(held.contains_key("grid_r1c1"));
    }

    #[tokio::test]
    async fn set_mode_key_role_over_real_dbus_flips_the_active_profiles_role() {
        let server = TestServer::start().await;

        server
            .proxy
            .set_mode_key_role("bound")
            .await
            .expect("SetModeKeyRole must succeed");

        let config = server.proxy.get_config().await.unwrap();
        server.shut_down().await;

        let profiles: wire::Dict = config.get("profiles").unwrap().clone().try_into().unwrap();
        let default_profile: wire::Dict = profiles
            .get(DEFAULT_PROFILE_NAME)
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let role: String = default_profile
            .get("mode_key_role")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(role, "bound");
    }

    #[tokio::test]
    async fn set_mode_key_role_over_real_dbus_with_an_invalid_role_is_rejected() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .set_mode_key_role("bogus")
            .await
            .expect_err("an unparseable role must be rejected");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn create_profile_over_real_dbus_adds_an_empty_profile_and_persists() {
        let server = TestServer::start().await;

        server
            .proxy
            .create_profile("Gaming")
            .await
            .expect("CreateProfile must succeed");

        let config = server.proxy.get_config().await.unwrap();
        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        let profiles: wire::Dict = config.get("profiles").unwrap().clone().try_into().unwrap();
        assert!(profiles.contains_key("Gaming"));
        assert!(on_disk.contains("[profiles.Gaming]"));
    }

    #[tokio::test]
    async fn create_profile_over_real_dbus_with_a_duplicate_name_is_rejected() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .create_profile(DEFAULT_PROFILE_NAME)
            .await
            .expect_err("creating a Profile with an existing name must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.AlreadyExists")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn delete_profile_over_real_dbus_rejects_deleting_the_active_profile() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .delete_profile(DEFAULT_PROFILE_NAME)
            .await
            .expect_err("deleting the active Profile must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn delete_profile_over_real_dbus_on_an_unknown_name_returns_not_found() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .delete_profile("Nonexistent")
            .await
            .expect_err("deleting an unknown Profile must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.NotFound")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn rename_profile_over_real_dbus_renames_and_persists() {
        let server = TestServer::start().await;

        server
            .proxy
            .rename_profile(DEFAULT_PROFILE_NAME, "Renamed")
            .await
            .expect("RenameProfile must succeed");

        let config = server.proxy.get_config().await.unwrap();
        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        let profiles: wire::Dict = config.get("profiles").unwrap().clone().try_into().unwrap();
        assert!(!profiles.contains_key(DEFAULT_PROFILE_NAME));
        assert!(profiles.contains_key("Renamed"));
        let active_profile: String = config
            .get("active_profile")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(active_profile, "Renamed");
        assert!(on_disk.contains("active_profile = \"Renamed\""));
    }

    /// Ticket 19's live demo, exercised without real hardware: creating a
    /// second Profile, switching to it over real D-Bus, and getting the
    /// `ActiveProfileChanged` push a subscribed client (the GUI/tray) relies
    /// on.
    #[tokio::test]
    async fn switch_profile_over_real_dbus_changes_active_profile_and_pushes_the_signal() {
        let server = TestServer::start().await;
        server.proxy.create_profile("Gaming").await.unwrap();
        let mut signals = server.proxy.receive_active_profile_changed().await.unwrap();

        server
            .proxy
            .switch_profile("Gaming")
            .await
            .expect("SwitchProfile must succeed");

        let signal = signals.next().await.expect("signal must be delivered");
        assert_eq!(signal.args().unwrap().name, "Gaming");

        let (profile, ..) = server.proxy.get_state().await.unwrap();
        drop(signals);
        server.shut_down().await;

        assert_eq!(profile, "Gaming");
    }

    #[tokio::test]
    async fn switch_profile_over_real_dbus_on_an_unknown_name_returns_not_found() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .switch_profile("Nonexistent")
            .await
            .expect_err("switching to an unknown Profile must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.NotFound")
        );

        server.shut_down().await;
    }

    /// Ticket 19's core behavioral requirement, exercised end-to-end over
    /// real D-Bus and a real dispatch task: a Toggle left running in one
    /// Profile is force-stopped — with an exact-key release, not a stuck
    /// key — the instant `SwitchProfile` switches away from it.
    #[tokio::test]
    async fn switch_profile_over_real_dbus_force_stops_an_active_toggle_with_exact_key_release() {
        let server = TestServer::start().await;
        server.proxy.create_profile("Gaming").await.unwrap();

        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::Toggle,
            action: crate::config::Action::Macro {
                steps: vec![
                    crate::config::MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                    crate::config::MacroStepDto::Delay(50),
                ],
            },
        });
        server
            .proxy
            .set_binding("grid_r1c1", "base", binding)
            .await
            .expect("SetBinding with a Macro/Toggle payload must succeed");

        server.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        let (_, _, active_toggles, _) = server.proxy.get_state().await.unwrap();
        assert_eq!(active_toggles, vec!["grid_r1c1".to_string()]);

        server
            .proxy
            .switch_profile("Gaming")
            .await
            .expect("SwitchProfile must succeed");

        let (_, _, active_toggles, _) = server.proxy.get_state().await.unwrap();
        assert!(
            active_toggles.is_empty(),
            "the Toggle must be force-stopped by the switch"
        );

        let batches = server.shut_down().await;

        assert_eq!(batches.len(), 2, "one KeyDown lap, then the force-release");
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
