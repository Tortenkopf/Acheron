// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! Every Stepper list's Daemon-side runtime cursor (ticket 03/54,
//! post-release ticket 12 — CONTEXT.md: Stepper cursor) — carved out of
//! `dispatch.rs` as a pure, synchronous core so the wrap-around, the
//! clamp-on-shrink, the drop-on-delete and the default-to-first rules are
//! table-testable without spawning `run`, an injector, seven channels and a
//! tempfile.
//!
//! `step` advances one list's cursor and returns the newly-selected
//! `StepperItem`; `dispatch` compiles it via `executor::compile_stepper_item`
//! and spawns the firing. `reconcile` folds an `edit::plan` list-definition
//! change (delete / shrink / empty) back into the stored position.
//! `snapshot` is the `GetState()` read model. Nothing here does I/O, spawns a
//! task, or imports from `executor`, `edit`, `injector`, `chord`, `trigger`
//! or `dispatch`; it never sees a `Config`, a `Layer` or a `CaptureMode`.

use std::collections::HashMap;

use crate::config::{StepDirection, StepperDef, StepperId, StepperItem};

/// Every Stepper list's per-list runtime cursor position, keyed by
/// `StepperId` (ticket 03/54 — CONTEXT.md: Stepper cursor). A missing entry
/// is "at the list's first item" (index 0); a fresh dispatch task start
/// begins with the map empty, so a Daemon restart always resets every list
/// to its first item. Dispatch-internal — `DispatchState` owns the one
/// instance and it is never part of any module's interface.
#[derive(Default)]
pub(crate) struct Cursors {
    positions: HashMap<StepperId, usize>,
}

impl Cursors {
    /// Advances (`Forward`) or retreats (`Backward`) `id`'s cursor by one and
    /// returns the newly-selected item — "one motion moves the cursor and
    /// fires" (ticket 03's Answer). Wraps at either end. A zero-item list
    /// returns `None` with the cursor untouched: nothing to select, nothing
    /// to fire. A stored position past the current end (a shrink that landed
    /// with no intervening `reconcile`) is clamped before the step, never
    /// panics. `id` is assumed to name a real `StepperDef` — every
    /// `Action::Step` that reaches here is validated by `SetBinding` and by
    /// `config::parse`, so a missing entry is a bug, not a user error.
    pub(crate) fn step(
        &mut self,
        steppers: &HashMap<StepperId, StepperDef>,
        id: &StepperId,
        direction: StepDirection,
    ) -> Option<StepperItem> {
        let def = steppers.get(id).expect(
            "SetBinding/config::parse validate every Action::Step references an existing StepperDef",
        );
        let len = def.items.len();
        if len == 0 {
            return None;
        }
        let current = self.positions.get(id).copied().unwrap_or(0).min(len - 1);
        let next = match direction {
            StepDirection::Forward => (current + 1) % len,
            StepDirection::Backward => (current + len - 1) % len,
        };
        self.positions.insert(id.clone(), next);
        Some(def.items[next])
    }

    /// Folds a list-definition change back into the stored cursor (ticket
    /// 03/54) — `dispatch::run_effects`' handler for
    /// `edit::Effect::ReconcileStepperCursor`, run against the just-committed
    /// `Config`:
    ///
    /// - `id` absent from `steppers` (a `DeleteStepper` landed) — drop the
    ///   entry, so a later `CreateStepper` on the same freed slug starts at
    ///   the first item rather than inheriting a stale position.
    /// - list present but empty (`SetStepperItems` to `[]`) — drop the entry;
    ///   `snapshot` and `step` then both agree on index 0 for free.
    /// - list present and non-empty — clamp an existing position to
    ///   `items.len() - 1`; a no-op when there is no entry or it is already
    ///   in range.
    pub(crate) fn reconcile(&mut self, steppers: &HashMap<StepperId, StepperDef>, id: &StepperId) {
        match steppers.get(id) {
            None => {
                self.positions.remove(id);
            }
            Some(def) if def.items.is_empty() => {
                self.positions.remove(id);
            }
            Some(def) => {
                if let Some(position) = self.positions.get_mut(id) {
                    *position = (*position).min(def.items.len() - 1);
                }
            }
        }
    }

    /// Every library entry's reported cursor for `GetState()` (ticket 03/54)
    /// — one entry per `steppers` key, defaulting to `0` ("the list's first
    /// item") for a list this task has never stepped. Richer for the GUI than
    /// only reporting touched entries.
    pub(crate) fn snapshot(
        &self,
        steppers: &HashMap<StepperId, StepperDef>,
    ) -> HashMap<StepperId, usize> {
        steppers
            .keys()
            .map(|id| (id.clone(), self.positions.get(id).copied().unwrap_or(0)))
            .collect()
    }

    /// `id`'s current position (`0` if never stepped). Test-assertion surface
    /// only — production reads go through `snapshot`.
    #[cfg(test)]
    pub(crate) fn position(&self, id: &StepperId) -> usize {
        self.positions.get(id).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Modifiers;
    use evdev::KeyCode;

    fn key(code: KeyCode) -> StepperItem {
        StepperItem::Key {
            key: code,
            modifiers: Modifiers::default(),
        }
    }

    /// A `steppers` map with one list, `id` "wheel", holding `n` distinct
    /// `Key` items (`KEY_1`, `KEY_2`, …).
    fn one_list(n: usize) -> (HashMap<StepperId, StepperDef>, StepperId) {
        let id = StepperId::from("wheel");
        let codes = [
            KeyCode::KEY_1,
            KeyCode::KEY_2,
            KeyCode::KEY_3,
            KeyCode::KEY_4,
        ];
        let items = codes.iter().take(n).map(|&c| key(c)).collect();
        let mut map = HashMap::new();
        map.insert(
            id.clone(),
            StepperDef {
                name: "Wheel".into(),
                items,
            },
        );
        (map, id)
    }

    #[test]
    fn forward_advances_one_and_returns_the_new_item() {
        let (steppers, id) = one_list(3);
        let mut cursors = Cursors::default();
        assert_eq!(
            cursors.step(&steppers, &id, StepDirection::Forward),
            Some(key(KeyCode::KEY_2))
        );
        assert_eq!(cursors.position(&id), 1);
    }

    #[test]
    fn backward_from_the_first_item_wraps_to_the_last() {
        let (steppers, id) = one_list(3);
        let mut cursors = Cursors::default();
        assert_eq!(
            cursors.step(&steppers, &id, StepDirection::Backward),
            Some(key(KeyCode::KEY_3))
        );
        assert_eq!(cursors.position(&id), 2);
    }

    #[test]
    fn forward_from_the_last_item_wraps_to_the_first() {
        let (steppers, id) = one_list(3);
        let mut cursors = Cursors::default();
        cursors.step(&steppers, &id, StepDirection::Forward); // 0 -> 1
        cursors.step(&steppers, &id, StepDirection::Forward); // 1 -> 2
        assert_eq!(
            cursors.step(&steppers, &id, StepDirection::Forward), // 2 -> 0
            Some(key(KeyCode::KEY_1))
        );
        assert_eq!(cursors.position(&id), 0);
    }

    #[test]
    fn a_never_stepped_list_reads_as_position_zero() {
        let (_, id) = one_list(3);
        let cursors = Cursors::default();
        assert_eq!(cursors.position(&id), 0);
    }

    #[test]
    fn multi_step_tracks_the_position_across_calls() {
        let (steppers, id) = one_list(4);
        let mut cursors = Cursors::default();
        for expected in [1usize, 2, 3, 0, 1] {
            cursors.step(&steppers, &id, StepDirection::Forward);
            assert_eq!(cursors.position(&id), expected);
        }
    }

    #[test]
    fn stepping_a_zero_item_list_returns_none_and_leaves_the_cursor_untouched() {
        let (steppers, id) = one_list(0);
        let mut cursors = Cursors::default();
        assert_eq!(cursors.step(&steppers, &id, StepDirection::Forward), None);
        assert_eq!(cursors.position(&id), 0);
    }

    #[test]
    fn stepping_a_stored_position_past_a_shrunk_list_clamps_first_never_panics() {
        let (mut steppers, id) = one_list(4);
        let mut cursors = Cursors::default();
        for _ in 0..3 {
            cursors.step(&steppers, &id, StepDirection::Forward); // -> 3
        }
        assert_eq!(cursors.position(&id), 3);
        // Shrink the list under the cursor with no reconcile.
        steppers.get_mut(&id).unwrap().items.truncate(2);
        // current clamps to 1, forward wraps 1 -> 0.
        assert_eq!(
            cursors.step(&steppers, &id, StepDirection::Forward),
            Some(key(KeyCode::KEY_1))
        );
        assert_eq!(cursors.position(&id), 0);
    }

    #[test]
    fn reconcile_drops_the_entry_when_the_list_is_gone() {
        let (mut steppers, id) = one_list(3);
        let mut cursors = Cursors::default();
        cursors.step(&steppers, &id, StepDirection::Forward);
        steppers.remove(&id);
        cursors.reconcile(&steppers, &id);
        assert_eq!(cursors.position(&id), 0);
    }

    #[test]
    fn reconcile_drops_the_entry_when_the_list_is_emptied() {
        let (mut steppers, id) = one_list(3);
        let mut cursors = Cursors::default();
        cursors.step(&steppers, &id, StepDirection::Forward);
        assert_eq!(cursors.position(&id), 1);
        steppers.get_mut(&id).unwrap().items.clear();
        cursors.reconcile(&steppers, &id);
        assert_eq!(cursors.position(&id), 0);
        // Still reported by snapshot (the list exists, just empty) — at 0.
        assert_eq!(cursors.snapshot(&steppers)[&id], 0);
    }

    #[test]
    fn reconcile_clamps_a_position_stranded_by_a_shrink() {
        let (mut steppers, id) = one_list(4);
        let mut cursors = Cursors::default();
        for _ in 0..3 {
            cursors.step(&steppers, &id, StepDirection::Forward); // -> 3
        }
        steppers.get_mut(&id).unwrap().items.truncate(2);
        cursors.reconcile(&steppers, &id);
        assert_eq!(cursors.position(&id), 1);
    }

    #[test]
    fn reconcile_is_a_no_op_when_the_position_is_already_in_range_or_absent() {
        let (steppers, id) = one_list(3);
        let mut cursors = Cursors::default();
        cursors.step(&steppers, &id, StepDirection::Forward); // -> 1
        cursors.reconcile(&steppers, &id);
        assert_eq!(
            cursors.position(&id),
            1,
            "an in-range position is untouched"
        );

        // A list this task has never stepped — no entry to clamp.
        let untouched = StepperId::from("untouched");
        let mut with_untouched = steppers.clone();
        with_untouched.insert(
            untouched.clone(),
            StepperDef {
                name: "U".into(),
                items: vec![key(KeyCode::KEY_1)],
            },
        );
        cursors.reconcile(&with_untouched, &untouched);
        assert_eq!(cursors.position(&untouched), 0);
    }

    #[test]
    fn snapshot_reports_one_entry_per_list_defaulting_to_zero() {
        let id_a = StepperId::from("a");
        let id_b = StepperId::from("b");
        let mut steppers = HashMap::new();
        steppers.insert(
            id_a.clone(),
            StepperDef {
                name: "A".into(),
                items: vec![key(KeyCode::KEY_1), key(KeyCode::KEY_2)],
            },
        );
        steppers.insert(
            id_b.clone(),
            StepperDef {
                name: "B".into(),
                items: vec![key(KeyCode::KEY_3), key(KeyCode::KEY_4)],
            },
        );

        let mut cursors = Cursors::default();
        cursors.step(&steppers, &id_a, StepDirection::Forward); // a -> 1

        let snap = cursors.snapshot(&steppers);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[&id_a], 1);
        assert_eq!(snap[&id_b], 0);
    }
}
