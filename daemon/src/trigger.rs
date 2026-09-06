// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The Trigger-mode firing matrix (ticket 17, post-release ticket 08) — carved
//! out of `dispatch.rs` as a pure, synchronous core so the recurring
//! `(TriggerMode, EventState, Action-shape)` carve-outs (tickets 75, 76, 78,
//! 79, 80, 82) stop being re-tuned in two near-verbatim `match` bodies.
//!
//! `decide` answers one question — what does a `(Binding, EventState,
//! slot-liveness)` triple do? — with a data-only `TriggerDecision` then
//! performed by `Slots::perform` (below) against the `(firings, toggles)`
//! handle pair `Slots<K>` owns, keyed by `Input` for the individual path and
//! by `ChordKey` for the Chord path. `decide` itself does no I/O, spawns no
//! task, takes no `&Injector`, and reads nothing from `executor` / `injector`
//! / `edit`. It replaces the old `dispatch::fire` and
//! `dispatch::execute_chord_fire` (ex-`fire_chord`), whose `match` bodies were
//! arm-for-arm identical.
//!
//! `Slots<K>` (post-release ticket 15) is the runtime `(HashMap<K,
//! FiringHandle>, HashMap<K, ActiveToggle>)` pair `decide`'s output is
//! performed against — firing-wins liveness of one key (`slot`, the overlap
//! guard's input), toggle-wins liveness of every key (`snapshot`,
//! `chord::feed`'s completion input), the performance of a decision
//! (`perform`), and the teardown paths (`force_release` / `stop_toggle` /
//! `stop_all_toggles`, absorbing what were three hand-rolled copies of the same
//! two lines plus `dispatch::stop_all_toggles`). It owns `FiringHandle` /
//! `ActiveToggle` and its `perform` is `async`, so `Slots<K>` is deliberately
//! *not* part of the pure core — the same carve-out the old
//! `force_release_stuck` / `stop_toggle` free functions it absorbs always had.
//! `compile_action` (below) stays synchronous: it turns a `Binding`'s `Action`
//! into the flat step sequence `perform` spawns.

use std::collections::HashMap;
use std::hash::Hash;
use std::io;
use std::time::Duration;

use evdev::KeyCode;

use crate::capture::EventState;
use crate::config::{
    Action, Binding, Config, MacroDef, MacroId, Modifiers, StepperDef, StepperId, TriggerMode,
};
use crate::executor::{self, ActiveToggle, FiringHandle, MacroStep};
use crate::injector::Injector;
use crate::input::is_mouse_button;
use crate::stepper;

/// Liveness of one firing/toggle slot, passed IN to `decide` rather than
/// held — the function stays pure, tests construct it directly. Absent
/// (`None`) means no live firing or toggle for that key. Folds in the old
/// `chord::ChordSlot` verbatim (same three states); `chord::feed` and
/// `Slots::snapshot` now speak `trigger::Slot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// An active Toggle-mode firing.
    Toggle,
    /// A Fire-once / Hold-to-repeat firing still in flight.
    FiringUnfinished,
    /// A Fire-once firing that has already completed on its own — the map
    /// entry lingers (never cleaned, mirroring the old `fire`'s own
    /// `in_flight`), so this must be distinct from `None` or the slot could
    /// never fire again.
    FiringFinished,
}

/// What a `(Binding, EventState, Slot)` triple resolves to. Data-only; the
/// dispatch-side executor performs it. `SpawnFireOnce` / `StartToggleLoop`
/// stay abstract (no compiled steps) so `compile_action` runs in the executor
/// *after* `decide` has cleared the overlap guard — a dropped firing must
/// never advance a Stepper cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerDecision {
    /// Overlap guard hit (`Slot::FiringUnfinished`), or an inert
    /// (state, mode) pair (Fire-once + Repeat/Up, Toggle + Repeat/Up, a
    /// `ControllerButton` / mouse-button Hold-to-repeat Repeat).
    Nothing,
    /// Compile `binding.action` and spawn a one-shot firing.
    SpawnFireOnce,
    /// Hold a bare, unbalanced `KeyDown` — mouse-button / `ControllerButton`
    /// Hold-to-repeat `Down` (tickets 75/76, 79/80). Released later by
    /// `ForceReleaseStuck` on the individual path, or
    /// `ChordEffect::ReleaseChordFiring` on the Chord path.
    HoldKeyDown(KeyCode),
    /// Compile `binding.action` and start a looping Toggle.
    StartToggleLoop,
    /// Start a single-held Toggle — mouse-button / `ControllerButton` Toggle
    /// (tickets 78, 82/83).
    StartToggleHeld(KeyCode),
    /// Force-release whatever this key's firing left stuck — Fire-once /
    /// Hold-to-repeat / Analog-repeat `Up` on the individual path.
    ForceReleaseStuck,
}

/// How a Binding's held target behaves downstream when held or repeated
/// (spec-kernel-shaped-repeat.md §2.1) — the classification `decide` needs to
/// tell a "held single key" (which must present as genuine kernel autorepeat)
/// apart from a mouse/gamepad button (which the kernel never autorepeats) and
/// from a multi-step target. Replaces the bare `Option<KeyCode>` that
/// `sustained_hold_key` returns today.
///
/// - `SustainedNoRepeat(code)` — today's `sustained_hold_key` set: an
///   `Action::ControllerButton`, or an `Action::Keypress` on a mouse-button
///   code. On `Repeat` these resolve to `D::Nothing`.
/// - `AutorepeatKey(mods, code)` — a single keyboard key: an `Action::Keypress`
///   (with or without modifiers) on a non-mouse code, or an `Action::Macro`
///   whose compiled steps satisfy `executor::single_held_key`. Ticket 03 routes
///   this onto exactly the decisions the old `sustained_hold_key`'s `None`
///   produced (the ordinary keyboard arms); ticket 04 splits it onto the
///   `value=2` autorepeat path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoldKind {
    SustainedNoRepeat(KeyCode),
    AutorepeatKey(Modifiers, KeyCode),
}

/// A single held key's `HoldKind`, keyboard vs. mouse-button split — the
/// kernel autorepeats keyboard keys but never `BTN_*`, so a mouse-button code
/// is `SustainedNoRepeat` even though it arrived through the same
/// `[KeyDown(k), KeyUp(k)]` shape. Shared by `hold_repeat_kind`'s `Keypress`
/// and `Macro` arms so a single-key Macro classifies *exactly* like the
/// equivalent Keypress (`Action::Keypress.key` and a macro step accept the
/// same unvalidated `KeyCode`, `BTN_*` included).
fn key_hold_kind(modifiers: Modifiers, key: KeyCode) -> HoldKind {
    if is_mouse_button(key) {
        HoldKind::SustainedNoRepeat(key)
    } else {
        HoldKind::AutorepeatKey(modifiers, key)
    }
}

/// Classifies a Binding's held target into a `HoldKind` (spec §2.1) — the
/// successor to `sustained_hold_key`. For `Action::Keypress` /
/// `Action::ControllerButton` it reads `mods` + `key` straight off the action;
/// for `Action::Macro` it resolves the `MacroDef`, compiles the steps
/// (`executor::compile`), and runs `executor::single_held_key`. Multi-step
/// Macro, Stepper, Profile switch → `None` (multi-step, or handled earlier).
fn hold_repeat_kind(action: &Action, macros: &HashMap<MacroId, MacroDef>) -> Option<HoldKind> {
    match action {
        Action::ControllerButton { button } => Some(HoldKind::SustainedNoRepeat(*button)),
        Action::Keypress { modifiers, key } => Some(key_hold_kind(*modifiers, *key)),
        Action::Macro { .. } => {
            let steps = executor::compile(action, macros);
            executor::single_held_key(&steps).map(|(mods, key)| key_hold_kind(mods, key))
        }
        Action::Step { .. } | Action::ProfileSwitch { .. } => None,
    }
}

/// The pure decision core. `binding` carries `.trigger` and `.action`; `macros`
/// resolves an `Action::Macro`'s compiled shape for `hold_repeat_kind` (every
/// call site already holds it — `config.macros` / `deps.config.macros`); `slot`
/// is the liveness of this key's existing firing/toggle (`None` == absent).
/// No I/O, no async. `ProfileSwitch` never reaches here — it is intercepted
/// upstream (`dispatch_individual_down` / `handle_event`'s `Repeat | Up` arm),
/// and a Chord's own Action can never be `ProfileSwitch`.
///
/// This is the old `fire` / `execute_chord_fire` matrix, arm-for-arm. The
/// bare-hold carve-out arms match `HoldToRepeat` only — `AnalogRepeat` rides
/// the ordinary `SpawnFireOnce` arm exactly as it did in `fire`; the Chord
/// path never reaches the `AnalogRepeat` or `Up` arms at all
/// (`config::validate` rejects an `AnalogRepeat` Chord, and `chord::feed` only
/// ever emits `Down` / `Repeat`).
pub(crate) fn decide(
    binding: &Binding,
    macros: &HashMap<MacroId, MacroDef>,
    state: EventState,
    slot: Option<Slot>,
) -> TriggerDecision {
    use EventState::{Down, Repeat, Up};
    use TriggerDecision as D;
    use TriggerMode::{AnalogRepeat, FireOnce, HoldToRepeat, Toggle};

    // The old `if let Some(handle) = firings.get(&key) && !handle.is_finished()`
    // overlap guard: a still-running same-key firing means this one is
    // dropped, not queued. Only `FiringUnfinished` blocks — a lingering
    // `FiringFinished` entry (never cleaned) must not exclude a fresh fire.
    let guarded = |proceed: D| {
        if matches!(slot, Some(Slot::FiringUnfinished)) {
            D::Nothing
        } else {
            proceed
        }
    };
    // `sustained_hold_key`'s successor: only the mouse-button / gamepad
    // `SustainedNoRepeat` shape takes the bare-hold carve-out arms. A single
    // keyboard key (`AutorepeatKey`) and a multi-step target (`None`) both ride
    // the ordinary keyboard arms below — exactly the decisions
    // `sustained_hold_key`'s `None` produced. Ticket 04 splits `AutorepeatKey`
    // onto the `value=2` autorepeat path.
    //
    // Only the `HoldToRepeat` and `Toggle` arms consult `hold_key`, so the
    // classification — an `executor::compile` of an `Action::Macro` among it —
    // is skipped entirely for `FireOnce` / `AnalogRepeat`, which the old eager
    // `sustained_hold_key` call also computed but never read.
    let hold_key = match binding.trigger {
        HoldToRepeat | Toggle => match hold_repeat_kind(&binding.action, macros) {
            Some(HoldKind::SustainedNoRepeat(code)) => Some(code),
            Some(HoldKind::AutorepeatKey(..)) | None => None,
        },
        FireOnce | AnalogRepeat => None,
    };

    match (binding.trigger, state) {
        // Tickets 75/76 & 79/80: a mouse / gamepad button under Hold-to-repeat
        // holds one bare unbalanced `KeyDown` on `Down` (released later by
        // `ForceReleaseStuck` / `ReleaseChordFiring`) and ignores every
        // kernel-autorepeat `Repeat` — no hardware button autorepeats.
        (HoldToRepeat, Repeat) if hold_key.is_some() => D::Nothing,
        (HoldToRepeat, Down) if hold_key.is_some() => {
            guarded(D::HoldKeyDown(hold_key.expect("matched by the arm guard")))
        }

        // Fire-once fires only on `Down`; Hold-to-repeat / Analog-repeat on
        // `Down` and every `Repeat`. `compile_action` runs in the executor,
        // behind this same guard, so a dropped Step firing never advances the
        // cursor.
        (FireOnce, Down) | (HoldToRepeat | AnalogRepeat, Down | Repeat) => {
            guarded(D::SpawnFireOnce)
        }

        // Toggle starts only on `Down`: a mouse / gamepad button latches as a
        // single held `KeyDown` (tickets 78, 82/83), everything else loops.
        (Toggle, Down) => match hold_key {
            Some(code) => D::StartToggleHeld(code),
            None => D::StartToggleLoop,
        },

        // Ticket 33's stuck-key fix — force-release whatever this key's most
        // recent firing left down (a no-op for a balanced Macro). Toggle's own
        // `Up` is inert (its stop is a second `Down`). The Chord path never
        // sends `Up` here.
        (FireOnce | HoldToRepeat | AnalogRepeat, Up) => D::ForceReleaseStuck,

        _ => D::Nothing,
    }
}

/// The `(firings, toggles)` runtime handle pair `decide`'s output is performed
/// against (post-release ticket 15), generic over the slot key — `Input` on
/// the individual path, `ChordKey` on the Chord path, and a coming
/// `Slots<StageKey>` for dual-stage keys instead of a fourth hand-rolled copy.
/// Both maps are **private**: every read and mutation the two-map protocol
/// needs is a method here, so the two deliberately-inverse tie-breaks
/// (`slot` firing-wins, `snapshot` toggle-wins) live next to the state they
/// guard rather than in a doc comment on a free function. `DispatchState`
/// holds two — `individual: Slots<Input>` and `chord_slots: Slots<ChordKey>` —
/// the way `axis::Engine` bundles its own two maps.
pub(crate) struct Slots<K> {
    firings: HashMap<K, FiringHandle>,
    toggles: HashMap<K, ActiveToggle>,
}

impl<K> Default for Slots<K> {
    fn default() -> Self {
        Slots {
            firings: HashMap::new(),
            toggles: HashMap::new(),
        }
    }
}

/// What `Slots::perform` needs beyond the two maps it already owns — the tail
/// of the old `dispatch::TriggerCtx` minus `firings` / `toggles`. Built at each
/// call site with `PerformDeps::new` (a one-liner, no macro) rather than the
/// old `trigger_ctx!`: the macro existed only because "a `&mut self` method
/// can't be generic over which map type `K` selects", and that reason is gone
/// now that `Slots<K>` *is* the `self`. `&macros` / `&steppers` are split out
/// of `Config` here so `Slots` never sees a `Config` (ticket 05), matching
/// `executor::compile`'s own signature.
pub(crate) struct PerformDeps<'a> {
    pub injector: &'a Injector,
    pub macros: &'a HashMap<MacroId, MacroDef>,
    pub steppers: &'a HashMap<StepperId, StepperDef>,
    pub cursors: &'a mut stepper::Cursors,
    pub toggle_lap_target: Duration,
}

impl<'a> PerformDeps<'a> {
    /// The call-site constructor — takes `&Config` (read-only) and the two
    /// disjoint `DispatchState` borrows, so each retargeted site is one line
    /// instead of the six-field literal.
    pub(crate) fn new(
        injector: &'a Injector,
        config: &'a Config,
        cursors: &'a mut stepper::Cursors,
        toggle_lap_target: Duration,
    ) -> Self {
        PerformDeps {
            injector,
            macros: &config.macros,
            steppers: &config.steppers,
            cursors,
            toggle_lap_target,
        }
    }
}

impl<K: Eq + Hash + Clone> Slots<K> {
    /// Firing-wins liveness of one key — `decide`'s overlap guard input. The
    /// firing slot first (`FiringUnfinished` / `FiringFinished` by
    /// `handle.is_finished()`), `Slot::Toggle` only as a fallback when no
    /// firing entry exists. The old `dispatch::slot_for`, verbatim. This is
    /// the deliberate inverse of `snapshot`'s tie-break: `decide` only ever
    /// blocks on `Some(Slot::FiringUnfinished)`, so a key holding *both* a
    /// live firing and a Toggle must report the firing here.
    pub(crate) fn slot(&self, key: &K) -> Option<Slot> {
        if let Some(handle) = self.firings.get(key) {
            return Some(if handle.is_finished() {
                Slot::FiringFinished
            } else {
                Slot::FiringUnfinished
            });
        }
        self.toggles.contains_key(key).then_some(Slot::Toggle)
    }

    /// Toggle-wins liveness of every key — `chord::feed`'s completion input.
    /// Every firing inserted first, then overwritten with `Slot::Toggle` for
    /// every key in `toggles`, so a key holding both reports the Toggle. The
    /// old `dispatch::chord_slots`, verbatim. Rebuilt per call — no cache; the
    /// Chord map is ≤ ~12 keys and a cache would need invalidation on every
    /// mutation. The deliberate inverse of `slot`'s tie-break —
    /// `chord.rs`'s "second completion stops the Toggle" branch reads
    /// `Some(Slot::Toggle)`, which firing-wins would hide.
    pub(crate) fn snapshot(&self) -> HashMap<K, Slot> {
        let mut live = HashMap::new();
        for (key, handle) in &self.firings {
            let slot = if handle.is_finished() {
                Slot::FiringFinished
            } else {
                Slot::FiringUnfinished
            };
            live.insert(key.clone(), slot);
        }
        for key in self.toggles.keys() {
            live.insert(key.clone(), Slot::Toggle);
        }
        live
    }

    /// Performs one `decision` against the pair — `compile_action` (behind the
    /// overlap guard `decide` already cleared, so a dropped Fire-once /
    /// Hold-to-repeat `Step` firing never advances the cursor) + `executor::
    /// spawn_fire_once` / `ActiveToggle::spawn{,_held}` + the map insert, or
    /// `force_release`. Never produces an `edit::Edit` (`ProfileSwitch` is
    /// handled before this is ever reached). The old
    /// `dispatch::perform_trigger` executor, now a `&mut self` method.
    pub(crate) async fn perform(
        &mut self,
        decision: TriggerDecision,
        key: K,
        binding: &Binding,
        deps: PerformDeps<'_>,
    ) -> io::Result<()> {
        use TriggerDecision as D;
        match decision {
            D::Nothing => {}
            D::SpawnFireOnce => {
                let steps =
                    compile_action(&binding.action, deps.macros, deps.steppers, deps.cursors);
                let handle = executor::spawn_fire_once(deps.injector.clone(), steps);
                self.firings.insert(key, handle);
            }
            D::HoldKeyDown(code) => {
                // A bare, unbalanced `KeyDown` mirroring the physical hold —
                // released by a `ForceReleaseStuck` (individual) or
                // `ChordEffect::ReleaseChordFiring` (Chord) later, reusing
                // ticket 33's force-release path rather than inventing new
                // architecture.
                let handle = executor::spawn_fire_once(
                    deps.injector.clone(),
                    vec![MacroStep::KeyDown(code)],
                );
                self.firings.insert(key, handle);
            }
            D::StartToggleLoop => {
                let steps =
                    compile_action(&binding.action, deps.macros, deps.steppers, deps.cursors);
                self.toggles.insert(
                    key,
                    ActiveToggle::spawn(deps.injector.clone(), steps, deps.toggle_lap_target),
                );
            }
            D::StartToggleHeld(code) => {
                self.toggles
                    .insert(key, ActiveToggle::spawn_held(deps.injector.clone(), code));
            }
            D::ForceReleaseStuck => {
                self.force_release(&key, deps.injector).await;
            }
        }
        Ok(())
    }

    /// Ticket 33's force-release, factored out of the individual `Up` arm, the
    /// Chord `ReleaseChordFiring` effect, and the Chord `ForceReleaseIndividual`
    /// effect — three hand-rolled copies of the same two lines. Releases (but
    /// **never removes**) `key`'s firing entry: a balanced Macro has already
    /// self-released (`held` empty, a no-op); a bare unbalanced `KeyDown` (a
    /// `HoldKeyDown` decision) is exactly what this cleans up. The entry
    /// lingers so a later `FiringFinished` slot stays distinct from `None`.
    pub(crate) async fn force_release(&self, key: &K, injector: &Injector) {
        if let Some(firing) = self.firings.get(key) {
            firing.force_release_stuck(injector).await;
        }
    }

    /// Stops and **removes** `key`'s Toggle, awaiting its force-release.
    /// Shared by `handle_event`'s inline toggle-stop-on-`Down` (whose `bool`
    /// return drives the "this press is consumed by the stop" early return),
    /// the `StopToggle` effect, and the Chord `StopChordToggle` effect.
    /// Returns whether a Toggle was actually present.
    pub(crate) async fn stop_toggle(&mut self, key: &K) -> bool {
        match self.toggles.remove(key) {
            Some(toggle) => {
                toggle.stop().await;
                true
            }
            None => false,
        }
    }

    /// Drains and stops every Toggle — `SwitchProfile`'s `StopAllToggles`
    /// effect and the `StopAllToggles` command (ticket 25). The old free
    /// `dispatch::stop_all_toggles`, verbatim.
    pub(crate) async fn stop_all_toggles(&mut self) {
        for (_, toggle) in self.toggles.drain() {
            toggle.stop().await;
        }
    }

    /// The keys with a currently-active Toggle — `GetState`'s `active_toggles`
    /// read model.
    pub(crate) fn active_toggle_keys(&self) -> impl Iterator<Item = &K> {
        self.toggles.keys()
    }

    /// Force-releases every live firing (entries linger, matching
    /// `force_release`'s own contract) and drains every Toggle — full
    /// teardown for a caller that's about to discard this `Slots<K>`'s
    /// liveness entirely, e.g. `stage::Engine::stop_all()`
    /// (`tartarus-dual-stage-keys` ticket 03) on a Layer/Profile switch or an
    /// Analog→Digital capture-mode flip. Unlike `stop_all_toggles`, also
    /// covers a stuck bare `KeyDown` (`HoldKeyDown`) a live Fire-once/
    /// Hold-to-repeat firing may be holding. Same narrow, pre-existing race
    /// `force_release` itself always had: a firing spawned an instant
    /// earlier that `tokio` hasn't polled yet has nothing in `held` to
    /// release, and the caller discarding this `Slots<K>` right after (as
    /// `stage::Engine::stop_all()` does) means nothing will ever reach it
    /// again — not a new risk this method introduces, just this method's
    /// own share of it.
    pub(crate) async fn stop_all(&mut self, injector: &Injector) {
        for firing in self.firings.values() {
            firing.force_release_stuck(injector).await;
        }
        self.stop_all_toggles().await;
    }
}

/// Compiles a `Binding`'s `Action` into the flat step sequence `Slots::perform`
/// (and `analog_repeat`'s spawn path) spawns — `executor::compile` for every
/// ordinary Action, or, for `Action::Step`, `stepper::Cursors::step` (which
/// advances the Daemon-owned per-list cursor `executor::compile` has no access
/// to, ticket 03/54) followed by `executor::compile_stepper_item`. A zero-item
/// list steps to nothing. Called *inside* `perform`, after `decide` has
/// cleared the overlap guard, so a dropped `Step` firing never advances the
/// cursor.
pub(crate) fn compile_action(
    action: &Action,
    macros: &HashMap<MacroId, MacroDef>,
    steppers: &HashMap<StepperId, StepperDef>,
    cursors: &mut stepper::Cursors,
) -> Vec<MacroStep> {
    match action {
        Action::Step {
            stepper: id,
            direction,
        } => cursors
            .step(steppers, id, *direction)
            .map(executor::compile_stepper_item)
            .unwrap_or_default(),
        other => executor::compile(other, macros),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Modifiers;

    fn binding(trigger: TriggerMode, action: Action) -> Binding {
        Binding { trigger, action }
    }

    fn keyboard(key: KeyCode) -> Action {
        Action::Keypress {
            modifiers: Modifiers::default(),
            key,
        }
    }

    const KBD: KeyCode = KeyCode::KEY_A;
    const MOUSE: KeyCode = KeyCode::BTN_LEFT;
    const PAD: KeyCode = KeyCode::BTN_SOUTH;

    /// The exhaustive `(TriggerMode × EventState × Action-shape × Option<Slot>)
    /// → TriggerDecision` table. This is the new decision surface — no tokio,
    /// no injector, no tempfile.
    #[test]
    fn decision_table() {
        use EventState::{Down, Repeat, Up};
        use TriggerDecision as D;
        use TriggerMode::{AnalogRepeat, FireOnce, HoldToRepeat, Toggle};

        // Most rows here are a Keypress / ControllerButton shape, so
        // `hold_repeat_kind` never reads the macro map. The single-key Macro
        // row below does — it shares this map, holding a `[KeyDown, KeyUp]`
        // macro that must classify exactly like the equivalent Keypress.
        use crate::config::MacroStepDto;
        let macros = macros_with(
            "hold-a",
            vec![MacroStepDto::KeyDown(KBD), MacroStepDto::KeyUp(KBD)],
        );
        let decide =
            |b: &Binding, s: EventState, slot: Option<Slot>| super::decide(b, &macros, s, slot);

        // Every slot state the overlap guard distinguishes.
        let slots = [
            None,
            Some(Slot::FiringUnfinished),
            Some(Slot::FiringFinished),
            Some(Slot::Toggle),
        ];
        // Whether the guard is clear for a given slot (only FiringUnfinished
        // blocks).
        let clear = |slot: Option<Slot>| !matches!(slot, Some(Slot::FiringUnfinished));

        for slot in slots {
            let guarded = |proceed: D| if clear(slot) { proceed } else { D::Nothing };

            // ── Fire-once (keyboard) ────────────────────────────────────────
            let fo = binding(FireOnce, keyboard(KBD));
            assert_eq!(decide(&fo, Down, slot), guarded(D::SpawnFireOnce));
            assert_eq!(decide(&fo, Repeat, slot), D::Nothing);
            assert_eq!(decide(&fo, Up, slot), D::ForceReleaseStuck);

            // ── Hold-to-repeat (keyboard): Down + every Repeat, not Up ──────
            let htr = binding(HoldToRepeat, keyboard(KBD));
            assert_eq!(decide(&htr, Down, slot), guarded(D::SpawnFireOnce));
            assert_eq!(decide(&htr, Repeat, slot), guarded(D::SpawnFireOnce));
            assert_eq!(decide(&htr, Up, slot), D::ForceReleaseStuck);

            // ── Analog-repeat rides the Hold-to-repeat arms ────────────────
            let ar = binding(AnalogRepeat, keyboard(KBD));
            assert_eq!(decide(&ar, Down, slot), guarded(D::SpawnFireOnce));
            assert_eq!(decide(&ar, Repeat, slot), guarded(D::SpawnFireOnce));
            assert_eq!(decide(&ar, Up, slot), D::ForceReleaseStuck);

            // ── ControllerButton Hold-to-repeat carve (tickets 75/76) ──────
            let cb_htr = binding(HoldToRepeat, Action::ControllerButton { button: PAD });
            assert_eq!(decide(&cb_htr, Down, slot), guarded(D::HoldKeyDown(PAD)));
            assert_eq!(decide(&cb_htr, Repeat, slot), D::Nothing);
            assert_eq!(decide(&cb_htr, Up, slot), D::ForceReleaseStuck);

            // ── mouse-button Hold-to-repeat carve (tickets 79/80) ──────────
            let mb_htr = binding(HoldToRepeat, keyboard(MOUSE));
            assert_eq!(decide(&mb_htr, Down, slot), guarded(D::HoldKeyDown(MOUSE)));
            assert_eq!(decide(&mb_htr, Repeat, slot), D::Nothing);
            assert_eq!(decide(&mb_htr, Up, slot), D::ForceReleaseStuck);

            // ── Toggle (keyboard): looping, Down only ──────────────────────
            let tg = binding(Toggle, keyboard(KBD));
            assert_eq!(decide(&tg, Down, slot), D::StartToggleLoop);
            assert_eq!(decide(&tg, Repeat, slot), D::Nothing);
            assert_eq!(decide(&tg, Up, slot), D::Nothing);

            // ── Toggle + mouse-button / ControllerButton → single held ─────
            let mb_tg = binding(Toggle, keyboard(MOUSE));
            assert_eq!(decide(&mb_tg, Down, slot), D::StartToggleHeld(MOUSE));
            let cb_tg = binding(Toggle, Action::ControllerButton { button: PAD });
            assert_eq!(decide(&cb_tg, Down, slot), D::StartToggleHeld(PAD));

            // ── AnalogRepeat + ControllerButton stays on the SpawnFireOnce
            //    arm — the bare-hold carve-outs are `HoldToRepeat`-only, just
            //    as in `fire` (the digital-sourced fallback). ───────────────
            let ar_cb = binding(AnalogRepeat, Action::ControllerButton { button: PAD });
            assert_eq!(decide(&ar_cb, Down, slot), guarded(D::SpawnFireOnce));
            assert_eq!(decide(&ar_cb, Repeat, slot), guarded(D::SpawnFireOnce));

            // ── Single-key Macro == the equivalent Keypress (ticket 03) ────
            //    `hold-a` compiles to `[KeyDown(A), KeyUp(A)]` — the same
            //    shape `keyboard(KBD)` produces. Every (mode, state) decision
            //    must match, arm for arm: this locks the "no behaviour change
            //    yet" contract that ticket 04 then deliberately breaks by
            //    routing `AutorepeatKey` onto the `value=2` path.
            let mac = binding(HoldToRepeat, macro_action("hold-a"));
            for state in [Down, Repeat, Up] {
                assert_eq!(
                    decide(&mac, state, slot),
                    decide(&binding(HoldToRepeat, keyboard(KBD)), state, slot),
                    "single-key Macro must decide like the equivalent Keypress ({state:?})"
                );
            }
            let mac_tg = binding(Toggle, macro_action("hold-a"));
            assert_eq!(
                decide(&mac_tg, Down, slot),
                decide(&binding(Toggle, keyboard(KBD)), Down, slot),
            );
            assert_eq!(decide(&mac_tg, Down, slot), D::StartToggleLoop);
        }
    }

    #[test]
    fn overlap_guard_only_blocks_on_an_unfinished_firing() {
        let no_macros: HashMap<MacroId, MacroDef> = HashMap::new();
        let decide =
            |b: &Binding, s: EventState, slot: Option<Slot>| super::decide(b, &no_macros, s, slot);
        let htr = binding(TriggerMode::HoldToRepeat, keyboard(KBD));
        assert_eq!(
            decide(&htr, EventState::Down, Some(Slot::FiringUnfinished)),
            TriggerDecision::Nothing
        );
        assert_eq!(
            decide(&htr, EventState::Down, Some(Slot::FiringFinished)),
            TriggerDecision::SpawnFireOnce
        );
        assert_eq!(
            decide(&htr, EventState::Down, None),
            TriggerDecision::SpawnFireOnce
        );
        // A Toggle slot does not block a fresh individual fire (the guard
        // only ever inspected the firings map).
        assert_eq!(
            decide(&htr, EventState::Down, Some(Slot::Toggle)),
            TriggerDecision::SpawnFireOnce
        );
    }

    /// The one place ticket 03's wiring is *not* verbatim-unchanged: a Macro
    /// that compiles to a single mouse-button press used to classify as
    /// `sustained_hold_key`'s `None` (Macro ⇒ never sustained) and loop /
    /// pulse; ticket 02's `hold_repeat_kind` deliberately made a single-key
    /// Macro classify like the equivalent Keypress, so it is now
    /// `SustainedNoRepeat(BTN_LEFT)` and takes the latched-hold arms — exactly
    /// what `keyboard(MOUSE)` already does. A `Keypress` on `BTN_LEFT` can
    /// never itself be a Macro, so no keyboard-key path regresses. Locked here
    /// so ticket 04+ don't silently move it again.
    #[test]
    fn single_mouse_button_macro_takes_the_sustained_hold_arms_like_the_equivalent_keypress() {
        use crate::config::MacroStepDto;
        use EventState::{Down, Repeat};
        let macros = macros_with(
            "hold-click",
            vec![MacroStepDto::KeyDown(MOUSE), MacroStepDto::KeyUp(MOUSE)],
        );
        let decide =
            |b: &Binding, s: EventState, slot: Option<Slot>| super::decide(b, &macros, s, slot);

        let mac_htr = binding(TriggerMode::HoldToRepeat, macro_action("hold-click"));
        let kbd_htr = binding(TriggerMode::HoldToRepeat, keyboard(MOUSE));
        assert_eq!(
            decide(&mac_htr, Down, None),
            decide(&kbd_htr, Down, None),
            "== the equivalent mouse-button Keypress"
        );
        assert_eq!(
            decide(&mac_htr, Down, None),
            TriggerDecision::HoldKeyDown(MOUSE)
        );
        assert_eq!(decide(&mac_htr, Repeat, None), TriggerDecision::Nothing);

        let mac_tg = binding(TriggerMode::Toggle, macro_action("hold-click"));
        assert_eq!(
            decide(&mac_tg, Down, None),
            TriggerDecision::StartToggleHeld(MOUSE),
        );
    }

    fn macro_action(id: &str) -> Action {
        Action::Macro {
            macro_id: MacroId::from(id),
        }
    }

    fn macros_with(
        id: &str,
        steps: Vec<crate::config::MacroStepDto>,
    ) -> HashMap<MacroId, MacroDef> {
        let mut macros = HashMap::new();
        macros.insert(
            MacroId::from(id),
            MacroDef {
                name: id.to_string(),
                steps,
            },
        );
        macros
    }

    #[test]
    fn hold_repeat_kind_classifies_keypress_controller_and_step() {
        let no_macros: HashMap<MacroId, MacroDef> = HashMap::new();

        // keyboard Keypress, plain + modified → AutorepeatKey off the action.
        assert_eq!(
            hold_repeat_kind(&keyboard(KBD), &no_macros),
            Some(HoldKind::AutorepeatKey(Modifiers::default(), KBD))
        );
        let mods = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            hold_repeat_kind(
                &Action::Keypress {
                    modifiers: mods,
                    key: KBD,
                },
                &no_macros
            ),
            Some(HoldKind::AutorepeatKey(mods, KBD))
        );

        // mouse-button Keypress and ControllerButton → SustainedNoRepeat.
        assert_eq!(
            hold_repeat_kind(&keyboard(MOUSE), &no_macros),
            Some(HoldKind::SustainedNoRepeat(MOUSE))
        );
        assert_eq!(
            hold_repeat_kind(&Action::ControllerButton { button: PAD }, &no_macros),
            Some(HoldKind::SustainedNoRepeat(PAD))
        );

        // Step → None (multi-step: each Repeat targets a different item).
        assert_eq!(
            hold_repeat_kind(
                &Action::Step {
                    stepper: StepperId::from("s"),
                    direction: crate::config::StepDirection::Forward,
                },
                &no_macros
            ),
            None
        );
    }

    #[test]
    fn hold_repeat_kind_single_key_macro_matches_the_equivalent_keypress() {
        use crate::config::MacroStepDto;
        let macros = macros_with(
            "hold-x",
            vec![
                MacroStepDto::KeyDown(KeyCode::KEY_X),
                MacroStepDto::KeyUp(KeyCode::KEY_X),
            ],
        );

        let via_macro = hold_repeat_kind(&macro_action("hold-x"), &macros);
        assert_eq!(
            via_macro,
            hold_repeat_kind(&keyboard(KeyCode::KEY_X), &macros),
            "a single-key Macro classifies exactly like the equivalent Keypress"
        );
        assert_eq!(
            via_macro,
            Some(HoldKind::AutorepeatKey(
                Modifiers::default(),
                KeyCode::KEY_X
            ))
        );
    }

    #[test]
    fn hold_repeat_kind_single_mouse_button_macro_matches_the_equivalent_keypress() {
        // A macro step accepts any `KeyCode`, `BTN_*` included — a single
        // mouse-button macro must classify as `SustainedNoRepeat`, exactly
        // like the equivalent mouse-button Keypress, not as an autorepeat key
        // (the kernel never autorepeats `BTN_*`).
        use crate::config::MacroStepDto;
        let macros = macros_with(
            "hold-click",
            vec![MacroStepDto::KeyDown(MOUSE), MacroStepDto::KeyUp(MOUSE)],
        );

        let via_macro = hold_repeat_kind(&macro_action("hold-click"), &macros);
        assert_eq!(
            via_macro,
            hold_repeat_kind(&keyboard(MOUSE), &macros),
            "a single mouse-button Macro classifies like the equivalent Keypress"
        );
        assert_eq!(via_macro, Some(HoldKind::SustainedNoRepeat(MOUSE)));
    }

    #[test]
    fn hold_repeat_kind_multi_step_macro_is_none() {
        use crate::config::MacroStepDto;
        let macros = macros_with(
            "combo",
            vec![
                MacroStepDto::KeyDown(KeyCode::KEY_A),
                MacroStepDto::KeyUp(KeyCode::KEY_A),
                MacroStepDto::KeyDown(KeyCode::KEY_B),
                MacroStepDto::KeyUp(KeyCode::KEY_B),
            ],
        );
        assert_eq!(hold_repeat_kind(&macro_action("combo"), &macros), None);
    }
}

#[cfg(test)]
mod slots {
    //! `Slots<K>` is the runtime half — an injector and a `tokio` runtime, but
    //! no `Config`, no D-Bus, no tempfile. The primary surface here is the
    //! **two deliberately-inverse tie-breaks** (`slot` firing-wins,
    //! `snapshot` toggle-wins), which used to be guarded only by a doc comment
    //! on `dispatch::slot_for`, plus `perform`'s decision → map-mutation table.

    use super::*;
    use crate::config::Modifiers;
    use crate::injector::{self, testing::RecordingSink};

    type Key = u32;
    const K: Key = 1;
    const BTN: KeyCode = KeyCode::BTN_LEFT;

    /// An injector wired to a recording sink plus the empty `macros` /
    /// `steppers` / `cursors` `perform` needs — no `Config` in sight.
    struct Fixture {
        inj: Injector,
        sink: RecordingSink,
        macros: HashMap<MacroId, MacroDef>,
        steppers: HashMap<StepperId, StepperDef>,
        cursors: stepper::Cursors,
    }

    impl Fixture {
        fn new() -> Self {
            let sink = RecordingSink::new();
            // The gamepad sink is the same recorder, so `BTN_LEFT` (routed to
            // the gamepad device) and keyboard codes land in one batch list.
            let (inj, _handle) = injector::spawn(sink.clone(), sink.clone());
            Fixture {
                inj,
                sink,
                macros: HashMap::new(),
                steppers: HashMap::new(),
                cursors: stepper::Cursors::default(),
            }
        }

        fn deps(&mut self) -> PerformDeps<'_> {
            PerformDeps {
                injector: &self.inj,
                macros: &self.macros,
                steppers: &self.steppers,
                cursors: &mut self.cursors,
                toggle_lap_target: executor::MIN_TOGGLE_LAP,
            }
        }

        fn key_events(&self) -> Vec<(KeyCode, i32)> {
            self.sink
                .batches()
                .iter()
                .flatten()
                .filter_map(|e| match e.destructure() {
                    evdev::EventSummary::Key(_, code, value) => Some((code, value)),
                    _ => None,
                })
                .collect()
        }
    }

    fn kbd_binding() -> Binding {
        Binding {
            trigger: TriggerMode::FireOnce,
            action: Action::Keypress {
                modifiers: Modifiers::default(),
                key: KeyCode::KEY_A,
            },
        }
    }

    /// A firing that never completes (a 1-hour `Delay` under paused time) —
    /// `is_finished()` stays false, so it reads as `FiringUnfinished`.
    fn unfinished_firing(inj: &Injector) -> FiringHandle {
        executor::spawn_fire_once(
            inj.clone(),
            vec![MacroStep::Delay(Duration::from_secs(3600))],
        )
    }

    /// A firing whose (empty) step list has already been walked —
    /// `is_finished()` is true, so it reads as `FiringFinished`.
    async fn finished_firing(inj: &Injector) -> FiringHandle {
        let handle = executor::spawn_fire_once(inj.clone(), Vec::new());
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(
            handle.is_finished(),
            "empty firing should finish immediately"
        );
        handle
    }

    fn held_toggle(inj: &Injector) -> ActiveToggle {
        ActiveToggle::spawn_held(inj.clone(), BTN)
    }

    #[tokio::test(start_paused = true)]
    async fn slot_is_firing_wins_and_snapshot_is_toggle_wins_across_every_combination() {
        let fx = Fixture::new();

        // ── The load-bearing case: one key present in BOTH maps ──────────────
        let mut both: Slots<Key> = Slots::default();
        both.firings.insert(K, unfinished_firing(&fx.inj));
        both.toggles.insert(K, held_toggle(&fx.inj));
        assert_eq!(
            both.slot(&K),
            Some(Slot::FiringUnfinished),
            "slot() is firing-wins — the overlap guard must see the firing"
        );
        assert_eq!(
            both.snapshot().get(&K),
            Some(&Slot::Toggle),
            "snapshot() is toggle-wins — chord::feed's second-completion stop needs the Toggle"
        );

        // ── firing only ─────────────────────────────────────────────────────
        let mut firing_only: Slots<Key> = Slots::default();
        firing_only.firings.insert(K, unfinished_firing(&fx.inj));
        assert_eq!(firing_only.slot(&K), Some(Slot::FiringUnfinished));
        assert_eq!(
            firing_only.snapshot().get(&K),
            Some(&Slot::FiringUnfinished)
        );

        // ── finished firing only ────────────────────────────────────────────
        let mut finished_only: Slots<Key> = Slots::default();
        finished_only
            .firings
            .insert(K, finished_firing(&fx.inj).await);
        assert_eq!(finished_only.slot(&K), Some(Slot::FiringFinished));
        assert_eq!(
            finished_only.snapshot().get(&K),
            Some(&Slot::FiringFinished)
        );

        // ── toggle only ─────────────────────────────────────────────────────
        let mut toggle_only: Slots<Key> = Slots::default();
        toggle_only.toggles.insert(K, held_toggle(&fx.inj));
        assert_eq!(toggle_only.slot(&K), Some(Slot::Toggle));
        assert_eq!(toggle_only.snapshot().get(&K), Some(&Slot::Toggle));

        // ── absent ──────────────────────────────────────────────────────────
        let absent: Slots<Key> = Slots::default();
        assert_eq!(absent.slot(&K), None);
        assert!(absent.snapshot().is_empty());

        for mut s in [both, firing_only, finished_only, toggle_only] {
            s.stop_all_toggles().await;
        }
    }

    /// The firing variants `slot()` can report for a key with a firing entry —
    /// `perform` never branches on whether the firing has finished walking
    /// (that is `decide`'s guard), so a test that only cares "a firing landed"
    /// accepts either.
    fn is_firing(slot: Option<Slot>) -> bool {
        matches!(slot, Some(Slot::FiringUnfinished | Slot::FiringFinished))
    }

    #[tokio::test(start_paused = true)]
    async fn perform_routes_each_decision_to_the_expected_map() {
        let mut fx = Fixture::new();
        let binding = kbd_binding();
        let mut slots: Slots<Key> = Slots::default();

        // Asserted through the public `slot()` / `snapshot()` reads, not the
        // private maps — `slot()` is firing-wins, so a firing key reports a
        // `Firing*` variant and a toggle-only key reports `Toggle`.
        slots
            .perform(TriggerDecision::Nothing, 10, &binding, fx.deps())
            .await
            .unwrap();
        assert_eq!(slots.slot(&10), None);
        assert!(slots.snapshot().is_empty());

        slots
            .perform(TriggerDecision::SpawnFireOnce, 20, &binding, fx.deps())
            .await
            .unwrap();
        assert!(
            is_firing(slots.slot(&20)),
            "SpawnFireOnce → the firings map"
        );
        assert!(is_firing(slots.snapshot().get(&20).copied()));

        slots
            .perform(TriggerDecision::HoldKeyDown(BTN), 30, &binding, fx.deps())
            .await
            .unwrap();
        assert!(is_firing(slots.slot(&30)), "HoldKeyDown → the firings map");

        slots
            .perform(TriggerDecision::StartToggleLoop, 40, &binding, fx.deps())
            .await
            .unwrap();
        assert_eq!(
            slots.slot(&40),
            Some(Slot::Toggle),
            "StartToggleLoop → the toggles map (no firing, so slot() falls back to Toggle)"
        );

        slots
            .perform(
                TriggerDecision::StartToggleHeld(BTN),
                50,
                &binding,
                fx.deps(),
            )
            .await
            .unwrap();
        assert_eq!(
            slots.slot(&50),
            Some(Slot::Toggle),
            "StartToggleHeld → the toggles map"
        );

        slots.stop_all_toggles().await;
    }

    #[tokio::test(start_paused = true)]
    async fn perform_is_unconditional_it_does_not_re_check_the_slot_state() {
        // `perform` runs the arm `decide` already chose — it never inspects a
        // pre-existing entry for the key (that guard is `decide`'s). Firing a
        // `SpawnFireOnce` at a key that already holds a Toggle adds to the
        // firings map and leaves the Toggle, and `slot()`'s firing-wins rule
        // then reports the fresh firing.
        let mut fx = Fixture::new();
        let binding = kbd_binding();
        let mut slots: Slots<Key> = Slots::default();
        slots.toggles.insert(K, held_toggle(&fx.inj));

        slots
            .perform(TriggerDecision::SpawnFireOnce, K, &binding, fx.deps())
            .await
            .unwrap();

        assert!(
            is_firing(slots.slot(&K)),
            "the fresh firing wins the slot() read"
        );
        assert_eq!(
            slots.snapshot().get(&K),
            Some(&Slot::Toggle),
            "but the Toggle is untouched — snapshot() still sees it"
        );

        slots.stop_all_toggles().await;
    }

    #[tokio::test(start_paused = true)]
    async fn perform_force_release_releases_a_stuck_hold_and_keeps_the_entry() {
        let mut fx = Fixture::new();
        let binding = kbd_binding();
        let mut slots: Slots<Key> = Slots::default();

        // A bare unbalanced KeyDown — the sustained-hold shape.
        slots
            .perform(TriggerDecision::HoldKeyDown(BTN), K, &binding, fx.deps())
            .await
            .unwrap();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(fx.key_events(), vec![(BTN, 1)]);
        assert!(slots.firings.contains_key(&K));

        slots
            .perform(TriggerDecision::ForceReleaseStuck, K, &binding, fx.deps())
            .await
            .unwrap();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(
            slots.firings.contains_key(&K),
            "force_release releases the held key but never removes the entry"
        );
        assert_eq!(
            fx.key_events(),
            vec![(BTN, 1), (BTN, 0)],
            "the stuck KeyDown is now balanced by a force-released KeyUp"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stop_toggle_removes_one_entry_and_reports_presence() {
        let fx = Fixture::new();
        let mut slots: Slots<Key> = Slots::default();
        slots.toggles.insert(K, held_toggle(&fx.inj));

        assert!(slots.stop_toggle(&K).await, "a Toggle was present");
        assert!(!slots.toggles.contains_key(&K), "and it is now removed");
        assert!(!slots.stop_toggle(&K).await, "second call finds nothing");
    }

    #[tokio::test(start_paused = true)]
    async fn stop_all_toggles_drains_every_entry() {
        let fx = Fixture::new();
        let mut slots: Slots<Key> = Slots::default();
        slots.toggles.insert(1, held_toggle(&fx.inj));
        slots.toggles.insert(2, held_toggle(&fx.inj));
        slots.toggles.insert(3, held_toggle(&fx.inj));

        slots.stop_all_toggles().await;

        assert!(slots.toggles.is_empty());
        assert!(slots.active_toggle_keys().next().is_none());
    }
}
