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
/// 17; `device_connected` is real as of ticket 20, reflecting the
/// `CaptureSource`'s poll loop's current view.
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
    /// Creates a new, empty Profile (ticket 19) — both Layers present with
    /// empty Binding maps, `mode_key_role` defaulting to `LayerSwitch`, same
    /// shape as the seed `Default` Profile. Fails `AlreadyExists` if `name`
    /// is already taken, or `InvalidRequest` if `name` is empty/whitespace —
    /// validated here (not just client-side by the GUI's own popover) since
    /// any `com.acheron.Daemon` caller can reach this Command.
    CreateProfile {
        name: String,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Deletes a Profile by name. Fails `NotFound` if it doesn't exist, or
    /// `InvalidRequest` if it's the active Profile — a Config's
    /// `active_profile` must always name a real Profile (the same invariant
    /// `load_or_seed` enforces on startup), so the active one can never be
    /// deleted out from under itself. Since a lone remaining Profile is
    /// always the active one, this also guarantees at least one Profile
    /// always survives.
    DeleteProfile {
        name: String,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Renames a Profile, updating `active_profile` too if the renamed one
    /// is currently active. Fails `NotFound` if `old_name` doesn't exist,
    /// `AlreadyExists` if `new_name` is already taken by a different
    /// Profile, or `InvalidRequest` if `new_name` is empty/whitespace.
    RenameProfile {
        old_name: String,
        new_name: String,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Switches the active Profile (ticket 19). Force-stops every currently
    /// running Toggle — force-releasing each one's tracked held keys via the
    /// injector (the same mechanism ticket 17's `ActiveToggle::stop` uses) —
    /// before the new Profile's state becomes active, per spec.md's "Toggle
    /// behavior across Layer/Profile switches". Fails `NotFound` if `name`
    /// doesn't name a real Profile.
    SwitchProfile {
        name: String,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Force-stops every currently running Toggle without switching
    /// anything else (ticket 25) — the GUI's deliberate guard against a
    /// Toggle left running unnoticed once its own window gains focus (e.g.
    /// alt-tabbing out of a game with a macro still going). Same underlying
    /// mechanism as `SwitchProfile`'s force-stop, minus the Profile change.
    /// Never fails: draining an already-empty `toggles` map is a no-op.
    StopAllToggles { reply: oneshot::Sender<()> },
}

/// Errors a `Command` can fail with. Deliberately narrower than the D-Bus
/// surface's `com.acheron.Daemon.Error.*` set (`dbus::DaemonError`) —
/// malformed wire payloads are rejected before a `Command` is ever built.
/// `AlreadyExists`/`InvalidRequest` start being used as of ticket 19's
/// Profile CRUD; `InvalidRequest` maps onto the wire's `InvalidBinding`
/// bucket (issue 08's "small named set", not one error per validation rule)
/// since it's the closest fit for "the request itself is malformed/
/// disallowed," even for a non-Binding request like deleting the active
/// Profile.
#[derive(Debug)]
pub enum CommandError {
    NotFound,
    AlreadyExists,
    InvalidRequest(String),
    IoError(String),
}

impl From<crate::config::ConfigError> for CommandError {
    fn from(err: crate::config::ConfigError) -> Self {
        CommandError::IoError(err.to_string())
    }
}
