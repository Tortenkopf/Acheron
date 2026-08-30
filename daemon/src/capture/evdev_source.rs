// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The real `CaptureSource`: grabs the Tartarus Pro's three evdev nodes
//! exclusively (`EVIOCGRAB`) and normalizes their raw events into
//! `PhysicalEvent`s, per issue 07's design.
//!
//! Ticket 20's failure-handling split: a node whose device-absent condition
//! (nodes don't exist — boot-before-plugin, or a mid-run unplug surfacing as
//! `ENODEV` on the next read) is non-fatal — that node's task polls its own
//! `/dev/input/by-id/...` path every `POLL_INTERVAL` until it reopens
//! cleanly, then resumes, rather than exiting. A genuine capture error
//! (anything else — permission errors, unexpected fd errors) remains
//! fatal-exit, per issue 07's original "any capture failure is fatal" call.

use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use evdev::{Device, EventSummary, RelativeAxisCode};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::{CaptureSource, EventState, PhysicalEvent};
use crate::input::{self, Input, Node, WheelEvent};

/// Shared with `analog::grid_task_blocking` (ticket 22) via `pub(super)` —
/// both retry loops wait the same interval between silent absence-retries.
pub(super) const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How often a node's relay loop wakes up with no new event pending, purely
/// so it can notice a shutdown request promptly (ticket 23) — evdev's
/// `fetch_events` has no built-in timeout, so responsiveness to a supervisor
/// swap/process shutdown depends entirely on this poll cadence. Generous
/// relative to real key-press latency; this only bounds how quickly a *stop*
/// is noticed, not normal event delivery (which is poll-then-read, not
/// poll-interval-throttled).
const SHUTDOWN_POLL_TIMEOUT_MS: i32 = 200;

/// Grabs a fixed subset of the Tartarus Pro's evdev nodes exclusively and
/// relays every physical input unchanged into the shared channel. Each node
/// is read on its own `spawn_blocking` background task, since evdev's
/// `fetch_events` blocks the OS thread waiting for input.
///
/// Ticket 22 generalized this from "always all three nodes" so
/// `analog::AnalogCaptureSource` can reuse the same open/grab/relay/retry
/// logic (including `is_device_absent`) for a `[Node::Main, Node::If02]`
/// subset, without duplicating it — see `spawn_nodes` below.
pub struct EvdevCaptureSource {
    nodes: &'static [Node],
    /// Ticket 23: shared with every node task this source spawns. Setting it
    /// makes every task stop cleanly (release its grab) at the next poll
    /// tick or absence-retry check, rather than blocking forever — the
    /// supervisor's live source-swap and the process's SIGTERM/SIGINT
    /// shutdown path both rely on this to know a source has actually let go
    /// of its evdev nodes before starting or relocking the next thing.
    shutdown: Arc<AtomicBool>,
}

impl EvdevCaptureSource {
    /// The pre-ticket-23 behavior: all three nodes, Digital Capture mode's
    /// only source. `shutdown` is this attempt's own flag — the supervisor
    /// (ticket 23) hands each `CaptureSource` attempt a fresh one and sets
    /// it to request a clean stop.
    pub fn all(shutdown: Arc<AtomicBool>) -> EvdevCaptureSource {
        EvdevCaptureSource {
            nodes: &Node::ALL,
            shutdown,
        }
    }
}

impl CaptureSource for EvdevCaptureSource {
    async fn run(
        self,
        tx: mpsc::Sender<PhysicalEvent>,
        connection_tx: mpsc::Sender<bool>,
    ) -> io::Result<()> {
        let mut tasks = JoinSet::new();
        spawn_nodes(
            &mut tasks,
            self.nodes,
            0,
            presence_for(self.nodes.len()),
            tx,
            connection_tx,
            self.shutdown.clone(),
        );
        join_first(tasks, &self.shutdown).await
    }
}

/// A fresh, all-absent presence view sized for `len` tracked slots.
pub(super) fn presence_for(len: usize) -> Arc<Mutex<Vec<bool>>> {
    Arc::new(Mutex::new(vec![false; len]))
}

/// Spawns one `spawn_blocking` task per entry in `nodes` into the caller's
/// `JoinSet`, each reporting its presence into `presence` at
/// `base_index + offset` — letting a caller with extra tracked slots (ticket
/// 22's grid task, at the next index after `[Node::Main, Node::If02]`) share
/// one combined connectivity view across nodes it opens itself and nodes it
/// delegates here, per ticket 18 §1's "one more task, same bookkeeping."
pub(super) fn spawn_nodes(
    tasks: &mut JoinSet<io::Result<()>>,
    nodes: &'static [Node],
    base_index: usize,
    presence: Arc<Mutex<Vec<bool>>>,
    tx: mpsc::Sender<PhysicalEvent>,
    connection_tx: mpsc::Sender<bool>,
    shutdown: Arc<AtomicBool>,
) {
    for (offset, &node) in nodes.iter().enumerate() {
        let index = base_index + offset;
        let tx = tx.clone();
        let connection_tx = connection_tx.clone();
        let presence = presence.clone();
        let shutdown = shutdown.clone();
        tasks.spawn_blocking(move || {
            capture_node_blocking(node, index, tx, connection_tx, presence, shutdown)
        });
    }
}

/// Awaits the first task in `tasks` to finish. Device-absence is handled
/// entirely inside `capture_node_blocking`'s own poll loop and never bubbles
/// up here, so under normal operation reaching this point means either clean
/// shutdown (channel closed) or a genuine, non-absent error, matching issue
/// 07's original "any capture failure is fatal" call.
///
/// Ticket 23: if `shutdown` is set, the first task to finish is expected to
/// be *this* source's own deliberate stop (a supervisor swap or process
/// shutdown), not a fatal surprise — every sibling task shares the same flag
/// and is also winding down, so this drains the rest of `tasks` before
/// returning, rather than the caller (the supervisor) racing to start a new
/// `CaptureSource` against evdev/hidraw nodes this one hasn't actually let
/// go of yet (a stale grab fails the new source's own `grab()` with
/// `EBUSY`, which is not an absence condition and would be fatal).
pub(super) async fn join_first(
    mut tasks: JoinSet<io::Result<()>>,
    shutdown: &AtomicBool,
) -> io::Result<()> {
    let first = match tasks.join_next().await {
        Some(Ok(result)) => result,
        Some(Err(join_err)) => Err(io::Error::other(join_err)),
        None => Ok(()),
    };
    if shutdown.load(Ordering::Relaxed) {
        while tasks.join_next().await.is_some() {}
    }
    first
}

/// Updates this node's slot in the shared `presence` view and pushes the
/// combined "every tracked slot present" value into `connection_tx`.
pub(super) fn report_presence(
    presence: &Mutex<Vec<bool>>,
    index: usize,
    connected: bool,
    connection_tx: &mpsc::Sender<bool>,
) {
    let all_connected = {
        let mut guard = presence.lock().unwrap();
        guard[index] = connected;
        guard.iter().all(|&c| c)
    };
    let _ = connection_tx.blocking_send(all_connected);
}

/// One node's full lifecycle: open+grab, relay events, and — on a
/// device-absent condition at any of those stages — fall back to polling
/// `POLL_INTERVAL` apart until the node reopens cleanly, then resume. Returns
/// `Ok(())` on a genuine, non-absent error, the shared channel closing, or
/// `shutdown` being set (ticket 23); a genuine non-absent error still
/// propagates as `Err`.
fn capture_node_blocking(
    node: Node,
    index: usize,
    tx: mpsc::Sender<PhysicalEvent>,
    connection_tx: mpsc::Sender<bool>,
    presence: Arc<Mutex<Vec<bool>>>,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }

        let mut device = match Device::open(node.device_path()) {
            Ok(device) => device,
            Err(err) if is_device_absent(&err) => {
                report_presence(&presence, index, false, &connection_tx);
                interruptible_sleep(POLL_INTERVAL, &shutdown);
                continue;
            }
            Err(err) => return Err(err),
        };

        if let Err(err) = device.grab() {
            if is_device_absent(&err) {
                report_presence(&presence, index, false, &connection_tx);
                interruptible_sleep(POLL_INTERVAL, &shutdown);
                continue;
            }
            return Err(err);
        }
        // Ticket 23: without this, `fetch_events` in `relay_events_blocking`
        // below blocks the OS thread indefinitely between physical events,
        // with no way to notice `shutdown` — a live source swap or process
        // signal would have to wait for the next keypress to ever see it.
        device.set_nonblocking(true)?;

        report_presence(&presence, index, true, &connection_tx);

        match relay_events_blocking(&mut device, node, &tx, &shutdown) {
            Ok(()) => return Ok(()),
            Err(err) if is_device_absent(&err) => {
                report_presence(&presence, index, false, &connection_tx);
                interruptible_sleep(POLL_INTERVAL, &shutdown);
                continue;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Relays events for as long as the node stays open and the shared channel
/// stays alive. Returns `Ok(())` once `tx` closes (clean shutdown) or
/// `shutdown` is set (ticket 23, checked every `SHUTDOWN_POLL_TIMEOUT_MS`);
/// returns `Err` on any read failure, absent-device or genuine alike — the
/// caller tells those apart via `is_device_absent`.
fn relay_events_blocking(
    device: &mut Device,
    node: Node,
    tx: &mpsc::Sender<PhysicalEvent>,
    shutdown: &AtomicBool,
) -> io::Result<()> {
    let fd = device.as_raw_fd();
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !poll_readable(fd, SHUTDOWN_POLL_TIMEOUT_MS)? {
            continue;
        }
        let events = match device.fetch_events() {
            Ok(events) => events,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => continue,
            Err(err) => return Err(err),
        };
        for event in events {
            let Some(physical) = normalize(node, event.destructure()) else {
                continue;
            };
            if tx.blocking_send(physical).is_err() {
                return Ok(());
            }
        }
    }
}

/// Blocks up to `timeout_ms` for `fd` to become readable, retrying
/// transparently on `EINTR` (a signal interrupting the syscall, not a real
/// failure). Shared by every blocking capture loop (evdev nodes here,
/// `analog::grid_task_blocking`'s `hidraw` reads) — all of them need the
/// same "wake up periodically even with nothing pending" shape so a
/// `shutdown` flag can be noticed promptly (ticket 23) rather than blocking
/// on the underlying `read()` indefinitely.
pub(super) fn poll_readable(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    loop {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if ret < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        // `POLLHUP`/`POLLERR` must count as "readable" too, not just
        // `POLLIN` — see `analog`'s own
        // `a_dropped_connection_force_releases_...` test, which pins this
        // for the `hidraw` case: once a peer closes and the buffer drains,
        // `poll` can report `POLLHUP` alone (no `POLLIN` bit), and treating
        // that as "not readable" spins this loop in a tight, immediately-
        // returning busy-loop forever instead of ever calling `read()` to
        // observe the EOF. A subsequent `read()`/`fetch_events()` on a
        // `POLLHUP`/`POLLERR` fd correctly returns `0`/`WouldBlock` or an
        // `Err`, which every caller already handles.
        const READABLE: i16 = libc::POLLIN | libc::POLLHUP | libc::POLLERR;
        return Ok(ret > 0 && pfd.revents & READABLE != 0);
    }
}

/// A `std::thread::sleep(duration)` that gives up early — in small
/// increments, so the check itself never runs for longer than one
/// increment — the moment `shutdown` is set. Every absence-retry wait in
/// this module and `analog::grid_task_blocking` uses this instead of a bare
/// sleep (ticket 23), so a live source swap or process shutdown lands within
/// a fraction of a second even mid-retry rather than up to a full
/// `POLL_INTERVAL` (2s) later.
pub(super) fn interruptible_sleep(duration: Duration, shutdown: &AtomicBool) {
    const CHECK_INTERVAL: Duration = Duration::from_millis(100);
    let mut remaining = duration;
    while remaining > Duration::ZERO {
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        let step = remaining.min(CHECK_INTERVAL);
        std::thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
}

/// Linux's `ENODEV` errno — not pulled from a `libc` dependency for one
/// constant; this value is part of the stable POSIX/Linux ABI, not
/// platform-detected at runtime.
const ENODEV: i32 = 19;

/// Device-absent covers both causes ticket 20 asks to treat identically: the
/// node doesn't exist yet (`NotFound`, e.g. boot-before-plugin — `open`
/// fails this way) and a mid-run unplug surfacing as `ENODEV` on a
/// still-open fd (`grab`/`fetch_events` fail this way instead of `NotFound`,
/// since the fd itself is still valid, just the underlying device is gone).
///
/// `pub(super)`: `analog::is_grid_absent` (ticket 22) builds on this rather
/// than re-deriving the `NotFound`/`ENODEV` half of its own, broader check
/// (which also treats `PermissionDenied`/`EIO` as absence — a not-yet-
/// installed udev rule, and a `hidraw` read failing differently from evdev's
/// `fetch_events` on unplug).
pub(super) fn is_device_absent(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::NotFound || err.raw_os_error() == Some(ENODEV)
}

fn normalize(node: Node, summary: EventSummary) -> Option<PhysicalEvent> {
    match summary {
        EventSummary::Key(_, code, value) => {
            let input = input::input_for_key(node, code)?;
            let state = key_value_to_state(value)?;
            Some(PhysicalEvent {
                input,
                state,
                depth: None,
            })
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
                depth: None,
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
