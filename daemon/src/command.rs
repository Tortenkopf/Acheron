// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! `Command`: the message shape D-Bus method calls (ticket 15) push into the
//! dispatch task's channel, alongside capture's `PhysicalEvent`s (issue 07's
//! "D-Bus interleaving" — one state-owning consumer, no second lock or state
//! copy). The D-Bus-facing `dbus` module builds these and decodes the replies.
//! Every mutating call is a single `Apply` carrying an `edit::Edit` (ticket
//! 11), so this type is no longer `Config`-free — an `Edit` carries `Binding` /
//! `AxisTarget` / `ModeKeyRole` / … . The final module DAG is
//! `dbus → command → edit → config`.

use std::collections::HashMap;

use tokio::sync::oneshot;

use crate::config::{Config, StepperId};
use crate::edit::{CommandError, CreatedId, Edit};
use crate::input::Input;

/// The live runtime snapshot `GetState()` returns. `layer` reflects the
/// dispatch task's real active Layer (ticket 18: `"base"`/`"held"`, flips
/// under `Mode key` press/release when the active Profile's
/// `mode_key_role` is `LayerSwitch`). `active_toggles` is real as of ticket
/// 17; `device_connected` is real as of ticket 20, reflecting the
/// `CaptureSource`'s poll loop's current view. `capture_mode` (`"analog"`/
/// `"digital"`) is real as of ticket 23, reflecting the supervisor's actual
/// current `CaptureSource`. `daemon_version` (ticket 99) is the compile-time
/// `crate::VERSION` string, reported here so the GUI's About dialog can show
/// it alongside its own `__version__`.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub profile: String,
    pub layer: &'static str,
    pub active_toggles: Vec<Input>,
    pub device_connected: bool,
    pub capture_mode: &'static str,
    pub daemon_version: &'static str,
    /// The connected Tartarus Pro's firmware version (`vX.Y`) and serial
    /// number, read once per connection over the Interface-2 control channel
    /// (ticket 100/101) and surfaced for the GUI's About dialog (ticket
    /// 102). `None` — and absent from `GetState()`'s wire dict — when the
    /// device is disconnected or the read failed, mirroring how
    /// `device_connected` flips rather than a dedicated `GetDeviceInfo()`
    /// call (the data never changes within a connection).
    pub firmware_version: Option<String>,
    pub serial_number: Option<String>,
    /// Every Stepper library entry's Daemon-side-only runtime cursor (ticket
    /// 03/54 — CONTEXT.md: Stepper), keyed by `StepperId`, one entry per
    /// entry in `Config.steppers` (defaulting to `0`, "the list's first
    /// item," for one never yet stepped) — threaded into `GetState()` for
    /// the GUI's benefit, the same way `capture_mode` is. Never persisted;
    /// always resets to all-zero on a fresh Daemon start.
    pub stepper_cursors: HashMap<StepperId, usize>,
}

/// A GUI-originated read or mutation, as pushed through the dispatch task's
/// channel. Each variant carries a `oneshot` reply channel so the D-Bus
/// method handler can await the dispatch task's answer.
///
/// The 22 non-create mutating methods and the two create methods all reach
/// dispatch through the single `Apply` variant, carrying an `edit::Edit`
/// (ticket 11) — the per-operation `Command` mirror of `edit::Edit` is gone.
/// `StopAllToggles` stays its own variant: pure runtime, no `Config` touch,
/// no `Edit`.
pub enum Command {
    GetConfig(oneshot::Sender<Config>),
    GetState(oneshot::Sender<State>),
    /// Force-stops every currently running Toggle without switching
    /// anything else (ticket 25) — the GUI's deliberate guard against a
    /// Toggle left running unnoticed once its own window gains focus (e.g.
    /// alt-tabbing out of a game with a macro still going). Same underlying
    /// mechanism as `Edit::SwitchProfile`'s force-stop, minus the Profile
    /// change. Never fails: draining an already-empty `toggles` map is a
    /// no-op.
    StopAllToggles {
        reply: oneshot::Sender<()>,
    },
    /// Every mutating operation (ticket 11): `edit::apply` the `Edit`, reply
    /// before running its effects. `reply` carries `Outcome.created` —
    /// `Some` for `CreateMacro` / `CreateStepper`, `None` for the other 22.
    Apply {
        edit: Edit,
        reply: oneshot::Sender<Result<Option<CreatedId>, CommandError>>,
    },
}
