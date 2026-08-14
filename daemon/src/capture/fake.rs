//! A scripted `CaptureSource` for tests: feeds a fixed sequence of
//! `PhysicalEvent`s into the shared channel, no real device involved.

use super::{CaptureSource, PhysicalEvent};
use tokio::sync::mpsc;

pub struct FakeCaptureSource {
    events: Vec<PhysicalEvent>,
}

impl FakeCaptureSource {
    pub fn new(events: Vec<PhysicalEvent>) -> Self {
        Self { events }
    }
}

impl CaptureSource for FakeCaptureSource {
    async fn run(self, tx: mpsc::Sender<PhysicalEvent>) -> std::io::Result<()> {
        for event in self.events {
            if tx.send(event).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::EventState;
    use crate::input::Input;

    #[tokio::test]
    async fn replays_the_scripted_sequence_in_order() {
        let events = vec![
            PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Down,
            },
            PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Up,
            },
        ];
        let (tx, mut rx) = mpsc::channel(8);
        FakeCaptureSource::new(events.clone())
            .run(tx)
            .await
            .unwrap();

        for expected in events {
            assert_eq!(rx.recv().await, Some(expected));
        }
        assert_eq!(rx.recv().await, None);
    }
}
