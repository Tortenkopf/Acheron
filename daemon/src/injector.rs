//! The injector task: owns the single `uinput` virtual device for the
//! process lifetime and serializes every output write through one channel,
//! so no other task ever touches the fd directly (issue 07).

use std::fmt;
use std::io;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, InputEvent, KeyCode, KeyEvent, RelativeAxisCode, RelativeAxisEvent};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::capture::{EventState, PhysicalEvent};
use crate::input::{self, Input, WheelEvent};

/// Where the injector task writes translated evdev events. The real
/// implementation wraps the single uinput `VirtualDevice`; tests substitute
/// a recording sink (`testing::RecordingSink`) so injected output can be
/// asserted on without a real device.
pub trait InjectSink: Send + 'static {
    fn emit(&mut self, events: &[InputEvent]) -> io::Result<()>;
}

impl InjectSink for VirtualDevice {
    fn emit(&mut self, events: &[InputEvent]) -> io::Result<()> {
        VirtualDevice::emit(self, events)
    }
}

/// Builds the one virtual device the Daemon holds for its whole lifetime,
/// declaring support for every key/button an `Input` can inject plus the
/// wheel's relative axes.
pub fn build_device() -> io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in input::all_injectable_key_codes() {
        keys.insert(code);
    }
    let axes = AttributeSet::from_iter([
        RelativeAxisCode::REL_WHEEL,
        RelativeAxisCode::REL_WHEEL_HI_RES,
    ]);

    VirtualDevice::builder()?
        .name("Acheron Virtual Tartarus Pro")
        .with_keys(&keys)?
        .with_relative_axes(&axes)?
        .build()
}

/// A write-command sent to the injector task — either a passthrough
/// `PhysicalEvent` (ticket 13) or a single key state change, one `KeyDown`/
/// `KeyUp` step of a compiled Binding firing (ticket 17's shared executor,
/// `executor::run_once`/`ActiveToggle`) — so both ever go through the one
/// channel/task/fd (issue 07).
#[derive(Debug, Clone, Copy, PartialEq)]
enum InjectorMessage {
    Physical(PhysicalEvent),
    KeyState { key: KeyCode, down: bool },
}

/// The injector task's channel has closed, meaning the task itself has
/// died — per issue 07, a genuine fatal error for callers to propagate.
#[derive(Debug)]
pub struct InjectorClosed;

impl fmt::Display for InjectorClosed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "injector task's channel is closed")
    }
}

impl std::error::Error for InjectorClosed {}

/// Handle for sending write-commands to the injector task.
#[derive(Clone)]
pub struct Injector {
    tx: mpsc::Sender<InjectorMessage>,
}

impl Injector {
    /// Re-emits a captured `PhysicalEvent` unchanged (passthrough).
    pub async fn inject_physical(&self, event: PhysicalEvent) -> Result<(), InjectorClosed> {
        self.tx
            .send(InjectorMessage::Physical(event))
            .await
            .map_err(|_| InjectorClosed)
    }

    /// Emits a single key down/up transition — one `KeyDown`/`KeyUp` step of
    /// a compiled Binding firing (ticket 17's shared executor). Each step is
    /// its own `SYN_REPORT` frame, same as a real physical key press.
    pub async fn set_key_state(&self, key: KeyCode, down: bool) -> Result<(), InjectorClosed> {
        self.tx
            .send(InjectorMessage::KeyState { key, down })
            .await
            .map_err(|_| InjectorClosed)
    }
}

/// Spawns the injector task, which owns `sink` for the life of the task.
pub fn spawn<S: InjectSink>(sink: S) -> (Injector, JoinHandle<io::Result<()>>) {
    let (tx, rx) = mpsc::channel(256);
    let handle = tokio::spawn(injector_loop(sink, rx));
    (Injector { tx }, handle)
}

async fn injector_loop<S: InjectSink>(
    mut sink: S,
    mut rx: mpsc::Receiver<InjectorMessage>,
) -> io::Result<()> {
    while let Some(message) = rx.recv().await {
        match message {
            InjectorMessage::Physical(event) => {
                let batch = translate(event);
                if !batch.is_empty() {
                    sink.emit(&batch)?;
                }
            }
            InjectorMessage::KeyState { key, down } => {
                let value = if down { 1 } else { 0 };
                sink.emit(&[*KeyEvent::new(key, value)])?;
            }
        }
    }
    Ok(())
}

/// Translates one `PhysicalEvent` into the raw evdev events that reproduce
/// it unchanged. Everything but a wheel-scroll tick is a single `EV_KEY`
/// event; a wheel-scroll tick is the paired `REL_WHEEL`/`REL_WHEEL_HI_RES`
/// events issue 01 recorded landing in the same `SYN_REPORT` — `emit`
/// appends that `SYN_REPORT` automatically for whatever batch it's given.
fn translate(event: PhysicalEvent) -> Vec<InputEvent> {
    match event.input {
        Input::Wheel(WheelEvent::ScrollUp) => wheel_scroll_events(1),
        Input::Wheel(WheelEvent::ScrollDown) => wheel_scroll_events(-1),
        other => {
            let Some(code) = input::key_code_for_input(other) else {
                return Vec::new();
            };
            let value = match event.state {
                EventState::Down => 1,
                EventState::Repeat => 2,
                EventState::Up => 0,
            };
            vec![*KeyEvent::new(code, value)]
        }
    }
}

fn wheel_scroll_events(direction: i32) -> Vec<InputEvent> {
    vec![
        *RelativeAxisEvent::new(RelativeAxisCode::REL_WHEEL, direction),
        *RelativeAxisEvent::new(RelativeAxisCode::REL_WHEEL_HI_RES, direction * 120),
    ]
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Records every emitted batch instead of writing to a real device, so
    /// tests can assert on injected output without `uinput` access.
    #[derive(Clone, Default)]
    pub struct RecordingSink {
        batches: Arc<Mutex<Vec<Vec<InputEvent>>>>,
    }

    impl RecordingSink {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn batches(&self) -> Vec<Vec<InputEvent>> {
            self.batches.lock().unwrap().clone()
        }
    }

    impl InjectSink for RecordingSink {
        fn emit(&mut self, events: &[InputEvent]) -> io::Result<()> {
            self.batches.lock().unwrap().push(events.to_vec());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Direction, Node};

    #[test]
    fn translate_passthrough_key_preserves_code_and_state() {
        let event = PhysicalEvent {
            input: Input::ModeKey,
            state: EventState::Down,
        };
        let batch = translate(event);
        assert_eq!(batch.len(), 1);
        match batch[0].destructure() {
            evdev::EventSummary::Key(_, code, value) => {
                assert_eq!(code, evdev::KeyCode::KEY_LEFTALT);
                assert_eq!(value, 1);
            }
            other => panic!("expected a key event, got {other:?}"),
        }
    }

    #[test]
    fn translate_wheel_scroll_emits_paired_rel_events() {
        let event = PhysicalEvent {
            input: Input::Wheel(WheelEvent::ScrollUp),
            state: EventState::Down,
        };
        let batch = translate(event);
        assert_eq!(batch.len(), 2);
        assert_eq!(axis_and_value(batch[0]), (RelativeAxisCode::REL_WHEEL, 1));
        assert_eq!(
            axis_and_value(batch[1]),
            (RelativeAxisCode::REL_WHEEL_HI_RES, 120)
        );
    }

    fn axis_and_value(event: InputEvent) -> (RelativeAxisCode, i32) {
        match event.destructure() {
            evdev::EventSummary::RelativeAxis(_, axis, value) => (axis, value),
            other => panic!("expected a relative-axis event, got {other:?}"),
        }
    }

    #[test]
    fn translate_thumbstick_round_trips() {
        let event = PhysicalEvent {
            input: Input::Thumbstick(Direction::Left),
            state: EventState::Repeat,
        };
        let batch = translate(event);
        assert_eq!(batch.len(), 1);
        let evdev::EventSummary::Key(_, code, _) = batch[0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(
            input::input_for_key(Node::Main, code),
            Some(Input::Thumbstick(Direction::Left))
        );
    }

    fn key_and_value(event: InputEvent) -> (KeyCode, i32) {
        match event.destructure() {
            evdev::EventSummary::Key(_, code, value) => (code, value),
            other => panic!("expected a key event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_key_state_emits_one_frame_per_transition() {
        let sink = testing::RecordingSink::new();
        let (injector, handle) = spawn(sink.clone());

        injector.set_key_state(KeyCode::KEY_F1, true).await.unwrap();
        injector
            .set_key_state(KeyCode::KEY_F1, false)
            .await
            .unwrap();
        drop(injector);
        handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(batches.len(), 2);
        assert_eq!(
            batches[0]
                .iter()
                .copied()
                .map(key_and_value)
                .collect::<Vec<_>>(),
            vec![(KeyCode::KEY_F1, 1)]
        );
        assert_eq!(
            batches[1]
                .iter()
                .copied()
                .map(key_and_value)
                .collect::<Vec<_>>(),
            vec![(KeyCode::KEY_F1, 0)]
        );
    }
}
