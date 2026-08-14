//! The dispatch task: single consumer of the capture channel. With no
//! config/Profiles/Bindings yet (ticket 13), every captured `PhysicalEvent`
//! is re-emitted unchanged — pure passthrough. Binding-lookup/Trigger-mode
//! logic lands here in a later ticket, per issue 07's design.

use std::io;

use tokio::sync::mpsc;

use crate::capture::PhysicalEvent;
use crate::injector::Injector;

/// Returns an error once the injector channel closes — meaning the injector
/// task has died, which per issue 07 is a genuine, fatal capture-pipeline
/// error rather than something to swallow silently.
pub async fn run(mut rx: mpsc::Receiver<PhysicalEvent>, injector: Injector) -> io::Result<()> {
    while let Some(event) = rx.recv().await {
        injector.inject(event).await.map_err(io::Error::other)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::fake::FakeCaptureSource;
    use crate::capture::{CaptureSource, EventState};
    use crate::injector::testing::RecordingSink;
    use crate::injector::{self};
    use crate::input::{Direction, Input, WheelEvent};

    #[tokio::test]
    async fn passthrough_reinjects_every_captured_event_unchanged() {
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

        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone());

        let (tx, rx) = mpsc::channel(8);
        let dispatch_handle = tokio::spawn(run(rx, inj.clone()));

        FakeCaptureSource::new(scripted.clone())
            .run(tx)
            .await
            .unwrap();

        drop(inj);
        dispatch_handle.await.unwrap().unwrap();
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
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
}
