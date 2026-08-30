// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! A scripted `CaptureSource` for tests: feeds a fixed sequence of
//! `PhysicalEvent`s (and, per ticket 20, device-connection transitions) into
//! the shared channels, no real device involved.

use super::{CaptureSource, PhysicalEvent};
use tokio::sync::mpsc;

/// One scripted step: either a captured `PhysicalEvent`, or a device
/// connection transition (ticket 20's `device_connected`/
/// `DeviceConnectionChanged` — exercised here without a real evdev poll
/// loop).
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptedEvent {
    Physical(PhysicalEvent),
    Connection(bool),
}

pub struct FakeCaptureSource {
    events: Vec<ScriptedEvent>,
}

impl FakeCaptureSource {
    pub fn new(events: Vec<PhysicalEvent>) -> Self {
        Self {
            events: events.into_iter().map(ScriptedEvent::Physical).collect(),
        }
    }

    /// Scripts a mix of `PhysicalEvent`s and connection transitions, in
    /// order — the seam ticket 20's dispatch-side tests use to exercise
    /// `device_connected`/`DeviceConnectionChanged` without a real poll loop.
    pub fn scripted(events: Vec<ScriptedEvent>) -> Self {
        Self { events }
    }
}

impl CaptureSource for FakeCaptureSource {
    async fn run(
        self,
        tx: mpsc::Sender<PhysicalEvent>,
        connection_tx: mpsc::Sender<bool>,
    ) -> std::io::Result<()> {
        for event in self.events {
            let sent = match event {
                ScriptedEvent::Physical(event) => tx.send(event).await.is_ok(),
                ScriptedEvent::Connection(connected) => connection_tx.send(connected).await.is_ok(),
            };
            if !sent {
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
                depth: None,
            },
            PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Up,
                depth: None,
            },
        ];
        let (tx, mut rx) = mpsc::channel(8);
        let (connection_tx, _connection_rx) = mpsc::channel(8);
        FakeCaptureSource::new(events.clone())
            .run(tx, connection_tx)
            .await
            .unwrap();

        for expected in events {
            assert_eq!(rx.recv().await, Some(expected));
        }
        assert_eq!(rx.recv().await, None);
    }

    #[tokio::test]
    async fn scripts_analog_depth_on_grid_inputs_without_any_raw_byte_simulation() {
        // Ticket 22: report-parsing and the threshold state machine
        // (`capture::analog::observe`) are separately, already unit-tested
        // pure functions, so `FakeCaptureSource` only needs to carry
        // `depth: Some(_)` through unchanged — no `hidraw` report bytes
        // involved, per ticket 17's widened `PhysicalEvent` shape.
        let events = vec![
            PhysicalEvent {
                input: Input::Grid(2, 3),
                state: EventState::Down,
                depth: Some(150),
            },
            PhysicalEvent {
                input: Input::Grid(2, 3),
                state: EventState::Repeat,
                depth: Some(200),
            },
            PhysicalEvent {
                input: Input::Grid(2, 3),
                state: EventState::Up,
                depth: Some(90),
            },
        ];
        let (tx, mut rx) = mpsc::channel(8);
        let (connection_tx, _connection_rx) = mpsc::channel(8);
        FakeCaptureSource::new(events.clone())
            .run(tx, connection_tx)
            .await
            .unwrap();

        for expected in events {
            assert_eq!(rx.recv().await, Some(expected));
        }
        assert_eq!(rx.recv().await, None);
    }

    #[tokio::test]
    async fn scripted_connection_transitions_are_replayed_in_order_alongside_events() {
        let events = vec![
            ScriptedEvent::Connection(false),
            ScriptedEvent::Physical(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Down,
                depth: None,
            }),
            ScriptedEvent::Connection(true),
        ];
        let (tx, mut rx) = mpsc::channel(8);
        let (connection_tx, mut connection_rx) = mpsc::channel(8);
        FakeCaptureSource::scripted(events)
            .run(tx, connection_tx)
            .await
            .unwrap();

        assert_eq!(connection_rx.recv().await, Some(false));
        assert_eq!(
            rx.recv().await,
            Some(PhysicalEvent {
                input: Input::ModeKey,
                state: EventState::Down,
                depth: None,
            })
        );
        assert_eq!(connection_rx.recv().await, Some(true));
    }
}
