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
use crate::config::Modifiers;
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
/// `PhysicalEvent` (ticket 13) or a Binding's compiled Keypress firing
/// (ticket 14) — so both ever go through the one channel/task/fd (issue 07).
#[derive(Debug, Clone, PartialEq)]
enum InjectorMessage {
    Physical(PhysicalEvent),
    Keypress { modifiers: Modifiers, key: KeyCode },
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

    /// Fires a Keypress Action: presses `modifiers` then `key` in one frame,
    /// then releases `key` then `modifiers` (reverse order) in a second
    /// frame — the "canned modifier-down/key-down/key-up/modifier-up
    /// sequence" issue 06 describes for a compiled Keypress.
    pub async fn fire_keypress(
        &self,
        modifiers: Modifiers,
        key: KeyCode,
    ) -> Result<(), InjectorClosed> {
        self.tx
            .send(InjectorMessage::Keypress { modifiers, key })
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
            InjectorMessage::Keypress { modifiers, key } => {
                let (press, release) = keypress_batches(modifiers, key);
                sink.emit(&press)?;
                sink.emit(&release)?;
            }
        }
    }
    Ok(())
}

/// The modifier key codes a chord presses, in a fixed ctrl/shift/alt/super
/// order (released in reverse).
fn modifier_codes(modifiers: Modifiers) -> Vec<KeyCode> {
    let mut codes = Vec::with_capacity(4);
    if modifiers.ctrl {
        codes.push(KeyCode::KEY_LEFTCTRL);
    }
    if modifiers.shift {
        codes.push(KeyCode::KEY_LEFTSHIFT);
    }
    if modifiers.alt {
        codes.push(KeyCode::KEY_LEFTALT);
    }
    if modifiers.super_key {
        codes.push(KeyCode::KEY_LEFTMETA);
    }
    codes
}

/// Builds the press frame (modifiers down, then key down) and release frame
/// (key up, then modifiers up in reverse) for a compiled Keypress firing.
fn keypress_batches(modifiers: Modifiers, key: KeyCode) -> (Vec<InputEvent>, Vec<InputEvent>) {
    let mods = modifier_codes(modifiers);

    let mut press = Vec::with_capacity(mods.len() + 1);
    press.extend(mods.iter().map(|&code| *KeyEvent::new(code, 1)));
    press.push(*KeyEvent::new(key, 1));

    let mut release = Vec::with_capacity(mods.len() + 1);
    release.push(*KeyEvent::new(key, 0));
    release.extend(mods.iter().rev().map(|&code| *KeyEvent::new(code, 0)));

    (press, release)
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

    #[test]
    fn keypress_batches_with_no_modifiers_is_just_the_key_down_then_up() {
        let (press, release) = keypress_batches(Modifiers::default(), KeyCode::KEY_F1);
        assert_eq!(
            press.into_iter().map(key_and_value).collect::<Vec<_>>(),
            vec![(KeyCode::KEY_F1, 1)]
        );
        assert_eq!(
            release.into_iter().map(key_and_value).collect::<Vec<_>>(),
            vec![(KeyCode::KEY_F1, 0)]
        );
    }

    #[test]
    fn keypress_batches_presses_modifiers_before_key_and_releases_in_reverse() {
        let modifiers = Modifiers {
            ctrl: true,
            shift: true,
            alt: false,
            super_key: false,
        };
        let (press, release) = keypress_batches(modifiers, KeyCode::KEY_T);

        assert_eq!(
            press.into_iter().map(key_and_value).collect::<Vec<_>>(),
            vec![
                (KeyCode::KEY_LEFTCTRL, 1),
                (KeyCode::KEY_LEFTSHIFT, 1),
                (KeyCode::KEY_T, 1),
            ]
        );
        assert_eq!(
            release.into_iter().map(key_and_value).collect::<Vec<_>>(),
            vec![
                (KeyCode::KEY_T, 0),
                (KeyCode::KEY_LEFTSHIFT, 0),
                (KeyCode::KEY_LEFTCTRL, 0),
            ]
        );
    }

    #[tokio::test]
    async fn fire_keypress_emits_a_press_batch_then_a_release_batch() {
        let sink = testing::RecordingSink::new();
        let (injector, handle) = spawn(sink.clone());

        injector
            .fire_keypress(Modifiers::default(), KeyCode::KEY_F1)
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
