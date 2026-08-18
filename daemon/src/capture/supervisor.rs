//! The capture supervisor (ticket 18 §6, wired here in ticket 23): owns
//! *which* `CaptureSource` is currently running — `AnalogCaptureSource` or
//! `EvdevCaptureSource` — and swaps between them without tearing down
//! dispatch, injector, or the D-Bus server (`main.rs` spawns this in their
//! place). Three, and only three, moments ever trigger an attempt to
//! (re)acquire Analog: Daemon startup, a genuine device reconnect while
//! Digital is running only because Analog didn't come up, and an explicit
//! `SetForceDigital(false)` call — never a background timer (ticket 18 §6).
//!
//! `event_tx`/`connection_tx` are this task's own persistent clones (per
//! ticket 18 §6's suggested route): each `CaptureSource` attempt gets a
//! fresh `tx.clone()`, so a swap never risks the shared channels seeing a
//! spurious close in the gap between stopping one source and starting the
//! next.
//!
//! Swapping is a real stop-then-start, not a detach-and-abandon: the
//! outgoing source's `shutdown` flag is set and its task is *awaited* to
//! completion (ticket 23's cooperative-cancellation additions to
//! `evdev_source`/`analog`) before the incoming one is spawned. Without
//! that, the outgoing source's Main/If02 evdev grabs could still be held
//! when the incoming source tries to `grab()` the same nodes, failing with
//! `EBUSY` — not an absence condition, and fatal.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::analog::AnalogCaptureSource;
use super::evdev_source::EvdevCaptureSource;
use super::{CaptureMode, CaptureSource, PhysicalEvent};
use crate::config::ActuationPoint;
use crate::input::Input;

/// How long a fresh Analog attempt gets to reach a fully-connected grid
/// before the supervisor gives up and falls back to Digital (ticket 18 §6/
/// §7's "failure/fallback" case). The grid task's own absence-retry bucket
/// (ticket 22) treats a missing udev rule (`PermissionDenied`) exactly like
/// a not-yet-plugged-in device — silent, retried forever, never fatal — so
/// there is no error to catch here; the only external signal available is
/// "did the grid's presence slot ever go true." Generous relative to real
/// hardware's sub-100ms unlock latency (tickets 13/16) and the grid task's
/// own `POLL_INTERVAL` (2s) retry cadence, so a slow-but-working unlock
/// isn't mistaken for a missing rule, but short enough not to noticeably
/// delay startup when the rule genuinely is missing.
const ANALOG_STARTUP_GRACE: Duration = Duration::from_secs(6);

/// One supervised attempt's outcome: either its `CaptureSource` task ended
/// on its own (a genuine fatal error, or `event_tx`'s receiver closing —
/// dispatch exited), or the supervisor decided to swap to a different mode.
enum Outcome {
    Ended(io::Result<()>),
    SwapTo(CaptureMode),
}

/// Runs for the process's whole lifetime, participating in `main.rs`'s
/// top-level `tokio::select!` as the "capture" branch exactly where
/// `EvdevCaptureSource::ALL.run(...)` used to sit directly. `force_digital`
/// is `Config.force_digital`'s value at startup; `control_rx` carries every
/// later value dispatch pushes on a `SetForceDigital` call.
pub async fn run(
    event_tx: mpsc::Sender<PhysicalEvent>,
    connection_tx: mpsc::Sender<bool>,
    actuation_rx: watch::Receiver<HashMap<Input, ActuationPoint>>,
    mode_tx: mpsc::Sender<CaptureMode>,
    mut control_rx: mpsc::Receiver<bool>,
    force_digital: bool,
) -> io::Result<()> {
    let mut force_digital = force_digital;
    let mut mode = if force_digital {
        CaptureMode::Digital
    } else {
        CaptureMode::Analog
    };

    loop {
        let _ = mode_tx.send(mode).await;

        let shutdown = Arc::new(AtomicBool::new(false));
        let (inner_conn_tx, mut inner_conn_rx) = mpsc::channel::<bool>(8);
        let mut handle: JoinHandle<io::Result<()>> = match mode {
            CaptureMode::Analog => {
                let source = AnalogCaptureSource::new(actuation_rx.clone(), shutdown.clone());
                tokio::spawn(source.run(event_tx.clone(), inner_conn_tx))
            }
            CaptureMode::Digital => {
                let source = EvdevCaptureSource::all(shutdown.clone());
                tokio::spawn(source.run(event_tx.clone(), inner_conn_tx))
            }
        };

        // Only Analog ever needs a grace-period fallback — Digital *is* the
        // fallback. `grace_armed` gates the branch below so an unused
        // `Sleep` (the Digital case) is simply never polled, rather than
        // needing an `Option<Sleep>`.
        let grace_armed_initially = mode == CaptureMode::Analog;
        let grace = tokio::time::sleep(if grace_armed_initially {
            ANALOG_STARTUP_GRACE
        } else {
            Duration::ZERO
        });
        tokio::pin!(grace);
        let mut grace_armed = grace_armed_initially;
        let mut last_connected: Option<bool> = None;
        // Ticket 23, found live: a *fresh* `EvdevCaptureSource::all`
        // attempt's own three nodes (Main/If01/If02) reach "all connected"
        // one at a time, each publishing the combined presence value as it
        // changes (`evdev_source::report_presence`) — so the very first
        // messages from a brand-new Digital attempt are typically `false`
        // (one or two nodes up, not all three yet) followed by `true`. Read
        // naively, that `false`-then-`true` is indistinguishable from a
        // genuine external reconnect and fired `should_retry_upgrade`
        // immediately on every fresh fallback, before the device had done
        // anything at all — an infinite Digital/Analog thrash loop.
        // `ever_connected` gates the reconnect check on this attempt having
        // reached a *real* fully-connected state at least once already, so
        // only a later, genuine disconnect-then-reconnect counts.
        let mut ever_connected = false;

        let outcome = loop {
            tokio::select! {
                result = &mut handle => {
                    break Outcome::Ended(result.unwrap_or_else(|join_err| Err(io::Error::other(join_err))));
                }
                connected = inner_conn_rx.recv() => {
                    let Some(connected) = connected else {
                        // The running source's own connection-status half
                        // closed without `handle` finishing — not expected
                        // given `CaptureSource::run`'s contract, but there's
                        // nothing useful to do except keep waiting on
                        // `handle` on the next loop iteration.
                        continue;
                    };
                    let _ = connection_tx.send(connected).await;
                    if connected {
                        grace_armed = false;
                    }
                    if should_retry_upgrade(mode, force_digital, ever_connected, last_connected, connected) {
                        break Outcome::SwapTo(CaptureMode::Analog);
                    }
                    if connected {
                        ever_connected = true;
                    }
                    last_connected = Some(connected);
                }
                () = &mut grace, if grace_armed => {
                    // Ticket 18 §6/§7: Analog never came up within the grace
                    // window — most likely a missing udev rule (`hidraw`
                    // open denied) or the device simply isn't plugged in
                    // yet. Fall back to full Digital capture so the 20 grid
                    // keys keep working via evdev passthrough (the map's
                    // standing "never leave the user with a crippled
                    // keypad" discipline) — `should_retry_upgrade` picks
                    // Analog back up automatically once a genuine reconnect
                    // is observed.
                    break Outcome::SwapTo(CaptureMode::Digital);
                }
                control = control_rx.recv() => {
                    let Some(new_force_digital) = control else {
                        // Dispatch is gone — nothing left to react to
                        // `SetForceDigital` calls with, but the current
                        // source keeps running until `handle` finishes on
                        // its own.
                        continue;
                    };
                    force_digital = new_force_digital;
                    let desired = if force_digital { CaptureMode::Digital } else { CaptureMode::Analog };
                    if desired != mode {
                        break Outcome::SwapTo(desired);
                    }
                }
            }
        };

        let next_mode = match outcome {
            Outcome::Ended(result) => return result,
            Outcome::SwapTo(next_mode) => next_mode,
        };

        // Stop the outgoing source and *wait* for it to actually finish —
        // `evdev_source::join_first`'s shutdown-aware draining (ticket 23)
        // means every grab/fd it held is released by the time `handle`
        // resolves, so the next source's own `grab()` calls can't race an
        // `EBUSY` against a still-open fd from this one.
        shutdown.store(true, Ordering::Relaxed);
        let _ = handle.await;
        if mode == CaptureMode::Analog {
            // Best-effort: a failure here (device already unplugged, no
            // hidraw access) doesn't block the swap itself — there would be
            // nothing left to relock in that case anyway.
            if let Err(err) = super::analog::relock() {
                eprintln!(
                    "acheron-daemon: relock before swapping away from analog failed (continuing anyway): {err}"
                );
            }
        }
        mode = next_mode;
    }
}

/// Ticket 18 §6's third retry-upgrade trigger: a genuine device reconnect
/// while Digital is running *only because* Analog didn't come up — never
/// while `force_digital` is set, which is an explicit user override, not a
/// fallback. Two guards distinguish a real reconnect from noise, both
/// needed (confirmed live — see `run`'s `ever_connected` comment for the bug
/// this fixes): `ever_connected` requires this attempt to have reached a
/// genuine fully-connected state at least once already — a *fresh*
/// Digital attempt's own three nodes converge to "all connected" one at a
/// time and publish `false` in between, which looks identical to a
/// disconnect if this weren't gated. `last_connected == Some(false)` on top
/// of that requires an actual, later absence-then-presence edge, not just
/// "already connected, connected again" — without it, a fallback that never
/// truly drops (the common case once past its own startup) would retrigger
/// on every redundant `true` push and loop straight back into the
/// still-broken Analog path forever, exactly the "never a background timer"
/// property ticket 18 §6 rules out.
fn should_retry_upgrade(
    mode: CaptureMode,
    force_digital: bool,
    ever_connected: bool,
    last_connected: Option<bool>,
    connected: bool,
) -> bool {
    mode == CaptureMode::Digital
        && !force_digital
        && ever_connected
        && last_connected == Some(false)
        && connected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reconnect_after_a_real_disconnect_triggers_upgrade() {
        assert!(should_retry_upgrade(
            CaptureMode::Digital,
            false,
            true,
            Some(false),
            true
        ));
    }

    #[test]
    fn the_first_connection_of_a_fresh_fallback_does_not_trigger_upgrade() {
        assert!(!should_retry_upgrade(
            CaptureMode::Digital,
            false,
            false,
            None,
            true
        ));
    }

    /// The live bug this ticket found and fixed: a fresh
    /// `EvdevCaptureSource::all` attempt's own three nodes converge to "all
    /// connected" one at a time, so its first messages are typically
    /// `false` (not every node up yet) then `true` — indistinguishable from
    /// a real reconnect unless `ever_connected` gates it. Without the gate,
    /// this exact sequence caused an infinite Digital/Analog thrash loop on
    /// real hardware within seconds of every fallback.
    #[test]
    fn a_fresh_attempts_own_startup_convergence_does_not_trigger_upgrade() {
        assert!(!should_retry_upgrade(
            CaptureMode::Digital,
            false,
            false,
            Some(false),
            true
        ));
    }

    #[test]
    fn a_disconnect_never_triggers_upgrade() {
        assert!(!should_retry_upgrade(
            CaptureMode::Digital,
            false,
            true,
            Some(true),
            false
        ));
    }

    #[test]
    fn force_digital_suppresses_reconnect_triggered_upgrade() {
        assert!(!should_retry_upgrade(
            CaptureMode::Digital,
            true,
            true,
            Some(false),
            true
        ));
    }

    #[test]
    fn already_running_analog_never_triggers_upgrade() {
        assert!(!should_retry_upgrade(
            CaptureMode::Analog,
            false,
            true,
            Some(false),
            true
        ));
    }
}
