// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The config-transaction module (ticket 05): one pure function, `plan`,
//! decides what a requested mutation does to the stored `Config` — it applies
//! the mutation to a clone, runs `config::validate` against the result, and
//! describes the post-commit effects the dispatch task must run — with no
//! I/O, no async, and no channels. `apply` is the thin async wrapper that
//! `plan`s, then `config::persist`s the planned `Config`, then assigns it on
//! success (rollback is just "don't assign").
//!
//! This is the third step on the path tickets 03 and 04 started: ticket 03
//! made *edit + persist* atomic (`config::persist_edit`), ticket 04
//! single-sourced *validation* on that path (`config::validate`), and this
//! ticket lifts the whole transaction — edit, validate, persist, and the
//! post-commit effect derivation — out of `dispatch.rs` and into one deep,
//! pure, synchronously-testable module. `config::validate` is unchanged and
//! stays the single invariant point. Ticket 11 then removed the shallow
//! `Command` envelope that mirrored `Edit` field-for-field: the D-Bus layer
//! now builds `Edit` values directly and the dispatch task runs them through
//! one `Command::Apply` arm, so `Edit` (and `CommandError`, moved here) is the
//! whole contract between `dbus` and the config transaction.

use std::path::Path;

use crate::config::{
    self, Action, ActuationPoint, AxisTarget, Binding, ChordKey, Config, Layer, MacroId,
    MacroStepDto, ModeKeyRole, Profile, StagingMode, StatusLeds, StepDirection, StepperId,
    StepperItem,
};
use crate::input::Input;

/// A single requested mutation to the stored `Config` — one data-only variant
/// per mutating operation the D-Bus surface exposes (24). `GetConfig` /
/// `GetState` / `StopAllToggles` have no `Edit`: they never touch `Config`, so
/// they stay wholly in `dispatch` / `command`. Each variant's doc comment is
/// the failure-mode contract `plan` enforces for it — the `Err` cases and why.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Creates or edits a Binding on the active Profile's `layer`. Fails
    /// `InvalidRequest` if the Action/Trigger combination fails
    /// `config::validate` (a Macro/Stepper naming an unknown library entry, a
    /// `ControllerButton` naming an unknown gamepad button, `ProfileSwitch`
    /// paired with a non-fire-once trigger, `analog_repeat` on a non-Grid
    /// Input, an existing Axis assignment for `(layer, input)`). Assigning a
    /// `Step` Action silently steals that `(stepper, direction)` off its old
    /// Input or Chord (ticket 03/40). Teardown effects: see
    /// `reconcile_teardowns` (a *replacement* that changes the trigger or
    /// Action releases a live individual Toggle on that key; a fresh bind or a
    /// byte-identical GUI re-Save orphans nothing).
    SetBinding {
        input: Input,
        layer: Layer,
        binding: Binding,
    },
    /// Removes a Binding (ordinary passthrough resumes). Fails `NotFound` if
    /// `input` has no Binding on `layer`. Drops any deep Binding the removed
    /// primary carried on `layer` (`drop_orphaned_deep_binding` — a deep
    /// Binding can't outlive its primary). Teardown effects (`StopToggle` for
    /// a live individual Toggle, `StopStage` for the orphaned deep slot): see
    /// `reconcile_teardowns`.
    ClearBinding { input: Input, layer: Layer },
    /// Flips the active Profile's `mode_key_role` (ticket 18). Never fails on
    /// its own account — the active Profile always exists — but `plan` still
    /// returns a `Result` for symmetry with the other mutating `Edit`s and
    /// room for a future validation rule. Teardown effect (`StopToggle(ModeKey)`
    /// on the `Bound → LayerSwitch` transition): see `reconcile_teardowns`.
    SetModeKeyRole { role: ModeKeyRole },
    /// Creates a new, empty Profile (ticket 19) — both Layers present with
    /// empty Binding maps, `mode_key_role` defaulting to `LayerSwitch`, same
    /// shape as the seed `Default` Profile. Fails `AlreadyExists` if `name`
    /// is already taken, or `InvalidRequest` if `name` is empty/whitespace —
    /// validated here (not just client-side by the GUI's own popover) since
    /// any `com.acheron.Daemon` caller can reach this operation.
    CreateProfile { name: String },
    /// Deletes a Profile by name. Fails `NotFound` if it doesn't exist, or
    /// `InvalidRequest` if it's the active Profile — a Config's
    /// `active_profile` must always name a real Profile (the same invariant
    /// `load_or_seed` enforces on startup), so the active one can never be
    /// deleted out from under itself. Since a lone remaining Profile is
    /// always the active one, this also guarantees at least one Profile
    /// always survives.
    DeleteProfile { name: String },
    /// Renames a Profile, updating `active_profile` too if the renamed one
    /// is currently active. Fails `NotFound` if `old_name` doesn't exist,
    /// `AlreadyExists` if `new_name` is already taken by a different
    /// Profile, or `InvalidRequest` if `new_name` is empty/whitespace. A
    /// rename to the same name is not special-cased — it existence-checks
    /// like any other, then rewrites byte-identical `config.toml`.
    RenameProfile { old_name: String, new_name: String },
    /// Switches the active Profile (ticket 19). Force-stops every currently
    /// running Toggle — force-releasing each one's tracked held keys via the
    /// injector (the same mechanism ticket 17's `ActiveToggle::stop` uses) —
    /// before the new Profile's state becomes active, per spec.md's "Toggle
    /// behavior across Layer/Profile switches". Fails `NotFound` if `name`
    /// doesn't name a real Profile.
    SwitchProfile { name: String },
    /// Sets a per-key Actuation/Release point override on the active
    /// Profile (ticket 17 §5/§7). Fails `InvalidRequest` if `input` isn't a
    /// `Grid` variant, or if `release > actuation`.
    SetActuationPoint {
        input: Input,
        actuation: u8,
        release: u8,
    },
    /// Removes a per-key override, reverting that key to the active
    /// Profile's `default_actuation`. Fails `InvalidRequest` if `input`
    /// isn't a `Grid` variant; idempotent otherwise (clearing an
    /// already-unoverridden key succeeds with no effect).
    ClearActuationPoint { input: Input },
    /// Sets the active Profile's `default_actuation` — the Actuation/Release
    /// point every Grid key uses unless it has its own override. Fails
    /// `InvalidRequest` if `release > actuation`.
    SetDefaultActuation { actuation: u8, release: u8 },
    /// Sets the active Profile's `default_deep_actuation` (ticket 08) — the
    /// deep Actuation/Release pair the GUI seeds a *new* deep stage from.
    /// Relies on the trailing `config::validate` for the hysteresis check
    /// (`ConfigError::ReleaseNotBelowActuation`, locus `"default deep"`). No
    /// `Effect` — the runtime never reads this field, only `+ Add deep
    /// stage` does.
    SetDefaultDeepActuation { actuation: u8, release: u8 },
    /// Clears every per-key override on the active Profile in one
    /// `config.toml` rewrite — the GUI's "reset all keys to Profile default"
    /// affordance (ticket 17 §5), not 20 individual `ClearActuationPoint`
    /// operations. Never fails on validation grounds (clearing an
    /// already-empty `actuation_overrides` map is a no-op) — the `Result`
    /// exists only for the same `config.toml`-write-failure case every other
    /// persisting `Edit` carries.
    ResetActuationPoints,
    /// Sets `Config.force_digital` — the user-facing override that forces
    /// Digital Capture mode even when Analog would otherwise unlock (ticket
    /// 17 §4). Persists the flag and (ticket 23, on a successful persist)
    /// triggers the supervisor's live capture-source swap.
    SetForceDigital { force: bool },
    /// Creates a new Macro library entry (ticket 15/51) — a `MacroId` is
    /// derived from `name` via the slug algorithm (`config::unique_macro_id`)
    /// and frozen; `Outcome.created` carries that assigned id back to the
    /// caller, unlike `CreateProfile` (whose identity *is* the caller-chosen
    /// name). Fails `InvalidRequest` if `name` is empty/whitespace. No
    /// `AlreadyExists` case — a slug collision is resolved automatically
    /// (numeric suffix), never rejected.
    CreateMacro {
        name: String,
        steps: Vec<MacroStepDto>,
    },
    /// Renames a Macro — a pure `MacroDef.name` field write, the `MacroId`
    /// itself never changes (ticket 15's Answer: identity is deliberately
    /// decoupled from the editable display name). Fails `NotFound` if
    /// `macro_id` doesn't exist, or `InvalidRequest` if `new_name` is
    /// empty/whitespace. No `AlreadyExists` — display names aren't unique.
    RenameMacro { macro_id: MacroId, new_name: String },
    /// Deletes a Macro. Fails `NotFound` if it doesn't exist, or
    /// `InvalidRequest` if any Binding anywhere (`base`/`held`, any Profile)
    /// still references its `MacroId` — mirrors `DeleteProfile`'s identical
    /// reasoning, making a dangling `macro_id` structurally impossible.
    DeleteMacro { macro_id: MacroId },
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
    },
    /// Creates a new Stepper library entry (ticket 03/54), mirroring
    /// `CreateMacro` exactly — a `StepperId` is derived from `name` via the
    /// slug algorithm (`config::unique_stepper_id`) and frozen;
    /// `Outcome.created` carries that assigned id back to the caller. Fails
    /// `InvalidRequest` if `name` is empty/whitespace. No `AlreadyExists`
    /// case — a slug collision is resolved automatically (numeric suffix),
    /// never rejected.
    CreateStepper {
        name: String,
        items: Vec<StepperItem>,
    },
    /// Renames a Stepper — a pure `StepperDef.name` field write, the
    /// `StepperId` itself never changes, mirroring `RenameMacro` exactly.
    /// Fails `NotFound` if `stepper_id` doesn't exist, or `InvalidRequest`
    /// if `new_name` is empty/whitespace.
    RenameStepper {
        stepper_id: StepperId,
        new_name: String,
    },
    /// Deletes a Stepper. Fails `NotFound` if it doesn't exist, or
    /// `InvalidRequest` if any Binding anywhere (`base`/`held`, any Profile,
    /// either direction) still references its `StepperId` — mirrors
    /// `DeleteMacro`'s identical reasoning, making a dangling `stepper_id`
    /// structurally impossible.
    DeleteStepper { stepper_id: StepperId },
    /// Overwrites a Stepper's item list — a pure `StepperDef.items` field
    /// write, the `StepperId` and `name` untouched, mirroring
    /// `SetMacroSteps` exactly. Fails `NotFound` if `stepper_id` doesn't
    /// exist.
    SetStepperItems {
        stepper_id: StepperId,
        items: Vec<StepperItem>,
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
    /// itself. Teardown effect: see `reconcile_teardowns` (a *replacement*
    /// that changes the trigger or Action force-releases a live Chord Toggle /
    /// firing on that key — otherwise permanently unstoppable once its old
    /// binding is gone; a byte-identical GUI re-Save orphans nothing).
    SetChordBinding {
        inputs: std::collections::BTreeSet<Input>,
        layer: Layer,
        binding: Binding,
    },
    /// Removes a Chord Binding by its exact member set. Fails `NotFound` if
    /// no Chord with exactly that member set exists on `layer`. Teardown
    /// effect (`StopChord(key)` for a live Chord Toggle or Hold-to-repeat
    /// firing this remove orphans — the moment its key leaves `chords(layer)`,
    /// `chord::feed` can no longer route a stop to it): see
    /// `reconcile_teardowns`.
    ClearChordBinding {
        inputs: std::collections::BTreeSet<Input>,
        layer: Layer,
    },
    /// Creates or edits an Axis assignment on the active Profile (ticket
    /// 59/71 — CONTEXT.md: Axis assignment): atomic/immediately-applied/
    /// immediately-persisted, mirroring `SetBinding` exactly but clearing
    /// any existing Binding *and* any Chord membership for `(layer, input)`
    /// atomically alongside the insert (ticket 59 §2's mutual exclusion —
    /// unlike `SetBinding`/`SetChordBinding`, which reject rather than
    /// overwrite an existing Axis assignment there). Fails `InvalidRequest`
    /// if `input` isn't a `Grid` variant. Its only operation-specific effect
    /// is `Effect::RecomputeAxes { layer }`; the teardown effects for the live
    /// runtime slots the clear orphans (`StopToggle` for a primary Binding,
    /// `StopStage` for a deep stage it carried, `StopChord` for every Chord
    /// membership) all fall out of the maps it mutates, via
    /// `reconcile_teardowns`.
    SetAxisAssignment {
        input: Input,
        layer: Layer,
        target: AxisTarget,
    },
    /// Removes an Axis assignment, reverting `input` to ordinary passthrough
    /// on `layer`. Fails `NotFound` if `input` has no Axis assignment there.
    ClearAxisAssignment { input: Input, layer: Layer },
    /// Sets the active Profile's Status LED assignment (CONTEXT.md: Status LED
    /// assignment). The whole triple in one call — one hardware frame drives
    /// all three channels, so no per-channel edit and no channel-name enum on
    /// the wire. Pushes `Effect::AssertStatusLeds` unconditionally, like
    /// `SetActuationPoint` → `RepublishActuation`. Never fails on its own
    /// account (every `(bool, bool, bool)` is valid); the `Result` is the
    /// shared `config.toml`-write-failure case.
    SetStatusLeds {
        orange: bool,
        green: bool,
        blue: bool,
    },
    /// Creates or edits the deep Binding on the active Profile's `layer`
    /// for a dual-stage grid key (tartarus-dual-stage-keys ticket 05 —
    /// CONTEXT.md: Actuation stage). Mirrors `SetBinding` one level deeper.
    /// Relies entirely on the trailing `config::validate(&next)?` (no
    /// inline check) for `DeepStageWithoutPrimary`/`DeepStageMissingConfig`
    /// — sequencing across the primary Binding, the deep Binding, and the
    /// `deep_stages` config is the caller's job, the same way
    /// `SetAxisAssignment` leaves "was there already a Binding here" to
    /// `validate`'s reachable states. Teardown effect (`StopStage(input)` when
    /// this insert *replaces* a differing deep Binding under a live deep slot —
    /// ticket 25 delta 1, matching `SetBinding`-replace → `StopToggle`): see
    /// `reconcile_teardowns`.
    SetDeepStage {
        input: Input,
        layer: Layer,
        binding: Binding,
    },
    /// Removes the deep Binding on `layer`. Fails `NotFound` if `input` has
    /// no deep Binding there. Leaves `deep_stages` (the Actuation/mode config)
    /// untouched — legal and inert with no matching `deep_base`/`deep_held`
    /// entry, same as the primary-removal cascade. Teardown effect
    /// (`StopStage(input)` for a live deep firing this remove orphans — a
    /// Toggle, or a single-key Hold-to-repeat in `value=1` autorepeat — which
    /// would otherwise wait for a next Up that may never come): see
    /// `reconcile_teardowns`.
    ClearDeepStage { input: Input, layer: Layer },
    /// Sets a grid key's deep Actuation/Release point pair on the active
    /// Profile, `.entry(input).or_default()`-creating a fresh
    /// `DeepStageConfig` (mode defaulting to `StagingMode::Handoff`) if none
    /// exists yet. **No `Effect`** — unlike `SetActuationPoint`, nothing
    /// needs a live snapshot pushed to it: `stage::Engine` lives in dispatch
    /// and reads `Config` directly each tick. Safe to move the point under a
    /// key that is *currently* holding a live deep slot (post-release ticket
    /// 20 case B8, decided keep): the next `Engine::update` tick re-thresholds
    /// `rt.deep` against the new point exactly as `SetActuationPoint` does
    /// under a held primary, and a band the point now excludes emits a clean
    /// `ReleaseDeep` + `RepressPrimary` — no orphan is reachable.
    SetDeepActuation {
        input: Input,
        actuation: u8,
        release: u8,
    },
    /// Sets a grid key's Staging mode on the active Profile, the same
    /// `.or_default()`-creation as `SetDeepActuation`. Teardown effect
    /// (`Effect::StopStage(input)` whenever the `.mode` field actually
    /// changes — post-release ticket 23 case B7): a mode change, unlike an
    /// Actuation-point change, can strand Quick-Skip's own per-press phase
    /// machine (`rt.quick_skip`), since the next `advance` would run the *new*
    /// mode's transition table against a phase value the *old* mode wrote.
    /// There is no clean "the next tick reconciles" guarantee here (there is
    /// for `SetDeepActuation`, where `analog::observe` just re-thresholds).
    /// Force-releasing the live deep slot lets the new mode start from a known
    /// state — the next `Engine::update` tick re-adopts at the current Depth
    /// under the new mode with a clean `quick_skip = None`. Only sound because
    /// ticket 23 also makes `stage::Engine::stop_stage` reset-and-keep the
    /// runtime entry: a `SetStagingMode` on the active Layer with the key held
    /// would otherwise hit the same re-fire bug B12 fixes (the deep Binding is
    /// untouched, so the `deep_layer` guard stays true). The effect itself is
    /// derived by `reconcile_teardowns` (ticket 25), not pushed here.
    SetStagingMode { input: Input, mode: StagingMode },
}

/// Why `DispatchState::tear_down` is running — the one place the "what
/// ephemeral runtime state gets released on a Layer switch / Profile switch /
/// disconnect / Digital-mode flip" matrix lives (`post-release-development`
/// ticket 19, ADR-0010). Each variant's `tear_down` arm names every
/// participant — `axis`, `analog_repeat`, `stage`, `individual` (firings and
/// toggles separately), `chord_machine`, `chord_slots` — with an explicit
/// `//` line for each participant it deliberately leaves alone. Behaviour is
/// exactly today's, transcribed from the four former call sites; turning one
/// of the `//`-marked skips into a real call is ticket 20's job.
///
/// A plain `Copy` data enum, it lives here next to `Effect` (and
/// `CommandError`) so `edit` stays a leaf module — `dispatch` already
/// imports `edit`, not the reverse, and `Effect::TearDown` needs the type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TeardownReason {
    /// Mode key edge under `ModeKeyRole::LayerSwitch` — `active_layer`
    /// flipped. Individual Toggles deliberately survive (a Toggle held
    /// across a Layer switch keeps running — CONTEXT.md Toggle).
    LayerSwitch,
    /// `Edit::SwitchProfile` committed. Strongest sweep: individual
    /// Toggles drain too. Chord Toggles still survive (an active Chord
    /// Toggle survives a Profile switch today — see `SwitchProfile` below).
    ProfileSwitch,
    /// Device reported disconnected.
    Disconnect,
    /// Capture mode flipped to Digital (no Depth).
    CaptureModeToDigital,
}

/// A post-commit effect the caller must run — described here by `plan`,
/// performed by the dispatch task against the runtime state it owns
/// (`dispatch::run_effects` + its private `EffectCtx`). `edit` never runs
/// these itself: it has no injector, no channels, and no async.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Republish the active Profile's resolved Actuation-point snapshot
    /// (ticket 18 §5) into the live-Depth grid task's watch channel.
    RepublishActuation,
    /// Recompute and re-emit every `ABS_*` code the given Layer's
    /// Axis-assignment map touches. A no-op in `run_effects` when `layer`
    /// isn't the currently-active Layer (the check `plan` can't make).
    RecomputeAxes { layer: Layer },
    /// Drop the given Input's live axis contribution from the axis engine.
    ForgetAxisContribution(Input),
    /// Tell the capture supervisor to swap the live capture source (ticket
    /// 23) — `SetForceDigital`'s only side effect.
    SignalCaptureMode(bool),
    /// Force-stop the running Toggle on the given Input, if any. `plan`'s sole
    /// source for this is `reconcile_teardowns` (ADR-0011) — a committed edit
    /// that removes or changes a primary `Binding` under a live individual
    /// Toggle, or flips `mode_key_role` `Bound → LayerSwitch`. `run_effects`'
    /// `stop_toggle` no-ops when nothing is live.
    StopToggle(Input),
    /// Run the dispatch task's one lifecycle-teardown matrix
    /// (`DispatchState::tear_down`) for the given reason — the
    /// Config-commit entry point into it (ticket 19, ADR-0010). Pushed only
    /// by `SwitchProfile` (`TeardownReason::ProfileSwitch`): a Profile switch
    /// mutates `Config`, so its ephemeral teardown — drain individual Toggles
    /// and firings, reset axes, stop Analog-repeats, release deep stages —
    /// must run from `run_effects`, the sole commit point, rather than as a
    /// direct call the way the three momentary-state situations (Layer
    /// switch, disconnect, Digital flip) reach `tear_down`. Replaced the five
    /// separate `StopAllToggles` / `ReleaseAllHolds` / `ResetAxisOutputs` /
    /// `StopAllAnalogRepeats` / `StopAllStages` variants, whose fan-out order
    /// now lives in the `ProfileSwitch` arm of the match. `RepublishActuation`
    /// / `AssertStatusLeds` / `AnnounceProfileChange` stay separate effects.
    TearDown(TeardownReason),
    /// Force-release the given Input's live dual-stage deep slot immediately
    /// (`stage::Engine::stop_stage`, which resets-and-keeps the runtime
    /// entry, suppresses `deep_repeat`, and carries `primary_handed_off`
    /// across the reset so the `feed` path stays in step — post-release
    /// tickets 23, 24). `plan`'s sole source for this is `reconcile_teardowns`
    /// (ADR-0011), which derives it by diffing the active Profile: a
    /// `deep_base`/`deep_held` entry removed (`ClearDeepStage`, or
    /// `ClearBinding` / `SetAxisAssignment` dropping the primary the deep
    /// Binding hung off, via `drop_orphaned_deep_binding`) or *changed*
    /// (`SetDeepStage`-replace — ticket 25 delta 1); or `deep_stages[input]
    /// .mode` changed (`SetStagingMode` — ticket 23 B7, a mid-press mode flip
    /// can strand Quick-Skip's phase machine). An actuation-only
    /// `DeepStageConfig` change (`SetDeepActuation`) re-thresholds cleanly on
    /// the next tick and orphans nothing (ticket 20 B8). Where it is emitted,
    /// a live deep slot must not linger past the Binding or mode backing it,
    /// and can't wait for a next Up that may never come.
    StopStage(Input),
    /// Force-stop the running Chord Toggle *and* force-release any live Chord
    /// Hold-to-repeat firing on `key` — `dispatch`'s `chord_slots.stop_toggle`
    /// then `chord_slots.stop_firing`. `post-release-development` ticket 22
    /// (cases B9 / B11), the Chord-keyspace sibling of `StopStage`. `plan`'s
    /// sole source for this is `reconcile_teardowns` (ADR-0011), which derives
    /// it by diffing the active Profile's `chords_base`/`chords_held`: a Chord
    /// Binding removed (`ClearChordBinding`, or `SetAxisAssignment` clearing a
    /// membership for `(layer, input)`) or changed in trigger or Action
    /// (`SetChordBinding`-replace — a byte-identical GUI re-Save orphans
    /// nothing). Once a Chord's key leaves `chords(layer)` a live
    /// Chord Toggle is permanently unstoppable — it stops only via a fresh
    /// full-member completion routed through `chord::feed`, whose `stopping`
    /// filter iterates `chords.keys()` — and a live Chord firing is stranded
    /// the same way (the completed member's `Up` that would
    /// `ReleaseChordFiring` can't reach it). `run_effects` no-ops when neither
    /// a Toggle nor a firing is present, matching `StopStage`'s contract.
    StopChord(ChordKey),
    /// Reconcile the given Stepper's Daemon-side runtime cursor against the
    /// just-committed `Config` — its list definition changed
    /// (`DeleteStepper` removes it, `SetStepperItems` reshapes it).
    /// `dispatch` drops the cursor if the list is gone or empty, clamps it if
    /// the list shrank, leaves it otherwise (`stepper::Cursors::reconcile`).
    ReconcileStepperCursor(StepperId),
    /// Emit `ActiveProfileChanged(name)`.
    AnnounceProfileChange(String),
    /// Re-assert the active Profile's Status LED assignment on the hardware
    /// (CONTEXT.md: Status LED assignment; ADR-0006). `run_effects` routes it
    /// to dispatch's `push_status_leds(&config)` — the same helper the connect
    /// edge uses. Pushed by `SetStatusLeds` and by `SwitchProfile`.
    AssertStatusLeds,
}

/// The freshly-minted id a `CreateMacro` / `CreateStepper` mints — the D-Bus
/// reply carries it back to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreatedId {
    Macro(MacroId),
    Stepper(StepperId),
}

/// Everything `plan` derives beyond the resulting `Config` itself: the
/// post-commit effects to run, and (for the two create commands) the id to
/// hand back over D-Bus.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Outcome {
    pub(crate) effects: Vec<Effect>,
    /// Set only by `CreateMacro` / `CreateStepper`.
    pub(crate) created: Option<CreatedId>,
}

/// The deep module. Clones `config`, applies `edit` to the clone, runs
/// `config::validate` against the result, and returns the new `Config` by
/// value plus the effects to run. No I/O, no async — rollback is just
/// dropping the clone.
///
/// Operation preconditions (`NotFound`, `AlreadyExists`, "can't delete the
/// active Profile", "still-referenced Macro/Stepper", blank create/rename
/// name) are explicit early-return `Err` in each arm, with their existing
/// messages preserved verbatim. Structural invariants of the resulting
/// `Config` stay in `config::validate`, run once here at the end.
///
/// Each arm collects only its operation-specific effects into `arm_effects`;
/// the teardown `Effect`s a committed edit orphans (`StopToggle` / `StopStage`
/// / `StopChord`) are derived once by `reconcile_teardowns` diffing `config`
/// vs. the result, and lead the returned vec (ADR-0011).
pub(crate) fn plan(config: &Config, edit: Edit) -> Result<(Config, Outcome), CommandError> {
    let mut next = config.clone();
    let mut arm_effects = Vec::new();
    let mut created = None;

    match edit {
        Edit::SetBinding {
            input,
            layer,
            binding,
        } => {
            // Ticket 03's Answer: assigning a Stepper list to a new Input
            // silently moves it off its old one, in either keyspace (ticket
            // 40 widened it to Chords) — at most one Input *or* Chord may
            // carry a given (stepper, direction) at a time.
            if let Action::Step { stepper, direction } = &binding.action {
                let active_profile = next.active_profile.clone();
                take_stepper_direction_elsewhere(
                    &mut next,
                    stepper,
                    *direction,
                    Some((&active_profile, layer, input)),
                );
                take_stepper_direction_elsewhere_from_chords(&mut next, stepper, *direction, None);
            }
            active_profile_mut(&mut next)
                .layer_mut(layer)
                .insert(input, binding);
            // No deep-stage cascade here: *overwriting* a primary Binding
            // leaves a primary in place, so the deep stage stays valid and
            // is deliberately kept (the GUI edits either stage and Saves
            // both — a trigger tweak to the primary must not wipe the deep
            // Binding). Only *removing* the primary orphans the deep stage —
            // see `ClearBinding` below. A replacement primary that would
            // make the deep stage illegal (`analog_repeat`, a Chord member)
            // is already rejected by the trailing `config::validate(&next)`.
            //
            // Teardown effects (a `StopToggle` on a replacement that changes
            // the binding under a live individual Toggle): see
            // `reconcile_teardowns`.
        }
        Edit::ClearBinding { input, layer } => {
            if active_profile_mut(&mut next)
                .layer_mut(layer)
                .remove(&input)
                .is_none()
            {
                return Err(CommandError::NotFound);
            }
            // Removing the primary Binding orphans any deep Binding it carried
            // on this Layer — a deep Binding can never exist without a matching
            // primary (`ConfigError::DeepStageWithoutPrimary`), so drop it from
            // `next` now or the trailing `config::validate` rejects the whole
            // edit. Teardown effects (`StopToggle` for a live individual
            // Toggle, `StopStage` for the orphaned deep slot): see
            // `reconcile_teardowns`.
            drop_orphaned_deep_binding(&mut next, layer, input);
        }
        Edit::SetModeKeyRole { role } => {
            active_profile_mut(&mut next).mode_key_role = role;
            // Teardown effect (`StopToggle(ModeKey)` on the `Bound →
            // LayerSwitch` transition — once `LayerSwitch` takes over,
            // `handle_event` intercepts every `Input::ModeKey` press before
            // the stop-toggle check): see `reconcile_teardowns`.
        }
        Edit::CreateProfile { name } => {
            if next.profiles.contains_key(&name) {
                return Err(CommandError::AlreadyExists);
            }
            next.profiles.insert(name, Profile::default());
        }
        Edit::DeleteProfile { name } => {
            if name == next.active_profile {
                return Err(CommandError::InvalidRequest(
                    "cannot delete the active Profile".to_string(),
                ));
            }
            if profile_switch_references(&next, &name) {
                return Err(CommandError::InvalidRequest(format!(
                    "Profile {name:?} is still referenced by a Profile Switch Binding"
                )));
            }
            if next.profiles.remove(&name).is_none() {
                return Err(CommandError::NotFound);
            }
        }
        Edit::RenameProfile { old_name, new_name } => {
            if !next.profiles.contains_key(&old_name) {
                return Err(CommandError::NotFound);
            }
            if old_name != new_name && next.profiles.contains_key(&new_name) {
                return Err(CommandError::AlreadyExists);
            }
            let profile = next
                .profiles
                .remove(&old_name)
                .expect("just checked old_name exists");
            next.profiles.insert(new_name.clone(), profile);
            if next.active_profile == old_name {
                next.active_profile = new_name.clone();
            }
            cascade_rename_profile_switch_targets(&mut next, &old_name, &new_name);
        }
        Edit::SwitchProfile { name } => {
            if !next.profiles.contains_key(&name) {
                return Err(CommandError::NotFound);
            }
            next.active_profile = name.clone();
            // The whole ephemeral teardown — drain individual Toggles and
            // firings, reset axes, stop Analog-repeats, release deep stages,
            // in that order — is the `ProfileSwitch` arm of dispatch's one
            // lifecycle-teardown matrix (`DispatchState::tear_down`, ticket
            // 19 / ADR-0010). A Profile switch mutates `Config`, so it reaches
            // that matrix as an `Effect` through `run_effects` (the sole
            // commit point), where the three momentary-state situations
            // (Layer switch, disconnect, Digital flip) reach it by direct
            // call. The matrix records the load-bearing survival rules that
            // used to live in this comment: individual Toggles drain on a
            // Profile switch (the strongest sweep) but Chord Toggles survive
            // (`StopAllToggles` only ever drained the individual
            // `Slots<Input>`), and a single-key Hold-to-repeat's bare
            // unbalanced `KeyDown` (`spec-kernel-shaped-repeat.md` §7) is
            // drained here because the incoming Profile's binding for that
            // key may never release it. Safe to run after this firing's own
            // `Edit::SwitchProfile` was already produced: `update_stages` /
            // `stage::Engine::feed` fully complete (and this `Edit` is
            // returned) before `commit_input_edits` ever reaches
            // `edit::apply`, so the triggering firing is never interrupted by
            // its own consequence.
            arm_effects.push(Effect::TearDown(TeardownReason::ProfileSwitch));
            // The new Profile's resolved Actuation snapshot goes out after
            // the teardown — `publish_actuation_snapshot` only re-pushes the
            // actuation watch-channel snapshot to the capture grid task,
            // independent of axis centering and hold draining, so its move
            // from between `ReleaseAllHolds` and `ResetAxisOutputs` to here
            // is behaviour-neutral (ticket 19).
            arm_effects.push(Effect::RepublishActuation);
            // The physical indicator follows the active Profile deterministically
            // (`tartarus-status-leds` ticket 03). Order is irrelevant — the LEDs
            // are independent of Toggles / axes / Analog-repeat.
            arm_effects.push(Effect::AssertStatusLeds);
            arm_effects.push(Effect::AnnounceProfileChange(name));
        }
        Edit::SetActuationPoint {
            input,
            actuation,
            release,
        } => {
            active_profile_mut(&mut next)
                .actuation_overrides
                .insert(input, ActuationPoint { actuation, release });
            arm_effects.push(Effect::RepublishActuation);
        }
        Edit::ClearActuationPoint { input } => {
            active_profile_mut(&mut next)
                .actuation_overrides
                .remove(&input);
            arm_effects.push(Effect::RepublishActuation);
        }
        Edit::SetDefaultActuation { actuation, release } => {
            active_profile_mut(&mut next).default_actuation = ActuationPoint { actuation, release };
            arm_effects.push(Effect::RepublishActuation);
        }
        Edit::SetDefaultDeepActuation { actuation, release } => {
            active_profile_mut(&mut next).default_deep_actuation =
                Some(ActuationPoint { actuation, release });
        }
        Edit::ResetActuationPoints => {
            active_profile_mut(&mut next).actuation_overrides.clear();
            arm_effects.push(Effect::RepublishActuation);
        }
        Edit::SetForceDigital { force } => {
            next.force_digital = force;
            arm_effects.push(Effect::SignalCaptureMode(force));
        }
        Edit::CreateMacro { name, steps } => {
            if name.trim().is_empty() {
                return Err(CommandError::InvalidRequest(
                    "Macro name can't be empty".to_string(),
                ));
            }
            let macro_id = config::unique_macro_id(&next, &name);
            next.macros
                .insert(macro_id.clone(), config::MacroDef { name, steps });
            created = Some(CreatedId::Macro(macro_id));
        }
        Edit::RenameMacro { macro_id, new_name } => {
            if new_name.trim().is_empty() {
                return Err(CommandError::InvalidRequest(
                    "Macro name can't be empty".to_string(),
                ));
            }
            let def = next
                .macros
                .get_mut(&macro_id)
                .ok_or(CommandError::NotFound)?;
            def.name = new_name;
        }
        Edit::DeleteMacro { macro_id } => {
            if macro_references(&next, &macro_id) {
                return Err(CommandError::InvalidRequest(format!(
                    "{macro_id:?} is still referenced by a Macro Binding"
                )));
            }
            if next.macros.remove(&macro_id).is_none() {
                return Err(CommandError::NotFound);
            }
        }
        Edit::SetMacroSteps { macro_id, steps } => {
            let def = next
                .macros
                .get_mut(&macro_id)
                .ok_or(CommandError::NotFound)?;
            def.steps = steps;
        }
        Edit::CreateStepper { name, items } => {
            if name.trim().is_empty() {
                return Err(CommandError::InvalidRequest(
                    "Stepper name can't be empty".to_string(),
                ));
            }
            let stepper_id = config::unique_stepper_id(&next, &name);
            next.steppers
                .insert(stepper_id.clone(), config::StepperDef { name, items });
            created = Some(CreatedId::Stepper(stepper_id));
        }
        Edit::RenameStepper {
            stepper_id,
            new_name,
        } => {
            if new_name.trim().is_empty() {
                return Err(CommandError::InvalidRequest(
                    "Stepper name can't be empty".to_string(),
                ));
            }
            let def = next
                .steppers
                .get_mut(&stepper_id)
                .ok_or(CommandError::NotFound)?;
            def.name = new_name;
        }
        Edit::DeleteStepper { stepper_id } => {
            if stepper_references(&next, &stepper_id) {
                return Err(CommandError::InvalidRequest(format!(
                    "{stepper_id:?} is still referenced by a Step Binding"
                )));
            }
            if next.steppers.remove(&stepper_id).is_none() {
                return Err(CommandError::NotFound);
            }
            // The runtime cursor is Daemon-side-only state — `dispatch`
            // reconciles it against the committed `Config` (here: the list is
            // gone, so the cursor is dropped) so a later `CreateStepper`
            // landing on the same freed slug starts at the list's first item
            // rather than inheriting a stale position.
            arm_effects.push(Effect::ReconcileStepperCursor(stepper_id));
        }
        Edit::SetStepperItems { stepper_id, items } => {
            let def = next
                .steppers
                .get_mut(&stepper_id)
                .ok_or(CommandError::NotFound)?;
            def.items = items;
            // The stored cursor may now point past the new end, or the list
            // may be empty — `dispatch` reconciles it against the committed
            // list (clamp on a shrink, drop when empty). `plan` just names
            // the list that moved; the drop-vs-clamp rule lives in
            // `stepper::Cursors`.
            arm_effects.push(Effect::ReconcileStepperCursor(stepper_id));
        }
        Edit::SetChordBinding {
            inputs,
            layer,
            binding,
        } => {
            let key = ChordKey::new(inputs);
            if let Action::Step { stepper, direction } = &binding.action {
                let active_profile = next.active_profile.clone();
                take_stepper_direction_elsewhere(&mut next, stepper, *direction, None);
                take_stepper_direction_elsewhere_from_chords(
                    &mut next,
                    stepper,
                    *direction,
                    Some((&active_profile, layer, &key)),
                );
            }
            active_profile_mut(&mut next)
                .chords_mut(layer)
                .insert(key, binding);
            // Teardown effect (`StopChord` on a replacement that changes the
            // Chord Binding under a live Chord Toggle / firing — otherwise
            // permanently unstoppable once the old binding is gone): see
            // `reconcile_teardowns`.
        }
        Edit::ClearChordBinding { inputs, layer } => {
            let key = ChordKey::new(inputs);
            if active_profile_mut(&mut next)
                .chords_mut(layer)
                .remove(&key)
                .is_none()
            {
                return Err(CommandError::NotFound);
            }
            // Teardown effect (`StopChord` for a live Chord Toggle or
            // Hold-to-repeat firing this remove orphans — the key is gone from
            // `chords(layer)`, so `chord::feed` can never route a stop to it):
            // see `reconcile_teardowns`.
        }
        Edit::SetAxisAssignment {
            input,
            layer,
            target,
        } => {
            // Ticket 59 §2's mutual exclusion: atomically clear any existing
            // Binding *and* any Chord membership for (layer, input) alongside
            // the insert. `config::validate` rejects the *illegal* end states;
            // the teardown effects for the live runtime slots this clear
            // orphans — `StopToggle` for a primary Binding, `StopStage` for a
            // deep stage that primary carried, `StopChord` for every Chord
            // membership — all fall out of the maps it mutates here, via
            // `reconcile_teardowns`.
            active_profile_mut(&mut next)
                .layer_mut(layer)
                .remove(&input);
            // An axis key can't carry a deep stage (`config::validate` would
            // reject the end state), so a deep Binding the removed primary
            // carried is dropped here exactly as `ClearBinding` does —
            // `SetAxisAssignment`'s `layer_mut(layer).remove` does not trigger
            // the deep drop on its own.
            drop_orphaned_deep_binding(&mut next, layer, input);
            let chords = active_profile_mut(&mut next).chords_mut(layer);
            let member_keys: Vec<ChordKey> = chords
                .keys()
                .filter(|key| key.members().contains(&input))
                .cloned()
                .collect();
            for key in member_keys {
                chords.remove(&key);
            }
            active_profile_mut(&mut next)
                .axis_layer_mut(layer)
                .insert(input, target);
            arm_effects.push(Effect::RecomputeAxes { layer });
        }
        Edit::ClearAxisAssignment { input, layer } => {
            if active_profile_mut(&mut next)
                .axis_layer_mut(layer)
                .remove(&input)
                .is_none()
            {
                return Err(CommandError::NotFound);
            }
            arm_effects.push(Effect::ForgetAxisContribution(input));
            arm_effects.push(Effect::RecomputeAxes { layer });
        }
        Edit::SetStatusLeds {
            orange,
            green,
            blue,
        } => {
            active_profile_mut(&mut next).status_leds = StatusLeds {
                orange,
                green,
                blue,
            };
            arm_effects.push(Effect::AssertStatusLeds);
        }
        Edit::SetDeepStage {
            input,
            layer,
            binding,
        } => {
            active_profile_mut(&mut next)
                .deep_layer_mut(layer)
                .insert(input, binding);
            // Teardown effect (`StopStage` when this insert replaces a
            // differing deep Binding under a live deep slot — delta 1): see
            // `reconcile_teardowns`.
        }
        Edit::ClearDeepStage { input, layer } => {
            if active_profile_mut(&mut next)
                .deep_layer_mut(layer)
                .remove(&input)
                .is_none()
            {
                return Err(CommandError::NotFound);
            }
            // Teardown effect (`StopStage` for a live deep firing this remove
            // orphans — the `deep_layer` entry is gone, so
            // `stage::Engine::update`'s `deep_layer(active_layer).contains_key`
            // guard skips the Input for good): see `reconcile_teardowns`.
        }
        Edit::SetDeepActuation {
            input,
            actuation,
            release,
        } => {
            active_profile_mut(&mut next)
                .deep_stages
                .entry(input)
                .or_default()
                .actuation = ActuationPoint { actuation, release };
        }
        Edit::SetStagingMode { input, mode } => {
            // Teardown effect (`StopStage` when the `.mode` field actually
            // changes under a live deep firing — a mid-press mode flip can
            // strand Quick-Skip's per-press phase machine, so the live deep
            // slot is force-released to start the new mode from a known state;
            // an actuation-only or idempotent re-apply pushes nothing): see
            // `reconcile_teardowns`.
            active_profile_mut(&mut next)
                .deep_stages
                .entry(input)
                .or_default()
                .mode = mode;
        }
    }

    // Every teardown `Effect` the committed edit implies — derived once by
    // diffing the active Profile's runtime-bearing maps before vs. after,
    // rather than pushed arm by arm (ADR-0011). Prepended so a live key is
    // released before the arm's own `RecomputeAxes` / `AssertStatusLeds` /
    // `RepublishActuation` re-asserts state on it.
    let mut effects = reconcile_teardowns(config, &next);
    effects.extend(arm_effects);
    config::validate(&next)?;
    Ok((next, Outcome { effects, created }))
}

/// Every teardown `Effect` a committed edit implies, derived by diffing the
/// active Profile's runtime-bearing maps before vs. after. `plan` calls this
/// once, just before `config::validate(&next)?`; its output is prepended to
/// the arm's own operation-specific effects. Pure and liveness-blind by the
/// same contract the arms had — `run_effects`' `stop_*` handlers no-op when
/// nothing is live (`Effect::StopToggle` / `StopStage` / `StopChord` docs) —
/// so it emits unconditionally on a config diff and lets dispatch sort out
/// what is actually running. The config-edit axis of ADR-0010's lifecycle
/// consolidation (`dispatch::tear_down`).
///
/// Active-Profile only: every binding / Chord / deep / axis / mode edit goes
/// through `active_profile_mut`, and the D-Bus surface has no Profile arg.
/// Returns empty when `before.active_profile != after.active_profile` — the
/// only edit that changes it is `SwitchProfile`, which owns
/// `Effect::TearDown(TeardownReason::ProfileSwitch)`; an active-Profile
/// rename leaves the maps byte-identical.
///
/// Inspects `base`, `held`, `chords_base`, `chords_held`, `deep_base`,
/// `deep_held`, `deep_stages` (the `.mode` field only), and `mode_key_role`.
/// Ignores `axis_base` / `axis_held`, `actuation_overrides`,
/// `default_actuation`, `status_leds`, `lighting`, `brightness`, `macros`,
/// `steppers`.
///
/// | before → after (active Profile) | emit |
/// |---|---|
/// | `base` / `held`[input] removed, or `Binding` differs | `StopToggle(input)` |
/// | `deep_base` / `deep_held`[input] removed, or `Binding` differs *(delta 1)* | `StopStage(input)` |
/// | `chords_base` / `chords_held`[key] removed, or `Binding` differs | `StopChord(key)` |
/// | `deep_stages`[input]`.mode` differs (actuation-only change → nothing) | `StopStage(input)` |
/// | `mode_key_role` `Bound → LayerSwitch` *(delta 2)* | `StopToggle(Input::ModeKey)` |
///
/// Output contract: fixed effect-type order `StopToggle` → `StopStage` →
/// `StopChord`; keys sorted (and de-duplicated) within each type; fully
/// deterministic regardless of `HashMap` iteration order.
pub(crate) fn reconcile_teardowns(before: &Config, after: &Config) -> Vec<Effect> {
    // `SwitchProfile` owns `Effect::TearDown(ProfileSwitch)`; an active-Profile
    // rename leaves the maps byte-identical. Either way this diff has nothing
    // to say — and without the bail it would see the entire active layer
    // "change".
    if before.active_profile != after.active_profile {
        return Vec::new();
    }
    let (Some(was), Some(now)) = (before.active_profile(), after.active_profile()) else {
        return Vec::new();
    };

    let mut toggles: Vec<Input> = Vec::new();
    let mut stages: Vec<Input> = Vec::new();
    let mut chords: Vec<ChordKey> = Vec::new();

    // Primary Bindings: a removed or changed `Binding` orphans a live
    // individual Toggle pinned to that key.
    toggles.extend(removed_or_changed(&was.base, &now.base).copied());
    toggles.extend(removed_or_changed(&was.held, &now.held).copied());

    // Mode key: `Bound → LayerSwitch` — a Toggle can only ever have been
    // started on the Mode key while `Bound`, and once `LayerSwitch` takes over
    // `handle_event` intercepts every `Input::ModeKey` press before the
    // stop-toggle check, so a still-running one becomes unstoppable (delta 2 —
    // a `LayerSwitch → LayerSwitch` re-apply now pushes nothing).
    if was.mode_key_role == ModeKeyRole::Bound && now.mode_key_role == ModeKeyRole::LayerSwitch {
        toggles.push(Input::ModeKey);
    }

    // Deep Bindings: a removed or changed deep `Binding` orphans a live deep
    // slot — `stage::Engine::update`'s `deep_layer(active_layer).contains_key`
    // guard would otherwise skip the Input for good (delta 1 covers the
    // *changed* case, matching `SetBinding`-replace → `StopToggle`).
    stages.extend(removed_or_changed(&was.deep_base, &now.deep_base).copied());
    stages.extend(removed_or_changed(&was.deep_held, &now.deep_held).copied());

    // Staging mode: a `.mode` change under a live deep firing can strand
    // Quick-Skip's per-press phase machine. An actuation-only `DeepStageConfig`
    // change is inert (the next tick re-thresholds cleanly — ticket 20 B8). An
    // absent `deep_stages` entry reads as the default mode, so `SetStagingMode`
    // `.or_default()`-creating a fresh entry with a non-default mode counts as
    // a change (matching the old `entry.mode != mode` guard).
    for input in was.deep_stages.keys().chain(now.deep_stages.keys()) {
        let mode = |p: &Profile| p.deep_stages.get(input).map(|c| c.mode).unwrap_or_default();
        if mode(was) != mode(now) {
            stages.push(*input);
        }
    }

    // Chord Bindings: a removed or changed Chord `Binding` orphans a live
    // Chord Toggle / firing — once the key has left `chords(layer)`,
    // `chord::feed` can never route a stop to it.
    chords.extend(removed_or_changed(&was.chords_base, &now.chords_base).cloned());
    chords.extend(removed_or_changed(&was.chords_held, &now.chords_held).cloned());

    toggles.sort_unstable();
    toggles.dedup();
    stages.sort_unstable();
    stages.dedup();
    chords.sort_unstable_by(|a, b| a.members().cmp(b.members()));
    chords.dedup();

    let mut effects = Vec::with_capacity(toggles.len() + stages.len() + chords.len());
    effects.extend(toggles.into_iter().map(Effect::StopToggle));
    effects.extend(stages.into_iter().map(Effect::StopStage));
    effects.extend(chords.into_iter().map(Effect::StopChord));
    effects
}

/// The keys present in `was` that `now` has either dropped outright or rebound
/// to a different value — the one diff shape `reconcile_teardowns` runs
/// against each of its three `(base, held)` map pairs.
fn removed_or_changed<'a, K, V>(
    was: &'a std::collections::HashMap<K, V>,
    now: &'a std::collections::HashMap<K, V>,
) -> impl Iterator<Item = &'a K>
where
    K: Eq + std::hash::Hash,
    V: PartialEq,
{
    was.iter()
        .filter(move |(key, value)| now.get(key).is_none_or(|current| current != *value))
        .map(|(key, _)| key)
}

/// The thin async wrapper: `plan`, then `config::persist` the planned
/// `Config`, then assign it on success. Supersedes `config::persist_edit` —
/// its snapshot-and-restore collapses to "don't assign on failure", since
/// `plan` never touches the caller's `config` and a failed `persist` leaves
/// it equally untouched.
pub(crate) async fn apply(
    config: &mut Config,
    path: &Path,
    edit: Edit,
) -> Result<Outcome, CommandError> {
    let (next, outcome) = plan(config, edit)?;
    config::persist(&next, path).await?;
    *config = next;
    Ok(outcome)
}

/// `tartarus-dual-stage-keys` ticket 06's "Cascade-delete": `ClearBinding` /
/// `SetAxisAssignment` removing `input`'s primary Binding on `layer` orphans
/// any deep Binding it carried there — a deep Binding can never exist without
/// a matching primary (`ConfigError::DeepStageWithoutPrimary`), so the removal
/// would otherwise be rejected outright by the trailing `config::validate`.
/// Drops the deep Binding from `next` and nothing else — the
/// `Effect::StopStage(input)` for a live deep slot the drop orphans falls out
/// of the `deep_*` diff in `reconcile_teardowns` (ADR-0011). `deep_stages`
/// (the Actuation/mode config) is left untouched — legal and inert with no
/// matching `deep_base`/`deep_held` entry.
fn drop_orphaned_deep_binding(next: &mut Config, layer: Layer, input: Input) {
    active_profile_mut(next)
        .deep_layer_mut(layer)
        .remove(&input);
}

/// The `Default` Profile always exists — `load_or_seed` refuses to start a
/// `Config` whose `active_profile` doesn't name a real Profile.
fn active_profile_mut(config: &mut Config) -> &mut Profile {
    config
        .active_profile_mut()
        .expect("load_or_seed validates active_profile names a real profile")
}

/// Every `Action::ProfileSwitch { target }` across every Profile's Base/Held
/// Binding map that targets `old_name` is repointed at `new_name` (ticket
/// 34) — a rename must not silently leave a dangling or wrong reference
/// behind.
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
/// while this is true (ticket 15/51/40).
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
/// refuses while this is true (ticket 03/54/40).
fn stepper_references(config: &Config, stepper_id: &StepperId) -> bool {
    config.profiles.values().any(|profile| {
        config::profile_all_bindings(profile)
            .any(|binding| matches!(&binding.action, Action::Step { stepper, .. } if stepper == stepper_id))
    })
}

/// Removes every other Binding, across every Profile/Layer, whose `Action`
/// is `Action::Step { stepper, direction }` matching the one being set —
/// ticket 03's Answer: "assigning it to a new pair silently moves it off its
/// old one." `except` (the Input currently being written) is left untouched
/// even if it already matches; `None` steals from every matching Input.
/// `take_stepper_direction_elsewhere_from_chords` is the exact mirror for a
/// Chord's own Step action — both keyspaces are swept together whenever
/// either kind of caller claims one.
fn take_stepper_direction_elsewhere(
    config: &mut Config,
    stepper: &StepperId,
    direction: StepDirection,
    except: Option<(&str, Layer, Input)>,
) {
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
                bindings.remove(&input);
            }
        }
    }
}

/// `take_stepper_direction_elsewhere`'s exact mirror for a Profile's Chord
/// Bindings (ticket 40). `except` is the `ChordKey` currently being written,
/// left untouched even if it already matches; `None` steals from every
/// matching Chord.
fn take_stepper_direction_elsewhere_from_chords(
    config: &mut Config,
    stepper: &StepperId,
    direction: StepDirection,
    except: Option<(&str, Layer, &ChordKey)>,
) {
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
                chords.remove(&key);
            }
        }
    }
}

/// The reason an `Edit` was rejected — produced entirely by `plan` (each
/// operation precondition, an explicit early-return `Err`) and, via the `From`
/// below, by `config::validate` (each structural invariant of the resulting
/// `Config`). Deliberately narrower than the D-Bus surface's
/// `com.acheron.Daemon.Error.*` set (`dbus::DaemonError`) — malformed wire
/// payloads are rejected before an `Edit` is ever built. `InvalidRequest` maps
/// onto the wire's `InvalidBinding` bucket (issue 08's "small named set", not
/// one error per validation rule) since it's the closest fit for "the request
/// itself is malformed/disallowed," even for a non-Binding request like
/// deleting the active Profile.
#[derive(Debug)]
pub enum CommandError {
    NotFound,
    AlreadyExists,
    InvalidRequest(String),
    IoError(String),
}

impl From<crate::config::ConfigError> for CommandError {
    fn from(err: crate::config::ConfigError) -> Self {
        // Ticket 04: `config::validate` now feeds this conversion too, so a
        // genuine disk-write failure stays `IoError` while every structural
        // invariant violation becomes `InvalidRequest` (→ the wire's
        // `InvalidBinding` bucket → the GUI's `InvalidBindingError`).
        let message = err.to_string();
        match err {
            crate::config::ConfigError::Io(_) => CommandError::IoError(message),
            _ => CommandError::InvalidRequest(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Action, AxisTarget, Binding, DEFAULT_PROFILE_NAME, MacroDef, MacroStepDto, Modifiers,
        Profile, StepDirection, StepperDef, StepperItem, TriggerMode,
    };
    use evdev::KeyCode;
    use std::collections::BTreeSet;

    fn seed() -> Config {
        Config::seed()
    }

    fn active(config: &mut Config) -> &mut Profile {
        let name = config.active_profile.clone();
        config.profiles.get_mut(&name).unwrap()
    }

    fn keypress() -> Binding {
        Binding {
            trigger: TriggerMode::HoldToRepeat,
            action: Action::Keypress {
                modifiers: Modifiers::default(),
                key: KeyCode::KEY_A,
            },
        }
    }

    fn chord(members: impl IntoIterator<Item = Input>) -> BTreeSet<Input> {
        members.into_iter().collect()
    }

    fn step(stepper: &str, direction: StepDirection) -> Binding {
        Binding {
            trigger: TriggerMode::FireOnce,
            action: Action::Step {
                stepper: StepperId::from(stepper),
                direction,
            },
        }
    }

    fn with_stepper(config: &mut Config, id: &str) -> StepperId {
        let sid = StepperId::from(id);
        config.steppers.insert(
            sid.clone(),
            StepperDef {
                name: id.to_string(),
                items: vec![
                    StepperItem::Key {
                        key: KeyCode::KEY_1,
                        modifiers: Modifiers::default(),
                    },
                    StepperItem::Key {
                        key: KeyCode::KEY_2,
                        modifiers: Modifiers::default(),
                    },
                ],
            },
        );
        sid
    }

    fn with_macro(config: &mut Config, id: &str) -> MacroId {
        let mid = MacroId::from(id);
        config.macros.insert(
            mid.clone(),
            MacroDef {
                name: id.to_string(),
                steps: vec![MacroStepDto::KeyDown(KeyCode::KEY_A)],
            },
        );
        mid
    }

    fn plan_ok(config: &Config, edit: Edit) -> (Config, Outcome) {
        plan(config, edit).expect("plan must succeed")
    }

    fn plan_err(config: &Config, edit: Edit) -> CommandError {
        plan(config, edit).expect_err("plan must reject the edit")
    }

    // --- resulting `Config` on the success path -------------------------------

    #[test]
    fn set_binding_inserts_into_the_active_profiles_layer() {
        let (next, outcome) = plan_ok(
            &seed(),
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: keypress(),
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)],
            keypress()
        );
        assert!(outcome.effects.is_empty());
    }

    #[test]
    fn set_binding_overwriting_a_primary_keeps_its_live_deep_binding() {
        // A primary Binding stays in place across an *overwrite*, so its
        // deep stage stays valid and is deliberately kept — the GUI edits
        // either stage and Saves both, and a trigger tweak to the primary
        // must not wipe the deep Binding. Only *removing* the primary
        // (`ClearBinding`) orphans the deep stage.
        let mut config = with_primary_and_deep_stage(Input::Grid(1, 1));
        active(&mut config)
            .deep_base
            .insert(Input::Grid(1, 1), keypress());

        let (next, outcome) = plan_ok(
            &config,
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: Binding {
                    trigger: TriggerMode::Toggle,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: KeyCode::KEY_B,
                    },
                },
            },
        );
        assert!(
            next.profiles[DEFAULT_PROFILE_NAME]
                .deep_base
                .contains_key(&Input::Grid(1, 1)),
            "the deep Binding must survive an overwrite of its primary"
        );
        // The deep stage is *kept* (no `StopStage`), but the changed primary
        // still releases a live individual Toggle on the key (ticket 22 B10).
        assert_eq!(
            outcome.effects,
            vec![Effect::StopToggle(Input::Grid(1, 1))],
            "a changed overwrite releases the individual Toggle but keeps the deep stage"
        );
    }

    #[test]
    fn set_binding_with_a_step_action_steals_the_direction_off_its_old_input() {
        let mut config = seed();
        with_stepper(&mut config, "wep");
        active(&mut config)
            .base
            .insert(Input::Grid(1, 1), step("wep", StepDirection::Forward));

        let (next, outcome) = plan_ok(
            &config,
            Edit::SetBinding {
                input: Input::Grid(2, 2),
                layer: Layer::Base,
                binding: step("wep", StepDirection::Forward),
            },
        );
        let base = &next.profiles[DEFAULT_PROFILE_NAME].base;
        assert!(!base.contains_key(&Input::Grid(1, 1)), "old owner cleared");
        assert!(base.contains_key(&Input::Grid(2, 2)), "new owner set");
        // The steal drops the old owner's `base` entry, so `reconcile_teardowns`
        // emits a `StopToggle` for it (ADR-0011: a structural consequence of the
        // map mutation — inert, since a `Step` Action is `FireOnce` and can hold
        // no live Toggle).
        assert_eq!(outcome.effects, vec![Effect::StopToggle(Input::Grid(1, 1))]);
    }

    #[test]
    fn set_binding_and_set_chord_binding_steal_a_step_direction_across_both_keyspaces() {
        let mut config = seed();
        with_stepper(&mut config, "wep");
        // A Chord currently owns (wep, Forward)...
        let members = chord([Input::Grid(1, 1), Input::Grid(1, 2)]);
        config
            .profiles
            .get_mut(DEFAULT_PROFILE_NAME)
            .unwrap()
            .chords_base
            .insert(
                ChordKey::new(members.clone()),
                step("wep", StepDirection::Forward),
            );

        // ...a plain SetBinding for the same (stepper, direction) takes it.
        let (next, _) = plan_ok(
            &config,
            Edit::SetBinding {
                input: Input::Grid(3, 3),
                layer: Layer::Base,
                binding: step("wep", StepDirection::Forward),
            },
        );
        assert!(next.profiles[DEFAULT_PROFILE_NAME].chords_base.is_empty());

        // ...and the reverse: a SetChordBinding takes it back off the Input.
        let mut with_input = seed();
        with_stepper(&mut with_input, "wep");
        with_input
            .profiles
            .get_mut(DEFAULT_PROFILE_NAME)
            .unwrap()
            .base
            .insert(Input::Grid(3, 3), step("wep", StepDirection::Forward));
        let (back, _) = plan_ok(
            &with_input,
            Edit::SetChordBinding {
                inputs: members,
                layer: Layer::Base,
                binding: step("wep", StepDirection::Forward),
            },
        );
        assert!(
            !back.profiles[DEFAULT_PROFILE_NAME]
                .base
                .contains_key(&Input::Grid(3, 3))
        );
    }

    #[test]
    fn delete_macro_is_rejected_when_only_a_chord_still_references_it() {
        let mut config = seed();
        let mid = with_macro(&mut config, "m");
        config
            .profiles
            .get_mut(DEFAULT_PROFILE_NAME)
            .unwrap()
            .chords_base
            .insert(
                ChordKey::new(chord([Input::Grid(1, 1), Input::Grid(1, 2)])),
                Binding {
                    trigger: TriggerMode::FireOnce,
                    action: Action::Macro {
                        macro_id: mid.clone(),
                    },
                },
            );
        assert!(matches!(
            plan_err(&config, Edit::DeleteMacro { macro_id: mid }),
            CommandError::InvalidRequest(_)
        ));
    }

    #[test]
    fn clear_binding_removes_it_and_rejects_an_absent_one() {
        let mut config = seed();
        active(&mut config)
            .base
            .insert(Input::Grid(1, 1), keypress());
        let (next, _) = plan_ok(
            &config,
            Edit::ClearBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
            },
        );
        assert!(next.profiles[DEFAULT_PROFILE_NAME].base.is_empty());

        assert!(matches!(
            plan_err(
                &seed(),
                Edit::ClearBinding {
                    input: Input::Grid(1, 1),
                    layer: Layer::Base,
                }
            ),
            CommandError::NotFound
        ));
    }

    #[test]
    fn clear_binding_removing_a_primary_with_a_live_deep_binding_cascades_it_away() {
        let mut config = with_primary_and_deep_stage(Input::Grid(1, 1));
        active(&mut config)
            .deep_base
            .insert(Input::Grid(1, 1), keypress());

        let (next, outcome) = plan_ok(
            &config,
            Edit::ClearBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
            },
        );
        assert!(next.profiles[DEFAULT_PROFILE_NAME].base.is_empty());
        assert!(
            next.profiles[DEFAULT_PROFILE_NAME].deep_base.is_empty(),
            "the orphaned deep Binding must be cascaded away"
        );
        assert!(!next.profiles[DEFAULT_PROFILE_NAME].deep_stages.is_empty());
        // `StopToggle` (ticket 22 — one rule across every primary-removing
        // arm) then the deep-stage cascade's `StopStage`.
        assert_eq!(
            outcome.effects,
            vec![
                Effect::StopToggle(Input::Grid(1, 1)),
                Effect::StopStage(Input::Grid(1, 1)),
            ]
        );
    }

    #[test]
    fn set_binding_accepts_the_valid_shapes_config_validate_allows() {
        // Grid Input + AnalogRepeat: only the *non-grid* reject path is a
        // dedicated table row, so pin the accept path too.
        let (next, _) = plan_ok(
            &seed(),
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: Binding {
                    trigger: TriggerMode::AnalogRepeat,
                    action: Action::Keypress {
                        modifiers: Modifiers::default(),
                        key: KeyCode::KEY_A,
                    },
                },
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)].trigger,
            TriggerMode::AnalogRepeat
        );

        // A ControllerButton in the gamepad allowlist is accepted.
        let (next, _) = plan_ok(
            &seed(),
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: Binding {
                    trigger: TriggerMode::HoldToRepeat,
                    action: Action::ControllerButton {
                        button: KeyCode::BTN_SOUTH,
                    },
                },
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)].action,
            Action::ControllerButton {
                button: KeyCode::BTN_SOUTH
            }
        );
    }

    #[test]
    fn set_mode_key_role_flips_the_field_and_emits_stop_toggle_only_on_bound_to_layer_switch() {
        let mut config = seed();
        active(&mut config).mode_key_role = crate::config::ModeKeyRole::Bound;
        // A Held-layer binding retained while `Bound` makes it unreachable
        // (config serde `skip_serializing_if`) must survive the role flip —
        // `plan` writes only `mode_key_role`, nothing else.
        active(&mut config)
            .held
            .insert(Input::Grid(1, 1), keypress());

        let (next, outcome) = plan_ok(
            &config,
            Edit::SetModeKeyRole {
                role: crate::config::ModeKeyRole::LayerSwitch,
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].mode_key_role,
            crate::config::ModeKeyRole::LayerSwitch
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].held[&Input::Grid(1, 1)],
            keypress(),
            "Held bindings survive the role flip"
        );
        assert_eq!(outcome.effects, vec![Effect::StopToggle(Input::ModeKey)]);

        // `LayerSwitch → Bound` pushes nothing.
        let (_, outcome) = plan_ok(
            &seed(),
            Edit::SetModeKeyRole {
                role: crate::config::ModeKeyRole::Bound,
            },
        );
        assert!(outcome.effects.is_empty());

        // Delta 2 (ticket 25): a `LayerSwitch → LayerSwitch` re-apply is a
        // config no-op and now pushes nothing — the old per-arm `if role ==
        // LayerSwitch` guard pushed a harmless `StopToggle(ModeKey)` here.
        let (_, outcome) = plan_ok(
            &seed(),
            Edit::SetModeKeyRole {
                role: crate::config::ModeKeyRole::LayerSwitch,
            },
        );
        assert!(
            outcome.effects.is_empty(),
            "a LayerSwitch → LayerSwitch re-apply orphans nothing"
        );
    }

    #[test]
    fn create_and_delete_and_rename_profile_round_trip() {
        let (with_gaming, _) = plan_ok(
            &seed(),
            Edit::CreateProfile {
                name: "Gaming".to_string(),
            },
        );
        assert!(with_gaming.profiles.contains_key("Gaming"));

        let (renamed, _) = plan_ok(
            &with_gaming,
            Edit::RenameProfile {
                old_name: "Gaming".to_string(),
                new_name: "Editing".to_string(),
            },
        );
        assert!(!renamed.profiles.contains_key("Gaming"));
        assert!(renamed.profiles.contains_key("Editing"));
        assert_eq!(renamed.active_profile, DEFAULT_PROFILE_NAME);

        let (without_editing, _) = plan_ok(
            &renamed,
            Edit::DeleteProfile {
                name: "Editing".to_string(),
            },
        );
        assert!(!without_editing.profiles.contains_key("Editing"));
    }

    #[test]
    fn rename_profile_follows_active_profile_and_cascades_switch_targets() {
        let mut config = seed();
        plan(
            &config,
            Edit::CreateProfile {
                name: "Gaming".to_string(),
            },
        )
        .map(|(c, _)| config = c)
        .unwrap();
        active(&mut config).base.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::ProfileSwitch {
                    target: "Gaming".to_string(),
                },
            },
        );
        config.active_profile = "Gaming".to_string();

        let (next, _) = plan_ok(
            &config,
            Edit::RenameProfile {
                old_name: "Gaming".to_string(),
                new_name: "Renamed".to_string(),
            },
        );
        assert_eq!(next.active_profile, "Renamed");
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].base[&Input::Grid(1, 1)].action,
            Action::ProfileSwitch {
                target: "Renamed".to_string()
            }
        );
    }

    #[test]
    fn switch_profile_sets_active_and_emits_its_ordered_effect_chain() {
        let mut config = seed();
        config
            .profiles
            .insert("Gaming".to_string(), Profile::default());

        let (next, outcome) = plan_ok(
            &config,
            Edit::SwitchProfile {
                name: "Gaming".to_string(),
            },
        );
        assert_eq!(next.active_profile, "Gaming");
        // The five former teardown effects collapsed into one
        // `TearDown(ProfileSwitch)` (ticket 19); its fan-out order is asserted
        // in `dispatch`'s `tear_down` matrix tests. `RepublishActuation` now
        // trails the teardown rather than sitting mid-chain — a
        // behaviour-neutral move.
        assert_eq!(
            outcome.effects,
            vec![
                Effect::TearDown(TeardownReason::ProfileSwitch),
                Effect::RepublishActuation,
                Effect::AssertStatusLeds,
                Effect::AnnounceProfileChange("Gaming".to_string()),
            ]
        );
    }

    #[test]
    fn set_status_leds_writes_the_active_profiles_triple_and_asks_for_an_assert() {
        let (next, outcome) = plan_ok(
            &seed(),
            Edit::SetStatusLeds {
                orange: true,
                green: false,
                blue: true,
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].status_leds,
            crate::config::StatusLeds {
                orange: true,
                green: false,
                blue: true,
            }
        );
        assert_eq!(outcome.effects, vec![Effect::AssertStatusLeds]);
    }

    #[test]
    fn actuation_edits_all_republish_the_snapshot() {
        for edit in [
            Edit::SetActuationPoint {
                input: Input::Grid(1, 1),
                actuation: 200,
                release: 100,
            },
            Edit::ClearActuationPoint {
                input: Input::Grid(1, 1),
            },
            Edit::SetDefaultActuation {
                actuation: 200,
                release: 100,
            },
            Edit::ResetActuationPoints,
        ] {
            let (_, outcome) = plan_ok(&seed(), edit);
            assert_eq!(outcome.effects, vec![Effect::RepublishActuation]);
        }
    }

    #[test]
    fn set_default_deep_actuation_records_it_with_no_effect() {
        // Ticket 08: a GUI-authoring seed — persisted, but the runtime never
        // reads it, so no `Effect` (unlike `SetDefaultActuation`'s
        // `RepublishActuation`).
        let (next, outcome) = plan_ok(
            &seed(),
            Edit::SetDefaultDeepActuation {
                actuation: 240,
                release: 205,
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].default_deep_actuation,
            Some(ActuationPoint {
                actuation: 240,
                release: 205,
            })
        );
        assert!(outcome.effects.is_empty());
    }

    #[test]
    fn set_force_digital_writes_the_flag_and_signals_the_supervisor() {
        let (next, outcome) = plan_ok(&seed(), Edit::SetForceDigital { force: true });
        assert!(next.force_digital);
        assert_eq!(outcome.effects, vec![Effect::SignalCaptureMode(true)]);
    }

    #[test]
    fn create_macro_mints_an_id_and_hands_it_back() {
        let (next, outcome) = plan_ok(
            &seed(),
            Edit::CreateMacro {
                name: "My Macro".to_string(),
                steps: vec![],
            },
        );
        let Some(CreatedId::Macro(id)) = outcome.created else {
            panic!("CreateMacro must set Outcome.created");
        };
        assert!(next.macros.contains_key(&id));
    }

    #[test]
    fn create_stepper_mints_an_id_and_hands_it_back() {
        let (next, outcome) = plan_ok(
            &seed(),
            Edit::CreateStepper {
                name: "My Stepper".to_string(),
                items: vec![],
            },
        );
        let Some(CreatedId::Stepper(id)) = outcome.created else {
            panic!("CreateStepper must set Outcome.created");
        };
        assert!(next.steppers.contains_key(&id));
    }

    #[test]
    fn set_binding_targets_the_named_layer_and_clear_actuation_on_a_non_grid_input_is_a_no_op() {
        let (next, _) = plan_ok(
            &seed(),
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Held,
                binding: keypress(),
            },
        );
        let profile = &next.profiles[DEFAULT_PROFILE_NAME];
        assert!(
            !profile.base.contains_key(&Input::Grid(1, 1)),
            "Base untouched"
        );
        assert!(profile.held.contains_key(&Input::Grid(1, 1)));

        // `reject_non_grid_input` is gone (ticket 04): clearing an override
        // that was never there — a non-grid key never has one — is a silent
        // no-op success, not a rejection.
        let (unchanged, outcome) = plan_ok(
            &seed(),
            Edit::ClearActuationPoint {
                input: Input::ModeKey,
            },
        );
        assert_eq!(unchanged, seed());
        assert_eq!(outcome.effects, vec![Effect::RepublishActuation]);
    }

    #[test]
    fn delete_stepper_emits_a_reconcile_effect_for_its_cursor() {
        let mut config = seed();
        let sid = with_stepper(&mut config, "wep");
        let (_, outcome) = plan_ok(
            &config,
            Edit::DeleteStepper {
                stepper_id: sid.clone(),
            },
        );
        assert_eq!(outcome.effects, vec![Effect::ReconcileStepperCursor(sid)]);
    }

    #[test]
    fn set_stepper_items_emits_a_reconcile_effect_for_the_list_shrunk_or_emptied() {
        let mut config = seed();
        let sid = with_stepper(&mut config, "wep");

        // The drop-vs-clamp decision now lives in `stepper::Cursors`
        // (covered by `stepper::tests`); `plan` just names the list that
        // moved, the same effect for a shrink and for an empty list.
        for items in [
            vec![StepperItem::Key {
                key: KeyCode::KEY_1,
                modifiers: Modifiers::default(),
            }],
            vec![],
        ] {
            let (_, outcome) = plan_ok(
                &config,
                Edit::SetStepperItems {
                    stepper_id: sid.clone(),
                    items,
                },
            );
            assert_eq!(
                outcome.effects,
                vec![Effect::ReconcileStepperCursor(sid.clone())]
            );
        }
    }

    #[test]
    fn rename_and_set_steps_are_pure_field_writes() {
        let mut config = seed();
        let mid = with_macro(&mut config, "m");
        let (next, _) = plan_ok(
            &config,
            Edit::RenameMacro {
                macro_id: mid.clone(),
                new_name: "renamed".to_string(),
            },
        );
        assert_eq!(next.macros[&mid].name, "renamed");
        let (next, _) = plan_ok(
            &config,
            Edit::SetMacroSteps {
                macro_id: mid.clone(),
                steps: vec![MacroStepDto::KeyUp(KeyCode::KEY_B)],
            },
        );
        assert_eq!(
            next.macros[&mid].steps,
            vec![MacroStepDto::KeyUp(KeyCode::KEY_B)]
        );
    }

    #[test]
    fn set_chord_binding_inserts_by_member_set_and_clear_removes_it() {
        let members = [Input::Grid(1, 1), Input::Grid(1, 2)];
        let (next, _) = plan_ok(
            &seed(),
            Edit::SetChordBinding {
                inputs: chord(members),
                layer: Layer::Base,
                binding: keypress(),
            },
        );
        assert_eq!(next.profiles[DEFAULT_PROFILE_NAME].chords_base.len(), 1);

        let (cleared, _) = plan_ok(
            &next,
            Edit::ClearChordBinding {
                inputs: chord(members),
                layer: Layer::Base,
            },
        );
        assert!(
            cleared.profiles[DEFAULT_PROFILE_NAME]
                .chords_base
                .is_empty()
        );
    }

    fn toggle_keypress(key: KeyCode) -> Binding {
        Binding {
            trigger: TriggerMode::Toggle,
            action: Action::Keypress {
                modifiers: Modifiers::default(),
                key,
            },
        }
    }

    #[test]
    fn clear_chord_binding_pushes_stop_chord_after_a_successful_remove() {
        // Ticket 22 B11: a live Chord Toggle / firing on this key becomes
        // unstoppable the moment its key leaves `chords(layer)` — the remove
        // must push the force-release, unconditionally (mirroring
        // `ClearDeepStage` → `StopStage`).
        let members = [Input::Grid(1, 1), Input::Grid(1, 2)];
        let mut config = seed();
        active(&mut config)
            .chords_base
            .insert(ChordKey::new(chord(members)), keypress());
        let key = ChordKey::new(chord(members));

        let (_, outcome) = plan_ok(
            &config,
            Edit::ClearChordBinding {
                inputs: chord(members),
                layer: Layer::Base,
            },
        );
        assert_eq!(outcome.effects, vec![Effect::StopChord(key)]);
    }

    #[test]
    fn set_chord_binding_replacing_a_differing_binding_pushes_stop_chord_an_identical_re_save_does_not()
     {
        let members = [Input::Grid(1, 1), Input::Grid(1, 2)];
        let key = ChordKey::new(chord(members));
        let mut config = seed();
        active(&mut config)
            .chords_base
            .insert(key.clone(), keypress());

        // A replacement that changes the Action → `StopChord`.
        let (_, outcome) = plan_ok(
            &config,
            Edit::SetChordBinding {
                inputs: chord(members),
                layer: Layer::Base,
                binding: toggle_keypress(KeyCode::KEY_B),
            },
        );
        assert_eq!(outcome.effects, vec![Effect::StopChord(key)]);

        // A byte-identical re-Save (the GUI does this) → nothing.
        let (_, outcome) = plan_ok(
            &config,
            Edit::SetChordBinding {
                inputs: chord(members),
                layer: Layer::Base,
                binding: keypress(),
            },
        );
        assert!(
            outcome.effects.is_empty(),
            "an identical re-Save must not drop a live Chord Toggle"
        );
    }

    #[test]
    fn set_binding_replacing_a_differing_binding_pushes_stop_toggle_a_fresh_bind_or_re_save_does_not()
     {
        let mut config = seed();
        active(&mut config)
            .base
            .insert(Input::Grid(1, 1), keypress());

        // A replacement that changes the Action → `StopToggle` (ticket 22 B10).
        let (_, outcome) = plan_ok(
            &config,
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: toggle_keypress(KeyCode::KEY_B),
            },
        );
        assert_eq!(outcome.effects, vec![Effect::StopToggle(Input::Grid(1, 1))]);

        // A byte-identical re-Save → nothing.
        let (_, outcome) = plan_ok(
            &config,
            Edit::SetBinding {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: keypress(),
            },
        );
        assert!(
            outcome.effects.is_empty(),
            "an identical re-Save is a no-op"
        );

        // A fresh bind (nothing there before) → nothing.
        let (_, outcome) = plan_ok(
            &seed(),
            Edit::SetBinding {
                input: Input::Grid(2, 2),
                layer: Layer::Base,
                binding: keypress(),
            },
        );
        assert!(
            outcome.effects.is_empty(),
            "a fresh bind has no live slot to orphan"
        );
    }

    #[test]
    fn set_axis_assignment_over_a_key_that_had_binding_chord_membership_and_a_deep_stage() {
        // Ticket 22 B9: the axis reassignment atomically clears the primary
        // Binding, the deep stage it carried, and every Chord membership —
        // each orphaning live runtime state `config::validate` won't flag.
        // `plan` pushes the matching teardown effect for all three, then the
        // recompute. (The starting state — a Chord member that also carries a
        // deep stage — isn't edit-reachable, but `plan` never validates its
        // *input*, only the result.)
        let members = [Input::Grid(1, 1), Input::Grid(1, 2)];
        let key = ChordKey::new(chord(members));
        let mut config = seed();
        active(&mut config)
            .base
            .insert(Input::Grid(1, 1), keypress());
        active(&mut config)
            .deep_base
            .insert(Input::Grid(1, 1), keypress());
        active(&mut config).deep_stages.insert(
            Input::Grid(1, 1),
            crate::config::DeepStageConfig {
                actuation: ActuationPoint {
                    actuation: 220,
                    release: 200,
                },
                mode: StagingMode::Handoff,
            },
        );
        active(&mut config)
            .chords_base
            .insert(key.clone(), keypress());

        let (next, outcome) = plan_ok(
            &config,
            Edit::SetAxisAssignment {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                target: AxisTarget::LeftTrigger,
            },
        );
        let profile = &next.profiles[DEFAULT_PROFILE_NAME];
        assert!(!profile.base.contains_key(&Input::Grid(1, 1)));
        assert!(!profile.deep_base.contains_key(&Input::Grid(1, 1)));
        assert!(profile.chords_base.is_empty());
        assert_eq!(
            outcome.effects,
            vec![
                Effect::StopToggle(Input::Grid(1, 1)),
                Effect::StopStage(Input::Grid(1, 1)),
                Effect::StopChord(key),
                Effect::RecomputeAxes { layer: Layer::Base },
            ]
        );
    }

    #[test]
    fn set_axis_assignment_clears_a_colliding_binding_and_asks_for_a_recompute() {
        let mut config = seed();
        active(&mut config)
            .base
            .insert(Input::Grid(1, 1), keypress());

        let (next, outcome) = plan_ok(
            &config,
            Edit::SetAxisAssignment {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                target: AxisTarget::LeftTrigger,
            },
        );
        let profile = &next.profiles[DEFAULT_PROFILE_NAME];
        assert!(!profile.base.contains_key(&Input::Grid(1, 1)));
        assert_eq!(
            profile.axis_base[&Input::Grid(1, 1)],
            AxisTarget::LeftTrigger
        );
        // The removed primary Binding also releases a live individual Toggle
        // on the key (ticket 22 B9), ahead of the recompute.
        assert_eq!(
            outcome.effects,
            vec![
                Effect::StopToggle(Input::Grid(1, 1)),
                Effect::RecomputeAxes { layer: Layer::Base },
            ]
        );
    }

    #[test]
    fn clear_axis_assignment_forgets_the_contribution_then_recomputes() {
        let mut config = seed();
        active(&mut config)
            .axis_base
            .insert(Input::Grid(1, 1), AxisTarget::LeftTrigger);

        let (next, outcome) = plan_ok(
            &config,
            Edit::ClearAxisAssignment {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
            },
        );
        assert!(next.profiles[DEFAULT_PROFILE_NAME].axis_base.is_empty());
        assert_eq!(
            outcome.effects,
            vec![
                Effect::ForgetAxisContribution(Input::Grid(1, 1)),
                Effect::RecomputeAxes { layer: Layer::Base },
            ]
        );

        assert!(matches!(
            plan_err(
                &seed(),
                Edit::ClearAxisAssignment {
                    input: Input::Grid(1, 1),
                    layer: Layer::Base,
                }
            ),
            CommandError::NotFound
        ));
    }

    // --- deep-stage Edit variants (tartarus-dual-stage-keys ticket 05) --------

    fn with_primary_and_deep_stage(input: Input) -> Config {
        let mut config = seed();
        active(&mut config).base.insert(input, keypress());
        active(&mut config).deep_stages.insert(
            input,
            crate::config::DeepStageConfig {
                actuation: ActuationPoint {
                    actuation: 220,
                    release: 200,
                },
                mode: StagingMode::Handoff,
            },
        );
        config
    }

    #[test]
    fn set_deep_stage_inserts_into_the_active_profiles_deep_layer_and_relies_on_validate() {
        let config = with_primary_and_deep_stage(Input::Grid(1, 1));
        let (next, outcome) = plan_ok(
            &config,
            Edit::SetDeepStage {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: keypress(),
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].deep_base[&Input::Grid(1, 1)],
            keypress()
        );
        assert!(
            outcome.effects.is_empty(),
            "a fresh deep bind has no live slot to orphan"
        );

        // Delta 1 (ticket 25): replacing an *existing* deep Binding with one
        // that differs force-releases a live deep slot on that key — matching
        // `SetBinding`-replace → `StopToggle` and `SetChordBinding`-replace →
        // `StopChord`. The old bare `.insert` arm pushed nothing.
        let mut with_deep = with_primary_and_deep_stage(Input::Grid(1, 1));
        active(&mut with_deep)
            .deep_base
            .insert(Input::Grid(1, 1), keypress());
        let (_, outcome) = plan_ok(
            &with_deep,
            Edit::SetDeepStage {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: toggle_keypress(KeyCode::KEY_B),
            },
        );
        assert_eq!(outcome.effects, vec![Effect::StopStage(Input::Grid(1, 1))]);

        // A byte-identical re-Save of the same deep Binding → nothing.
        let (_, outcome) = plan_ok(
            &with_deep,
            Edit::SetDeepStage {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
                binding: keypress(),
            },
        );
        assert!(
            outcome.effects.is_empty(),
            "an identical deep re-Save must not drop a live deep slot"
        );

        // No inline "needs a primary" check in `plan` itself — `validate`
        // alone rejects this (`DeepStageWithoutPrimary`/`DeepStageMissingConfig`).
        assert!(matches!(
            plan_err(
                &seed(),
                Edit::SetDeepStage {
                    input: Input::Grid(1, 1),
                    layer: Layer::Base,
                    binding: keypress(),
                }
            ),
            CommandError::InvalidRequest(_)
        ));
    }

    #[test]
    fn clear_deep_stage_removes_it_and_rejects_an_absent_one() {
        let mut config = with_primary_and_deep_stage(Input::Grid(1, 1));
        active(&mut config)
            .deep_base
            .insert(Input::Grid(1, 1), keypress());

        let (next, _) = plan_ok(
            &config,
            Edit::ClearDeepStage {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
            },
        );
        assert!(next.profiles[DEFAULT_PROFILE_NAME].deep_base.is_empty());

        assert!(matches!(
            plan_err(
                &seed(),
                Edit::ClearDeepStage {
                    input: Input::Grid(1, 1),
                    layer: Layer::Base,
                }
            ),
            CommandError::NotFound
        ));
    }

    #[test]
    fn clear_deep_stage_pushes_stop_stage_to_force_release_a_live_deep_slot() {
        // Ticket 18: mirrors `clear_binding_removing_a_primary_with_a_live_
        // deep_binding_cascades_it_away` — removing the deep Binding directly
        // orphans a live deep firing the same way the primary cascade does, so
        // it must force-release the slot rather than leave it stuck.
        let mut config = with_primary_and_deep_stage(Input::Grid(1, 1));
        active(&mut config)
            .deep_base
            .insert(Input::Grid(1, 1), keypress());

        let (next, outcome) = plan_ok(
            &config,
            Edit::ClearDeepStage {
                input: Input::Grid(1, 1),
                layer: Layer::Base,
            },
        );
        assert!(next.profiles[DEFAULT_PROFILE_NAME].deep_base.is_empty());
        // `deep_stages` (Actuation/mode config) is left behind, inert.
        assert!(!next.profiles[DEFAULT_PROFILE_NAME].deep_stages.is_empty());
        assert_eq!(outcome.effects, vec![Effect::StopStage(Input::Grid(1, 1))]);
    }

    #[test]
    fn set_deep_actuation_creates_a_fresh_deep_stage_config_defaulting_to_handoff() {
        let (next, outcome) = plan_ok(
            &seed(),
            Edit::SetDeepActuation {
                input: Input::Grid(1, 1),
                actuation: 220,
                release: 200,
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].deep_stages[&Input::Grid(1, 1)],
            crate::config::DeepStageConfig {
                actuation: ActuationPoint {
                    actuation: 220,
                    release: 200,
                },
                mode: StagingMode::Handoff,
            }
        );
        assert!(outcome.effects.is_empty());
    }

    #[test]
    fn set_deep_actuation_on_an_existing_entry_leaves_its_mode_untouched() {
        let mut config = seed();
        active(&mut config).deep_stages.insert(
            Input::Grid(1, 1),
            crate::config::DeepStageConfig {
                actuation: ActuationPoint {
                    actuation: 220,
                    release: 200,
                },
                mode: StagingMode::NoReturn,
            },
        );

        let (next, _) = plan_ok(
            &config,
            Edit::SetDeepActuation {
                input: Input::Grid(1, 1),
                actuation: 230,
                release: 210,
            },
        );
        let cfg = next.profiles[DEFAULT_PROFILE_NAME].deep_stages[&Input::Grid(1, 1)];
        assert_eq!(
            cfg.actuation,
            ActuationPoint {
                actuation: 230,
                release: 210,
            }
        );
        assert_eq!(cfg.mode, StagingMode::NoReturn);
    }

    #[test]
    fn set_staging_mode_creates_a_fresh_deep_stage_config_and_only_writes_mode() {
        // A low primary override keeps the fresh `DeepStageConfig`'s
        // default `ActuationPoint` (128/112) from overlapping the primary
        // band — `SetStagingMode` itself writes no `actuation` field, so
        // that has to come from somewhere for `validate` to accept this.
        let mut config = seed();
        active(&mut config).actuation_overrides.insert(
            Input::Grid(1, 1),
            ActuationPoint {
                actuation: 50,
                release: 40,
            },
        );

        let (next, outcome) = plan_ok(
            &config,
            Edit::SetStagingMode {
                input: Input::Grid(1, 1),
                mode: StagingMode::QuickSkip,
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].deep_stages[&Input::Grid(1, 1)].mode,
            StagingMode::QuickSkip
        );
        // Ticket 23 B7: a mode flip can strand Quick-Skip's phase machine, so
        // `SetStagingMode` force-releases the live deep slot.
        assert_eq!(outcome.effects, vec![Effect::StopStage(Input::Grid(1, 1))]);
    }

    #[test]
    fn set_staging_mode_pushes_stop_stage_even_on_an_existing_entry() {
        // Ticket 23 B7: the force-release is unconditional — pushed whether or
        // not a `DeepStageConfig` already existed, mirroring `ClearDeepStage`.
        let mut config = seed();
        active(&mut config).deep_stages.insert(
            Input::Grid(1, 1),
            crate::config::DeepStageConfig {
                actuation: ActuationPoint {
                    actuation: 220,
                    release: 200,
                },
                mode: StagingMode::Handoff,
            },
        );

        let (next, outcome) = plan_ok(
            &config,
            Edit::SetStagingMode {
                input: Input::Grid(1, 1),
                mode: StagingMode::QuickSkip,
            },
        );
        assert_eq!(
            next.profiles[DEFAULT_PROFILE_NAME].deep_stages[&Input::Grid(1, 1)].mode,
            StagingMode::QuickSkip
        );
        assert_eq!(outcome.effects, vec![Effect::StopStage(Input::Grid(1, 1))]);
    }

    #[test]
    fn set_staging_mode_to_the_same_mode_pushes_no_effect() {
        // Ticket 23 `/code-review`: an idempotent re-apply is a config no-op
        // and must not force-release a deep firing the user is holding.
        let mut config = seed();
        active(&mut config).deep_stages.insert(
            Input::Grid(1, 1),
            crate::config::DeepStageConfig {
                actuation: ActuationPoint {
                    actuation: 220,
                    release: 200,
                },
                mode: StagingMode::NoReturn,
            },
        );

        let (_next, outcome) = plan_ok(
            &config,
            Edit::SetStagingMode {
                input: Input::Grid(1, 1),
                mode: StagingMode::NoReturn,
            },
        );
        assert!(
            outcome.effects.is_empty(),
            "same-mode re-apply is a no-op — no StopStage"
        );
    }

    // --- `reconcile_teardowns` truth table (ticket 25 / ADR-0011) ------------

    #[test]
    fn reconcile_teardowns_covers_every_matrix_row_both_deltas_and_the_negatives() {
        use crate::config::{DeepStageConfig, ModeKeyRole};

        let g11 = Input::Grid(1, 1);
        let g22 = Input::Grid(2, 2);
        let ckey = || ChordKey::new(chord([Input::Grid(1, 1), Input::Grid(1, 2)]));
        let deep = |mode| DeepStageConfig {
            actuation: ActuationPoint {
                actuation: 220,
                release: 200,
            },
            mode,
        };

        // Each row: seed the "before" active Profile, then mutate a clone for
        // "after"; `reconcile_teardowns` must derive exactly `expect`.
        let check = |name: &str,
                     before: &dyn Fn(&mut Profile),
                     after: &dyn Fn(&mut Profile),
                     expect: Vec<Effect>| {
            let mut b = seed();
            before(active(&mut b));
            let mut a = b.clone();
            after(active(&mut a));
            assert_eq!(reconcile_teardowns(&b, &a), expect, "{name}");
        };

        // base / held primary Binding → StopToggle
        check(
            "base binding removed",
            &|p| {
                p.base.insert(g11, keypress());
            },
            &|p| {
                p.base.remove(&g11);
            },
            vec![Effect::StopToggle(g11)],
        );
        check(
            "base binding changed",
            &|p| {
                p.base.insert(g11, keypress());
            },
            &|p| {
                p.base.insert(g11, toggle_keypress(KeyCode::KEY_B));
            },
            vec![Effect::StopToggle(g11)],
        );
        check(
            "base binding present and byte-equal → nothing",
            &|p| {
                p.base.insert(g11, keypress());
            },
            &|_| {},
            vec![],
        );
        check(
            "held binding removed",
            &|p| {
                p.held.insert(g11, keypress());
            },
            &|p| {
                p.held.remove(&g11);
            },
            vec![Effect::StopToggle(g11)],
        );
        check(
            "fresh base bind orphans nothing",
            &|_| {},
            &|p| {
                p.base.insert(g11, keypress());
            },
            vec![],
        );

        // deep_base / deep_held deep Binding → StopStage (delta 1: "or differs")
        check(
            "deep binding removed",
            &|p| {
                p.base.insert(g11, keypress());
                p.deep_base.insert(g11, keypress());
                p.deep_stages.insert(g11, deep(StagingMode::Handoff));
            },
            &|p| {
                p.deep_base.remove(&g11);
            },
            vec![Effect::StopStage(g11)],
        );
        check(
            "deep binding changed (delta 1)",
            &|p| {
                p.base.insert(g11, keypress());
                p.deep_base.insert(g11, keypress());
                p.deep_stages.insert(g11, deep(StagingMode::Handoff));
            },
            &|p| {
                p.deep_base.insert(g11, toggle_keypress(KeyCode::KEY_B));
            },
            vec![Effect::StopStage(g11)],
        );
        check(
            "deep binding byte-equal → nothing",
            &|p| {
                p.base.insert(g11, keypress());
                p.deep_base.insert(g11, keypress());
                p.deep_stages.insert(g11, deep(StagingMode::Handoff));
            },
            &|_| {},
            vec![],
        );

        // chords_base / chords_held Chord Binding → StopChord
        check(
            "chord binding removed",
            &|p| {
                p.chords_base.insert(ckey(), keypress());
            },
            &|p| {
                p.chords_base.remove(&ckey());
            },
            vec![Effect::StopChord(ckey())],
        );
        check(
            "chord binding changed",
            &|p| {
                p.chords_base.insert(ckey(), keypress());
            },
            &|p| {
                p.chords_base
                    .insert(ckey(), toggle_keypress(KeyCode::KEY_B));
            },
            vec![Effect::StopChord(ckey())],
        );
        check(
            "chord binding byte-equal → nothing",
            &|p| {
                p.chords_base.insert(ckey(), keypress());
            },
            &|_| {},
            vec![],
        );

        // deep_stages.mode → StopStage; actuation-only → nothing
        check(
            "staging mode changed",
            &|p| {
                p.deep_stages.insert(g11, deep(StagingMode::Handoff));
            },
            &|p| {
                p.deep_stages.insert(g11, deep(StagingMode::QuickSkip));
            },
            vec![Effect::StopStage(g11)],
        );
        check(
            "deep_stages actuation-only change → nothing",
            &|p| {
                p.deep_stages.insert(g11, deep(StagingMode::Handoff));
            },
            &|p| {
                p.deep_stages.insert(
                    g11,
                    DeepStageConfig {
                        actuation: ActuationPoint {
                            actuation: 210,
                            release: 190,
                        },
                        mode: StagingMode::Handoff,
                    },
                );
            },
            vec![],
        );
        check(
            "fresh deep_stages entry with a non-default mode counts as a change",
            &|_| {},
            &|p| {
                p.deep_stages.insert(g11, deep(StagingMode::QuickSkip));
            },
            vec![Effect::StopStage(g11)],
        );

        // mode_key_role → StopToggle(ModeKey), only Bound → LayerSwitch (delta 2)
        check(
            "mode_key_role Bound → LayerSwitch",
            &|p| p.mode_key_role = ModeKeyRole::Bound,
            &|p| p.mode_key_role = ModeKeyRole::LayerSwitch,
            vec![Effect::StopToggle(Input::ModeKey)],
        );
        check(
            "mode_key_role LayerSwitch → LayerSwitch → nothing (delta 2)",
            &|p| p.mode_key_role = ModeKeyRole::LayerSwitch,
            &|_| {},
            vec![],
        );
        check(
            "mode_key_role LayerSwitch → Bound → nothing",
            &|p| p.mode_key_role = ModeKeyRole::LayerSwitch,
            &|p| p.mode_key_role = ModeKeyRole::Bound,
            vec![],
        );

        // Output contract: fixed type order, keys sorted within a type.
        check(
            "type order StopToggle → StopStage → StopChord, keys sorted",
            &|p| {
                p.base.insert(g22, keypress());
                p.base.insert(g11, keypress());
                p.deep_base.insert(g11, keypress());
                p.deep_stages.insert(g11, deep(StagingMode::Handoff));
                p.chords_base.insert(ckey(), keypress());
                p.mode_key_role = ModeKeyRole::Bound;
            },
            &|p| {
                p.base.remove(&g11);
                p.base.remove(&g22);
                p.deep_base.remove(&g11);
                p.chords_base.remove(&ckey());
                p.mode_key_role = ModeKeyRole::LayerSwitch;
            },
            vec![
                Effect::StopToggle(Input::ModeKey),
                Effect::StopToggle(g11),
                Effect::StopToggle(g22),
                Effect::StopStage(g11),
                Effect::StopChord(ckey()),
            ],
        );

        // Negatives that need a whole-Config diff, not a Profile one.
        let mut two = seed();
        two.profiles
            .insert("Gaming".to_string(), Profile::default());
        active(&mut two).base.insert(g11, keypress());
        let mut switched = two.clone();
        switched.active_profile = "Gaming".to_string();
        assert!(
            reconcile_teardowns(&two, &switched).is_empty(),
            "an active-Profile change bails — SwitchProfile owns TearDown(ProfileSwitch)"
        );

        let mut renamed = two.clone();
        let p = renamed.profiles.remove(DEFAULT_PROFILE_NAME).unwrap();
        renamed.profiles.insert("Renamed".to_string(), p);
        renamed.active_profile = "Renamed".to_string();
        assert!(
            reconcile_teardowns(&two, &renamed).is_empty(),
            "an active-Profile rename bails too"
        );
    }

    // --- preconditions and invariants, one row each ---------------------------

    struct Case {
        name: &'static str,
        setup: fn(&mut Config),
        edit: fn() -> Edit,
        matches: fn(&CommandError) -> bool,
    }

    fn is_invalid(err: &CommandError, needle: &str) -> bool {
        matches!(err, CommandError::InvalidRequest(m) if m.contains(needle))
    }

    #[test]
    fn every_precondition_and_invariant_path_has_a_dedicated_rejection() {
        let cases = [
            Case {
                name: "SetBinding: ProfileSwitch paired with a non-fire-once trigger (invariant)",
                setup: |_| {},
                edit: || Edit::SetBinding {
                    input: Input::Grid(1, 1),
                    layer: Layer::Base,
                    binding: Binding {
                        trigger: TriggerMode::Toggle,
                        action: Action::ProfileSwitch {
                            target: DEFAULT_PROFILE_NAME.to_string(),
                        },
                    },
                },
                matches: |e| is_invalid(e, "fire_once"),
            },
            Case {
                name: "SetBinding: analog_repeat on a non-grid Input (invariant)",
                setup: |_| {},
                edit: || Edit::SetBinding {
                    input: Input::ModeKey,
                    layer: Layer::Base,
                    binding: Binding {
                        trigger: TriggerMode::AnalogRepeat,
                        action: Action::Keypress {
                            modifiers: Modifiers::default(),
                            key: KeyCode::KEY_A,
                        },
                    },
                },
                matches: |e| is_invalid(e, "analog_repeat"),
            },
            Case {
                name: "CreateProfile: name already taken (precondition)",
                setup: |c| {
                    c.profiles.insert("Gaming".to_string(), Profile::default());
                },
                edit: || Edit::CreateProfile {
                    name: "Gaming".to_string(),
                },
                matches: |e| matches!(e, CommandError::AlreadyExists),
            },
            Case {
                name: "CreateProfile: blank name (invariant)",
                setup: |_| {},
                edit: || Edit::CreateProfile {
                    name: "   ".to_string(),
                },
                matches: |e| is_invalid(e, "whitespace-only name"),
            },
            Case {
                name: "DeleteProfile: the active Profile (precondition)",
                setup: |_| {},
                edit: || Edit::DeleteProfile {
                    name: DEFAULT_PROFILE_NAME.to_string(),
                },
                matches: |e| is_invalid(e, "cannot delete the active Profile"),
            },
            Case {
                name: "DeleteProfile: still referenced by a ProfileSwitch (precondition)",
                setup: |c| {
                    c.profiles.insert("Gaming".to_string(), Profile::default());
                    active(c).base.insert(
                        Input::Grid(1, 1),
                        Binding {
                            trigger: TriggerMode::FireOnce,
                            action: Action::ProfileSwitch {
                                target: "Gaming".to_string(),
                            },
                        },
                    );
                },
                edit: || Edit::DeleteProfile {
                    name: "Gaming".to_string(),
                },
                matches: |e| is_invalid(e, "still referenced by a Profile Switch Binding"),
            },
            Case {
                name: "DeleteProfile: unknown name (precondition)",
                setup: |_| {},
                edit: || Edit::DeleteProfile {
                    name: "Ghost".to_string(),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "RenameProfile: unknown old_name (precondition)",
                setup: |_| {},
                edit: || Edit::RenameProfile {
                    old_name: "Ghost".to_string(),
                    new_name: "Whatever".to_string(),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "RenameProfile: new_name already taken (precondition)",
                setup: |c| {
                    c.profiles.insert("Gaming".to_string(), Profile::default());
                },
                edit: || Edit::RenameProfile {
                    old_name: "Gaming".to_string(),
                    new_name: DEFAULT_PROFILE_NAME.to_string(),
                },
                matches: |e| matches!(e, CommandError::AlreadyExists),
            },
            Case {
                name: "SwitchProfile: unknown name (precondition)",
                setup: |_| {},
                edit: || Edit::SwitchProfile {
                    name: "Ghost".to_string(),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "SetActuationPoint: release >= actuation (invariant)",
                setup: |_| {},
                edit: || Edit::SetActuationPoint {
                    input: Input::Grid(1, 1),
                    actuation: 100,
                    release: 120,
                },
                matches: |e| is_invalid(e, "release point at or above"),
            },
            Case {
                name: "SetActuationPoint: non-grid Input (invariant)",
                setup: |_| {},
                edit: || Edit::SetActuationPoint {
                    input: Input::ModeKey,
                    actuation: 200,
                    release: 100,
                },
                matches: |e| is_invalid(e, "actuation override"),
            },
            Case {
                name: "SetDefaultActuation: release >= actuation (invariant)",
                setup: |_| {},
                edit: || Edit::SetDefaultActuation {
                    actuation: 100,
                    release: 100,
                },
                matches: |e| is_invalid(e, "default"),
            },
            Case {
                name: "SetDefaultDeepActuation: release >= actuation (invariant)",
                setup: |_| {},
                edit: || Edit::SetDefaultDeepActuation {
                    actuation: 200,
                    release: 200,
                },
                matches: |e| is_invalid(e, "default deep"),
            },
            Case {
                name: "CreateMacro: blank name (precondition)",
                setup: |_| {},
                edit: || Edit::CreateMacro {
                    name: "  ".to_string(),
                    steps: vec![],
                },
                matches: |e| is_invalid(e, "Macro name can't be empty"),
            },
            Case {
                name: "RenameMacro: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::RenameMacro {
                    macro_id: MacroId::from("ghost"),
                    new_name: "x".to_string(),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "DeleteMacro: still referenced (precondition)",
                setup: |c| {
                    let mid = with_macro(c, "m");
                    active(c).base.insert(
                        Input::Grid(1, 1),
                        Binding {
                            trigger: TriggerMode::FireOnce,
                            action: Action::Macro { macro_id: mid },
                        },
                    );
                },
                edit: || Edit::DeleteMacro {
                    macro_id: MacroId::from("m"),
                },
                matches: |e| is_invalid(e, "still referenced by a Macro Binding"),
            },
            Case {
                name: "DeleteMacro: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::DeleteMacro {
                    macro_id: MacroId::from("ghost"),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "SetMacroSteps: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::SetMacroSteps {
                    macro_id: MacroId::from("ghost"),
                    steps: vec![],
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "CreateStepper: blank name (precondition)",
                setup: |_| {},
                edit: || Edit::CreateStepper {
                    name: "  ".to_string(),
                    items: vec![],
                },
                matches: |e| is_invalid(e, "Stepper name can't be empty"),
            },
            Case {
                name: "RenameStepper: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::RenameStepper {
                    stepper_id: StepperId::from("ghost"),
                    new_name: "x".to_string(),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "DeleteStepper: still referenced (precondition)",
                setup: |c| {
                    let sid = with_stepper(c, "wep");
                    active(c).base.insert(
                        Input::Grid(1, 1),
                        step(sid.as_str(), StepDirection::Forward),
                    );
                },
                edit: || Edit::DeleteStepper {
                    stepper_id: StepperId::from("wep"),
                },
                matches: |e| is_invalid(e, "still referenced by a Step Binding"),
            },
            Case {
                name: "DeleteStepper: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::DeleteStepper {
                    stepper_id: StepperId::from("ghost"),
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "SetStepperItems: unknown id (precondition)",
                setup: |_| {},
                edit: || Edit::SetStepperItems {
                    stepper_id: StepperId::from("ghost"),
                    items: vec![],
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "SetChordBinding: fewer than two members (invariant)",
                setup: |_| {},
                edit: || Edit::SetChordBinding {
                    inputs: chord([Input::Grid(1, 1)]),
                    layer: Layer::Base,
                    binding: keypress(),
                },
                matches: |e| is_invalid(e, "fewer than two member"),
            },
            Case {
                name: "SetChordBinding: ProfileSwitch action (invariant)",
                setup: |_| {},
                edit: || Edit::SetChordBinding {
                    inputs: chord([Input::Grid(1, 1), Input::Grid(1, 2)]),
                    layer: Layer::Base,
                    binding: Binding {
                        trigger: TriggerMode::FireOnce,
                        action: Action::ProfileSwitch {
                            target: DEFAULT_PROFILE_NAME.to_string(),
                        },
                    },
                },
                matches: |e| is_invalid(e, "cannot be profile_switch"),
            },
            Case {
                name: "ClearChordBinding: no such member set (precondition)",
                setup: |_| {},
                edit: || Edit::ClearChordBinding {
                    inputs: chord([Input::Grid(1, 1), Input::Grid(1, 2)]),
                    layer: Layer::Base,
                },
                matches: |e| matches!(e, CommandError::NotFound),
            },
            Case {
                name: "SetAxisAssignment: non-grid Input (invariant)",
                setup: |_| {},
                edit: || Edit::SetAxisAssignment {
                    input: Input::ModeKey,
                    layer: Layer::Base,
                    target: AxisTarget::LeftTrigger,
                },
                matches: |e| is_invalid(e, "only Grid Inputs"),
            },
            Case {
                name: "SetDeepStage: no primary Binding on the layer (invariant)",
                setup: |_| {},
                edit: || Edit::SetDeepStage {
                    input: Input::Grid(1, 1),
                    layer: Layer::Base,
                    binding: Binding {
                        trigger: TriggerMode::HoldToRepeat,
                        action: Action::Keypress {
                            modifiers: Modifiers::default(),
                            key: KeyCode::KEY_A,
                        },
                    },
                },
                matches: |e| is_invalid(e, "no primary Binding"),
            },
        ];

        for case in cases {
            let mut config = seed();
            (case.setup)(&mut config);
            let before = config.clone();
            let err = plan(&config, (case.edit)()).expect_err(case.name);
            assert!((case.matches)(&err), "{}: wrong error {err:?}", case.name);
            assert_eq!(config, before, "{}: caller's Config was mutated", case.name);
        }
    }

    // --- `apply` (async wrapper) --------------------------------------------

    #[tokio::test]
    async fn apply_persists_the_planned_config_and_returns_its_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = seed();

        let outcome = apply(&mut config, &path, Edit::SetForceDigital { force: true })
            .await
            .expect("apply must succeed");

        assert_eq!(outcome.effects, vec![Effect::SignalCaptureMode(true)]);
        assert!(config.force_digital);
        let reloaded = config::load_or_seed(&path).expect("config.toml must exist");
        assert!(
            reloaded.force_digital,
            "the persisted file reflects the edit"
        );
    }

    #[tokio::test]
    async fn apply_leaves_config_untouched_when_the_persist_fails() {
        let dir = tempfile::tempdir().unwrap();
        // A regular file where a directory is expected — `create_dir_all` on
        // the parent then fails, so `persist` errors.
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"blocker").unwrap();
        let path = blocker.join("config.toml");

        let mut config = seed();
        let before = config.clone();

        let err = apply(&mut config, &path, Edit::SetForceDigital { force: true })
            .await
            .expect_err("an unwritable path must fail");

        assert!(matches!(err, CommandError::IoError(_)));
        assert_eq!(config, before, "a failed persist rolls nothing forward");
    }
}
