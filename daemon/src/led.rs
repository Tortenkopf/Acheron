// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The `led` task: the single writer of the three side Status LEDs
//! (CONTEXT.md: Status LED assignment; ADR-0006). A dedicated, **non-fatal**
//! task — `main.rs` `tokio::spawn`s it *outside* its top-level `select!`, so
//! an LED write failure never exits the process (the common failure is
//! `NotFound`: the device is simply absent).
//!
//! Dispatch is the sole decider (it owns `Config`): on Profile switch,
//! device (re)connect and Daemon startup it pushes the active Profile's
//! triple on a `tokio::sync::watch` channel, and this task drives it to the
//! hardware over a short-lived Interface-2 hidraw fd
//! (`analog::assert_status_leds`). `watch` semantics coalesce a burst of
//! Profile switches to the final triple — no queue of stale writes, no
//! out-of-order A→B→A landing — and writes are serialised here: each
//! `spawn_blocking` assert is awaited before the next `changed()`.
//!
//! The actual `HIDIOCSFEATURE` frame is not unit-tested (every byte is
//! hardware-verified — see `analog::assert_status_leds`); the tests below
//! exercise this task's channel seam through an injected [`AssertLeds`]
//! recorder, mirroring `injector`'s `InjectSink` / `RecordingSink` split.

use std::io;

use tokio::sync::watch;

use crate::capture::analog;
use crate::config::StatusLeds;

/// The device-write half of the `led` task, mockable in tests. The real
/// impl is one short-lived Interface-2 hidraw write; tests substitute a
/// recorder so the task's channel behaviour can be asserted without a
/// device.
pub trait AssertLeds: Clone + Send + 'static {
    fn assert(&self, leds: StatusLeds) -> io::Result<()>;
}

/// The production writer: `analog::assert_status_leds` on a `spawn_blocking`
/// thread.
#[derive(Debug, Clone, Copy)]
pub struct HidrawLeds;

impl AssertLeds for HidrawLeds {
    fn assert(&self, leds: StatusLeds) -> io::Result<()> {
        analog::assert_status_leds(leds)
    }
}

/// Spawn the `led` task with the production hidraw writer.
pub fn spawn(rx: watch::Receiver<Option<StatusLeds>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run(rx, HidrawLeds))
}

/// The task loop: wait for a change, take the latest `Option<StatusLeds>`,
/// and — if `Some` — drive it to the hardware on a blocking thread, awaiting
/// the write before looping (serialising writes within the task). A failed
/// write is logged once (device absent = `NotFound`, harmless) and the loop
/// keeps running. Returns when the `watch::Sender` is dropped (process
/// teardown).
pub async fn run(mut rx: watch::Receiver<Option<StatusLeds>>, writer: impl AssertLeds) {
    while rx.changed().await.is_ok() {
        let Some(leds) = *rx.borrow_and_update() else {
            continue;
        };
        let writer = writer.clone();
        match tokio::task::spawn_blocking(move || writer.assert(leds)).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => eprintln!(
                "acheron-daemon: led: could not assert Status LEDs \
                 (harmless if the device is absent): {err}"
            ),
            Err(join_err) => {
                eprintln!("acheron-daemon: led: Status-LED write task panicked: {join_err}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<StatusLeds>>>);

    impl Recorder {
        fn writes(&self) -> Vec<StatusLeds> {
            self.0.lock().unwrap().clone()
        }
    }

    impl AssertLeds for Recorder {
        fn assert(&self, leds: StatusLeds) -> io::Result<()> {
            self.0.lock().unwrap().push(leds);
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

    #[tokio::test]
    async fn a_some_triple_drives_exactly_one_assert_with_that_triple() {
        let (tx, rx) = watch::channel(None);
        let recorder = Recorder::default();
        let task = tokio::spawn(run(rx, recorder.clone()));

        tx.send(Some(leds(true, false, true))).unwrap();
        drop(tx);
        task.await.unwrap();

        assert_eq!(recorder.writes(), vec![leds(true, false, true)]);
    }

    #[tokio::test]
    async fn a_none_on_the_channel_drives_no_assert() {
        let (tx, rx) = watch::channel(Some(leds(true, true, true)));
        let recorder = Recorder::default();
        let task = tokio::spawn(run(rx, recorder.clone()));

        // The channel's initial value is never asserted (it's not a
        // "change"); an explicit `None` is skipped too.
        tx.send(None).unwrap();
        drop(tx);
        task.await.unwrap();

        assert!(recorder.writes().is_empty());
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

        let task = tokio::spawn(run(rx, recorder.clone()));
        drop(tx);
        task.await.unwrap();

        assert_eq!(recorder.writes(), vec![leds(false, false, true)]);
    }
}
