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
}

impl EvdevCaptureSource {
    /// The pre-ticket-22 behavior: all three nodes, Digital Capture mode's
    /// only source.
    pub const ALL: EvdevCaptureSource = EvdevCaptureSource { nodes: &Node::ALL };
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
        );
        join_first(tasks).await
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
) {
    for (offset, &node) in nodes.iter().enumerate() {
        let index = base_index + offset;
        let tx = tx.clone();
        let connection_tx = connection_tx.clone();
        let presence = presence.clone();
        tasks.spawn_blocking(move || {
            capture_node_blocking(node, index, tx, connection_tx, presence)
        });
    }
}

/// Awaits the first task in `tasks` to finish, treating that as fatal for
/// the whole source — device-absence is handled entirely inside
/// `capture_node_blocking`'s own poll loop and never bubbles up here, so
/// reaching this point means either clean shutdown (channel closed) or a
/// genuine, non-absent error, matching issue 07's original "any capture
/// failure is fatal" call.
pub(super) async fn join_first(mut tasks: JoinSet<io::Result<()>>) -> io::Result<()> {
    match tasks.join_next().await {
        Some(Ok(result)) => result,
        Some(Err(join_err)) => Err(io::Error::other(join_err)),
        None => Ok(()),
    }
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
/// `POLL_INTERVAL` apart until the node reopens cleanly, then resume. Only
/// returns on a genuine (non-absent) error or the shared channel closing.
fn capture_node_blocking(
    node: Node,
    index: usize,
    tx: mpsc::Sender<PhysicalEvent>,
    connection_tx: mpsc::Sender<bool>,
    presence: Arc<Mutex<Vec<bool>>>,
) -> io::Result<()> {
    loop {
        let mut device = match Device::open(node.device_path()) {
            Ok(device) => device,
            Err(err) if is_device_absent(&err) => {
                report_presence(&presence, index, false, &connection_tx);
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
            Err(err) => return Err(err),
        };

        if let Err(err) = device.grab() {
            if is_device_absent(&err) {
                report_presence(&presence, index, false, &connection_tx);
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
            return Err(err);
        }

        report_presence(&presence, index, true, &connection_tx);

        match relay_events_blocking(&mut device, node, &tx) {
            Ok(()) => return Ok(()),
            Err(err) if is_device_absent(&err) => {
                report_presence(&presence, index, false, &connection_tx);
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
            Err(err) => return Err(err),
        }
    }
}

/// Relays events for as long as the node stays open and the shared channel
/// stays alive. Returns `Ok(())` only once `tx` closes (clean shutdown);
/// returns `Err` on any read failure, absent-device or genuine alike — the
/// caller tells those apart via `is_device_absent`.
fn relay_events_blocking(
    device: &mut Device,
    node: Node,
    tx: &mpsc::Sender<PhysicalEvent>,
) -> io::Result<()> {
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
