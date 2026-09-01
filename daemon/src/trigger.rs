// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The Trigger-mode firing matrix (ticket 17, post-release ticket 08) — carved
//! out of `dispatch.rs` as a pure, synchronous core so the recurring
//! `(TriggerMode, EventState, Action-shape)` carve-outs (tickets 75, 76, 78,
//! 79, 80, 82) stop being re-tuned in two near-verbatim `match` bodies.
//!
//! `decide` answers one question — what does a `(Binding, EventState,
//! slot-liveness)` triple do? — with a data-only `TriggerDecision` the
//! `dispatch`-side executor (`dispatch::perform_trigger`) then performs
//! against the runtime state it owns, keyed by `Input` for the individual
//! path and by `ChordKey` for the Chord path. `decide` does no I/O, spawns no
//! task, takes no `&Injector`, and imports nothing from `executor`,
//! `injector`, or `edit`. It replaces the old `dispatch::fire` and
//! `dispatch::execute_chord_fire` (ex-`fire_chord`), whose `match` bodies were
//! arm-for-arm identical.
//!
//! `force_release_stuck` / `stop_toggle` at the bottom are the one place this
//! module touches `executor` / `injector` types — they are shared executor
//! helpers, deliberately *not* part of the pure core, kept here only because
//! both the `Input` path and the `ChordKey` path call them and a shared home
//! beats a third hand-rolled copy of two lines.

use std::collections::HashMap;
use std::hash::Hash;

use evdev::KeyCode;

use crate::capture::EventState;
use crate::config::{Action, Binding, TriggerMode};
use crate::executor::{ActiveToggle, FiringHandle};
use crate::injector::Injector;
use crate::input::is_mouse_button;

/// Liveness of one firing/toggle slot, passed IN to `decide` rather than
/// held — the function stays pure, tests construct it directly. Absent
/// (`None`) means no live firing or toggle for that key. Folds in the old
/// `chord::ChordSlot` verbatim (same three states); `chord::feed` and
/// `dispatch::chord_slots` now take `trigger::Slot`.
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

/// The `KeyCode` behind the two Action shapes that get *sustained-hold*
/// treatment — a real mouse button (`Action::Keypress` on a `BTN_*` code) or a
/// gamepad button (`Action::ControllerButton`) — instead of the ordinary
/// pulse / repeat-tap: a bare unbalanced `KeyDown` under Hold-to-repeat
/// (tickets 75/76, 79/80) and `spawn_held`'s single hold under Toggle
/// (tickets 78, 82/83). `None` for a keyboard Keypress, a Macro, or a Step.
fn sustained_hold_key(action: &Action) -> Option<KeyCode> {
    match action {
        Action::ControllerButton { button } => Some(*button),
        Action::Keypress { key, .. } if is_mouse_button(*key) => Some(*key),
        _ => None,
    }
}

/// The pure decision core. `binding` carries `.trigger` and `.action`; `slot`
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
pub(crate) fn decide(binding: &Binding, state: EventState, slot: Option<Slot>) -> TriggerDecision {
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
    let hold_key = sustained_hold_key(&binding.action);

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

/// Ticket 33's force-release, factored out of the individual `Up` arm, the
/// Chord `ReleaseChordFiring` effect, and the Chord `ForceReleaseIndividual`
/// effect — three hand-rolled copies of the same two lines. Releases (but
/// never removes) `key`'s firing entry: a balanced Macro has already
/// self-released (`held` empty, a no-op); a bare unbalanced `KeyDown` (a
/// `HoldKeyDown` decision) is exactly what this cleans up. The entry lingers
/// so a later `FiringFinished` slot stays distinct from `None`.
pub(crate) async fn force_release_stuck<K: Eq + Hash>(
    firings: &HashMap<K, FiringHandle>,
    key: &K,
    injector: &Injector,
) {
    if let Some(firing) = firings.get(key) {
        firing.force_release_stuck(injector).await;
    }
}

/// Stops and removes `key`'s Toggle, awaiting its force-release. Shared by
/// `handle_event`'s inline toggle-stop-on-`Down` (whose `bool` return drives
/// the "this press is consumed by the stop" early return) and the Chord
/// `StopChordToggle` effect. Returns whether a Toggle was actually present.
pub(crate) async fn stop_toggle<K: Eq + Hash>(
    toggles: &mut HashMap<K, ActiveToggle>,
    key: &K,
) -> bool {
    match toggles.remove(key) {
        Some(toggle) => {
            toggle.stop().await;
            true
        }
        None => false,
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
        }
    }

    #[test]
    fn overlap_guard_only_blocks_on_an_unfinished_firing() {
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
}
