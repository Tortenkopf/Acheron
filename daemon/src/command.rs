//! `Command`: the message shape D-Bus method calls (ticket 15) push into the
//! dispatch task's channel, alongside capture's `PhysicalEvent`s (issue 07's
//! "D-Bus interleaving" — one state-owning consumer, no second lock or state
//! copy). The D-Bus-facing `dbus` module builds these and decodes the
//! replies; it never touches `Config` directly.

use tokio::sync::oneshot;

use crate::config::{Binding, Config, Layer, ModeKeyRole};
use crate::input::Input;

/// The live runtime snapshot `GetState()` returns. `layer` reflects the
/// dispatch task's real active Layer (ticket 18: `"base"`/`"held"`, flips
/// under `Mode key` press/release when the active Profile's
/// `mode_key_role` is `LayerSwitch`). `active_toggles` is real as of ticket
/// 17; `device_connected` is hardcoded `true` (real detection is ticket
/// 20's scope).
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub profile: String,
    pub layer: &'static str,
    pub active_toggles: Vec<Input>,
    pub device_connected: bool,
}

/// A GUI-originated mutation or read, as pushed through the dispatch task's
/// channel. Each variant carries a `oneshot` reply channel so the D-Bus
/// method handler can await the dispatch task's answer.
pub enum Command {
    GetConfig(oneshot::Sender<Config>),
    GetState(oneshot::Sender<State>),
    SetBinding {
        input: Input,
        layer: Layer,
        binding: Binding,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    ClearBinding {
        input: Input,
        layer: Layer,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Flips the active Profile's `mode_key_role` (ticket 18). Never fails
    /// on its own account — the active Profile always exists — but keeps a
    /// `Result` reply for symmetry with the other mutating Commands and
    /// room for a future validation rule.
    SetModeKeyRole {
        role: ModeKeyRole,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
}

/// Errors a `Command` can fail with. Deliberately narrower than the D-Bus
/// surface's `com.acheron.Daemon.Error.*` set (`dbus::DaemonError`) —
/// `AlreadyExists`/`InvalidBinding` never originate here: malformed wire
/// payloads are rejected before a `Command` is ever built, and
/// `AlreadyExists` belongs to entities (Profiles) this ticket doesn't mutate.
#[derive(Debug)]
pub enum CommandError {
    NotFound,
    IoError(String),
}

impl From<crate::config::ConfigError> for CommandError {
    fn from(err: crate::config::ConfigError) -> Self {
        CommandError::IoError(err.to_string())
    }
}
