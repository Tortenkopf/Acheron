//! The D-Bus surface (ticket 15 / issue 08): one flat object,
//! `/com/acheron/Daemon`, on bus name `com.acheron.Daemon`, one combined
//! interface (also `com.acheron.Daemon`) — no `ObjectManager` hierarchy.
//! `Daemon` holds a `Command` sender for every read/mutation of `Config`
//! (forwarded to the dispatch task, the sole owner of `Config`, and awaited
//! over a `oneshot` reply — this type never touches `Config` directly), plus
//! an `Injector` handle used only by `SetOutputSuppressed` (ticket 24), which
//! is deliberately Config-free and so bypasses dispatch entirely.

pub mod wire;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

use crate::command::{Command, CommandError};
use crate::config::{MacroId, StepperId};
use crate::injector::Injector;
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

/// The injector task has stopped responding — only possible if it panicked
/// or exited, both genuine Daemon-internal failures (mirrors `dispatch_gone`
/// for the Config-free `SetOutputSuppressed` path, ticket 24).
fn injector_gone<T>(_: T) -> DaemonError {
    DaemonError::IoError("the injector task is not responding".to_string())
}

type DaemonResult<T> = Result<T, DaemonError>;

/// Tracks which connection, if any, currently holds output suppression
/// (ticket 24). `epoch` is bumped on every `SetOutputSuppressed` call,
/// whatever its value — the disconnect-watcher task spawned for a `true`
/// call captures the epoch it was born with and only auto-clears
/// suppression if that epoch is still current, so a stale watcher from a
/// since-superseded call can never clobber a newer one (last-write-wins,
/// disconnect-clear tied to whichever connection most recently set it).
struct SuppressionState {
    epoch: u64,
    watcher: Option<JoinHandle<()>>,
}

/// Ticket 26: which Grid Input, if any, the GUI's Actuation & release editor
/// currently wants live depth for — a single current target rather than a
/// set of subscribers, mirroring `SuppressionState`'s last-write-wins shape
/// rather than `active_toggles`-style tracking, since exactly one editor
/// popover is ever meaningfully open at a time in practice. `epoch` guards
/// the same race `SetOutputSuppressed` already documents: a `StartDepthStream`
/// racing a `StopDepthStream`/a newer `StartDepthStream` must never let the
/// loser's pump task clobber the winner's already-stored state.
struct DepthStreamState {
    epoch: u64,
    input: Option<Input>,
    watcher: Option<JoinHandle<()>>,
}

/// `DepthChanged`'s rate limit (ticket 19's prototype modeled ~30Hz; ticket
/// 13 measured the real device pushing changes roughly every 1ms while
/// moving, so this is what actually keeps the signal off the wire's firehose
/// the map's Notes warn against, not the capture layer's own publish rate).
const DEPTH_STREAM_INTERVAL: Duration = Duration::from_millis(33);

pub struct Daemon {
    commands: mpsc::Sender<Command>,
    injector: Injector,
    suppression: Arc<Mutex<SuppressionState>>,
    depth_rx: watch::Receiver<HashMap<Input, u8>>,
    depth_stream: Arc<Mutex<DepthStreamState>>,
}

impl Daemon {
    pub fn new(
        commands: mpsc::Sender<Command>,
        injector: Injector,
        depth_rx: watch::Receiver<HashMap<Input, u8>>,
    ) -> Self {
        Daemon {
            commands,
            injector,
            suppression: Arc::new(Mutex::new(SuppressionState {
                epoch: 0,
                watcher: None,
            })),
            depth_rx,
            depth_stream: Arc::new(Mutex::new(DepthStreamState {
                epoch: 0,
                input: None,
                watcher: None,
            })),
        }
    }

    fn parse_input(input: &str) -> DaemonResult<Input> {
        input
            .parse()
            .map_err(|_| DaemonError::InvalidBinding(format!("{input:?} is not a valid Input")))
    }
}

/// Ticket 26's depth-pump task: one spawned per `StartDepthStream` call,
/// aborted (not gracefully stopped) the moment it's superseded — by a
/// `StopDepthStream`, a fresh `StartDepthStream`, or the owning connection
/// disconnecting (`watch_disconnect`, shared with `SetOutputSuppressed`).
/// Samples `depth_rx` at `DEPTH_STREAM_INTERVAL` rather than reacting to
/// every `watch` change: the analog capture source overwrites it on every
/// incoming report (sub-millisecond while a key is moving, per ticket 13),
/// so reacting to every change would just reimplement the firehose the rate
/// limit exists to avoid. A `None` snapshot entry for `input` (Digital mode,
/// or no report has arrived yet for this key) simply skips that tick rather
/// than emitting a stale/absent value.
async fn run_depth_stream(
    connection: zbus::Connection,
    sender: Option<String>,
    depth_rx: watch::Receiver<HashMap<Input, u8>>,
    input: Input,
    state: Arc<Mutex<DepthStreamState>>,
    epoch: u64,
) {
    let Ok(emitter) = SignalEmitter::new(&connection, "/com/acheron/Daemon") else {
        return;
    };
    // `interval_at`, not `interval`: the latter's first tick resolves
    // immediately rather than after one `DEPTH_STREAM_INTERVAL`, which would
    // let a since-superseded `StartDepthStream` race one signal out before
    // its abort takes effect (observed in
    // `start_depth_stream_over_real_dbus_retargeting_replaces_the_previous_stream`).
    let mut interval = tokio::time::interval_at(
        tokio::time::Instant::now() + DEPTH_STREAM_INTERVAL,
        DEPTH_STREAM_INTERVAL,
    );
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let disconnect = watch_disconnect(connection.clone(), sender);
    tokio::pin!(disconnect);

    loop {
        tokio::select! {
            _ = &mut disconnect => break,
            _ = interval.tick() => {
                let depth = depth_rx.borrow().get(&input).copied();
                if let Some(depth) = depth {
                    let _ = Daemon::depth_changed(&emitter, &input.to_string(), depth).await;
                }
            }
        }
    }

    // Mirrors `set_output_suppressed`'s disconnect-clear: only clear the
    // shared state if this task's own epoch is still current — a since-
    // superseded task's late cleanup must never clobber a newer call's
    // already-stored watcher/input.
    let mut state = state.lock().unwrap();
    if state.epoch == epoch {
        state.input = None;
        state.watcher = None;
    }
}

/// Waits for whichever connection sent a `SetOutputSuppressed(true)` call to
/// disconnect. On a real message bus (`connection.is_bus()`), that's a
/// specific client among potentially several sharing the Daemon's one
/// session-bus connection, so it's tracked by unique name via
/// `org.freedesktop.DBus`'s `NameOwnerChanged` (the standard idiom for
/// "notice when my caller goes away" on a shared bus — zbus's own
/// `Connection::close_when_bus_name_disappears`-style tests use the same
/// `(0, name), (2, "")` match-arg filter). Over a private peer-to-peer
/// connection (the test harness's `TestServer`, and any future non-bus
/// transport), there is no bus daemon and no unique name to watch, but the
/// `zbus::Connection` itself *is* the one peer, so its own close detection
/// is exact.
async fn watch_disconnect(connection: zbus::Connection, sender: Option<String>) {
    if connection.is_bus()
        && let Some(sender) = sender
        && let Ok(dbus) = zbus::fdo::DBusProxy::new(&connection).await
        && let Ok(mut stream) = dbus
            .receive_name_owner_changed_with_args(&[(0, sender.as_str()), (2, "")])
            .await
    {
        // Subscribed *before* checking current ownership, never the other
        // way around, so a disconnect racing this setup can never be
        // missed: if the name is already gone by the time we check, either
        // it vanished before the subscription above took effect (caught by
        // this check) or after (already queued in `stream`, caught by
        // `stream.next()` below either way).
        if let Ok(name) = zbus::names::BusName::try_from(sender.as_str())
            && let Ok(false) = dbus.name_has_owner(name).await
        {
            return;
        }
        stream.next().await;
        return;
    }
    connection.closed().await;
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

    /// The live runtime snapshot, keyed by field name (ticket 25 — a
    /// positional tuple broke `app.py`'s `rebuild()` the moment `capture_mode`
    /// was added in ticket 21). `active_toggles` reflects the dispatch
    /// task's real `HashMap<Input, ActiveToggle>` as of ticket 17. `layer`
    /// reflects the dispatch task's real active Layer as of ticket 18.
    /// `device_connected` reflects the `CaptureSource`'s poll loop's current
    /// view as of ticket 20. `capture_mode` (`"analog"`/`"digital"`) is real
    /// as of ticket 23 — see `command::State`'s doc comment.
    async fn get_state(&self) -> Result<HashMap<String, OwnedValue>, DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::GetState(reply))
            .await
            .map_err(dispatch_gone)?;
        let state = rx.await.map_err(dispatch_gone)?;
        Ok(wire::state_to_dict(&state))
    }

    /// Level-sets whether the calling client wants Daemon output withheld —
    /// reflects "should output be suppressed right now," not an
    /// edge-triggered toggle, so a client (e.g. the GUI pushing its window's
    /// live focus state) can call this redundantly with the same value with
    /// no ill effect (ticket 23/24). Only the injector task's write to
    /// `uinput` is gated: Trigger-mode firing, Macro looping, and a running
    /// Toggle's `active_toggles` state are entirely unaffected, and resume
    /// emitting output exactly where they logically were the instant
    /// suppression clears.
    ///
    /// Last write wins across clients. If `suppressed` is `true`, a watcher
    /// is spawned that auto-clears suppression the instant this call's
    /// connection disconnects without an explicit clear — see
    /// `watch_disconnect` — so suppression can never get stuck on and
    /// silently mute the whole physical device.
    async fn set_output_suppressed(
        &self,
        suppressed: bool,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Result<(), DaemonError> {
        let sender = header.sender().map(ToString::to_string);

        // zbus spawns a fresh task per incoming method call (every method
        // here takes `&self`, never `&mut self`), so two `SetOutputSuppressed`
        // calls can genuinely run concurrently — the epoch bump and the old
        // watcher's take/abort must happen in one atomic critical section,
        // and `epoch` must be *this* call's own bumped value, not whatever
        // `state.epoch` happens to read moments later after an `.await` —
        // or a racing call's bump could get misattributed and this call's
        // final store below could clobber a newer call's already-stored
        // watcher.
        let epoch = {
            let mut state = self.suppression.lock().unwrap();
            state.epoch += 1;
            if let Some(handle) = state.watcher.take() {
                handle.abort();
            }
            state.epoch
        };

        self.injector
            .set_suppressed(suppressed)
            .await
            .map_err(injector_gone)?;

        if suppressed {
            let connection = connection.clone();
            let injector = self.injector.clone();
            let suppression = self.suppression.clone();
            let handle = tokio::spawn(async move {
                watch_disconnect(connection, sender).await;
                let should_clear = {
                    let mut state = suppression.lock().unwrap();
                    if state.epoch == epoch {
                        state.watcher = None;
                        true
                    } else {
                        false
                    }
                };
                if should_clear {
                    let _ = injector.set_suppressed(false).await;
                }
            });

            // Only claim the watcher slot if no concurrent call has
            // superseded this one since `epoch` was captured above —
            // otherwise abort immediately rather than overwrite whatever
            // that newer call already stored.
            let mut state = self.suppression.lock().unwrap();
            if state.epoch == epoch {
                state.watcher = Some(handle);
            } else {
                handle.abort();
            }
        }

        Ok(())
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

    /// Creates a new Macro library entry (ticket 15/51) — atomic/
    /// immediately-applied/immediately-persisted, same conventions as
    /// `CreateProfile`. Unlike a Profile (whose identity *is* the
    /// caller-chosen name), a Macro's identity is a `MacroId` slug derived
    /// from `name` and frozen at creation; the return value is that
    /// assigned id, which the caller needs in order to reference it from a
    /// Binding.
    async fn create_macro(
        &self,
        name: String,
        steps: Vec<HashMap<String, OwnedValue>>,
    ) -> Result<String, DaemonError> {
        let steps = steps
            .iter()
            .map(wire::macro_step_from_dict)
            .collect::<Result<Vec<_>, _>>()
            .map_err(DaemonError::InvalidBinding)?;

        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::CreateMacro { name, steps, reply })
            .await
            .map_err(dispatch_gone)?;
        let macro_id = rx
            .await
            .map_err(dispatch_gone)?
            .map_err(DaemonError::from)?;
        Ok(macro_id.to_string())
    }

    /// Renames a Macro — a pure display-name field write; the `MacroId`
    /// itself never changes. Errors `NotFound` if `macro_id` doesn't exist.
    async fn rename_macro(&self, macro_id: String, new_name: String) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::RenameMacro {
                macro_id: MacroId::from(macro_id),
                new_name,
                reply,
            })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Deletes a Macro. Errors `NotFound` if it doesn't exist, or
    /// `InvalidBinding` if any Binding anywhere still references it.
    async fn delete_macro(&self, macro_id: String) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::DeleteMacro {
                macro_id: MacroId::from(macro_id),
                reply,
            })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Overwrites a Macro's step sequence in place — the real persistence
    /// path ticket 52's library editor needs for add/remove/reorder edits
    /// against an already-created entry (`CreateMacro`'s `steps` argument
    /// only covers what the Macro is born with). Errors `NotFound` if
    /// `macro_id` doesn't exist.
    async fn set_macro_steps(
        &self,
        macro_id: String,
        steps: Vec<HashMap<String, OwnedValue>>,
    ) -> Result<(), DaemonError> {
        let steps = steps
            .iter()
            .map(wire::macro_step_from_dict)
            .collect::<Result<Vec<_>, _>>()
            .map_err(DaemonError::InvalidBinding)?;

        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SetMacroSteps {
                macro_id: MacroId::from(macro_id),
                steps,
                reply,
            })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Creates a new Stepper library entry (ticket 03/54) — atomic/
    /// immediately-applied/immediately-persisted, mirroring `create_macro`
    /// exactly. A Stepper's identity is a `StepperId` slug derived from
    /// `name` and frozen; the return value is that assigned id.
    async fn create_stepper(
        &self,
        name: String,
        items: Vec<HashMap<String, OwnedValue>>,
    ) -> Result<String, DaemonError> {
        let items = items
            .iter()
            .map(wire::stepper_item_from_dict)
            .collect::<Result<Vec<_>, _>>()
            .map_err(DaemonError::InvalidBinding)?;

        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::CreateStepper { name, items, reply })
            .await
            .map_err(dispatch_gone)?;
        let stepper_id = rx
            .await
            .map_err(dispatch_gone)?
            .map_err(DaemonError::from)?;
        Ok(stepper_id.to_string())
    }

    /// Renames a Stepper — a pure display-name field write; the `StepperId`
    /// itself never changes. Errors `NotFound` if `stepper_id` doesn't
    /// exist.
    async fn rename_stepper(
        &self,
        stepper_id: String,
        new_name: String,
    ) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::RenameStepper {
                stepper_id: StepperId::from(stepper_id),
                new_name,
                reply,
            })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Deletes a Stepper. Errors `NotFound` if it doesn't exist, or
    /// `InvalidBinding` if any Binding anywhere still references it.
    async fn delete_stepper(&self, stepper_id: String) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::DeleteStepper {
                stepper_id: StepperId::from(stepper_id),
                reply,
            })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Overwrites a Stepper's item list in place, mirroring
    /// `set_macro_steps` exactly. Errors `NotFound` if `stepper_id` doesn't
    /// exist.
    async fn set_stepper_items(
        &self,
        stepper_id: String,
        items: Vec<HashMap<String, OwnedValue>>,
    ) -> Result<(), DaemonError> {
        let items = items
            .iter()
            .map(wire::stepper_item_from_dict)
            .collect::<Result<Vec<_>, _>>()
            .map_err(DaemonError::InvalidBinding)?;

        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SetStepperItems {
                stepper_id: StepperId::from(stepper_id),
                items,
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

    /// Force-stops every currently running Toggle (ticket 25) — a deliberate
    /// GUI-side guard against a Toggle left running unnoticed once the GUI's
    /// own window gains focus, distinct from `SetOutputSuppressed`: that
    /// method alone never stops anything (its own doc comment above still
    /// holds — Trigger firing/Macro looping/`active_toggles` are untouched
    /// by suppression on its own), but the GUI calls this explicitly
    /// alongside `SetOutputSuppressed(true)` on every focus-gain, since
    /// nobody wants a macro silently still running in the background once
    /// they've alt-tabbed to the GUI to look at it. Same underlying
    /// force-stop mechanism as `SwitchProfile`, minus the Profile change.
    /// Never fails.
    async fn stop_all_toggles(&self) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::StopAllToggles { reply })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)
    }

    /// Sets a per-key Actuation/Release point override on the active
    /// Profile (ticket 17 §5/§7). Errors `InvalidBinding` if `input` isn't a
    /// `Grid` Input, or if `release > actuation`.
    async fn set_actuation_point(
        &self,
        input: String,
        actuation: u8,
        release: u8,
    ) -> Result<(), DaemonError> {
        let input = Self::parse_input(&input)?;

        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SetActuationPoint {
                input,
                actuation,
                release,
                reply,
            })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Removes a per-key override, reverting that key to the active
    /// Profile's default Actuation/Release point. Errors `InvalidBinding` if
    /// `input` isn't a `Grid` Input.
    async fn clear_actuation_point(&self, input: String) -> Result<(), DaemonError> {
        let input = Self::parse_input(&input)?;

        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::ClearActuationPoint { input, reply })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Sets the active Profile's default Actuation/Release point — what
    /// every Grid key without its own override uses. Errors
    /// `InvalidBinding` if `release > actuation`.
    async fn set_default_actuation(&self, actuation: u8, release: u8) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SetDefaultActuation {
                actuation,
                release,
                reply,
            })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Clears every per-key override on the active Profile in one call/one
    /// `config.toml` rewrite — the GUI's "reset all keys to Profile
    /// default" affordance (ticket 17 §5). Never fails on validation
    /// grounds; can still surface `IoError` if the rewrite itself fails.
    async fn reset_actuation_points(&self) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::ResetActuationPoints { reply })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// The live setter for `Config.force_digital` (ticket 17 §4) — the
    /// user-facing override that forces Digital Capture mode even when
    /// Analog would otherwise unlock. Persists the flag and (ticket 23)
    /// actually swaps the live capture source.
    async fn set_force_digital(&self, force: bool) -> Result<(), DaemonError> {
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::SetForceDigital { force, reply })
            .await
            .map_err(dispatch_gone)?;
        rx.await.map_err(dispatch_gone)?.map_err(DaemonError::from)
    }

    /// Starts (or retargets) live depth streaming for `input` — the GUI's
    /// Actuation & release editor's `DepthChanged` feed (ticket 19/26).
    /// Connection-scoped and last-write-wins, mirroring
    /// `SetOutputSuppressed`: a second `StartDepthStream` call, from this
    /// connection or another, simply replaces the current target rather than
    /// layering a second stream, and the stream auto-stops if the calling
    /// connection disconnects without an explicit `StopDepthStream` (same
    /// `watch_disconnect` this reuses). Config-free and bypasses dispatch
    /// entirely, same as `SetOutputSuppressed` — it reads `depth_rx`, never
    /// touches `Config`. Errors `InvalidBinding` if `input` isn't a `Grid`
    /// variant, since only Grid keys ever have depth.
    async fn start_depth_stream(
        &self,
        input: String,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Result<(), DaemonError> {
        let input = Self::parse_input(&input)?;
        if !matches!(input, Input::Grid(_, _)) {
            return Err(DaemonError::InvalidBinding(format!(
                "{input} has no depth to stream"
            )));
        }
        let sender = header.sender().map(ToString::to_string);

        let epoch = {
            let mut state = self.depth_stream.lock().unwrap();
            state.epoch += 1;
            if let Some(handle) = state.watcher.take() {
                handle.abort();
            }
            state.input = Some(input);
            state.epoch
        };

        let connection = connection.clone();
        let depth_rx = self.depth_rx.clone();
        let depth_stream = self.depth_stream.clone();
        let handle = tokio::spawn(run_depth_stream(
            connection,
            sender,
            depth_rx,
            input,
            depth_stream,
            epoch,
        ));

        let mut state = self.depth_stream.lock().unwrap();
        if state.epoch == epoch {
            state.watcher = Some(handle);
        } else {
            handle.abort();
        }
        Ok(())
    }

    /// Stops live depth streaming. `input` is validated for symmetry with
    /// `StartDepthStream` but otherwise unused: exactly one stream target
    /// exists at a time (see `DepthStreamState`), so this always stops
    /// whichever one is current, regardless of which connection started it —
    /// the same "level-set, not a per-caller toggle" shape
    /// `SetOutputSuppressed` already uses.
    async fn stop_depth_stream(&self, input: String) -> Result<(), DaemonError> {
        Self::parse_input(&input)?;
        let mut state = self.depth_stream.lock().unwrap();
        state.epoch += 1;
        if let Some(handle) = state.watcher.take() {
            handle.abort();
        }
        state.input = None;
        Ok(())
    }

    /// Fires at most every `DEPTH_STREAM_INTERVAL` (~30Hz) while
    /// `StartDepthStream` has an active target matching `input` (ticket
    /// 19/26). `depth` is 0-255 raw travel, the same units `SetActuationPoint`
    /// takes.
    #[zbus(signal)]
    pub async fn depth_changed(
        signal_emitter: &SignalEmitter<'_>,
        input: &str,
        depth: u8,
    ) -> zbus::Result<()>;

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

    /// Fires on every device-connection transition, as reported by the
    /// `CaptureSource`'s poll loop (ticket 20). The dispatch task calls this
    /// directly (it holds the `SignalEmitter` `main.rs` builds alongside the
    /// D-Bus connection, same pattern as `active_layer_changed` above), not
    /// through a `Command`.
    #[zbus(signal)]
    pub async fn device_connection_changed(
        signal_emitter: &SignalEmitter<'_>,
        connected: bool,
    ) -> zbus::Result<()>;

    /// Fires on Capture-mode transitions — `"analog"`/`"digital"` (ticket 17
    /// §4). Ticket 16 proved the actual mode can change under a *running*
    /// Daemon (survives suspend, not a power cycle), so this mirrors
    /// `device_connection_changed` exactly. Called from
    /// `dispatch::handle_capture_mode_change` as of ticket 23, whenever the
    /// supervisor's `CaptureMode` push actually changes the dispatch task's
    /// view.
    #[zbus(signal)]
    pub async fn capture_mode_changed(
        signal_emitter: &SignalEmitter<'_>,
        mode: &str,
    ) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{EventState, PhysicalEvent};
    use crate::config::{Config, DEFAULT_PROFILE_NAME, Profile};
    use crate::injector::testing::RecordingSink;
    use crate::injector::{self};
    use zbus::Connection;
    use zbus::proxy;

    #[proxy(
        interface = "com.acheron.Daemon",
        default_service = "com.acheron.Daemon",
        default_path = "/com/acheron/Daemon"
    )]
    trait DaemonProxy {
        fn get_config(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
        fn get_state(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
        fn set_binding(
            &self,
            input: &str,
            layer: &str,
            binding: HashMap<String, OwnedValue>,
        ) -> zbus::Result<()>;
        fn clear_binding(&self, input: &str, layer: &str) -> zbus::Result<()>;
        fn set_output_suppressed(&self, suppressed: bool) -> zbus::Result<()>;
        fn set_mode_key_role(&self, role: &str) -> zbus::Result<()>;
        fn create_profile(&self, name: &str) -> zbus::Result<()>;
        fn delete_profile(&self, name: &str) -> zbus::Result<()>;
        fn rename_profile(&self, old_name: &str, new_name: &str) -> zbus::Result<()>;
        fn create_macro(
            &self,
            name: &str,
            steps: Vec<HashMap<String, OwnedValue>>,
        ) -> zbus::Result<String>;
        fn rename_macro(&self, macro_id: &str, new_name: &str) -> zbus::Result<()>;
        fn delete_macro(&self, macro_id: &str) -> zbus::Result<()>;
        fn set_macro_steps(
            &self,
            macro_id: &str,
            steps: Vec<HashMap<String, OwnedValue>>,
        ) -> zbus::Result<()>;
        fn create_stepper(
            &self,
            name: &str,
            items: Vec<HashMap<String, OwnedValue>>,
        ) -> zbus::Result<String>;
        fn rename_stepper(&self, stepper_id: &str, new_name: &str) -> zbus::Result<()>;
        fn delete_stepper(&self, stepper_id: &str) -> zbus::Result<()>;
        fn set_stepper_items(
            &self,
            stepper_id: &str,
            items: Vec<HashMap<String, OwnedValue>>,
        ) -> zbus::Result<()>;
        fn switch_profile(&self, name: &str) -> zbus::Result<()>;
        fn stop_all_toggles(&self) -> zbus::Result<()>;
        fn set_actuation_point(&self, input: &str, actuation: u8, release: u8) -> zbus::Result<()>;
        fn clear_actuation_point(&self, input: &str) -> zbus::Result<()>;
        fn set_default_actuation(&self, actuation: u8, release: u8) -> zbus::Result<()>;
        fn reset_actuation_points(&self) -> zbus::Result<()>;
        fn set_force_digital(&self, force: bool) -> zbus::Result<()>;
        fn start_depth_stream(&self, input: &str) -> zbus::Result<()>;
        fn stop_depth_stream(&self, input: &str) -> zbus::Result<()>;

        #[zbus(signal)]
        fn active_profile_changed(&self, name: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn active_layer_changed(&self, layer: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn device_connection_changed(&self, connected: bool) -> zbus::Result<()>;

        #[zbus(signal)]
        fn capture_mode_changed(&self, mode: String) -> zbus::Result<()>;

        #[zbus(signal)]
        fn depth_changed(&self, input: String, depth: u8) -> zbus::Result<()>;
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
        conn_tx: mpsc::Sender<bool>,
        capture_mode_tx: mpsc::Sender<crate::capture::CaptureMode>,
        depth_tx: watch::Sender<HashMap<Input, u8>>,
        capture_control_rx: mpsc::Receiver<bool>,
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
                force_digital: false,
                macros: HashMap::new(),
                steppers: HashMap::new(),
            };
            crate::config::write(&config_path, &config).unwrap();

            let sink = RecordingSink::new();
            let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
            let (event_tx, event_rx) = mpsc::channel(8);
            let (conn_tx, conn_rx) = mpsc::channel(8);
            let (cmd_tx, cmd_rx) = mpsc::channel(8);
            let (depth_tx, depth_rx) = watch::channel(HashMap::new());

            let daemon = Daemon::new(cmd_tx, inj.clone(), depth_rx);
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
            let (actuation_tx, _actuation_rx) = tokio::sync::watch::channel(HashMap::new());
            let (capture_mode_tx, capture_mode_rx) = mpsc::channel(8);
            let (capture_control_tx, capture_control_rx) = mpsc::channel(8);
            let dispatch_handle = tokio::spawn(crate::dispatch::run(
                event_rx,
                conn_rx,
                cmd_rx,
                inj,
                config,
                config_path.clone(),
                Some(signal_emitter),
                actuation_tx,
                capture_mode_rx,
                capture_control_tx,
            ));

            TestServer {
                _dir: dir,
                config_path,
                proxy,
                event_tx,
                conn_tx,
                capture_mode_tx,
                depth_tx,
                capture_control_rx,
                sink,
                server_connection,
                dispatch_handle,
                inj_handle,
            }
        }

        /// Stands in for the `AnalogCaptureSource` grid task publishing a
        /// fresh per-report depth snapshot (ticket 26) — the seam
        /// `run_depth_stream`'s pump samples from.
        fn set_depths(&self, depths: &[(Input, u8)]) {
            self.depth_tx.send_replace(depths.iter().copied().collect());
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

        /// Creates a Macro via a real `CreateMacro` D-Bus round-trip and
        /// returns its assigned `macro_id` — this test module's shorthand
        /// for seeding a Macro Binding's referenced library entry (ticket
        /// 51: `SetBinding` now rejects a Macro Action naming an unknown
        /// `macro_id`, so these tests must create the entry for real first).
        async fn create_macro(
            &self,
            name: &str,
            steps: Vec<crate::config::MacroStepDto>,
        ) -> String {
            let steps: Vec<HashMap<String, OwnedValue>> =
                steps.iter().map(wire::macro_step_to_dict).collect();
            self.proxy
                .create_macro(name, steps)
                .await
                .expect("CreateMacro must succeed")
        }

        /// `create_macro`'s exact mirror for the Stepper library — creates a
        /// Stepper via a real `CreateStepper` D-Bus round-trip and returns
        /// its assigned `stepper_id` (ticket 03/54: `SetBinding` rejects an
        /// `Action::Step` naming an unknown `stepper_id`, so these tests
        /// must create the entry for real first).
        async fn create_stepper(
            &self,
            name: &str,
            items: Vec<crate::config::StepperItem>,
        ) -> String {
            let items: Vec<HashMap<String, OwnedValue>> =
                items.iter().map(wire::stepper_item_to_dict).collect();
            self.proxy
                .create_stepper(name, items)
                .await
                .expect("CreateStepper must succeed")
        }

        /// Stands in for the `CaptureSource`'s poll loop reporting a
        /// device-connection transition (ticket 20).
        async fn set_device_connected(&self, connected: bool) {
            self.conn_tx.send(connected).await.unwrap();
        }

        /// Stands in for the supervisor (ticket 23) reporting a capture-mode
        /// transition.
        async fn set_capture_mode(&self, mode: crate::capture::CaptureMode) {
            self.capture_mode_tx.send(mode).await.unwrap();
        }

        async fn shut_down(mut self) -> Vec<Vec<evdev::InputEvent>> {
            drop(self.proxy);
            drop(self.server_connection);
            drop(self.event_tx);
            drop(self.conn_tx);
            drop(self.capture_mode_tx);
            self.capture_control_rx.close();
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

        let macro_id = server
            .create_macro(
                "Test macro",
                vec![
                    crate::config::MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                    crate::config::MacroStepDto::Delay(50),
                ],
            )
            .await;
        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::Toggle,
            action: crate::config::Action::Macro {
                macro_id: crate::config::MacroId::from(macro_id),
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
    async fn get_state_over_real_dbus_returns_the_live_snapshot() {
        let server = TestServer::start().await;

        let state = server.proxy.get_state().await.unwrap();

        server.shut_down().await;

        let profile: String = state.get("profile").unwrap().clone().try_into().unwrap();
        let layer: String = state.get("layer").unwrap().clone().try_into().unwrap();
        let active_toggles: Vec<String> = state
            .get("active_toggles")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let device_connected: bool = state
            .get("device_connected")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let capture_mode: String = state
            .get("capture_mode")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();

        assert_eq!(profile, DEFAULT_PROFILE_NAME);
        assert_eq!(layer, "base");
        assert!(active_toggles.is_empty());
        assert!(device_connected);
        assert_eq!(capture_mode, "digital");
    }

    /// Ticket 20's end-to-end live demo, exercised without real hardware: a
    /// reported device-connection transition must both be reflected in a
    /// subsequent `GetState()` and push a real `DeviceConnectionChanged`
    /// signal to a subscribed client.
    #[tokio::test]
    async fn device_connection_transition_updates_get_state_and_pushes_the_signal_over_real_dbus() {
        let server = TestServer::start().await;

        let mut signals = server
            .proxy
            .receive_device_connection_changed()
            .await
            .unwrap();

        server.set_device_connected(false).await;
        let signal = signals.next().await.expect("signal must be delivered");
        assert!(!signal.args().unwrap().connected);

        let state = server.proxy.get_state().await.unwrap();
        let device_connected: bool = state
            .get("device_connected")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert!(!device_connected);

        server.set_device_connected(true).await;
        let signal = signals.next().await.expect("signal must be delivered");
        assert!(signal.args().unwrap().connected);

        drop(signals);
        server.shut_down().await;
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
        // `emitter` holds its own clone of `server_connection` (needed to
        // actually write the signal to the wire) — it must be dropped before
        // `shut_down()`, or `server_connection`'s refcount never reaches
        // zero, the `ObjectServer` never releases the `Daemon` it holds, and
        // `shut_down`'s `inj_handle.await` (which needs every `Injector`
        // clone gone, including the one `Daemon` now holds for ticket 24)
        // deadlocks waiting for a teardown this leftover clone is blocking.
        drop(emitter);
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
                depth: None,
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

    #[tokio::test]
    async fn create_macro_over_real_dbus_derives_a_slug_and_persists_it() {
        let server = TestServer::start().await;

        let macro_id = server
            .create_macro(
                "Screenshot Combo",
                vec![crate::config::MacroStepDto::KeyDown(evdev::KeyCode::KEY_A)],
            )
            .await;
        assert_eq!(macro_id, "screenshot-combo");

        let config = server.proxy.get_config().await.unwrap();
        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        let macros: wire::Dict = config.get("macros").unwrap().clone().try_into().unwrap();
        let def: wire::Dict = macros
            .get("screenshot-combo")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let name: String = def.get("name").unwrap().clone().try_into().unwrap();
        assert_eq!(name, "Screenshot Combo");
        assert!(on_disk.contains("[macros.screenshot-combo]"));
    }

    #[tokio::test]
    async fn rename_macro_over_real_dbus_changes_the_name_not_the_macro_id() {
        let server = TestServer::start().await;
        let macro_id = server.create_macro("Old Name", vec![]).await;

        server
            .proxy
            .rename_macro(&macro_id, "New Name")
            .await
            .expect("RenameMacro must succeed");

        let config = server.proxy.get_config().await.unwrap();
        server.shut_down().await;

        let macros: wire::Dict = config.get("macros").unwrap().clone().try_into().unwrap();
        let def: wire::Dict = macros.get(&macro_id).unwrap().clone().try_into().unwrap();
        let name: String = def.get("name").unwrap().clone().try_into().unwrap();
        assert_eq!(name, "New Name");
    }

    #[tokio::test]
    async fn set_macro_steps_over_real_dbus_overwrites_steps_and_persists() {
        let server = TestServer::start().await;
        let macro_id = server
            .create_macro(
                "Test macro",
                vec![crate::config::MacroStepDto::KeyDown(evdev::KeyCode::KEY_A)],
            )
            .await;

        let new_steps: Vec<wire::Dict> = vec![
            wire::macro_step_to_dict(&crate::config::MacroStepDto::KeyDown(evdev::KeyCode::KEY_B)),
            wire::macro_step_to_dict(&crate::config::MacroStepDto::Delay(25)),
        ];
        server
            .proxy
            .set_macro_steps(&macro_id, new_steps)
            .await
            .expect("SetMacroSteps must succeed");

        let config = server.proxy.get_config().await.unwrap();
        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        let macros: wire::Dict = config.get("macros").unwrap().clone().try_into().unwrap();
        let def: wire::Dict = macros.get(&macro_id).unwrap().clone().try_into().unwrap();
        let name: String = def.get("name").unwrap().clone().try_into().unwrap();
        assert_eq!(name, "Test macro");
        assert!(on_disk.contains("[macros.test-macro]"));
    }

    #[tokio::test]
    async fn set_macro_steps_over_real_dbus_on_an_unknown_macro_id_returns_not_found() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .set_macro_steps("nonexistent", vec![])
            .await
            .expect_err("setting steps on an unknown Macro must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.NotFound")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn delete_macro_over_real_dbus_rejects_deleting_a_referenced_macro() {
        let server = TestServer::start().await;
        let macro_id = server
            .create_macro(
                "Test macro",
                vec![crate::config::MacroStepDto::KeyDown(evdev::KeyCode::KEY_A)],
            )
            .await;
        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::FireOnce,
            action: crate::config::Action::Macro {
                macro_id: crate::config::MacroId::from(macro_id.clone()),
            },
        });
        server
            .proxy
            .set_binding("grid_r1c1", "base", binding)
            .await
            .expect("SetBinding referencing a real macro_id must succeed");

        let err = server
            .proxy
            .delete_macro(&macro_id)
            .await
            .expect_err("deleting a still-referenced Macro must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server
            .proxy
            .clear_binding("grid_r1c1", "base")
            .await
            .unwrap();
        server
            .proxy
            .delete_macro(&macro_id)
            .await
            .expect("deleting an unreferenced Macro must now succeed");

        server.shut_down().await;
    }

    #[tokio::test]
    async fn delete_macro_over_real_dbus_on_an_unknown_macro_id_returns_not_found() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .delete_macro("nonexistent")
            .await
            .expect_err("deleting an unknown Macro must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.NotFound")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn create_stepper_over_real_dbus_derives_a_slug_and_persists_it() {
        let server = TestServer::start().await;

        let stepper_id = server
            .create_stepper(
                "Weapon Wheel",
                vec![crate::config::StepperItem::Key {
                    key: evdev::KeyCode::KEY_1,
                    modifiers: crate::config::Modifiers::default(),
                }],
            )
            .await;
        assert_eq!(stepper_id, "weapon-wheel");

        let config = server.proxy.get_config().await.unwrap();
        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        let steppers: wire::Dict = config.get("steppers").unwrap().clone().try_into().unwrap();
        let def: wire::Dict = steppers
            .get("weapon-wheel")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let name: String = def.get("name").unwrap().clone().try_into().unwrap();
        assert_eq!(name, "Weapon Wheel");
        assert!(on_disk.contains("[steppers.weapon-wheel]"));
    }

    #[tokio::test]
    async fn rename_stepper_over_real_dbus_changes_the_name_not_the_stepper_id() {
        let server = TestServer::start().await;
        let stepper_id = server.create_stepper("Old Name", vec![]).await;

        server
            .proxy
            .rename_stepper(&stepper_id, "New Name")
            .await
            .expect("RenameStepper must succeed");

        let config = server.proxy.get_config().await.unwrap();
        server.shut_down().await;

        let steppers: wire::Dict = config.get("steppers").unwrap().clone().try_into().unwrap();
        let def: wire::Dict = steppers
            .get(&stepper_id)
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let name: String = def.get("name").unwrap().clone().try_into().unwrap();
        assert_eq!(name, "New Name");
    }

    #[tokio::test]
    async fn set_stepper_items_over_real_dbus_overwrites_items_and_persists() {
        let server = TestServer::start().await;
        let stepper_id = server
            .create_stepper(
                "Test stepper",
                vec![crate::config::StepperItem::Key {
                    key: evdev::KeyCode::KEY_1,
                    modifiers: crate::config::Modifiers::default(),
                }],
            )
            .await;

        let new_items: Vec<wire::Dict> = vec![
            wire::stepper_item_to_dict(&crate::config::StepperItem::Key {
                key: evdev::KeyCode::KEY_2,
                modifiers: crate::config::Modifiers::default(),
            }),
            wire::stepper_item_to_dict(&crate::config::StepperItem::Key {
                key: evdev::KeyCode::KEY_3,
                modifiers: crate::config::Modifiers::default(),
            }),
        ];
        server
            .proxy
            .set_stepper_items(&stepper_id, new_items)
            .await
            .expect("SetStepperItems must succeed");

        let config = server.proxy.get_config().await.unwrap();
        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        let steppers: wire::Dict = config.get("steppers").unwrap().clone().try_into().unwrap();
        let def: wire::Dict = steppers
            .get(&stepper_id)
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let name: String = def.get("name").unwrap().clone().try_into().unwrap();
        assert_eq!(name, "Test stepper");
        assert!(on_disk.contains("[steppers.test-stepper]"));
    }

    #[tokio::test]
    async fn set_stepper_items_over_real_dbus_round_trips_modifiers() {
        // Ticket 63's live-testing follow-up: reproduces the GUI's actual
        // SetStepperItems -> GetConfig round trip over a real D-Bus
        // connection (not just the in-process wire:: functions), to rule
        // out a marshaling gap the unit tests can't see.
        let server = TestServer::start().await;
        let stepper_id = server.create_stepper("Test stepper", vec![]).await;

        let new_items: Vec<wire::Dict> = vec![wire::stepper_item_to_dict(
            &crate::config::StepperItem::Key {
                key: evdev::KeyCode::KEY_3,
                modifiers: crate::config::Modifiers {
                    ctrl: true,
                    shift: false,
                    alt: false,
                    super_key: false,
                },
            },
        )];
        server
            .proxy
            .set_stepper_items(&stepper_id, new_items)
            .await
            .expect("SetStepperItems must succeed");

        let config = server.proxy.get_config().await.unwrap();
        server.shut_down().await;

        let steppers: wire::Dict = config.get("steppers").unwrap().clone().try_into().unwrap();
        let def: wire::Dict = steppers
            .get(&stepper_id)
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let items: Vec<wire::Dict> = def.get("items").unwrap().clone().try_into().unwrap();
        assert_eq!(items.len(), 1);
        let modifiers: Vec<String> = items[0]
            .get("modifiers")
            .expect("modifiers must survive the real GetConfig() round trip")
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(modifiers, vec!["ctrl".to_string()]);
    }

    #[tokio::test]
    async fn set_stepper_items_over_real_dbus_on_an_unknown_stepper_id_returns_not_found() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .set_stepper_items("nonexistent", vec![])
            .await
            .expect_err("setting items on an unknown Stepper must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.NotFound")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn delete_stepper_over_real_dbus_rejects_deleting_a_referenced_stepper() {
        let server = TestServer::start().await;
        let stepper_id = server
            .create_stepper(
                "Test stepper",
                vec![crate::config::StepperItem::Key {
                    key: evdev::KeyCode::KEY_1,
                    modifiers: crate::config::Modifiers::default(),
                }],
            )
            .await;
        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::FireOnce,
            action: crate::config::Action::Step {
                stepper: crate::config::StepperId::from(stepper_id.clone()),
                direction: crate::config::StepDirection::Forward,
            },
        });
        server
            .proxy
            .set_binding("grid_r1c1", "base", binding)
            .await
            .expect("SetBinding referencing a real stepper_id must succeed");

        let err = server
            .proxy
            .delete_stepper(&stepper_id)
            .await
            .expect_err("deleting a still-referenced Stepper must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server
            .proxy
            .clear_binding("grid_r1c1", "base")
            .await
            .unwrap();
        server
            .proxy
            .delete_stepper(&stepper_id)
            .await
            .expect("deleting an unreferenced Stepper must now succeed");

        server.shut_down().await;
    }

    #[tokio::test]
    async fn delete_stepper_over_real_dbus_on_an_unknown_stepper_id_returns_not_found() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .delete_stepper("nonexistent")
            .await
            .expect_err("deleting an unknown Stepper must fail");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.NotFound")
        );

        server.shut_down().await;
    }

    /// Ticket 03/54's end-to-end live demo, exercised without real hardware:
    /// a real `PhysicalEvent` Down on a Step Binding advances `GetState()`'s
    /// `stepper_cursors` and injects the newly-selected item's key.
    #[tokio::test]
    async fn step_binding_over_real_dbus_advances_the_cursor_and_injects_the_new_item() {
        let server = TestServer::start().await;
        let stepper_id = server
            .create_stepper(
                "Weapon Wheel",
                vec![
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_1,
                        modifiers: crate::config::Modifiers::default(),
                    },
                    crate::config::StepperItem::Key {
                        key: evdev::KeyCode::KEY_2,
                        modifiers: crate::config::Modifiers::default(),
                    },
                ],
            )
            .await;
        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::FireOnce,
            action: crate::config::Action::Step {
                stepper: crate::config::StepperId::from(stepper_id.clone()),
                direction: crate::config::StepDirection::Forward,
            },
        });
        server
            .proxy
            .set_binding("grid_r1c1", "base", binding)
            .await
            .expect("SetBinding with a Step payload must succeed");

        server.press(Input::Grid(1, 1)).await;
        let state = server.proxy.get_state().await.unwrap();
        let batches = server.shut_down().await;

        let cursors: wire::Dict = state
            .get("stepper_cursors")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let cursor: u64 = cursors
            .get(&stepper_id)
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(cursor, 1);

        assert_eq!(batches.len(), 2, "one press batch + one release batch");
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!((code, value), (evdev::KeyCode::KEY_2, 1));
    }

    #[tokio::test]
    async fn set_binding_over_real_dbus_rejects_a_toggle_step_binding() {
        let server = TestServer::start().await;
        let stepper_id = server
            .create_stepper(
                "Weapon Wheel",
                vec![crate::config::StepperItem::Key {
                    key: evdev::KeyCode::KEY_1,
                    modifiers: crate::config::Modifiers::default(),
                }],
            )
            .await;
        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::Toggle,
            action: crate::config::Action::Step {
                stepper: crate::config::StepperId::from(stepper_id),
                direction: crate::config::StepDirection::Forward,
            },
        });

        let err = server
            .proxy
            .set_binding("grid_r1c1", "base", binding)
            .await
            .expect_err("a Toggle Step Binding must be rejected");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server.shut_down().await;
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

        let state = server.proxy.get_state().await.unwrap();
        drop(signals);
        server.shut_down().await;

        let profile: String = state.get("profile").unwrap().clone().try_into().unwrap();
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

        let macro_id = server
            .create_macro(
                "Test macro",
                vec![
                    crate::config::MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                    crate::config::MacroStepDto::Delay(50),
                ],
            )
            .await;
        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::Toggle,
            action: crate::config::Action::Macro {
                macro_id: crate::config::MacroId::from(macro_id),
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
        let state = server.proxy.get_state().await.unwrap();
        let active_toggles: Vec<String> = state
            .get("active_toggles")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(active_toggles, vec!["grid_r1c1".to_string()]);

        server
            .proxy
            .switch_profile("Gaming")
            .await
            .expect("SwitchProfile must succeed");

        let state = server.proxy.get_state().await.unwrap();
        let active_toggles: Vec<String> = state
            .get("active_toggles")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
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

    /// Ticket 25's live-hardware finding: the GUI's guard against a Toggle
    /// left running once its own window gains focus needs a real end-to-end
    /// path — `StopAllToggles` force-stops a running Toggle with an exact
    /// KeyUp release even while `SetOutputSuppressed(true)` is already in
    /// effect (the exact sequence the GUI issues on every focus-gain: it
    /// pushes suppression on, then stops all Toggles). Regression coverage
    /// for the bug this ticket found: force-release used to be gated by
    /// suppression the same as any other write, silently dropping the
    /// release and leaving the key stuck down.
    #[tokio::test]
    async fn stop_all_toggles_over_real_dbus_releases_a_key_even_while_suppressed() {
        let server = TestServer::start().await;

        let macro_id = server
            .create_macro(
                "Test macro",
                vec![
                    crate::config::MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                    crate::config::MacroStepDto::Delay(50),
                ],
            )
            .await;
        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::Toggle,
            action: crate::config::Action::Macro {
                macro_id: crate::config::MacroId::from(macro_id),
            },
        });
        server
            .proxy
            .set_binding("grid_r1c1", "base", binding)
            .await
            .expect("SetBinding with a Macro/Toggle payload must succeed");

        // Started while unsuppressed, matching the real repro: the Toggle's
        // KeyDown reaches the sink for real before the GUI ever gains focus.
        server.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        let state = server.proxy.get_state().await.unwrap();
        let active_toggles: Vec<String> = state
            .get("active_toggles")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(active_toggles, vec!["grid_r1c1".to_string()]);

        // The GUI's own focus-gain sequence: suppress first, then stop.
        server.proxy.set_output_suppressed(true).await.unwrap();
        server.proxy.stop_all_toggles().await.unwrap();

        let state = server.proxy.get_state().await.unwrap();
        let active_toggles: Vec<String> = state
            .get("active_toggles")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        let profile: String = state.get("profile").unwrap().clone().try_into().unwrap();
        assert!(
            active_toggles.is_empty(),
            "the Toggle must be force-stopped"
        );
        assert_eq!(profile, DEFAULT_PROFILE_NAME);

        let batches = server.shut_down().await;
        assert_eq!(
            batches.len(),
            2,
            "one KeyDown lap, then the force-released KeyUp"
        );
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(
            (code, value),
            (evdev::KeyCode::KEY_A, 0),
            "force-release must bypass suppression, not be silently dropped"
        );
    }

    /// Ticket 24's core requirement: while `SetOutputSuppressed(true)` is in
    /// effect, a press that would otherwise passthrough to `uinput` produces
    /// no injected output at all; the same press after an explicit
    /// `SetOutputSuppressed(false)` reaches the sink normally.
    #[tokio::test]
    async fn set_output_suppressed_withholds_output_until_explicitly_cleared() {
        let server = TestServer::start().await;

        server
            .proxy
            .set_output_suppressed(true)
            .await
            .expect("SetOutputSuppressed(true) must succeed");

        server.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert!(
            server.sink.batches().is_empty(),
            "a press while suppression is set must never reach uinput"
        );

        server
            .proxy
            .set_output_suppressed(false)
            .await
            .expect("SetOutputSuppressed(false) must succeed");

        server.press(Input::Grid(1, 2)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }

        let batches = server.shut_down().await;
        assert_eq!(
            batches.len(),
            1,
            "only the press made after clearing suppression reaches uinput"
        );
        let evdev::EventSummary::Key(_, code, _) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(
            code,
            crate::input::key_code_for_input(Input::Grid(1, 2)).unwrap()
        );
    }

    /// Ticket 24's Toggle-safety requirement: a Toggle started before
    /// suppression keeps looping and reports active in `GetState()`
    /// throughout a suppress/resume cycle — suppression withholds its
    /// output, not its internal state — and its output reaches `uinput`
    /// again as soon as suppression clears, with no explicit restart.
    #[tokio::test]
    async fn output_suppression_withholds_a_running_toggles_output_without_stopping_it() {
        let server = TestServer::start().await;

        let macro_id = server
            .create_macro(
                "Test macro",
                vec![
                    crate::config::MacroStepDto::KeyDown(evdev::KeyCode::KEY_A),
                    crate::config::MacroStepDto::Delay(15),
                    crate::config::MacroStepDto::KeyUp(evdev::KeyCode::KEY_A),
                    crate::config::MacroStepDto::Delay(15),
                ],
            )
            .await;
        let binding = wire::binding_to_dict(&crate::config::Binding {
            trigger: crate::config::TriggerMode::Toggle,
            action: crate::config::Action::Macro {
                macro_id: crate::config::MacroId::from(macro_id),
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
        let state = server.proxy.get_state().await.unwrap();
        let active_toggles: Vec<String> = state
            .get("active_toggles")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(active_toggles, vec!["grid_r1c1".to_string()]);

        server
            .proxy
            .set_output_suppressed(true)
            .await
            .expect("SetOutputSuppressed(true) must succeed");
        let batches_at_suppression = server.sink.batches().len();

        // Give the loop several laps' worth of real time to prove it keeps
        // running (and stays reported as active) but produces no new writes.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert_eq!(
            server.sink.batches().len(),
            batches_at_suppression,
            "a running Toggle's output must not reach uinput while suppressed"
        );
        let state = server.proxy.get_state().await.unwrap();
        let active_toggles: Vec<String> = state
            .get("active_toggles")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(
            active_toggles,
            vec!["grid_r1c1".to_string()],
            "the Toggle must still be reported active while its output is suppressed"
        );

        server
            .proxy
            .set_output_suppressed(false)
            .await
            .expect("SetOutputSuppressed(false) must succeed");
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert!(
            server.sink.batches().len() > batches_at_suppression,
            "the Toggle's buffered/next output must reach uinput again once suppression clears"
        );

        // Same physical key stops the Toggle so `shut_down` observes a clean
        // force-release rather than tearing down mid-loop.
        server.press(Input::Grid(1, 1)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        let state = server.proxy.get_state().await.unwrap();
        let active_toggles: Vec<String> = state
            .get("active_toggles")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert!(active_toggles.is_empty());

        server.shut_down().await;
    }

    /// Ticket 23/24's disconnect-safety requirement: suppression must never
    /// get stuck on. This test cannot use `TestServer` — its `proxy` and the
    /// server's real dispatch/injector tasks are meant to outlive the test —
    /// so it builds its own minimal p2p harness whose client connection can
    /// be dropped mid-test while the server side keeps running, to prove the
    /// Daemon notices the disconnect itself rather than relying on an
    /// explicit clear call that, by construction, the vanished client can
    /// never make.
    #[tokio::test]
    async fn output_suppression_auto_clears_when_the_setting_client_disconnects() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let mut profiles = HashMap::new();
        profiles.insert(DEFAULT_PROFILE_NAME.to_string(), Profile::default());
        let config = Config {
            schema_version: crate::config::SCHEMA_VERSION,
            active_profile: DEFAULT_PROFILE_NAME.to_string(),
            profiles,
            force_digital: false,
            macros: HashMap::new(),
            steppers: HashMap::new(),
        };
        crate::config::write(&config_path, &config).unwrap();

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let (event_tx, event_rx) = mpsc::channel(8);
        let (_conn_tx, conn_rx) = mpsc::channel(8);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let (_depth_tx, depth_rx) = watch::channel(HashMap::new());

        let daemon = Daemon::new(cmd_tx, inj.clone(), depth_rx);
        let guid = zbus::Guid::generate();
        let (server_transport, client_transport) = tokio::net::UnixStream::pair().unwrap();

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

        let (actuation_tx, _actuation_rx) = tokio::sync::watch::channel(HashMap::new());
        let (_capture_mode_tx, capture_mode_rx) = mpsc::channel(8);
        let (capture_control_tx, _capture_control_rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(crate::dispatch::run(
            event_rx,
            conn_rx,
            cmd_rx,
            inj,
            config,
            config_path,
            None,
            actuation_tx,
            capture_mode_rx,
            capture_control_tx,
        ));

        {
            // Scoped so both the proxy and its connection clone are dropped
            // at the end of this block, closing the client's half of the
            // socket — simulating the GUI crashing/vanishing without ever
            // calling `SetOutputSuppressed(false)`.
            let proxy = DaemonProxyProxy::new(&client_connection).await.unwrap();
            proxy
                .set_output_suppressed(true)
                .await
                .expect("SetOutputSuppressed(true) must succeed");
        }
        drop(client_connection);

        // Poll rather than sleep-once-and-assert: the server must notice the
        // socket close and clear suppression on its own, but exactly how
        // long that takes is scheduler-dependent.
        let mut resumed = false;
        for _ in 0..50 {
            event_tx
                .send(PhysicalEvent {
                    input: Input::Grid(1, 1),
                    state: EventState::Down,
                    depth: None,
                })
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            if !sink.batches().is_empty() {
                resumed = true;
                break;
            }
        }
        assert!(
            resumed,
            "output must resume automatically once the client that set \
             suppression disconnects, with no explicit clear call"
        );

        drop(server_connection);
        drop(event_tx);
        dispatch_handle.await.unwrap().unwrap();
        inj_handle.await.unwrap().unwrap();
    }

    /// Ticket 21's D-Bus round-trip for `SetActuationPoint`: succeeds over a
    /// real connection and persists to `config.toml`.
    #[tokio::test]
    async fn set_actuation_point_over_real_dbus_persists_the_override() {
        let server = TestServer::start().await;

        server
            .proxy
            .set_actuation_point("grid_r1c1", 200, 180)
            .await
            .expect("SetActuationPoint over D-Bus must succeed");

        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        assert!(on_disk.contains("grid_r1c1"));
        assert!(on_disk.contains("200"));
        assert!(on_disk.contains("180"));
    }

    #[tokio::test]
    async fn set_actuation_point_over_real_dbus_with_a_non_grid_input_is_rejected() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .set_actuation_point("mode_key", 200, 180)
            .await
            .expect_err("a non-Grid Input must be rejected");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn set_actuation_point_over_real_dbus_with_release_above_actuation_is_rejected() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .set_actuation_point("grid_r1c1", 100, 150)
            .await
            .expect_err("release > actuation must be rejected");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn clear_actuation_point_over_real_dbus_removes_the_override() {
        // `GetConfig()`'s wire dict doesn't carry actuation fields (out of
        // this ticket's scope), so this asserts against `config.toml`
        // directly, same as `set_binding_over_real_dbus_...`'s own on-disk
        // assertions above.
        let server = TestServer::start().await;

        server
            .proxy
            .set_actuation_point("grid_r1c1", 200, 180)
            .await
            .expect("SetActuationPoint over D-Bus must succeed");
        server
            .proxy
            .clear_actuation_point("grid_r1c1")
            .await
            .expect("ClearActuationPoint over D-Bus must succeed");

        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        assert!(!on_disk.contains("actuation_overrides"));
    }

    #[tokio::test]
    async fn clear_actuation_point_over_real_dbus_with_a_non_grid_input_is_rejected() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .clear_actuation_point("mode_key")
            .await
            .expect_err("a non-Grid Input must be rejected");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn set_default_actuation_over_real_dbus_persists_the_profile_default() {
        let server = TestServer::start().await;

        server
            .proxy
            .set_default_actuation(140, 120)
            .await
            .expect("SetDefaultActuation over D-Bus must succeed");

        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        assert!(on_disk.contains("140"));
        assert!(on_disk.contains("120"));
    }

    #[tokio::test]
    async fn set_default_actuation_over_real_dbus_with_release_above_actuation_is_rejected() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .set_default_actuation(100, 150)
            .await
            .expect_err("release > actuation must be rejected");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server.shut_down().await;
    }

    #[tokio::test]
    async fn reset_actuation_points_over_real_dbus_clears_every_override() {
        let server = TestServer::start().await;

        server
            .proxy
            .set_actuation_point("grid_r1c1", 200, 180)
            .await
            .expect("SetActuationPoint over D-Bus must succeed");
        server
            .proxy
            .set_actuation_point("grid_r2c2", 90, 70)
            .await
            .expect("SetActuationPoint over D-Bus must succeed");

        server
            .proxy
            .reset_actuation_points()
            .await
            .expect("ResetActuationPoints over D-Bus must succeed");

        // `GetConfig()`'s wire dict doesn't carry actuation fields (out of
        // this ticket's scope), so this asserts against `config.toml`
        // directly.
        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        assert!(!on_disk.contains("actuation_overrides"));
    }

    #[tokio::test]
    async fn set_force_digital_over_real_dbus_persists_the_preference() {
        let mut server = TestServer::start().await;

        server
            .proxy
            .set_force_digital(true)
            .await
            .expect("SetForceDigital over D-Bus must succeed");

        // Ticket 23: a successful persist also forwards the new value to
        // whatever's listening on `capture_control_rx` — the real supervisor
        // in production, this test's raw receiver end here — so the live
        // swap actually gets triggered, not just the config write.
        assert_eq!(server.capture_control_rx.recv().await, Some(true));

        let on_disk = std::fs::read_to_string(&server.config_path).unwrap();
        server.shut_down().await;

        assert!(on_disk.contains("force_digital = true"));
    }

    #[tokio::test]
    async fn capture_mode_transitions_over_real_dbus_update_get_state_and_fire_the_signal() {
        let server = TestServer::start().await;
        let mut signals = server.proxy.receive_capture_mode_changed().await.unwrap();

        server
            .set_capture_mode(crate::capture::CaptureMode::Analog)
            .await;

        let signal = signals.next().await.unwrap();
        assert_eq!(signal.args().unwrap().mode, "analog");

        let state = server.proxy.get_state().await.unwrap();
        let capture_mode: String = state
            .get("capture_mode")
            .unwrap()
            .clone()
            .try_into()
            .unwrap();
        assert_eq!(capture_mode, "analog");

        server.shut_down().await;
    }

    /// Ticket 26's core requirement: once `StartDepthStream(input)` is
    /// called, live depth published into `depth_tx` (standing in here for
    /// the real `AnalogCaptureSource`'s per-report publish) reaches a
    /// subscribed client as `DepthChanged(input, depth)`, throttled to
    /// `DEPTH_STREAM_INTERVAL`.
    #[tokio::test]
    async fn start_depth_stream_over_real_dbus_pushes_depth_changed_for_the_requested_input() {
        let server = TestServer::start().await;
        let mut signals = server.proxy.receive_depth_changed().await.unwrap();

        server.set_depths(&[(Input::Grid(1, 1), 200), (Input::Grid(1, 2), 40)]);
        server
            .proxy
            .start_depth_stream("grid_r1c1")
            .await
            .expect("StartDepthStream must succeed for a Grid Input");

        let signal = tokio::time::timeout(std::time::Duration::from_millis(500), signals.next())
            .await
            .expect("a DepthChanged signal must arrive within the throttle window")
            .expect("the signal stream must not have closed");
        let args = signal.args().unwrap();
        assert_eq!(args.input, "grid_r1c1");
        assert_eq!(args.depth, 200);

        drop(signals);
        server.shut_down().await;
    }

    /// `StartDepthStream` is last-write-wins per connection, mirroring
    /// `SetOutputSuppressed` — a second call retargets the stream rather than
    /// layering a second one, and the previous target stops being reported.
    #[tokio::test]
    async fn start_depth_stream_over_real_dbus_retargeting_replaces_the_previous_stream() {
        let server = TestServer::start().await;
        let mut signals = server.proxy.receive_depth_changed().await.unwrap();

        server.set_depths(&[(Input::Grid(1, 1), 10), (Input::Grid(1, 2), 20)]);
        server.proxy.start_depth_stream("grid_r1c1").await.unwrap();
        server.proxy.start_depth_stream("grid_r1c2").await.unwrap();

        let signal = tokio::time::timeout(std::time::Duration::from_millis(500), signals.next())
            .await
            .expect("a DepthChanged signal must arrive")
            .expect("the signal stream must not have closed");
        assert_eq!(signal.args().unwrap().input, "grid_r1c2");

        drop(signals);
        server.shut_down().await;
    }

    /// `StopDepthStream` ends the pump — no further `DepthChanged` signals
    /// arrive even though `depth_tx` keeps publishing.
    #[tokio::test]
    async fn stop_depth_stream_over_real_dbus_ends_further_signals() {
        let server = TestServer::start().await;
        let mut signals = server.proxy.receive_depth_changed().await.unwrap();

        server.set_depths(&[(Input::Grid(1, 1), 10)]);
        server.proxy.start_depth_stream("grid_r1c1").await.unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(500), signals.next())
            .await
            .expect("the first signal must arrive")
            .expect("the signal stream must not have closed");

        server.proxy.stop_depth_stream("grid_r1c1").await.unwrap();
        server.set_depths(&[(Input::Grid(1, 1), 250)]);

        let outcome =
            tokio::time::timeout(std::time::Duration::from_millis(150), signals.next()).await;
        assert!(
            outcome.is_err(),
            "no DepthChanged signal must arrive after StopDepthStream"
        );

        drop(signals);
        server.shut_down().await;
    }

    #[tokio::test]
    async fn start_depth_stream_over_real_dbus_rejects_a_non_grid_input() {
        let server = TestServer::start().await;

        let err = server
            .proxy
            .start_depth_stream("mode_key")
            .await
            .expect_err("a non-Grid Input must be rejected");
        assert!(
            matches!(err, zbus::Error::MethodError(name, _, _) if name.as_str() == "com.acheron.Daemon.Error.InvalidBinding")
        );

        server.shut_down().await;
    }
}
