//! The dispatch task: single consumer of the capture channel, resolving
//! each `PhysicalEvent`'s `Input` against the active Profile's Base Layer
//! (ticket 14). A configured Binding fires its Action on `Down`; an absent
//! Binding passes through unchanged (ticket 13 behavior). Held Layer,
//! Trigger-mode branching beyond Fire-once, and `Action::Macro` all remain
//! future work (issues 17/18) — see `fire` below.

use std::collections::HashMap;
use std::io;

use tokio::sync::mpsc;

use crate::capture::{EventState, PhysicalEvent};
use crate::config::{Action, Binding};
use crate::injector::Injector;
use crate::input::Input;

/// Returns an error once the injector channel closes — meaning the injector
/// task has died, which per issue 07 is a genuine, fatal capture-pipeline
/// error rather than something to swallow silently.
pub async fn run(
    mut rx: mpsc::Receiver<PhysicalEvent>,
    injector: Injector,
    bindings: HashMap<Input, Binding>,
) -> io::Result<()> {
    while let Some(event) = rx.recv().await {
        match bindings.get(&event.input) {
            Some(binding) => {
                // Ticket 14 only wires Fire-once: the Action fires once on
                // Down, Repeat/Up are ignored outright (no passthrough of
                // the original key). Hold-to-repeat/Toggle's real firing
                // semantics — and branching on `binding.trigger` at all —
                // land in ticket 17.
                if event.state == EventState::Down {
                    fire(&injector, event.input, &binding.action).await?;
                }
            }
            None => {
                injector
                    .inject_physical(event)
                    .await
                    .map_err(io::Error::other)?;
            }
        }
    }
    Ok(())
}

async fn fire(injector: &Injector, input: Input, action: &Action) -> io::Result<()> {
    match action {
        Action::Keypress { modifiers, key } => injector
            .fire_keypress(*modifiers, *key)
            .await
            .map_err(io::Error::other),
        // Not implemented until ticket 17 — Action::Macro is a schema-only
        // stub for this ticket (issue 06). Logged rather than silently
        // dropped, so a hand-edited Macro binding doesn't look like a
        // dead/misconfigured key with no clue why nothing happened.
        Action::Macro { .. } => {
            eprintln!(
                "acheron-daemon: {input} is bound to a Macro action, which isn't implemented \
                 until ticket 17 — ignoring this press"
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::fake::FakeCaptureSource;
    use crate::capture::{CaptureSource, EventState};
    use crate::config::{Modifiers, TriggerMode};
    use crate::injector::testing::RecordingSink;
    use crate::injector::{self};
    use crate::input::{Direction, Input, WheelEvent};

    async fn run_scripted(
        scripted: Vec<PhysicalEvent>,
        bindings: HashMap<Input, Binding>,
    ) -> Vec<Vec<evdev::InputEvent>> {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone());

        let (tx, rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(rx, inj.clone(), bindings));

        FakeCaptureSource::new(scripted).run(tx).await.unwrap();

        drop(inj);
        dispatch_handle.await.unwrap().unwrap();
        inj_handle.await.unwrap().unwrap();

        sink.batches()
    }

    #[tokio::test]
    async fn passthrough_reinjects_every_captured_event_unchanged_when_unbound() {
        let scripted = vec![
            PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Down,
            },
            PhysicalEvent {
                input: Input::Grid(2, 3),
                state: EventState::Repeat,
            },
            PhysicalEvent {
                input: Input::Thumbstick(Direction::Up),
                state: EventState::Up,
            },
            PhysicalEvent {
                input: Input::Wheel(WheelEvent::ScrollDown),
                state: EventState::Down,
            },
        ];

        let batches = run_scripted(scripted.clone(), HashMap::new()).await;
        assert_eq!(batches.len(), scripted.len());

        // Grid(2,3) -> KEY_W, value 2 (Repeat).
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_W);
        assert_eq!(value, 2);

        // Thumbstick Up -> KEY_UP, value 0 (Up).
        let evdev::EventSummary::Key(_, code, value) = batches[2][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_UP);
        assert_eq!(value, 0);

        // Wheel ScrollDown -> paired REL_WHEEL(-1)/REL_WHEEL_HI_RES(-120).
        assert_eq!(batches[3].len(), 2);
    }

    #[tokio::test]
    async fn bound_input_fires_the_remapped_keypress_instead_of_passthrough() {
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_F1,
                },
            },
        );

        let scripted = vec![PhysicalEvent {
            input: Input::Grid(1, 1),
            state: EventState::Down,
        }];

        let batches = run_scripted(scripted, bindings).await;

        // One press batch + one release batch of KEY_F1 — not the grid
        // key's own passthrough code (KEY_1).
        assert_eq!(batches.len(), 2);
        let evdev::EventSummary::Key(_, code, value) = batches[0][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_F1);
        assert_eq!(value, 1);
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(code, evdev::KeyCode::KEY_F1);
        assert_eq!(value, 0);
    }

    #[tokio::test]
    async fn fire_once_binding_ignores_repeat_and_up_fires_only_on_down() {
        let mut bindings = HashMap::new();
        bindings.insert(
            Input::Grid(1, 1),
            Binding {
                trigger: TriggerMode::FireOnce,
                action: Action::Keypress {
                    modifiers: Modifiers::default(),
                    key: evdev::KeyCode::KEY_F1,
                },
            },
        );

        let scripted = vec![
            PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Down,
            },
            PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Repeat,
            },
            PhysicalEvent {
                input: Input::Grid(1, 1),
                state: EventState::Up,
            },
        ];

        let batches = run_scripted(scripted, bindings).await;

        // Only the Down produced output: one press batch + one release batch.
        assert_eq!(batches.len(), 2);
    }
}
