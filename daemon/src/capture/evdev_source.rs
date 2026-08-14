//! The real `CaptureSource`: grabs the Tartarus Pro's three evdev nodes
//! exclusively (`EVIOCGRAB`) and normalizes their raw events into
//! `PhysicalEvent`s, per issue 07's design.

use std::io;

use evdev::{Device, EventSummary, RelativeAxisCode};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::{CaptureSource, EventState, PhysicalEvent};
use crate::input::{self, Input, Node, WheelEvent};

/// Grabs all three of the Tartarus Pro's evdev nodes exclusively and relays
/// every physical input unchanged into the shared channel. Each node is read
/// on its own `spawn_blocking` background task, since evdev's `fetch_events`
/// blocks the OS thread waiting for input.
pub struct EvdevCaptureSource;

impl CaptureSource for EvdevCaptureSource {
    async fn run(self, tx: mpsc::Sender<PhysicalEvent>) -> io::Result<()> {
        let mut nodes = JoinSet::new();
        for node in Node::ALL {
            let tx = tx.clone();
            nodes.spawn_blocking(move || capture_node_blocking(node, tx));
        }

        // The three nodes only stop producing events on a genuine capture
        // error (device unplugged, read failure) — the first one to return
        // is fatal for the whole source, matching issue 07's original
        // "any capture failure is fatal" call for this ticket's scope.
        match nodes.join_next().await {
            Some(Ok(result)) => result,
            Some(Err(join_err)) => Err(io::Error::other(join_err)),
            None => Ok(()),
        }
    }
}

fn capture_node_blocking(node: Node, tx: mpsc::Sender<PhysicalEvent>) -> io::Result<()> {
    let mut device = Device::open(node.device_path())?;
    device.grab()?;

    loop {
        for event in device.fetch_events()? {
            let Some(physical) = normalize(node, event.destructure()) else {
                continue;
            };
            if tx.blocking_send(physical).is_err() {
                return Ok(());
            }
        }
    }
}

fn normalize(node: Node, summary: EventSummary) -> Option<PhysicalEvent> {
    match summary {
        EventSummary::Key(_, code, value) => {
            let input = input::input_for_key(node, code)?;
            let state = key_value_to_state(value)?;
            Some(PhysicalEvent { input, state })
        }
        EventSummary::RelativeAxis(_, RelativeAxisCode::REL_WHEEL, value) => {
            let wheel = if value > 0 {
                WheelEvent::ScrollUp
            } else {
                WheelEvent::ScrollDown
            };
            Some(PhysicalEvent {
                input: Input::Wheel(wheel),
                state: EventState::Down,
            })
        }
        _ => None,
    }
}

fn key_value_to_state(value: i32) -> Option<EventState> {
    match value {
        0 => Some(EventState::Up),
        1 => Some(EventState::Down),
        2 => Some(EventState::Repeat),
        _ => None,
    }
}
