// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The injector task: owns the single `uinput` virtual device for the
//! process lifetime and serializes every output write through one channel,
//! so no other task ever touches the fd directly (issue 07).

use std::fmt;
use std::io;
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AbsoluteAxisEvent, AttributeSet, InputEvent, KeyCode, KeyEvent,
    RelativeAxisCode, RelativeAxisEvent, UinputAbsSetup,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::capture::{EventState, PhysicalEvent};
use crate::input::{self, Input, WheelEvent};

/// The 5 unsigned `AxisTarget`s' `ABS_*` codes (ticket 71) — each declared
/// with a `0..=255` range, matching raw Depth directly with no rescaling.
const UNSIGNED_AXIS_CODES: [AbsoluteAxisCode; 5] = [
    AbsoluteAxisCode::ABS_Z,
    AbsoluteAxisCode::ABS_RZ,
    AbsoluteAxisCode::ABS_THROTTLE,
    AbsoluteAxisCode::ABS_GAS,
    AbsoluteAxisCode::ABS_BRAKE,
];

/// The 6 signed `AxisTarget` axes' `ABS_*` codes — each declared with a
/// `-255..=255` range, since a signed axis's two independently-assignable
/// halves emit `+depth`/`-depth` respectively (`dispatch`'s axis-conflict
/// resolution).
const SIGNED_AXIS_CODES: [AbsoluteAxisCode; 6] = [
    AbsoluteAxisCode::ABS_X,
    AbsoluteAxisCode::ABS_Y,
    AbsoluteAxisCode::ABS_RX,
    AbsoluteAxisCode::ABS_RY,
    AbsoluteAxisCode::ABS_RUDDER,
    AbsoluteAxisCode::ABS_WHEEL,
];

/// Every `ABS_*` code the gamepad `uinput` device advertises (ticket 71) —
/// the 11 distinct codes backing the 17 `AxisTarget` values (`config::
/// AxisTarget::abs_code`) that `build_gamepad_device`'s capability
/// declaration below is built from, exposed so a caller/test can enumerate
/// the same set without hand-duplicating it.
pub fn all_axis_abs_codes() -> [AbsoluteAxisCode; 11] {
    let mut codes = [AbsoluteAxisCode::ABS_Z; 11];
    codes[..5].copy_from_slice(&UNSIGNED_AXIS_CODES);
    codes[5..].copy_from_slice(&SIGNED_AXIS_CODES);
    codes
}

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

/// Builds the second `uinput` device `Action::ControllerButton` fires
/// against (ticket 14/43) — distinct from `build_device`'s keyboard/mouse
/// device, advertising only `input::gamepad_button_codes()`'s curated
/// 57-entry set (not the full `EV_KEY` range `build_device` declares: unlike
/// Keypress, a ControllerButton's `button` is already validated against this
/// exact set at `SetBinding`/`load_or_seed` time, so there's no "any code at
/// all" case to leave room for). Kernel `joydev` auto-attaches a `/dev/input/
/// jsX` node to any `uinput` device that advertises this `BTN_GAMEPAD`-class
/// bit set — confirmed by ticket 37's research, zero extra work needed here.
///
/// Ticket 71 additionally declares the 11 `ABS_*` codes `config::AxisTarget`'s
/// 17 targets drive — unsigned axes `0..=255` (matching raw Depth directly),
/// signed axes `-255..=255` (their two independently-assignable halves emit
/// `+depth`/`-depth`) — on this same single device, alongside the button
/// range above: confirmed via ticket 59's research that mixing `BTN_GAMEPAD`
/// buttons with HOTAS-style axes on one `uinput` device causes no OS/SDL
/// classification conflict (ticket 59 §3), so no second device is needed.
pub fn build_gamepad_device() -> io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in input::gamepad_button_codes() {
        keys.insert(code);
    }

    let mut builder = VirtualDevice::builder()?
        .name("Acheron Virtual Controller")
        .with_keys(&keys)?;
    for code in UNSIGNED_AXIS_CODES {
        builder = builder
            .with_absolute_axis(&UinputAbsSetup::new(code, AbsInfo::new(0, 0, 255, 0, 0, 0)))?;
    }
    for code in SIGNED_AXIS_CODES {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(
            code,
            AbsInfo::new(0, -255, 255, 0, 0, 0),
        ))?;
    }
    builder.build()
}

/// Retries a fallible open on `PermissionDenied` alone, bounded by
/// `attempts` with `delay` between each (ticket 28).
///
/// Written generically over `open`'s return type — rather than hardcoded to
/// `build_device`/`VirtualDevice` — so the retry/backoff decision can be
/// unit-tested with a plain fake instead of a real device open. `label` is
/// only used for the one-time diagnostic on the first retry.
pub async fn retry_on_permission_denied<T, F>(
    label: &str,
    mut open: F,
    delay: Duration,
    attempts: u32,
) -> io::Result<T>
where
    F: FnMut() -> io::Result<T>,
{
    let mut last_err = None;
    for attempt in 0..attempts {
        match open() {
            Ok(value) => return Ok(value),
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                if attempt == 0 {
                    eprintln!(
                        "acheron-daemon: {label} not accessible yet (permission denied), retrying: {err}"
                    );
                }
                last_err = Some(err);
                tokio::time::sleep(delay).await;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_err.expect("attempts is always > 0 at every call site"))
}

/// A write-command sent to the injector task — either a passthrough
/// `PhysicalEvent` (ticket 13), a single key state change, one `KeyDown`/
/// `KeyUp` step of a compiled Binding firing (ticket 17's shared executor,
/// `executor::run_once`/`ActiveToggle`), a suppression-bypassing force-
/// release (ticket 25), or a suppression-flag update (ticket 24) — so all
/// of them go through the one channel/task/fd (issue 07).
///
/// `KeyState` carries a reply channel reporting whether the write actually
/// reached `sink.emit` (`true`) or was withheld by suppression (`false`) —
/// `executor::execute_step` needs this to keep a Toggle's `held` bookkeeping
/// honest (ticket 25's live-hardware finding): blindly removing a key from
/// `held` on every `KeyUp` step regardless of suppression let a genuinely
/// still-down key silently drop out of `held` right before a stop's
/// force-release, which only re-releases what's still listed there.
#[derive(Debug)]
enum InjectorMessage {
    Physical(PhysicalEvent),
    KeyState {
        key: KeyCode,
        down: bool,
        applied: oneshot::Sender<bool>,
    },
    ForceRelease(KeyCode),
    SetSuppressed(bool),
    /// One resolved `ABS_*` axis value (ticket 71's `dispatch`-owned axis
    /// resolution — `config::resolve_axis_value` plus its runtime-conflict
    /// merge), always routed to the gamepad sink: every `AxisTarget` lives
    /// only on that device, unlike a `KeyState`/`ForceRelease` write, which
    /// needs `sink_for`'s per-code routing decision because `Action::
    /// Keypress`/`Action::ControllerButton` share one `KeyCode` type-space.
    /// No reply channel — fire-and-forget, like `Physical`.
    AxisValue {
        code: AbsoluteAxisCode,
        value: i32,
    },
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
    /// Returns whether the write actually reached `sink.emit` (`false` if
    /// suppression withheld it) — see `InjectorMessage::KeyState`'s doc
    /// comment for why `execute_step` needs this.
    pub async fn set_key_state(&self, key: KeyCode, down: bool) -> Result<bool, InjectorClosed> {
        let (applied, rx) = oneshot::channel();
        self.tx
            .send(InjectorMessage::KeyState { key, down, applied })
            .await
            .map_err(|_| InjectorClosed)?;
        rx.await.map_err(|_| InjectorClosed)
    }

    /// Releases `key` regardless of suppression — the one write this task
    /// never gates. Used only by `ActiveToggle::stop()`'s force-release
    /// (`executor::force_release`): a Toggle's `held` bookkeeping tracks
    /// what it logically thinks it's holding independent of whether
    /// suppression let the matching `KeyDown` actually reach `uinput`, so a
    /// key that went down for real *before* suppression turned on (started
    /// unfocused, then the GUI gained focus) would otherwise never get
    /// released while suppression stays on — left stuck down at the OS
    /// level even though `active_toggles` correctly shows the Toggle
    /// stopped. Found live against real hardware (ticket 25): gating this
    /// the same as every other write reproduced ticket 22's freeze on
    /// stop, not just on start.
    pub async fn force_release_key(&self, key: KeyCode) -> Result<(), InjectorClosed> {
        self.tx
            .send(InjectorMessage::ForceRelease(key))
            .await
            .map_err(|_| InjectorClosed)
    }

    /// Level-sets whether the injector withholds every subsequent write to
    /// the virtual device (ticket 24/spec.md's "Daemon output suppression").
    /// Queued as a message like any other write-command, so it takes effect
    /// in the same order the D-Bus caller observed relative to firings that
    /// were already in flight — never a separate out-of-band flag racing the
    /// channel. Firing logic, Macro looping, and `active_toggles` are
    /// untouched; only whether a write actually reaches `sink.emit` changes.
    pub async fn set_suppressed(&self, suppressed: bool) -> Result<(), InjectorClosed> {
        self.tx
            .send(InjectorMessage::SetSuppressed(suppressed))
            .await
            .map_err(|_| InjectorClosed)
    }

    /// Writes one resolved `ABS_*` axis value (ticket 71) to the gamepad
    /// device — always that device, never `sink_for`'s routing decision (see
    /// `InjectorMessage::AxisValue`'s doc comment). Gated by suppression like
    /// every write but `ForceRelease`.
    pub async fn set_axis_value(
        &self,
        code: AbsoluteAxisCode,
        value: i32,
    ) -> Result<(), InjectorClosed> {
        self.tx
            .send(InjectorMessage::AxisValue { code, value })
            .await
            .map_err(|_| InjectorClosed)
    }
}

/// Spawns the injector task, which owns both `sink` (keyboard/mouse) and
/// `gamepad_sink` (ticket 14/43's second `uinput` device) for the life of
/// the task. A single generic `S` (rather than two distinct sink types) —
/// every real call site builds both from the same `VirtualDevice`/
/// `InjectSink` impl, and tests that don't care about gamepad routing can
/// pass the same `RecordingSink` clone for both, landing every batch in one
/// shared recording.
pub fn spawn<S: InjectSink>(sink: S, gamepad_sink: S) -> (Injector, JoinHandle<io::Result<()>>) {
    let (tx, rx) = mpsc::channel(256);
    let handle = tokio::spawn(injector_loop(sink, gamepad_sink, rx));
    (Injector { tx }, handle)
}

/// Routes a `KeyState`/`ForceRelease` write to the gamepad sink instead of
/// the keyboard/mouse one when `key` is one of ticket 43's curated gamepad
/// codes — the injector-level device distinction ticket 14 asked for,
/// invisible to `executor.rs`'s generic `KeyDown`/`KeyUp` steps.
fn sink_for<'a, S: InjectSink>(
    sink: &'a mut S,
    gamepad_sink: &'a mut S,
    key: KeyCode,
) -> &'a mut S {
    if input::is_gamepad_button(key) {
        gamepad_sink
    } else {
        sink
    }
}

async fn injector_loop<S: InjectSink>(
    mut sink: S,
    mut gamepad_sink: S,
    mut rx: mpsc::Receiver<InjectorMessage>,
) -> io::Result<()> {
    let mut suppressed = false;
    while let Some(message) = rx.recv().await {
        match message {
            InjectorMessage::Physical(event) => {
                let batch = translate(event);
                if !batch.is_empty() && !suppressed {
                    // A physical Input's passthrough code is always a
                    // keyboard/mouse code (input.rs's Input variants never
                    // map onto a gamepad button) — always the primary sink.
                    sink.emit(&batch)?;
                }
            }
            InjectorMessage::KeyState { key, down, applied } => {
                if !suppressed {
                    let value = if down { 1 } else { 0 };
                    sink_for(&mut sink, &mut gamepad_sink, key)
                        .emit(&[*KeyEvent::new(key, value)])?;
                }
                let _ = applied.send(!suppressed);
            }
            InjectorMessage::ForceRelease(key) => {
                sink_for(&mut sink, &mut gamepad_sink, key).emit(&[*KeyEvent::new(key, 0)])?;
            }
            InjectorMessage::SetSuppressed(value) => suppressed = value,
            InjectorMessage::AxisValue { code, value } => {
                if !suppressed {
                    gamepad_sink.emit(&[*AbsoluteAxisEvent::new(code, value)])?;
                }
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
            depth: None,
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
            depth: None,
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
            depth: None,
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
        let (injector, handle) = spawn(sink.clone(), sink.clone());

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

    #[tokio::test]
    async fn set_key_state_routes_gamepad_codes_to_the_gamepad_sink() {
        let sink = testing::RecordingSink::new();
        let gamepad_sink = testing::RecordingSink::new();
        let (injector, handle) = spawn(sink.clone(), gamepad_sink.clone());

        injector
            .set_key_state(KeyCode::BTN_SOUTH, true)
            .await
            .unwrap();
        injector.set_key_state(KeyCode::KEY_F1, true).await.unwrap();
        drop(injector);
        handle.await.unwrap().unwrap();

        assert_eq!(
            sink.batches()
                .into_iter()
                .flatten()
                .map(key_and_value)
                .collect::<Vec<_>>(),
            vec![(KeyCode::KEY_F1, 1)],
            "a keyboard code must never reach the gamepad device"
        );
        assert_eq!(
            gamepad_sink
                .batches()
                .into_iter()
                .flatten()
                .map(key_and_value)
                .collect::<Vec<_>>(),
            vec![(KeyCode::BTN_SOUTH, 1)],
            "a gamepad code must never reach the keyboard/mouse device"
        );
    }

    #[tokio::test]
    async fn force_release_routes_gamepad_codes_to_the_gamepad_sink() {
        let sink = testing::RecordingSink::new();
        let gamepad_sink = testing::RecordingSink::new();
        let (injector, handle) = spawn(sink.clone(), gamepad_sink.clone());

        injector
            .force_release_key(KeyCode::BTN_TRIGGER_HAPPY1)
            .await
            .unwrap();
        drop(injector);
        handle.await.unwrap().unwrap();

        assert!(sink.batches().is_empty());
        assert_eq!(
            gamepad_sink
                .batches()
                .into_iter()
                .flatten()
                .map(key_and_value)
                .collect::<Vec<_>>(),
            vec![(KeyCode::BTN_TRIGGER_HAPPY1, 0)]
        );
    }

    #[tokio::test]
    async fn set_axis_value_always_routes_to_the_gamepad_sink() {
        let sink = testing::RecordingSink::new();
        let gamepad_sink = testing::RecordingSink::new();
        let (injector, handle) = spawn(sink.clone(), gamepad_sink.clone());

        injector
            .set_axis_value(evdev::AbsoluteAxisCode::ABS_X, -200)
            .await
            .unwrap();
        drop(injector);
        handle.await.unwrap().unwrap();

        assert!(sink.batches().is_empty());
        let batches = gamepad_sink.batches();
        assert_eq!(batches.len(), 1);
        match batches[0][0].destructure() {
            evdev::EventSummary::AbsoluteAxis(_, axis, value) => {
                assert_eq!(axis, evdev::AbsoluteAxisCode::ABS_X);
                assert_eq!(value, -200);
            }
            other => panic!("expected an absolute-axis event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_axis_value_is_withheld_while_suppressed() {
        let sink = testing::RecordingSink::new();
        let (injector, handle) = spawn(sink.clone(), sink.clone());

        injector.set_suppressed(true).await.unwrap();
        injector
            .set_axis_value(evdev::AbsoluteAxisCode::ABS_Z, 100)
            .await
            .unwrap();
        drop(injector);
        handle.await.unwrap().unwrap();

        assert!(sink.batches().is_empty());
    }

    #[test]
    fn build_gamepad_device_declares_all_11_axis_codes() {
        let codes = all_axis_abs_codes();
        assert_eq!(codes.len(), 11);
        assert!(codes.contains(&AbsoluteAxisCode::ABS_Z));
        assert!(codes.contains(&AbsoluteAxisCode::ABS_X));
    }

    #[tokio::test]
    async fn retry_on_permission_denied_succeeds_once_the_open_stops_failing() {
        let mut remaining_denials = 3;
        let result = retry_on_permission_denied(
            "fake",
            || {
                if remaining_denials > 0 {
                    remaining_denials -= 1;
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                } else {
                    Ok(42)
                }
            },
            Duration::from_millis(0),
            5,
        )
        .await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retry_on_permission_denied_gives_up_after_the_attempt_bound() {
        let mut calls = 0;
        let result: io::Result<()> = retry_on_permission_denied(
            "fake",
            || {
                calls += 1;
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            },
            Duration::from_millis(0),
            3,
        )
        .await;
        assert_eq!(calls, 3);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn retry_on_permission_denied_does_not_retry_other_error_kinds() {
        let mut calls = 0;
        let result: io::Result<()> = retry_on_permission_denied(
            "fake",
            || {
                calls += 1;
                Err(io::Error::from(io::ErrorKind::NotFound))
            },
            Duration::from_millis(0),
            5,
        )
        .await;
        assert_eq!(calls, 1);
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
