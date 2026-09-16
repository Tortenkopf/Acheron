// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The `led` task: the single writer of both the three side Status LEDs
//! (CONTEXT.md: Status LED assignment; ADR-0006) and the RGB backlight
//! (CONTEXT.md: Lighting assignment; `tartarus-backlight`, ADR-0012) — one
//! task, because the device has one Interface-2 control channel and frames
//! from either feature must never interleave. A dedicated, **non-fatal**
//! task — `main.rs` `tokio::spawn`s it *outside* its top-level `select!`, so
//! a write failure never exits the process (the common failure is
//! `NotFound`: the device is simply absent).
//!
//! Dispatch is the sole decider (it owns `Config`): on Profile switch,
//! device (re)connect and Daemon startup it pushes the active Profile's
//! Status-LED triple and Lighting state on two independent `tokio::sync::
//! watch` channels (`led_tx`/`led_rx`, `lighting_tx`/`lighting_rx`), and this
//! task drives whichever changed to the hardware over a short-lived
//! Interface-2 hidraw fd (`analog::assert_status_leds` /
//! `analog::assert_lighting`). `watch` semantics coalesce a burst on either
//! channel to its final value — no queue of stale writes, no out-of-order
//! A→B→A landing — and writes are serialised across *both* channels: the
//! `select!` loop below only ever has one `spawn_blocking` write in flight,
//! fully awaited before the next `changed()`, regardless of which channel
//! woke it. Either channel's sender closing (never happens in production —
//! both live in `DispatchState` for the process's lifetime) just disables
//! that arm; the task only returns once both have closed.
//!
//! The actual `HIDIOCSFEATURE` frames are not unit-tested here (every byte
//! is hardware-verified — see `analog::assert_status_leds` /
//! `analog::assert_lighting`); the tests below exercise this task's channel
//! seam through an injected [`AssertLeds`]/[`AssertLighting`] recorder,
//! mirroring `injector`'s `InjectSink` / `RecordingSink` split.

use std::io;

use tokio::sync::watch;

use crate::capture::analog;
use crate::config::{LightingState, StatusLeds};

/// The Status-LED write half, mockable in tests. The real impl is one
/// short-lived Interface-2 hidraw write; tests substitute a recorder so the
/// task's channel behaviour can be asserted without a device.
pub trait AssertLeds: Clone + Send + 'static {
    fn assert(&self, leds: StatusLeds) -> io::Result<()>;
}

/// The Lighting write half, alongside [`AssertLeds`] — a separate trait
/// (rather than a second method on the same one, given the two channels'
/// independent value types) so a writer implements both without a method
/// name clash.
pub trait AssertLighting: Clone + Send + 'static {
    fn assert_lighting(&self, state: LightingState) -> io::Result<()>;
}

/// The production writer: `analog::assert_status_leds` / `analog::
/// assert_lighting`, each on a `spawn_blocking` thread.
#[derive(Debug, Clone, Copy)]
pub struct HidrawLeds;

impl AssertLeds for HidrawLeds {
    fn assert(&self, leds: StatusLeds) -> io::Result<()> {
        analog::assert_status_leds(leds)
    }
}

impl AssertLighting for HidrawLeds {
    fn assert_lighting(&self, state: LightingState) -> io::Result<()> {
        analog::assert_lighting(state)
    }
}

/// Spawn the `led` task with the production hidraw writer.
pub fn spawn(
    led_rx: watch::Receiver<Option<StatusLeds>>,
    lighting_rx: watch::Receiver<Option<LightingState>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run(led_rx, lighting_rx, HidrawLeds))
}

/// The task loop: a `tokio::select!` over both receivers. Each arm waits for
/// a change, takes the latest `Option<_>`, and — if `Some` — drives it to
/// the hardware on a blocking thread, awaiting the write before looping
/// (serialising writes across both channels within this one task). A failed
/// write is logged once (device absent = `NotFound`, harmless) and the loop
/// keeps running. An arm whose sender has dropped disables itself rather
/// than ending the task — the task only returns once both channels have
/// closed (in production, that's process teardown; a closed `led_tx` alone
/// must not silence Lighting writes, and vice versa).
pub async fn run(
    mut led_rx: watch::Receiver<Option<StatusLeds>>,
    mut lighting_rx: watch::Receiver<Option<LightingState>>,
    writer: impl AssertLeds + AssertLighting,
) {
    let mut led_open = true;
    let mut lighting_open = true;
    while led_open || lighting_open {
        tokio::select! {
            changed = led_rx.changed(), if led_open => {
                match changed {
                    Ok(()) => {
                        let Some(leds) = *led_rx.borrow_and_update() else {
                            continue;
                        };
                        let writer = writer.clone();
                        match tokio::task::spawn_blocking(move || writer.assert(leds)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) => eprintln!(
                                "acheron-daemon: led: could not assert Status LEDs \
                                 (harmless if the device is absent): {err}"
                            ),
                            Err(join_err) => eprintln!(
                                "acheron-daemon: led: Status-LED write task panicked: {join_err}"
                            ),
                        }
                    }
                    Err(_) => {
                        led_open = false;
                        eprintln!(
                            "acheron-daemon: led: the Status-LED channel closed unexpectedly \
                             (its sender should live for the process's lifetime); Lighting \
                             writes continue"
                        );
                    }
                }
            }
            changed = lighting_rx.changed(), if lighting_open => {
                match changed {
                    Ok(()) => {
                        let Some(state) = lighting_rx.borrow_and_update().clone() else {
                            continue;
                        };
                        let writer = writer.clone();
                        match tokio::task::spawn_blocking(move || writer.assert_lighting(state)).await {
                            Ok(Ok(())) => {}
                            Ok(Err(err)) => eprintln!(
                                "acheron-daemon: led: could not assert Lighting \
                                 (harmless if the device is absent): {err}"
                            ),
                            Err(join_err) => eprintln!(
                                "acheron-daemon: led: Lighting write task panicked: {join_err}"
                            ),
                        }
                    }
                    Err(_) => {
                        lighting_open = false;
                        eprintln!(
                            "acheron-daemon: led: the Lighting channel closed unexpectedly \
                             (its sender should live for the process's lifetime); Status-LED \
                             writes continue"
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Colour, FixedEffect, LightingAssignment};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct Recorder {
        leds: Arc<Mutex<Vec<StatusLeds>>>,
        lighting: Arc<Mutex<Vec<LightingState>>>,
        /// Set while a write is in progress, so a second write starting
        /// before the first clears it proves the two channels' writes ran
        /// concurrently — the exact regression the task's serialisation
        /// discipline exists to prevent.
        busy: Arc<AtomicBool>,
        interleaved: Arc<AtomicBool>,
    }

    impl Recorder {
        fn led_writes(&self) -> Vec<StatusLeds> {
            self.leds.lock().unwrap().clone()
        }

        fn lighting_writes(&self) -> Vec<LightingState> {
            self.lighting.lock().unwrap().clone()
        }

        fn saw_interleaving(&self) -> bool {
            self.interleaved.load(Ordering::SeqCst)
        }

        /// Shared by both trait impls: mark busy, sleep briefly (so a genuine
        /// interleaving regression has a window to land), record, clear busy.
        fn simulate_write(&self) {
            if self.busy.swap(true, Ordering::SeqCst) {
                self.interleaved.store(true, Ordering::SeqCst);
            }
            std::thread::sleep(Duration::from_millis(5));
            self.busy.store(false, Ordering::SeqCst);
        }
    }

    impl AssertLeds for Recorder {
        fn assert(&self, leds: StatusLeds) -> io::Result<()> {
            self.simulate_write();
            self.leds.lock().unwrap().push(leds);
            Ok(())
        }
    }

    impl AssertLighting for Recorder {
        fn assert_lighting(&self, state: LightingState) -> io::Result<()> {
            self.simulate_write();
            self.lighting.lock().unwrap().push(state);
            Ok(())
        }
    }

    fn leds(orange: bool, green: bool, blue: bool) -> StatusLeds {
        StatusLeds {
            orange,
            green,
            blue,
        }
    }

    fn lighting_state(brightness: u8) -> LightingState {
        LightingState {
            assignment: LightingAssignment::FixedEffect {
                effect: FixedEffect::Static {
                    colour: Colour {
                        r: 0xFF,
                        g: 0x00,
                        b: 0x00,
                    },
                },
            },
            brightness,
        }
    }

    /// Both channels start closed (no sender ever created) — used by the
    /// Status-LED-only tests below so `run`'s signature can take a live
    /// `lighting_rx` without any test needing to care about it.
    fn closed_lighting_channel() -> watch::Receiver<Option<LightingState>> {
        let (tx, rx) = watch::channel(None);
        drop(tx);
        rx
    }

    fn closed_led_channel() -> watch::Receiver<Option<StatusLeds>> {
        let (tx, rx) = watch::channel(None);
        drop(tx);
        rx
    }

    #[tokio::test]
    async fn a_some_triple_drives_exactly_one_assert_with_that_triple() {
        let (tx, rx) = watch::channel(None);
        let recorder = Recorder::default();
        let task = tokio::spawn(run(rx, closed_lighting_channel(), recorder.clone()));

        tx.send(Some(leds(true, false, true))).unwrap();
        drop(tx);
        task.await.unwrap();

        assert_eq!(recorder.led_writes(), vec![leds(true, false, true)]);
    }

    #[tokio::test]
    async fn a_none_on_the_channel_drives_no_assert() {
        let (tx, rx) = watch::channel(Some(leds(true, true, true)));
        let recorder = Recorder::default();
        let task = tokio::spawn(run(rx, closed_lighting_channel(), recorder.clone()));

        // The channel's initial value is never asserted (it's not a
        // "change"); an explicit `None` is skipped too.
        tx.send(None).unwrap();
        drop(tx);
        task.await.unwrap();

        assert!(recorder.led_writes().is_empty());
    }

    #[tokio::test]
    async fn a_burst_coalesces_to_the_final_triple() {
        let (tx, rx) = watch::channel(None);
        let recorder = Recorder::default();

        // All three sends land before the task first polls `changed()`, so
        // `watch` coalesces them — only the final triple is written.
        tx.send(Some(leds(true, false, false))).unwrap();
        tx.send(Some(leds(false, true, false))).unwrap();
        tx.send(Some(leds(false, false, true))).unwrap();

        let task = tokio::spawn(run(rx, closed_lighting_channel(), recorder.clone()));
        drop(tx);
        task.await.unwrap();

        assert_eq!(recorder.led_writes(), vec![leds(false, false, true)]);
    }

    #[tokio::test]
    async fn a_some_lighting_state_drives_exactly_one_assert_with_that_state() {
        let (tx, rx) = watch::channel(None);
        let recorder = Recorder::default();
        let state = lighting_state(0x80);
        let task = tokio::spawn(run(closed_led_channel(), rx, recorder.clone()));

        tx.send(Some(state.clone())).unwrap();
        drop(tx);
        task.await.unwrap();

        assert_eq!(recorder.lighting_writes(), vec![state]);
    }

    #[tokio::test]
    async fn a_none_on_the_lighting_channel_drives_no_assert() {
        let (tx, rx) = watch::channel(Some(lighting_state(0xFF)));
        let recorder = Recorder::default();
        let task = tokio::spawn(run(closed_led_channel(), rx, recorder.clone()));

        tx.send(None).unwrap();
        drop(tx);
        task.await.unwrap();

        assert!(recorder.lighting_writes().is_empty());
    }

    #[tokio::test]
    async fn a_burst_of_lighting_states_coalesces_to_the_final_state() {
        let (tx, rx) = watch::channel(None);
        let recorder = Recorder::default();

        tx.send(Some(lighting_state(0x10))).unwrap();
        tx.send(Some(lighting_state(0x20))).unwrap();
        tx.send(Some(lighting_state(0x30))).unwrap();

        let task = tokio::spawn(run(closed_led_channel(), rx, recorder.clone()));
        drop(tx);
        task.await.unwrap();

        assert_eq!(recorder.lighting_writes(), vec![lighting_state(0x30)]);
    }

    #[tokio::test]
    async fn both_channels_firing_in_the_same_tick_never_interleave() {
        let (led_tx, led_rx) = watch::channel(None);
        let (lighting_tx, lighting_rx) = watch::channel(None);
        let recorder = Recorder::default();
        let state = lighting_state(0x42);

        // Both land before the task's first poll — mirrors a real
        // simultaneous device-connect Status-LED + Lighting push.
        led_tx.send(Some(leds(true, false, false))).unwrap();
        lighting_tx.send(Some(state.clone())).unwrap();

        let task = tokio::spawn(run(led_rx, lighting_rx, recorder.clone()));
        drop(led_tx);
        drop(lighting_tx);
        task.await.unwrap();

        assert_eq!(recorder.led_writes(), vec![leds(true, false, false)]);
        assert_eq!(recorder.lighting_writes(), vec![state]);
        assert!(
            !recorder.saw_interleaving(),
            "the two channels' writes must never overlap"
        );
    }
}
