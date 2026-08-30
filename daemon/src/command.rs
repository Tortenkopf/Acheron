//! `Command`: the message shape D-Bus method calls (ticket 15) push into the
//! dispatch task's channel, alongside capture's `PhysicalEvent`s (issue 07's
//! "D-Bus interleaving" — one state-owning consumer, no second lock or state
//! copy). The D-Bus-facing `dbus` module builds these and decodes the
//! replies; it never touches `Config` directly.

use std::collections::{BTreeSet, HashMap};

use tokio::sync::oneshot;

use crate::config::{
    AxisTarget, Binding, Config, Layer, MacroId, MacroStepDto, ModeKeyRole, StepperId, StepperItem,
};
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
    StopAllToggles {
        reply: oneshot::Sender<()>,
    },
    /// Sets a per-key Actuation/Release point override on the active
    /// Profile (ticket 17 §5/§7). Fails `InvalidRequest` if `input` isn't a
    /// `Grid` variant, or if `release > actuation`.
    SetActuationPoint {
        input: Input,
        actuation: u8,
        release: u8,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Removes a per-key override, reverting that key to the active
    /// Profile's `default_actuation`. Fails `InvalidRequest` if `input`
    /// isn't a `Grid` variant; idempotent otherwise (clearing an
    /// already-unoverridden key succeeds with no effect).
    ClearActuationPoint {
        input: Input,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Sets the active Profile's `default_actuation` — the Actuation/Release
    /// point every Grid key uses unless it has its own override. Fails
    /// `InvalidRequest` if `release > actuation`.
    SetDefaultActuation {
        actuation: u8,
        release: u8,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Clears every per-key override on the active Profile in one call/one
    /// `config.toml` rewrite — the GUI's "reset all keys to Profile default"
    /// affordance (ticket 17 §5), not 20 individual `ClearActuationPoint`
    /// calls. Never fails on validation grounds (clearing an already-empty
    /// `actuation_overrides` map is a no-op) — the `Result` reply exists
    /// only for the same `config.toml`-write-failure case every other
    /// persisting Command carries.
    ResetActuationPoints {
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Sets `Config.force_digital` — the user-facing override that forces
    /// Digital Capture mode even when Analog would otherwise unlock (ticket
    /// 17 §4). Persists the flag and (ticket 23, on a successful persist)
    /// triggers the supervisor's live capture-source swap.
    SetForceDigital {
        force: bool,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Creates a new Macro library entry (ticket 15/51) — a `MacroId` is
    /// derived from `name` via the slug algorithm (`config::unique_macro_id`)
    /// and frozen; the reply carries that assigned id back to the caller,
    /// unlike `CreateProfile` (whose identity *is* the caller-chosen name).
    /// Fails `InvalidRequest` if `name` is empty/whitespace. No
    /// `AlreadyExists` case — a slug collision is resolved automatically
    /// (numeric suffix), never rejected.
    CreateMacro {
        name: String,
        steps: Vec<MacroStepDto>,
        reply: oneshot::Sender<Result<MacroId, CommandError>>,
    },
    /// Renames a Macro — a pure `MacroDef.name` field write, the `MacroId`
    /// itself never changes (ticket 15's Answer: identity is deliberately
    /// decoupled from the editable display name). Fails `NotFound` if
    /// `macro_id` doesn't exist, or `InvalidRequest` if `new_name` is
    /// empty/whitespace. No `AlreadyExists` — display names aren't unique.
    RenameMacro {
        macro_id: MacroId,
        new_name: String,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Deletes a Macro. Fails `NotFound` if it doesn't exist, or
    /// `InvalidRequest` if any Binding anywhere (`base`/`held`, any Profile)
    /// still references its `MacroId` — mirrors `DeleteProfile`'s identical
    /// reasoning, making a dangling `macro_id` structurally impossible.
    DeleteMacro {
        macro_id: MacroId,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Overwrites a Macro's step sequence — a pure `MacroDef.steps` field
    /// write, the `MacroId` and `name` untouched. Ticket 52's library
    /// editor needs this to persist add/remove/reorder edits made against
    /// an *existing* library entry; `CreateMacro` alone only covers the
    /// steps a Macro is born with. Fails `NotFound` if `macro_id` doesn't
    /// exist. No `InvalidRequest` case — an empty step sequence is exactly
    /// as valid here as it is for a freshly `CreateMacro`'d entry.
    SetMacroSteps {
        macro_id: MacroId,
        steps: Vec<MacroStepDto>,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Creates a new Stepper library entry (ticket 03/54), mirroring
    /// `CreateMacro` exactly — a `StepperId` is derived from `name` via the
    /// slug algorithm (`config::unique_stepper_id`) and frozen; the reply
    /// carries that assigned id back to the caller. Fails `InvalidRequest`
    /// if `name` is empty/whitespace. No `AlreadyExists` case — a slug
    /// collision is resolved automatically (numeric suffix), never rejected.
    CreateStepper {
        name: String,
        items: Vec<StepperItem>,
        reply: oneshot::Sender<Result<StepperId, CommandError>>,
    },
    /// Renames a Stepper — a pure `StepperDef.name` field write, the
    /// `StepperId` itself never changes, mirroring `RenameMacro` exactly.
    /// Fails `NotFound` if `stepper_id` doesn't exist, or `InvalidRequest`
    /// if `new_name` is empty/whitespace.
    RenameStepper {
        stepper_id: StepperId,
        new_name: String,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Deletes a Stepper. Fails `NotFound` if it doesn't exist, or
    /// `InvalidRequest` if any Binding anywhere (`base`/`held`, any Profile,
    /// either direction) still references its `StepperId` — mirrors
    /// `DeleteMacro`'s identical reasoning, making a dangling `stepper_id`
    /// structurally impossible.
    DeleteStepper {
        stepper_id: StepperId,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Overwrites a Stepper's item list — a pure `StepperDef.items` field
    /// write, the `StepperId` and `name` untouched, mirroring
    /// `SetMacroSteps` exactly. Fails `NotFound` if `stepper_id` doesn't
    /// exist.
    SetStepperItems {
        stepper_id: StepperId,
        items: Vec<StepperItem>,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Creates or edits a Chord Binding on the active Profile (ticket 01/40
    /// — CONTEXT.md: Chord): atomic/immediately-applied/immediately-
    /// persisted, mirroring `SetBinding` exactly but keyed by `inputs`
    /// (≥2 members) instead of a single `Input`. Fails `InvalidRequest` if
    /// `inputs` has fewer than two members, if the Action/Trigger
    /// combination fails the same validation `SetBinding` already runs
    /// (Macro/Stepper naming a real library entry, ControllerButton naming a
    /// real gamepad button, ProfileSwitch pairing only with Fire-once), or
    /// if `inputs`' member set is a subset or superset of an existing
    /// Chord's on the same Layer (ticket 01's amended Answer) — editing the
    /// exact same member set back (same `inputs`) is not a conflict with
    /// itself.
    SetChordBinding {
        inputs: BTreeSet<Input>,
        layer: Layer,
        binding: Binding,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Removes a Chord Binding by its exact member set. Fails `NotFound` if
    /// no Chord with exactly that member set exists on `layer`.
    ClearChordBinding {
        inputs: BTreeSet<Input>,
        layer: Layer,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Creates or edits an Axis assignment on the active Profile (ticket
    /// 59/71 — CONTEXT.md: Axis assignment): atomic/immediately-applied/
    /// immediately-persisted, mirroring `SetBinding` exactly but clearing
    /// any existing Binding *and* any Chord membership for `(layer, input)`
    /// atomically alongside the insert (ticket 59 §2's mutual exclusion —
    /// unlike `SetBinding`/`SetChordBinding`, which reject rather than
    /// overwrite an existing Axis assignment there). Fails `InvalidRequest`
    /// if `input` isn't a `Grid` variant.
    SetAxisAssignment {
        input: Input,
        layer: Layer,
        target: AxisTarget,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
    /// Removes an Axis assignment, reverting `input` to ordinary passthrough
    /// on `layer`. Fails `NotFound` if `input` has no Axis assignment there.
    ClearAxisAssignment {
        input: Input,
        layer: Layer,
        reply: oneshot::Sender<Result<(), CommandError>>,
    },
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
