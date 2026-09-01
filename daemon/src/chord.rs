// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The Chord-detection state machine (ticket 01/40, post-release ticket 07) —
//! carved out of `dispatch.rs` as a pure, synchronous core so the recurring
//! hardware-tuned timing rules (tickets 40, 67, the thumbstick-diagonal
//! worked example) can be tested without spawning `run`, an injector, seven
//! channels and a tempfile.
//!
//! `feed` routes one `PhysicalEvent` (a fresh chord-eligible `Down`, or any
//! later `Repeat`/`Up` for an Input the machine still owns) and `tick` fires
//! the window-timeout; both answer with a `Vec<ChordEffect>` the `dispatch`
//! executor performs against the runtime state it owns (the `ChordKey`-keyed
//! `FiringHandle` / `ActiveToggle` maps, `&Injector`, the config transaction).
//! Nothing here does I/O, spawns a task, touches a channel, or takes an
//! `&Injector` — the module imports nothing from `executor`, `injector`, or
//! `edit`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

use tokio::time::Instant;

use crate::capture::{EventState, PhysicalEvent};
use crate::config::{Binding, ChordKey, TriggerMode};
use crate::input::Input;

/// The fixed simultaneity window between a Chord's first and last member
/// going down (ticket 01's Answer, §"Simultaneity detection") — a Rust
/// constant, deliberately not a persisted `Config` value or a v1.0 user
/// setting. Moved here verbatim from `dispatch.rs` (post-release ticket 07).
const CHORD_WINDOW: Duration = Duration::from_millis(50);

/// The currently-developing press-combo a Chord may complete from (ticket
/// 01/40): every chord-eligible Input pressed since the window opened, and
/// the absolute instant it closes. At most one window is ever open at a
/// time — a fresh chord-eligible Down either joins the existing window or,
/// if none is open, starts a new one. Moved here verbatim from `dispatch.rs`.
struct ChordWindow {
    // `BTreeSet`, not `HashSet`: compared directly against a `ChordKey`'s
    // own `BTreeSet<Input>` membership via `is_subset` below, which requires
    // the same set type on both sides.
    down: BTreeSet<Input>,
    deadline: Instant,
}

/// The Chord-detection state machine's own state — pure bookkeeping only.
/// Reset fresh on every dispatch task start, same as the old `ChordState`.
/// The `ChordKey`-keyed firing/toggle *handles* are NOT here — they stay in
/// `dispatch::ChordRuntime` and their liveness is passed into every `feed`
/// call as a `ChordSlot` snapshot.
#[derive(Default)]
pub struct ChordMachine {
    window: Option<ChordWindow>,
    /// Every Input currently "owned" by the Chord machinery — either still
    /// inside an open window, or physically held down as a member of a
    /// Chord that has since fired. Routes that Input's later Repeat/Up
    /// events back through the Chord path rather than the ordinary
    /// per-Input one, even after a fresh membership check would otherwise
    /// still call it chord-eligible (ticket 01: "the remaining member(s)
    /// don't fall back to their individual Bindings until they're released
    /// and re-pressed fresh").
    claimed: HashSet<Input>,
}

/// Liveness of one Chord's dispatch-side firing/toggle, passed IN to `feed`
/// rather than held by the machine — the machine stays pure, tests construct
/// the snapshot map directly. An absent key means no live firing or toggle
/// for that Chord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChordSlot {
    /// An active Toggle-mode Chord.
    Toggle,
    /// A Fire-once / Hold-to-repeat Chord firing still in flight.
    FiringUnfinished,
    /// A Fire-once Chord whose firing has already completed on its own — the
    /// map entry lingers (never cleaned, mirroring `fire`'s own `in_flight`),
    /// so this must be a distinct state from "absent" or the Chord could
    /// never complete again.
    FiringFinished,
}

impl ChordSlot {
    /// Whether this slot represents a live (or lingering-but-finished)
    /// Fire-once / Hold-to-repeat firing — i.e. bare presence in dispatch's
    /// `ChordRuntime::firings` map, the old `chord_in_flight.contains_key`
    /// check.
    fn is_firing(&self) -> bool {
        matches!(
            self,
            ChordSlot::FiringUnfinished | ChordSlot::FiringFinished
        )
    }
}

/// A post-decision effect the `dispatch` executor must perform. Ordering
/// within a returned `Vec<ChordEffect>` is significant and preserves the
/// pre-carve behaviour: completions (`FireChord`) before re-completion stops
/// (`StopChordToggle`); a `FireIndividual` immediately followed by its
/// `ForceReleaseIndividual` on the early-release path.
#[derive(Debug, Clone, PartialEq)]
pub enum ChordEffect {
    /// Run this Chord's own Trigger-mode dispatch (the old `fire_chord`),
    /// keyed by member set.
    FireChord {
        key: ChordKey,
        binding: Binding,
        state: EventState,
    },
    /// Force-release this Chord's in-flight Fire-once / Hold-to-repeat firing
    /// on a completed member's physical `Up` (the old `release_chord_firing`).
    ReleaseChordFiring { key: ChordKey },
    /// Stop this Chord's active Toggle — its full member set completed again
    /// (the Toggle's own "second Down").
    StopChordToggle { key: ChordKey },
    /// Fire `input`'s own individual Binding as a synthetic fresh Down (the
    /// window elapsed, or the member was released before completing). The
    /// executor resolves it against the active Layer via
    /// `dispatch::dispatch_individual_down`, which is also the ordinary
    /// input path's Down tail — so a member whose individual Binding is
    /// `Action::ProfileSwitch` still switches the Profile, via the
    /// `edit::Edit::SwitchProfile` that helper returns.
    FireIndividual { input: Input },
    /// Immediately force-release whatever the preceding `FireIndividual`
    /// started — emitted only on the early-release path, never on a timeout.
    /// The machine knows which case it is, so it states it rather than making
    /// the executor re-derive it.
    ForceReleaseIndividual { input: Input },
}

/// The answer `feed` / `tick` give the caller.
#[derive(Debug, Clone, PartialEq)]
pub enum ChordOutcome {
    /// Not an event the Chord machine owns — `handle_event` falls through to
    /// ordinary Binding lookup. Replaces `handle_event`'s old inline
    /// `chord_state.claimed` / `chord_keys_containing` check.
    NotMine,
    Handled(Vec<ChordEffect>),
}

/// Routes one `PhysicalEvent` through the Chord machine and updates the
/// window bookkeeping. `chords` is `profile.chords(active_layer)` — the
/// machine needs no other `Config` view (macro / stepper / individual-Binding
/// lookup is the executor's job). `live` is the per-`ChordKey` liveness
/// snapshot `dispatch` derives from its `ChordRuntime`.
pub fn feed(
    machine: &mut ChordMachine,
    chords: &HashMap<ChordKey, Binding>,
    live: &HashMap<ChordKey, ChordSlot>,
    event: PhysicalEvent,
) -> ChordOutcome {
    let owned = machine.claimed.contains(&event.input);
    let fresh_chord_down =
        event.state == EventState::Down && chords_with_member(chords, event.input).next().is_some();
    if !owned && !fresh_chord_down {
        return ChordOutcome::NotMine;
    }

    let effects = match event.state {
        EventState::Down => feed_down(machine, chords, live, event.input),
        EventState::Repeat => feed_repeat(chords, live, event.input),
        EventState::Up => feed_up(machine, chords, live, event.input),
    };
    ChordOutcome::Handled(effects)
}

/// A fresh chord-eligible `Down`: joins (or opens) the window, then completes
/// every Chord whose full member set is now down in one pass. Firing one can
/// only ever *shrink* `down` (its own members are removed), never grow it, so
/// a single completion pass is enough; a single Down can complete more than
/// one Chord at once when an Input belongs to several of them (ticket 01's
/// amended Answer — the thumbstick-diagonal worked example).
fn feed_down(
    machine: &mut ChordMachine,
    chords: &HashMap<ChordKey, Binding>,
    live: &HashMap<ChordKey, ChordSlot>,
    input: Input,
) -> Vec<ChordEffect> {
    machine.claimed.insert(input);
    let window = machine.window.get_or_insert_with(|| ChordWindow {
        down: BTreeSet::new(),
        deadline: Instant::now() + CHORD_WINDOW,
    });
    window.down.insert(input);
    let down_snapshot = window.down.clone();

    // A stale-but-*finished* firing entry must not permanently exclude a
    // FireOnce/HoldToRepeat Chord from ever completing again — the executor
    // only force-releases it, it never removes the map entry (mirroring
    // `fire`'s own single-Input `in_flight`), so a `FiringFinished` slot is
    // treated as "may complete again", only `FiringUnfinished` / `Toggle`
    // block a fresh completion.
    let starting: Vec<(ChordKey, Binding)> = chords
        .iter()
        .filter(|(key, _)| {
            !matches!(
                live.get(*key),
                Some(ChordSlot::Toggle | ChordSlot::FiringUnfinished)
            ) && key.members().is_subset(&down_snapshot)
        })
        .map(|(key, binding)| (key.clone(), binding.clone()))
        .collect();

    // A Toggle Chord that's already active and whose full member set just
    // completed *again* is the Toggle's own "second Down" — stops it,
    // mirroring a single Input's own Toggle (ticket 67 correction).
    let stopping: Vec<ChordKey> = chords
        .keys()
        .filter(|key| {
            matches!(live.get(*key), Some(ChordSlot::Toggle))
                && key.members().is_subset(&down_snapshot)
        })
        .cloned()
        .collect();

    let mut effects = Vec::new();
    for (key, binding) in starting {
        effects.push(ChordEffect::FireChord {
            key: key.clone(),
            binding,
            state: EventState::Down,
        });
        clear_members(machine, &key);
    }
    for key in stopping {
        effects.push(ChordEffect::StopChordToggle { key: key.clone() });
        clear_members(machine, &key);
    }
    close_window_if_drained(machine);
    effects
}

/// A `Repeat` on an Input the machine owns. A still-pending (not yet
/// completed) member is "held, not fired" — Repeat is a no-op for it,
/// mirroring `fire`'s own FireOnce/Toggle handling of Repeat. Only a member
/// of an already-active Hold-to-repeat Chord re-fires, and only the member
/// sorted first by `ChordKey`'s `BTreeSet` ordering drives it: while a Chord
/// is active every member stays physically down, so the kernel independently
/// autorepeats each of them, and re-firing on any member's Repeat would make
/// an N-member Chord repeat up to N times as fast as a single Input ever
/// would (hardware-verified regression, ticket 67).
fn feed_repeat(
    chords: &HashMap<ChordKey, Binding>,
    live: &HashMap<ChordKey, ChordSlot>,
    input: Input,
) -> Vec<ChordEffect> {
    chords_with_member(chords, input)
        .filter(|(key, _)| key.members().iter().next() == Some(&input))
        .filter(|(key, _)| live.get(*key).is_some_and(ChordSlot::is_firing))
        .filter(|(_, binding)| binding.trigger == TriggerMode::HoldToRepeat)
        .map(|(key, binding)| ChordEffect::FireChord {
            key: key.clone(),
            binding: binding.clone(),
            state: EventState::Repeat,
        })
        .collect()
}

/// An `Up` on an Input the machine owns. A still-pending member resolves
/// right now rather than waiting out the rest of the window on a key that's
/// no longer even down (ticket 01: a pending member always eventually fires
/// retroactively — an early release just means "now" instead of "at the
/// deadline"), then immediately force-releases whatever that retroactive Down
/// started. A member that already completed a Chord instead force-releases
/// that Chord's in-flight Fire-once / Hold-to-repeat firing (Toggle Chords
/// are deliberately untouched — ticket 67).
fn feed_up(
    machine: &mut ChordMachine,
    chords: &HashMap<ChordKey, Binding>,
    live: &HashMap<ChordKey, ChordSlot>,
    input: Input,
) -> Vec<ChordEffect> {
    machine.claimed.remove(&input);

    let was_pending = machine
        .window
        .as_mut()
        .is_some_and(|window| window.down.remove(&input));
    if was_pending {
        close_window_if_drained(machine);
        return vec![
            ChordEffect::FireIndividual { input },
            ChordEffect::ForceReleaseIndividual { input },
        ];
    }

    chords_with_member(chords, input)
        .filter(|(key, _)| live.get(*key).is_some_and(ChordSlot::is_firing))
        .map(|(key, _)| ChordEffect::ReleaseChordFiring { key: key.clone() })
        .collect()
}

/// The window deadline elapsed with members still unresolved (ticket 01's
/// Answer): every Input still in `down` never completed a Chord, so each
/// fires its own individual Binding retroactively — delayed by the window,
/// exactly as designed. Members already claimed by a fired Chord were removed
/// from `down` when they fired, so this only ever touches genuinely-pending
/// ones. `now` guards against a spurious call before the deadline; in `run`
/// the `select!` `sleep_until` branch guarantees `now >= deadline`.
pub fn tick(machine: &mut ChordMachine, now: Instant) -> ChordOutcome {
    let elapsed = machine
        .window
        .as_ref()
        .is_some_and(|window| now >= window.deadline);
    if !elapsed {
        return ChordOutcome::Handled(Vec::new());
    }
    let window = machine.window.take().expect("checked Some above");
    let effects = window
        .down
        .into_iter()
        .map(|input| {
            machine.claimed.remove(&input);
            ChordEffect::FireIndividual { input }
        })
        .collect();
    ChordOutcome::Handled(effects)
}

/// The active window's deadline, or `None`. The `run` loop's `select!`
/// timeout branch arms on this (replacing the old `chord_window_deadline`).
pub fn next_deadline(machine: &ChordMachine) -> Option<Instant> {
    machine.window.as_ref().map(|window| window.deadline)
}

/// Removes `key`'s members from the open window (if any) once it has fired or
/// stopped — the completion pass works against a snapshot taken up front, so
/// this only shrinks the live set.
fn clear_members(machine: &mut ChordMachine, key: &ChordKey) {
    if let Some(window) = machine.window.as_mut() {
        for member in key.members() {
            window.down.remove(member);
        }
    }
}

/// Closes the window once its last member has fired, stopped, or been
/// released — every path that drains `down` ends by calling this.
fn close_window_if_drained(machine: &mut ChordMachine) {
    if machine.window.as_ref().is_some_and(|w| w.down.is_empty()) {
        machine.window = None;
    }
}

/// Every `(ChordKey, Binding)` in `chords` with `input` among its members —
/// an Input may belong to any number of Chords (ticket 01's amended Answer).
/// The old `chord_keys_containing` free function, as a borrowing iterator.
fn chords_with_member(
    chords: &HashMap<ChordKey, Binding>,
    input: Input,
) -> impl Iterator<Item = (&ChordKey, &Binding)> {
    chords
        .iter()
        .filter(move |(key, _)| key.members().contains(&input))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Action;
    use crate::input::Direction;
    use evdev::KeyCode;

    fn chord<const N: usize>(members: [Input; N]) -> ChordKey {
        ChordKey::new(members.into_iter().collect())
    }

    fn keypress(trigger: TriggerMode, key: KeyCode) -> Binding {
        Binding {
            trigger,
            action: Action::Keypress {
                modifiers: crate::config::Modifiers::default(),
                key,
            },
        }
    }

    fn fire_once(key: KeyCode) -> Binding {
        keypress(TriggerMode::FireOnce, key)
    }

    fn down(input: Input) -> PhysicalEvent {
        PhysicalEvent {
            input,
            state: EventState::Down,
            depth: None,
        }
    }

    fn repeat(input: Input) -> PhysicalEvent {
        PhysicalEvent {
            input,
            state: EventState::Repeat,
            depth: None,
        }
    }

    fn up(input: Input) -> PhysicalEvent {
        PhysicalEvent {
            input,
            state: EventState::Up,
            depth: None,
        }
    }

    fn handled(outcome: ChordOutcome) -> Vec<ChordEffect> {
        match outcome {
            ChordOutcome::Handled(effects) => effects,
            ChordOutcome::NotMine => panic!("expected Handled, got NotMine"),
        }
    }

    const G11: Input = Input::Grid(1, 1);
    const G12: Input = Input::Grid(1, 2);
    const G13: Input = Input::Grid(1, 3);

    #[test]
    fn an_event_for_an_unowned_non_member_is_not_mine() {
        let mut machine = ChordMachine::default();
        let chords = HashMap::from([(chord([G11, G12]), fire_once(KeyCode::KEY_C))]);
        assert_eq!(
            feed(
                &mut machine,
                &chords,
                &HashMap::new(),
                down(Input::Grid(4, 4))
            ),
            ChordOutcome::NotMine
        );
        // A Repeat/Up for a member never claimed is also not ours — the
        // machine only owns an Input once its own Down opened a window.
        assert_eq!(
            feed(&mut machine, &chords, &HashMap::new(), repeat(G11)),
            ChordOutcome::NotMine
        );
        assert_eq!(
            feed(&mut machine, &chords, &HashMap::new(), up(G11)),
            ChordOutcome::NotMine
        );
    }

    #[test]
    fn a_member_down_opens_a_window_but_does_not_complete_alone() {
        let mut machine = ChordMachine::default();
        let chords = HashMap::from([(chord([G11, G12]), fire_once(KeyCode::KEY_C))]);
        let effects = handled(feed(&mut machine, &chords, &HashMap::new(), down(G11)));
        assert!(effects.is_empty());
        assert!(next_deadline(&machine).is_some());
    }

    #[test]
    fn the_completing_member_down_fires_the_chord_and_closes_the_window() {
        let key = chord([G11, G12]);
        let binding = fire_once(KeyCode::KEY_C);
        let mut machine = ChordMachine::default();
        let chords = HashMap::from([(key.clone(), binding.clone())]);
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G11)));
        let effects = handled(feed(&mut machine, &chords, &HashMap::new(), down(G12)));
        assert_eq!(
            effects,
            vec![ChordEffect::FireChord {
                key,
                binding,
                state: EventState::Down,
            }]
        );
        assert_eq!(
            next_deadline(&machine),
            None,
            "window closes once nothing pends"
        );
    }

    #[test]
    fn thumbstick_diagonals_fire_independently_and_share_a_member() {
        // Moved from the dispatch harness: Up is reusable across both
        // diagonals once released and re-pressed fresh (ticket 01's Answer).
        let up_r = chord([
            Input::Thumbstick(Direction::Up),
            Input::Thumbstick(Direction::Right),
        ]);
        let up_l = chord([
            Input::Thumbstick(Direction::Up),
            Input::Thumbstick(Direction::Left),
        ]);
        let b1 = fire_once(KeyCode::KEY_1);
        let b2 = fire_once(KeyCode::KEY_2);
        let chords = HashMap::from([(up_r.clone(), b1.clone()), (up_l.clone(), b2.clone())]);
        let mut machine = ChordMachine::default();

        handled(feed(
            &mut machine,
            &chords,
            &HashMap::new(),
            down(Input::Thumbstick(Direction::Up)),
        ));
        let effects = handled(feed(
            &mut machine,
            &chords,
            &HashMap::new(),
            down(Input::Thumbstick(Direction::Right)),
        ));
        assert_eq!(
            effects,
            vec![ChordEffect::FireChord {
                key: up_r,
                binding: b1,
                state: EventState::Down,
            }]
        );

        // Release both members — Up falls out of `claimed`, ready to be
        // re-pressed fresh for the other diagonal.
        handled(feed(
            &mut machine,
            &chords,
            &HashMap::new(),
            up(Input::Thumbstick(Direction::Up)),
        ));
        handled(feed(
            &mut machine,
            &chords,
            &HashMap::new(),
            up(Input::Thumbstick(Direction::Right)),
        ));

        handled(feed(
            &mut machine,
            &chords,
            &HashMap::new(),
            down(Input::Thumbstick(Direction::Up)),
        ));
        let effects = handled(feed(
            &mut machine,
            &chords,
            &HashMap::new(),
            down(Input::Thumbstick(Direction::Left)),
        ));
        assert_eq!(
            effects,
            vec![ChordEffect::FireChord {
                key: up_l,
                binding: b2,
                state: EventState::Down,
            }]
        );
    }

    #[test]
    fn one_down_completes_every_subset_and_superset_sharing_it() {
        // ticket 01's amended Answer: pressing the shared last member with
        // several nested Chords already down fires all of them in one pass.
        let ab = chord([G11, G12]);
        let ac = chord([G11, G13]);
        let abc = chord([G11, G12, G13]);
        let chords = HashMap::from([
            (ab.clone(), fire_once(KeyCode::KEY_1)),
            (ac.clone(), fire_once(KeyCode::KEY_2)),
            (abc.clone(), fire_once(KeyCode::KEY_3)),
        ]);
        let mut machine = ChordMachine::default();
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G12)));
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G13)));
        let effects = handled(feed(&mut machine, &chords, &HashMap::new(), down(G11)));

        let fired: HashSet<ChordKey> = effects
            .iter()
            .map(|effect| match effect {
                ChordEffect::FireChord { key, state, .. } => {
                    assert_eq!(*state, EventState::Down);
                    key.clone()
                }
                other => panic!("expected only FireChord, got {other:?}"),
            })
            .collect();
        assert_eq!(fired, HashSet::from([ab, ac, abc]));
        assert_eq!(next_deadline(&machine), None);
    }

    #[test]
    fn hold_to_repeat_chord_refires_only_on_the_leader_members_repeat() {
        // Moved from the dispatch harness (ticket 67). `G11` sorts first in
        // the member `BTreeSet`, so only its Repeat re-fires.
        let key = chord([G11, G12]);
        let binding = keypress(TriggerMode::HoldToRepeat, KeyCode::KEY_C);
        let chords = HashMap::from([(key.clone(), binding.clone())]);
        let mut machine = ChordMachine::default();
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G11)));
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G12)));

        let live = HashMap::from([(key.clone(), ChordSlot::FiringUnfinished)]);
        // Non-leader Repeat: no-op.
        assert!(handled(feed(&mut machine, &chords, &live, repeat(G12))).is_empty());
        // Leader Repeat: re-fires.
        assert_eq!(
            handled(feed(&mut machine, &chords, &live, repeat(G11))),
            vec![ChordEffect::FireChord {
                key,
                binding,
                state: EventState::Repeat,
            }]
        );
    }

    #[test]
    fn toggle_chord_survives_releasing_one_member_and_stops_on_a_fresh_completion() {
        // Moved from the dispatch harness (ticket 67): a Toggle Chord stays
        // on past a single member's release and stops only when the full
        // member set completes again.
        let key = chord([G11, G12]);
        let binding = keypress(TriggerMode::Toggle, KeyCode::KEY_LEFTCTRL);
        let chords = HashMap::from([(key.clone(), binding.clone())]);
        let mut machine = ChordMachine::default();
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G11)));
        let effects = handled(feed(&mut machine, &chords, &HashMap::new(), down(G12)));
        assert_eq!(
            effects,
            vec![ChordEffect::FireChord {
                key: key.clone(),
                binding: binding.clone(),
                state: EventState::Down,
            }]
        );

        let live = HashMap::from([(key.clone(), ChordSlot::Toggle)]);
        // Releasing one completed member does nothing to a Toggle Chord.
        assert!(handled(feed(&mut machine, &chords, &live, up(G11))).is_empty());

        // A fresh completion of the full set stops it.
        handled(feed(&mut machine, &chords, &live, down(G11)));
        let effects = handled(feed(&mut machine, &chords, &live, down(G12)));
        assert_eq!(effects, vec![ChordEffect::StopChordToggle { key }]);
    }

    #[test]
    fn a_fire_once_chord_fires_again_after_being_fully_released_and_re_pressed() {
        // Moved from the dispatch harness: a lingering *finished* firing slot
        // must not permanently exclude the Chord from completing again.
        let key = chord([G11, G12]);
        let binding = fire_once(KeyCode::KEY_C);
        let chords = HashMap::from([(key.clone(), binding.clone())]);
        let mut machine = ChordMachine::default();
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G11)));
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G12)));

        // The firing has finished on its own; its map entry lingers.
        let live = HashMap::from([(key.clone(), ChordSlot::FiringFinished)]);
        assert_eq!(
            handled(feed(&mut machine, &chords, &live, up(G11))),
            vec![ChordEffect::ReleaseChordFiring { key: key.clone() }]
        );
        handled(feed(&mut machine, &chords, &live, up(G12)));

        handled(feed(&mut machine, &chords, &live, down(G11)));
        let effects = handled(feed(&mut machine, &chords, &live, down(G12)));
        assert_eq!(
            effects,
            vec![ChordEffect::FireChord {
                key,
                binding,
                state: EventState::Down,
            }]
        );
    }

    #[test]
    fn an_early_release_of_a_pending_member_fires_and_force_releases_it() {
        let key = chord([G11, G12]);
        let chords = HashMap::from([(key, fire_once(KeyCode::KEY_C))]);
        let mut machine = ChordMachine::default();
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G11)));
        let effects = handled(feed(&mut machine, &chords, &HashMap::new(), up(G11)));
        assert_eq!(
            effects,
            vec![
                ChordEffect::FireIndividual { input: G11 },
                ChordEffect::ForceReleaseIndividual { input: G11 },
            ]
        );
        assert_eq!(next_deadline(&machine), None);
    }

    #[test]
    fn the_window_timeout_fires_every_still_pending_member_retroactively() {
        // A 3-member Chord with only two members down never completes, so the
        // window times out with both still pending.
        let key = chord([G11, G12, G13]);
        let chords = HashMap::from([(key, fire_once(KeyCode::KEY_C))]);
        let mut machine = ChordMachine::default();
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G11)));
        handled(feed(&mut machine, &chords, &HashMap::new(), down(G12)));

        // Before the deadline: nothing.
        assert!(handled(tick(&mut machine, Instant::now())).is_empty());
        assert!(next_deadline(&machine).is_some());

        let deadline = next_deadline(&machine).expect("window open");
        let effects = handled(tick(&mut machine, deadline + Duration::from_millis(1)));
        assert_eq!(
            effects,
            vec![
                ChordEffect::FireIndividual { input: G11 },
                ChordEffect::FireIndividual { input: G12 },
            ]
        );
        assert_eq!(next_deadline(&machine), None);
        // Nothing left owned — a later Up for either member is not ours.
        assert_eq!(
            feed(&mut machine, &HashMap::new(), &HashMap::new(), up(G11)),
            ChordOutcome::NotMine
        );
    }

    /// The `(TriggerMode, EventState, ChordSlot)` decision table — a
    /// completing `Down`, a leader `Repeat` while active, and a completed
    /// member's `Up`, over every slot state — exercised directly rather than
    /// only transitively through the dispatch harness.
    #[test]
    fn trigger_mode_event_state_slot_table() {
        let key = chord([G11, G12]);

        struct Case {
            trigger: TriggerMode,
            event: EventState,
            slot: Option<ChordSlot>,
            expect: Vec<ChordEffect>,
        }

        let cases = [
            // A fresh completing Down.
            Case {
                trigger: TriggerMode::FireOnce,
                event: EventState::Down,
                slot: None,
                expect: vec![ChordEffect::FireChord {
                    key: key.clone(),
                    binding: keypress(TriggerMode::FireOnce, KeyCode::KEY_C),
                    state: EventState::Down,
                }],
            },
            Case {
                trigger: TriggerMode::FireOnce,
                event: EventState::Down,
                slot: Some(ChordSlot::FiringUnfinished),
                expect: vec![],
            },
            Case {
                trigger: TriggerMode::FireOnce,
                event: EventState::Down,
                slot: Some(ChordSlot::FiringFinished),
                expect: vec![ChordEffect::FireChord {
                    key: key.clone(),
                    binding: keypress(TriggerMode::FireOnce, KeyCode::KEY_C),
                    state: EventState::Down,
                }],
            },
            Case {
                trigger: TriggerMode::Toggle,
                event: EventState::Down,
                slot: Some(ChordSlot::Toggle),
                expect: vec![ChordEffect::StopChordToggle { key: key.clone() }],
            },
            // A leader Repeat while active.
            Case {
                trigger: TriggerMode::HoldToRepeat,
                event: EventState::Repeat,
                slot: Some(ChordSlot::FiringUnfinished),
                expect: vec![ChordEffect::FireChord {
                    key: key.clone(),
                    binding: keypress(TriggerMode::HoldToRepeat, KeyCode::KEY_C),
                    state: EventState::Repeat,
                }],
            },
            Case {
                trigger: TriggerMode::HoldToRepeat,
                event: EventState::Repeat,
                slot: Some(ChordSlot::FiringFinished),
                expect: vec![ChordEffect::FireChord {
                    key: key.clone(),
                    binding: keypress(TriggerMode::HoldToRepeat, KeyCode::KEY_C),
                    state: EventState::Repeat,
                }],
            },
            Case {
                trigger: TriggerMode::FireOnce,
                event: EventState::Repeat,
                slot: Some(ChordSlot::FiringUnfinished),
                expect: vec![],
            },
            Case {
                trigger: TriggerMode::HoldToRepeat,
                event: EventState::Repeat,
                slot: None,
                expect: vec![],
            },
            // A completed member's Up.
            Case {
                trigger: TriggerMode::HoldToRepeat,
                event: EventState::Up,
                slot: Some(ChordSlot::FiringUnfinished),
                expect: vec![ChordEffect::ReleaseChordFiring { key: key.clone() }],
            },
            Case {
                trigger: TriggerMode::FireOnce,
                event: EventState::Up,
                slot: Some(ChordSlot::FiringFinished),
                expect: vec![ChordEffect::ReleaseChordFiring { key: key.clone() }],
            },
            Case {
                trigger: TriggerMode::Toggle,
                event: EventState::Up,
                slot: Some(ChordSlot::Toggle),
                expect: vec![],
            },
            Case {
                trigger: TriggerMode::FireOnce,
                event: EventState::Up,
                slot: None,
                expect: vec![],
            },
        ];

        for (i, case) in cases.into_iter().enumerate() {
            let binding = keypress(case.trigger, KeyCode::KEY_C);
            let chords = HashMap::from([(key.clone(), binding)]);
            let live = match &case.slot {
                Some(slot) => HashMap::from([(key.clone(), slot.clone())]),
                None => HashMap::new(),
            };
            let mut machine = ChordMachine::default();

            let outcome = match case.event {
                EventState::Down => {
                    // The completing Down: G11 opens the window, G12 completes
                    // it, with the slot already reflecting whatever firing or
                    // toggle a prior completion left live.
                    handled(feed(&mut machine, &chords, &live, down(G11)));
                    feed(&mut machine, &chords, &live, down(G12))
                }
                EventState::Repeat | EventState::Up => {
                    // The Chord already completed and both members are still
                    // physically held (so still owned), window closed.
                    machine.claimed.insert(G11);
                    machine.claimed.insert(G12);
                    feed(&mut machine, &chords, &live, physical(G11, case.event))
                }
            };

            assert_eq!(
                handled(outcome),
                case.expect,
                "case {i}: {:?}/{:?}/{:?}",
                case.trigger,
                case.event,
                case.slot
            );
        }
    }

    fn physical(input: Input, state: EventState) -> PhysicalEvent {
        PhysicalEvent {
            input,
            state,
            depth: None,
        }
    }
}
