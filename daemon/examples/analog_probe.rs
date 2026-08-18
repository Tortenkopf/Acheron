//! Manual HITL verification tool for ticket 22's `AnalogCaptureSource` —
//! not part of the daemon itself, and not wired into `main.rs` (that's
//! ticket 23's job). Runs the real `analog::AnalogCaptureSource` against
//! the connected Tartarus Pro and prints every `PhysicalEvent`/connection
//! transition it produces, so a human can confirm ticket 22's checklist:
//!
//!   - unlock succeeds and report 0x06 arrives (a `connected` line appears)
//!   - all 20 Grid keys threshold correctly at the default 128/112
//!     Actuation/Release points (press each key at varying depth, confirm
//!     Down fires around half-travel and Up a little before full release)
//!   - Hold-to-repeat synthesizes at a rate that looks right against the
//!     cached kernel delay/period (hold a Grid key down, count Repeats)
//!   - Main/If02 events (Mode key, thumbstick, wheel) keep flowing
//!     unaffected alongside the grid task
//!   - a permission-denied hidraw open degrades silently (run this once
//!     *before* granting hidraw access — no `connected` line, no crash,
//!     just silence until access is granted and the next poll succeeds)
//!
//! Usage (`/dev/hidraw0-2` need read/write access first — either install
//! `packaging/60-acheron-tartarus-pro.rules` via `install.sh` and be a
//! member of the `plugdev` group, run this whole binary under `sudo`, or
//! `sudo chmod 660 /dev/hidraw0 /dev/hidraw1 /dev/hidraw2` first):
//!
//!     cargo run --example analog_probe [duration_seconds]
//!
//! Ctrl-C or the timer (default 60s) ends the run. The device is left in
//! Analog Capture mode afterward — re-lock it with the already-verified
//! prototype (`sudo python3 prototype/13-analog-grid-capture/prototype.py
//! relock`) so the 20 Grid keys work normally again before unplugging or
//! relying on them outside this probe.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use acheron_daemon::capture::analog::AnalogCaptureSource;
use acheron_daemon::capture::{CaptureSource, PhysicalEvent};
use acheron_daemon::config::Profile;

#[tokio::main]
async fn main() {
    let duration_secs: u64 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(60);

    println!("acheron analog_probe: running AnalogCaptureSource for {duration_secs}s");
    println!(
        "press Grid keys at varying depth, hold one to check repeat, and try the Mode key/thumbstick/wheel too"
    );
    println!();

    let snapshot = Profile::default().resolved_actuation_points();
    let (actuation_tx, actuation_rx) = tokio::sync::watch::channel(snapshot);
    // Keep the sender alive for the probe's lifetime — a dropped Sender
    // would close the channel and the grid task would see every future
    // `.borrow()` return the last value anyway, but holding it matches how
    // `dispatch::run` owns it for real.
    let _actuation_tx = actuation_tx;

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);
    let (connection_tx, mut connection_rx) = tokio::sync::mpsc::channel(8);

    let source = AnalogCaptureSource::new(actuation_rx, Arc::new(AtomicBool::new(false)));
    tokio::spawn(async move {
        if let Err(err) = source.run(event_tx, connection_tx).await {
            eprintln!("AnalogCaptureSource exited with a fatal error: {err}");
        }
    });

    let deadline = tokio::time::sleep(Duration::from_secs(duration_secs));
    tokio::pin!(deadline);

    let mut event_count: u64 = 0;
    loop {
        tokio::select! {
            () = &mut deadline => {
                println!("\n{duration_secs}s elapsed, stopping. {event_count} PhysicalEvent(s) observed.");
                break;
            }
            connected = connection_rx.recv() => {
                match connected {
                    Some(connected) => println!("*** connection: {connected} ***"),
                    None => {
                        println!("connection channel closed — AnalogCaptureSource has exited");
                        break;
                    }
                }
            }
            event = event_rx.recv() => {
                match event {
                    Some(PhysicalEvent { input, state, depth }) => {
                        event_count += 1;
                        println!("{input}\t{state:?}\tdepth={depth:?}");
                    }
                    None => {
                        println!("event channel closed — AnalogCaptureSource has exited");
                        break;
                    }
                }
            }
        }
    }

    println!();
    println!("Reminder: the device is still in Analog Capture mode.");
    println!("Re-lock it: sudo python3 prototype/13-analog-grid-capture/prototype.py relock");
}
