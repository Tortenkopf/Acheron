//! The Analog `CaptureSource` (CONTEXT.md: Capture mode — avoid "driver
//! mode", the research/prototype write-ups' working name): the Mode key,
//! thumbstick and wheel keep arriving over evdev exactly as
//! `EvdevCaptureSource` already reads them (ticket 16 confirmed driver mode
//! silences the 20 Grid keys and nothing else), and a `hidraw`-based grid
//! task reads the analog depth stream and synthesizes Down/Repeat/Up from
//! per-key Actuation/Release points (ticket 18's ten settled decisions).
//!
//! Structurally "one more node" (ticket 18 §1): `AnalogCaptureSource` reuses
//! `evdev_source::spawn_nodes` unchanged for `[Node::Main, Node::If02]` and
//! adds the grid task to the same `JoinSet`/presence/`connection_tx`
//! bookkeeping, rather than running as a second parallel `CaptureSource`.
//!
//! Protocol bytes (unlock/relock buffers, CRC, the `HIDIOCSFEATURE` ioctl
//! number, `/sys/class/hidraw` discovery) are ported from
//! `prototype/13-analog-grid-capture/prototype.py`, the working reference
//! verified byte-for-byte against the real device in tickets 13/16 — see
//! `protocol_constants_match_the_verified_prototype` below for the same
//! cross-check `prototype.py selftest` runs.
//!
//! Ticket 18 §9's test-seam goal: `observe` (Depth -> `EventState`
//! hysteresis) and `RepeatSchedule` (synthesized Hold-to-repeat timing) are
//! pure, hardware-free functions, unit-tested exhaustively below with no
//! channels/tokio/hardware involved — the grid task itself only wires them
//! to real I/O.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use evdev::Device;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use super::evdev_source::{self, interruptible_sleep, poll_readable};
use super::{CaptureSource, EventState, PhysicalEvent};
use crate::config::ActuationPoint;
use crate::input::{Input, Node};

// ---------------------------------------------------------------------------
// Protocol constants and byte layout — ported, not recomputed, from
// `prototype/13-analog-grid-capture/prototype.py` (research §2/§3, tickets
// 12/13/16).
// ---------------------------------------------------------------------------

const VENDOR_ID: &str = "1532";
const PRODUCT_ID: &str = "0244";

/// `hidraw` interface indices, per `bInterfaceNumber` (research §3.1):
/// interface 1 streams report `0x06`; interface 2 takes the feature report.
const ANALOG_INTERFACE: u8 = 1;
const CONTROL_INTERFACE: u8 = 2;

const ANALOG_REPORT_ID: u8 = 0x06;
const NUM_KEYS: usize = 20;

/// `transaction_id 0x01` is what `open-tartarus-driver` captured Synapse
/// sending, and the one variant ticket 12's research never reported to reset
/// the device — do not substitute OpenRazer's 0x1F/0xFF.
const TRANSACTION_ID: u8 = 0x01;
const CMD_CLASS_STANDARD: u8 = 0x00;
const CMD_SET_DEVICE_MODE: u8 = 0x04;
const MODE_DRIVER: u8 = 0x03;
const MODE_NORMAL: u8 = 0x00;

/// Ticket 18 §3: prototype observed report `0x06` arriving ~3ms after
/// unlock; 500ms is the generous upper bound before the fd is treated as
/// poisoned.
const UNLOCK_TIMEOUT: Duration = Duration::from_millis(500);
/// How often the grid loop polls the analog fd for readiness even with no
/// new report pending — bounds how stale a synthesized Repeat's timing can
/// get if the device's own report cadence ever turns out sparser than
/// continuous streaming (ticket 18 §4's regression concern: repeat cadence
/// must look right against the cached kernel delay/period).
const REPORT_POLL_TIMEOUT: Duration = Duration::from_millis(8);
/// The Linux kernel's own default autorepeat timing (`kbd_repeat` in
/// `drivers/input/input.c`) — the fallback used only if reading the real
/// value off the If01 evdev node fails; ticket 18 §4 exists specifically so
/// this hardcoded pair is not what normally governs Hold-to-repeat.
const DEFAULT_REPEAT_DELAY_MS: u32 = 250;
const DEFAULT_REPEAT_PERIOD_MS: u32 = 33;

const RAZER_CMD_LEN: usize = 91;

/// Linux's `_IOC(dir, type, nr, size)` (`asm-generic/ioctl.h`), specialized
/// to `HIDIOCSFEATURE(size)` exactly as `prototype.py`'s `hidiocsfeature`
/// computes it — checked against the documented `0xC05B4806` for `size =
/// 91` by `hidiocsfeature_91_matches_the_documented_ioctl_number` below.
const fn hidiocsfeature(size: u32) -> u64 {
    const IOC_WRITE: u64 = 1;
    const IOC_READ: u64 = 2;
    ((IOC_WRITE | IOC_READ) << 30) | ((size as u64) << 16) | (b'H' as u64) << 8 | 0x06
}

/// Mirrors `prototype.py`'s `build_razer_cmd`: the 91-byte buffer
/// `HIDIOCSFEATURE` wants — a leading report-number byte (always 0 for this
/// device) plus a 90-byte `razer_report`. The CRC is the XOR of buffer
/// indices 3..=88 (struct bytes 2..=87); `transaction_id` at index 2 is
/// deliberately excluded from it.
fn build_razer_cmd(txn: u8, command_class: u8, command_id: u8, args: &[u8]) -> [u8; RAZER_CMD_LEN] {
    let mut buf = [0u8; RAZER_CMD_LEN];
    buf[2] = txn;
    buf[6] = args.len() as u8;
    buf[7] = command_class;
    buf[8] = command_id;
    buf[9..9 + args.len()].copy_from_slice(args);
    let crc = buf[3..89].iter().fold(0u8, |acc, &b| acc ^ b);
    buf[89] = crc;
    buf
}

fn unlock_cmd() -> [u8; RAZER_CMD_LEN] {
    build_razer_cmd(
        TRANSACTION_ID,
        CMD_CLASS_STANDARD,
        CMD_SET_DEVICE_MODE,
        &[MODE_DRIVER, 0x00],
    )
}

/// Built for parity with `prototype.py`'s `RELOCK_CMD` and to keep both
/// commands' construction visibly identical; sent by `relock` below (ticket
/// 23: mode lifecycle, re-lock on shutdown/swap-away-from-analog).
fn relock_cmd() -> [u8; RAZER_CMD_LEN] {
    build_razer_cmd(
        TRANSACTION_ID,
        CMD_CLASS_STANDARD,
        CMD_SET_DEVICE_MODE,
        &[MODE_NORMAL, 0x00],
    )
}

// ---------------------------------------------------------------------------
// Pure functions: Depth -> `EventState` hysteresis and synthesized-repeat
// scheduling (ticket 18 §9). No channels, no tokio, no hardware.
// ---------------------------------------------------------------------------

/// The physical hysteresis state a Grid key's Depth ramp is in — distinct
/// from `EventState`, which is the *transition* `observe` emits only when
/// this state actually changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyState {
    #[default]
    Up,
    Down,
}

/// Depth -> `EventState` hysteresis (CONTEXT.md: Actuation point/Release
/// point; ticket 18 §2/§9): crossing up through `point.actuation` fires a
/// `Down`; crossing down through `point.release` fires an `Up`; dwelling
/// anywhere else — including the hysteresis band between the two points —
/// produces no transition.
pub fn observe(prev: KeyState, depth: u8, point: ActuationPoint) -> (KeyState, Option<EventState>) {
    match prev {
        KeyState::Up if depth >= point.actuation => (KeyState::Down, Some(EventState::Down)),
        KeyState::Down if depth <= point.release => (KeyState::Up, Some(EventState::Up)),
        _ => (prev, None),
    }
}

/// Decides when a held Grid key's synthesized `Repeat` is due, matching the
/// cadence `Device::get_auto_repeat()` reports for the kernel's own
/// autorepeat on the other 8 Inputs (ticket 18 §4) — driver mode silences
/// the kernel's autorepeat for the 20 Grid keys only, so this reproduces it
/// rather than receiving it for free.
#[derive(Debug, Clone, Copy)]
pub struct RepeatSchedule {
    delay_ms: u32,
    period_ms: u32,
}

impl RepeatSchedule {
    pub fn new(delay_ms: u32, period_ms: u32) -> Self {
        // A zero period would never advance past the first repeat; floor at
        // 1ms rather than let a degenerate `AutoRepeat` read wedge the
        // schedule into firing every tick.
        Self {
            delay_ms,
            period_ms: period_ms.max(1),
        }
    }

    /// True once `held_for` has reached the next repeat's due time, given
    /// `fired` synthesized Repeats already emitted for this hold — the Nth
    /// repeat (`fired == N`) is due at `delay_ms + N * period_ms`.
    pub fn repeat_due(&self, held_for: Duration, fired: u32) -> bool {
        let due_at_ms = u128::from(self.delay_ms) + u128::from(fired) * u128::from(self.period_ms);
        held_for.as_millis() >= due_at_ms
    }
}

/// Runtime (non-pure) bookkeeping for one Grid key's currently-held Down —
/// when it started, and how many synthesized Repeats have fired for it so
/// far. Tracked per key by the grid task; not itself unit-tested since it
/// carries no logic beyond `RepeatSchedule::repeat_due`'s own inputs.
#[derive(Debug, Clone, Copy)]
struct HoldState {
    started: Instant,
    fired: u32,
}

/// 0-based report-byte index -> the `Input::Grid` it represents. Byte `n`
/// (1-indexed in the report, `index` here is 0-indexed) is keycap `n`,
/// confirmed per-key out of reading order by ticket 16's `mapping`
/// procedure — the same row-major flattening `daemon/src/input.rs`'s
/// `GRID_KEYS` table uses.
fn grid_input_for_byte(index: usize) -> Input {
    let row = (index / 5) as u8 + 1;
    let col = (index % 5) as u8 + 1;
    Input::Grid(row, col)
}

// ---------------------------------------------------------------------------
// `hidraw` discovery (ticket 18 §2) — walks `/sys/class/hidraw` on every
// (re)open since node numbers aren't stable across boots or reconnects.
// ---------------------------------------------------------------------------

fn discover_hidraw() -> HashMap<u8, PathBuf> {
    discover_hidraw_under(Path::new("/sys/class/hidraw"))
}

fn discover_hidraw_under(sysfs_hidraw: &Path) -> HashMap<u8, PathBuf> {
    let mut found = HashMap::new();
    let Ok(entries) = fs::read_dir(sysfs_hidraw) else {
        return found;
    };
    for entry in entries.flatten() {
        let Some(hidraw_name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // A device that is actively re-enumerating can leave a node
        // half-created or already gone by the time it's inspected — any
        // failure here just means "skip this node," matching
        // `prototype.py`'s `discover()`.
        let Ok(hid_dir) = fs::canonicalize(entry.path().join("device")) else {
            continue;
        };
        let Some(usb_interface) = hid_dir.parent() else {
            continue;
        };
        let Some(usb_device) = usb_interface.parent() else {
            continue;
        };
        if read_attr(&usb_device.join("idVendor")).as_deref() != Some(VENDOR_ID) {
            continue;
        }
        if read_attr(&usb_device.join("idProduct")).as_deref() != Some(PRODUCT_ID) {
            continue;
        }
        let Some(number_hex) = read_attr(&usb_interface.join("bInterfaceNumber")) else {
            continue;
        };
        let Ok(number) = u8::from_str_radix(&number_hex, 16) else {
            continue;
        };
        found.insert(number, Path::new("/dev").join(hidraw_name));
    }
    found
}

fn read_attr(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Absence covers every condition this task retries silently rather than
/// treating as fatal (ticket 18 §7): everything `evdev_source::is_device_absent`
/// already treats as absent (`NotFound`/`ENODEV`), plus the two extra causes
/// specific to `hidraw`: the not-yet-installed udev rule denying access at
/// open/ioctl time (`PermissionDenied`), and a `hidraw` read failing with
/// `EIO` rather than `ENODEV` on unplug.
fn is_grid_absent(err: &io::Error) -> bool {
    const EIO: i32 = 5;
    evdev_source::is_device_absent(err)
        || err.kind() == io::ErrorKind::PermissionDenied
        || err.raw_os_error() == Some(EIO)
}

/// The shared impure primitive: attempts the live kernel-autorepeat read off
/// `Node::If01`, `None` if the node is absent or doesn't report `EV_REP`.
/// `pub(crate)` beyond this module's own grid task — ticket 68's Toggle
/// pacing reads the same live cadence, off the same node, so there is
/// exactly one place the read itself happens. Each caller applies its own
/// domain-appropriate fallback on `None` rather than sharing one (this
/// module's `DEFAULT_REPEAT_DELAY_MS`/`DEFAULT_REPEAT_PERIOD_MS` are tuned
/// for synthesizing a live Depth-driven Repeat stream, not for Toggle's
/// flood-safety floor — the two must not silently drift onto one shared
/// number).
pub(crate) fn read_kernel_auto_repeat() -> Option<evdev::AutoRepeat> {
    Device::open(Node::If01.device_path())
        .ok()
        .and_then(|device| device.get_auto_repeat())
}

fn read_repeat_schedule() -> RepeatSchedule {
    match read_kernel_auto_repeat() {
        Some(evdev::AutoRepeat { delay, period }) => RepeatSchedule::new(delay, period),
        None => RepeatSchedule::new(DEFAULT_REPEAT_DELAY_MS, DEFAULT_REPEAT_PERIOD_MS),
    }
}

fn send_unlock(control: &fs::File) -> io::Result<()> {
    let mut buf = unlock_cmd();
    let ret = unsafe {
        libc::ioctl(
            control.as_raw_fd(),
            hidiocsfeature(RAZER_CMD_LEN as u32) as libc::c_ulong,
            buf.as_mut_ptr(),
        )
    };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn send_relock(control: &fs::File) -> io::Result<()> {
    let mut buf = relock_cmd();
    let ret = unsafe {
        libc::ioctl(
            control.as_raw_fd(),
            hidiocsfeature(RAZER_CMD_LEN as u32) as libc::c_ulong,
            buf.as_mut_ptr(),
        )
    };
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Best-effort, standalone relock: (re)discovers the control interface and
/// sends `MODE_NORMAL` on a freshly opened fd, independent of any running
/// grid task's own connection. Ticket 16 confirmed a relock from a fresh
/// handle that never sent the unlock still succeeds — the same property that
/// makes this safe to call unconditionally, whether or not this process ever
/// actually unlocked the device: main.rs's SIGTERM/SIGINT handler and the
/// supervisor's swap-away-from-analog path (ticket 23) both call this rather
/// than threading a live control-fd handle across task boundaries, so a
/// failure here (no udev access, device unplugged) is just reported, not
/// fatal — there is nothing left to clean up in that case anyway.
pub fn relock() -> io::Result<()> {
    let interfaces = discover_hidraw();
    let control_path = interfaces
        .get(&CONTROL_INTERFACE)
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    let control_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(control_path)?;
    send_relock(&control_file)
}

/// Waits up to `UNLOCK_TIMEOUT` for report `0x06` to arrive on the
/// already-open analog fd (ticket 18 §3). `false` means the fd is treated
/// as poisoned by the caller — closed, unlock only resent on the next full
/// reopen cycle, never on a tight timer.
fn wait_for_unlock_confirmation(analog: &mut fs::File) -> io::Result<bool> {
    let deadline = Instant::now() + UNLOCK_TIMEOUT;
    let fd = analog.as_raw_fd();
    let mut buf = [0u8; 64];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        let timeout_ms = remaining.as_millis().min(i64::from(i32::MAX) as u128) as i32;
        if !poll_readable(fd, timeout_ms)? {
            return Ok(false);
        }
        let n = analog.read(&mut buf)?;
        if n == 0 {
            return Ok(false);
        }
        if buf[0] == ANALOG_REPORT_ID {
            return Ok(true);
        }
        // Some other report ID arrived first (research §3.3 documents a few
        // besides 0x06) — keep waiting out the remaining budget, matching
        // `prototype.py`'s `on_report`, which only treats 0x06 as the
        // standby confirmation.
    }
}

/// The main relay loop once the unlock is confirmed: reads report `0x06`,
/// thresholds each of the 20 depth bytes against the live Actuation-point
/// snapshot, and synthesizes Hold-to-repeat. Returns `Ok(())` once `tx`
/// closes (clean shutdown) or `shutdown` is set (ticket 23, checked every
/// `REPORT_POLL_TIMEOUT` tick); returns `Err` on any read failure,
/// absent-device or genuine alike — the caller tells those apart via
/// `is_grid_absent`. On an `Err` exit, force-releases every Grid key still
/// tracked as held (see the trailing block below) before returning it —
/// deliberately skipped on the two clean-stop paths, matching the existing
/// "tx closed" case, since neither represents a dropped connection the next
/// reopen needs protecting against.
fn relay_grid_blocking(
    analog: &mut fs::File,
    schedule: RepeatSchedule,
    tx: &mpsc::Sender<PhysicalEvent>,
    actuation_rx: &mut watch::Receiver<HashMap<Input, ActuationPoint>>,
    depth_tx: &watch::Sender<HashMap<Input, u8>>,
    shutdown: &AtomicBool,
) -> io::Result<()> {
    let fd = analog.as_raw_fd();
    let mut key_states = [KeyState::Up; NUM_KEYS];
    let mut holds: [Option<HoldState>; NUM_KEYS] = [None; NUM_KEYS];
    let mut last_depth = [0u8; NUM_KEYS];
    let mut buf = [0u8; 64];
    // Cloned only when the watch channel actually changed (`has_changed`),
    // not on every single incoming report — code-review finding: this runs
    // on the hot capture-report path, and a Profile/Actuation-point
    // mutation is comparatively rare.
    let mut snapshot = actuation_rx.borrow_and_update().clone();

    let result: io::Result<()> = 'relay: loop {
        if shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }
        let timeout_ms = REPORT_POLL_TIMEOUT.as_millis() as i32;
        let readable = match poll_readable(fd, timeout_ms) {
            Ok(readable) => readable,
            Err(err) => break 'relay Err(err),
        };
        if readable {
            let n = match analog.read(&mut buf) {
                Ok(n) => n,
                Err(err) => break 'relay Err(err),
            };
            if n == 0 {
                break 'relay Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            if buf[0] == ANALOG_REPORT_ID && n > NUM_KEYS {
                if actuation_rx.has_changed().unwrap_or(false) {
                    snapshot = actuation_rx.borrow_and_update().clone();
                }
                for (i, state) in key_states.iter_mut().enumerate() {
                    let depth = buf[1 + i];
                    last_depth[i] = depth;
                    let input = grid_input_for_byte(i);
                    let point = snapshot.get(&input).copied().unwrap_or_default();
                    let (new_state, transition) = observe(*state, depth, point);
                    *state = new_state;
                    match transition {
                        Some(EventState::Down) => {
                            holds[i] = Some(HoldState {
                                started: Instant::now(),
                                fired: 0,
                            });
                            if tx
                                .blocking_send(PhysicalEvent {
                                    input,
                                    state: EventState::Down,
                                    depth: Some(depth),
                                })
                                .is_err()
                            {
                                return Ok(());
                            }
                        }
                        Some(EventState::Up) => {
                            holds[i] = None;
                            if tx
                                .blocking_send(PhysicalEvent {
                                    input,
                                    state: EventState::Up,
                                    depth: Some(depth),
                                })
                                .is_err()
                            {
                                return Ok(());
                            }
                        }
                        Some(EventState::Repeat) | None => {}
                    }
                }
                // Ticket 26: every 20-key depth sample, not just Down/Up/
                // Repeat transitions — the GUI's live Actuation & release bar
                // needs the raw travel while a key sits between the release
                // and actuation points, where `observe()` reports no
                // transition at all. `watch::Sender::send_replace` is a
                // cheap keep-latest overwrite; the D-Bus layer's own
                // ~30Hz-throttled pump (ticket 26) is what turns this
                // per-report cadence (sub-millisecond while moving, per
                // ticket 13) into the rate-limited `DepthChanged` signal.
                depth_tx.send_replace(
                    (0..NUM_KEYS)
                        .map(|i| (grid_input_for_byte(i), last_depth[i]))
                        .collect(),
                );
            }
        }

        // Repeat scheduling runs every tick regardless of whether this tick
        // read a fresh report, so a device report cadence sparser than
        // `REPORT_POLL_TIMEOUT` still can't starve Hold-to-repeat.
        for (i, hold) in holds.iter_mut().enumerate() {
            if let Some(state) = hold
                && schedule.repeat_due(state.started.elapsed(), state.fired)
            {
                state.fired += 1;
                if tx
                    .blocking_send(PhysicalEvent {
                        input: grid_input_for_byte(i),
                        state: EventState::Repeat,
                        depth: Some(last_depth[i]),
                    })
                    .is_err()
                {
                    return Ok(());
                }
            }
        }
    };

    // Code-review finding: the caller resets every key's hysteresis state
    // to `Up` on the next reopen (a fresh `relay_grid_blocking` call gets a
    // fresh `key_states`/`holds`). Without force-releasing here first, a
    // key still physically held across a transient dropout (unlock-
    // confirmation timeout, `ENODEV`/`EIO` mid-stream) would look like a
    // brand new press on reconnect — a stray, unpaired Down that double-
    // fires FireOnce/HoldToRepeat or silently stops a running Toggle the
    // user never actually released.
    if result.is_err() {
        for (i, hold) in holds.iter().enumerate() {
            if hold.is_some() {
                let _ = tx.blocking_send(PhysicalEvent {
                    input: grid_input_for_byte(i),
                    state: EventState::Up,
                    depth: Some(last_depth[i]),
                });
            }
        }
    }
    result
}

/// One full grid-task lifecycle cycle: discover, open, cache the repeat
/// schedule, unlock, confirm, relay — falling back to the absence-retry
/// bucket at every stage per ticket 18 §2/§3/§7, and only ever returning on
/// clean shutdown, `shutdown` being set (ticket 23), or a genuine
/// (non-absent) error.
fn grid_task_blocking(
    index: usize,
    tx: mpsc::Sender<PhysicalEvent>,
    connection_tx: mpsc::Sender<bool>,
    presence: std::sync::Arc<std::sync::Mutex<Vec<bool>>>,
    mut actuation_rx: watch::Receiver<HashMap<Input, ActuationPoint>>,
    depth_tx: watch::Sender<HashMap<Input, u8>>,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    // Reports absence and waits out one `POLL_INTERVAL` — every absence
    // exit below is this same pair, just reached from a different stage of
    // the lifecycle (discovery, either open, the unlock ioctl, the
    // confirmation wait, or the relay loop itself). `interruptible_sleep`
    // (ticket 23) gives up on the wait early the moment `shutdown` is set,
    // so a swap/process-shutdown mid-retry doesn't have to wait out a full
    // `POLL_INTERVAL`.
    let retry_after_absence = || {
        evdev_source::report_presence(&presence, index, false, &connection_tx);
        interruptible_sleep(evdev_source::POLL_INTERVAL, &shutdown);
    };

    loop {
        if shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }
        let interfaces = discover_hidraw();
        let (Some(analog_path), Some(control_path)) = (
            interfaces.get(&ANALOG_INTERFACE),
            interfaces.get(&CONTROL_INTERFACE),
        ) else {
            retry_after_absence();
            continue;
        };

        let mut analog_file = match fs::OpenOptions::new().read(true).open(analog_path) {
            Ok(file) => file,
            Err(err) if is_grid_absent(&err) => {
                retry_after_absence();
                continue;
            }
            Err(err) => return Err(err),
        };
        let control_file = match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(control_path)
        {
            Ok(file) => file,
            Err(err) if is_grid_absent(&err) => {
                retry_after_absence();
                continue;
            }
            Err(err) => return Err(err),
        };

        // Ticket 18 §4: read while still live in Digital mode, before the
        // unlock below switches the device into the analog stream.
        let schedule = read_repeat_schedule();

        if let Err(err) = send_unlock(&control_file) {
            if is_grid_absent(&err) {
                retry_after_absence();
                continue;
            }
            return Err(err);
        }

        match wait_for_unlock_confirmation(&mut analog_file) {
            Ok(true) => {}
            Ok(false) => {
                // Ticket 18 §3: no confirmation in time — the fd is
                // poisoned. Close it (drop below) and only resend on the
                // *next* full reopen cycle, never on a tight timer.
                retry_after_absence();
                continue;
            }
            Err(err) if is_grid_absent(&err) => {
                retry_after_absence();
                continue;
            }
            Err(err) => return Err(err),
        }

        evdev_source::report_presence(&presence, index, true, &connection_tx);

        match relay_grid_blocking(
            &mut analog_file,
            schedule,
            &tx,
            &mut actuation_rx,
            &depth_tx,
            &shutdown,
        ) {
            Ok(()) => return Ok(()),
            Err(err) if is_grid_absent(&err) => {
                retry_after_absence();
                continue;
            }
            Err(err) => return Err(err),
        }
    }
}

// ---------------------------------------------------------------------------
// The `CaptureSource` itself.
// ---------------------------------------------------------------------------

/// Composes the generalized evdev loop over `[Node::Main, Node::If02]` with
/// one more `JoinSet` task — the `hidraw` grid task — per ticket 18 §1.
pub struct AnalogCaptureSource {
    actuation_rx: watch::Receiver<HashMap<Input, ActuationPoint>>,
    /// Ticket 26: the live per-key depth snapshot this source publishes on
    /// every incoming report — the paired half of the D-Bus layer's own
    /// `depth_rx`, which throttles it down to `DepthChanged`'s ~30Hz.
    depth_tx: watch::Sender<HashMap<Input, u8>>,
    /// Ticket 23: shared with every task this source spawns (the two evdev
    /// nodes and the grid task alike). Setting it makes every task stop
    /// cleanly — relay loop exits, grabs/fds drop — at the next poll tick or
    /// absence-retry check, which the supervisor's live source-swap and
    /// main.rs's SIGTERM/SIGINT shutdown path both depend on: starting the
    /// other `CaptureSource` (or relocking) before this one has actually let
    /// go risks an `EBUSY` on a still-held evdev grab.
    shutdown: Arc<AtomicBool>,
}

impl AnalogCaptureSource {
    /// `actuation_rx` is the paired half of the `watch::Sender` dispatch
    /// publishes into (`dispatch::run`'s `actuation_tx` parameter, ticket 18
    /// §5). `depth_tx` is the paired half of the D-Bus layer's `depth_rx`
    /// (ticket 26). `shutdown` is this attempt's own flag — the supervisor
    /// (ticket 23) hands each `CaptureSource` attempt a fresh one and sets it
    /// to request a clean stop.
    pub fn new(
        actuation_rx: watch::Receiver<HashMap<Input, ActuationPoint>>,
        depth_tx: watch::Sender<HashMap<Input, u8>>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            actuation_rx,
            depth_tx,
            shutdown,
        }
    }
}

/// The three tracked presence slots, in `JoinSet` spawn order: `Main`,
/// `If02`, then the grid task.
const TRACKED_NODES: usize = 3;
const GRID_PRESENCE_INDEX: usize = 2;

impl CaptureSource for AnalogCaptureSource {
    async fn run(
        self,
        tx: mpsc::Sender<PhysicalEvent>,
        connection_tx: mpsc::Sender<bool>,
    ) -> io::Result<()> {
        let presence = evdev_source::presence_for(TRACKED_NODES);
        let mut tasks = JoinSet::new();
        evdev_source::spawn_nodes(
            &mut tasks,
            &[Node::Main, Node::If02],
            0,
            presence.clone(),
            tx.clone(),
            connection_tx.clone(),
            self.shutdown.clone(),
        );
        let actuation_rx = self.actuation_rx;
        let depth_tx = self.depth_tx;
        let grid_shutdown = self.shutdown.clone();
        tasks.spawn_blocking(move || {
            grid_task_blocking(
                GRID_PRESENCE_INDEX,
                tx,
                connection_tx,
                presence,
                actuation_rx,
                depth_tx,
                grid_shutdown,
            )
        });
        evdev_source::join_first(tasks, &self.shutdown).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ActuationPoint, Profile};

    fn point(actuation: u8, release: u8) -> ActuationPoint {
        ActuationPoint { actuation, release }
    }

    // -- `observe`: Depth -> EventState hysteresis (ticket 18 §9) ----------

    #[test]
    fn crossing_up_through_actuation_fires_down() {
        let (state, transition) = observe(KeyState::Up, 130, point(128, 112));
        assert_eq!(state, KeyState::Down);
        assert_eq!(transition, Some(EventState::Down));
    }

    #[test]
    fn exactly_at_actuation_fires_down() {
        let (state, transition) = observe(KeyState::Up, 128, point(128, 112));
        assert_eq!(state, KeyState::Down);
        assert_eq!(transition, Some(EventState::Down));
    }

    #[test]
    fn crossing_down_through_release_fires_up() {
        let (state, transition) = observe(KeyState::Down, 100, point(128, 112));
        assert_eq!(state, KeyState::Up);
        assert_eq!(transition, Some(EventState::Up));
    }

    #[test]
    fn exactly_at_release_fires_up() {
        let (state, transition) = observe(KeyState::Down, 112, point(128, 112));
        assert_eq!(state, KeyState::Up);
        assert_eq!(transition, Some(EventState::Up));
    }

    #[test]
    fn dwelling_in_the_hysteresis_band_while_down_produces_no_transition() {
        // Between release (112) and actuation (128): already Down, depth
        // dips to 120 — not low enough to release.
        let (state, transition) = observe(KeyState::Down, 120, point(128, 112));
        assert_eq!(state, KeyState::Down);
        assert_eq!(transition, None);
    }

    #[test]
    fn dwelling_below_actuation_while_up_produces_no_transition() {
        let (state, transition) = observe(KeyState::Up, 50, point(128, 112));
        assert_eq!(state, KeyState::Up);
        assert_eq!(transition, None);
    }

    #[test]
    fn staying_fully_pressed_while_already_down_produces_no_repeated_down() {
        let (state, transition) = observe(KeyState::Down, 255, point(128, 112));
        assert_eq!(state, KeyState::Down);
        assert_eq!(transition, None);
    }

    #[test]
    fn staying_at_zero_while_already_up_produces_no_repeated_up() {
        let (state, transition) = observe(KeyState::Up, 0, point(128, 112));
        assert_eq!(state, KeyState::Up);
        assert_eq!(transition, None);
    }

    #[test]
    fn a_full_press_release_cycle_round_trips_through_both_transitions() {
        let point = point(128, 112);
        let mut state = KeyState::Up;

        for depth in [0, 60, 127] {
            let (next, transition) = observe(state, depth, point);
            state = next;
            assert_eq!(transition, None);
        }
        let (next, transition) = observe(state, 128, point);
        state = next;
        assert_eq!(transition, Some(EventState::Down));

        for depth in [200, 255, 150, 113] {
            let (next, transition) = observe(state, depth, point);
            state = next;
            assert_eq!(transition, None);
        }
        let (next, transition) = observe(state, 112, point);
        state = next;
        assert_eq!(transition, Some(EventState::Up));
        assert_eq!(state, KeyState::Up);
    }

    // -- `RepeatSchedule` (ticket 18 §4/§9) ---------------------------------

    #[test]
    fn no_repeat_before_the_delay_elapses() {
        let schedule = RepeatSchedule::new(250, 33);
        assert!(!schedule.repeat_due(Duration::from_millis(249), 0));
    }

    #[test]
    fn first_repeat_is_due_exactly_at_the_delay() {
        let schedule = RepeatSchedule::new(250, 33);
        assert!(schedule.repeat_due(Duration::from_millis(250), 0));
    }

    #[test]
    fn second_repeat_is_due_one_period_after_the_first() {
        let schedule = RepeatSchedule::new(250, 33);
        assert!(!schedule.repeat_due(Duration::from_millis(282), 1));
        assert!(schedule.repeat_due(Duration::from_millis(283), 1));
    }

    #[test]
    fn repeated_dwelling_produces_repeats_at_the_right_cadence() {
        // Matches the kernel's own default 250ms delay / 33ms period —
        // walk a synthetic held-for timeline and count how many repeats a
        // stateful loop would have fired by each point, mirroring how the
        // grid task calls this every tick.
        let schedule = RepeatSchedule::new(250, 33);
        let mut fired = 0u32;
        let mut repeats_at = Vec::new();
        for ms in 0..400u64 {
            if schedule.repeat_due(Duration::from_millis(ms), fired) {
                fired += 1;
                repeats_at.push(ms);
            }
        }
        assert_eq!(repeats_at, vec![250, 283, 316, 349, 382]);
    }

    #[test]
    fn a_zero_period_floors_to_one_millisecond_rather_than_never_advancing() {
        let schedule = RepeatSchedule::new(10, 0);
        assert!(schedule.repeat_due(Duration::from_millis(11), 0));
        assert!(!schedule.repeat_due(Duration::from_millis(10), 1));
        assert!(schedule.repeat_due(Duration::from_millis(11), 1));
    }

    // -- byte -> Input mapping (ticket 16: byte n == keycap n) -------------

    #[test]
    fn grid_input_for_byte_matches_input_rs_row_major_layout() {
        assert_eq!(grid_input_for_byte(0), Input::Grid(1, 1));
        assert_eq!(grid_input_for_byte(4), Input::Grid(1, 5));
        assert_eq!(grid_input_for_byte(5), Input::Grid(2, 1));
        assert_eq!(grid_input_for_byte(19), Input::Grid(4, 5));
    }

    // -- protocol bytes: ported, not recomputed (research §2, prototype's
    // own `cmd_selftest`) ----------------------------------------------

    #[test]
    fn hidiocsfeature_91_matches_the_documented_ioctl_number() {
        assert_eq!(hidiocsfeature(91), 0xC05B4806);
    }

    #[test]
    fn unlock_command_matches_the_prototypes_verified_bytes() {
        let cmd = unlock_cmd();
        assert_eq!(cmd.len(), 91);
        assert_eq!(cmd[0], 0x00, "report number");
        assert_eq!(cmd[2], 0x01, "transaction_id");
        assert_eq!(cmd[6], 0x02, "data_size");
        assert_eq!(cmd[7], 0x00, "command_class");
        assert_eq!(cmd[8], 0x04, "command_id");
        assert_eq!(cmd[9], 0x03, "arguments[0] = MODE_DRIVER");
        assert_eq!(cmd[10], 0x00, "arguments[1]");
        assert_eq!(
            cmd[89], 0x05,
            "crc = data_size ^ command_class ^ command_id ^ arguments[0]"
        );
        assert_eq!(cmd[90], 0x00, "trailing byte unused");
    }

    #[test]
    fn relock_command_only_differs_from_unlock_in_its_mode_argument_and_crc() {
        let unlock = unlock_cmd();
        let relock = relock_cmd();
        assert_eq!(relock[9], 0x00, "arguments[0] = MODE_NORMAL");
        assert_eq!(
            relock[89], 0x06,
            "crc = data_size ^ command_class ^ command_id"
        );
        for i in 0..91 {
            if i == 9 || i == 89 {
                continue;
            }
            assert_eq!(
                unlock[i], relock[i],
                "byte {i} must match between unlock/relock"
            );
        }
    }

    #[test]
    fn crc_excludes_the_transaction_id() {
        let a = build_razer_cmd(0x01, 0x00, 0x04, &[0x03, 0x00]);
        let b = build_razer_cmd(0xFF, 0x00, 0x04, &[0x03, 0x00]);
        assert_eq!(a[89], b[89]);
        assert_ne!(a[2], b[2]);
    }

    #[test]
    fn crc_range_includes_the_far_end_of_the_buffer() {
        // With 80 arguments the last one lands on buffer index 88 — the
        // final byte inside the documented CRC range (3..=88) — so this
        // pins the range's far end the way the prototype's own selftest
        // does.
        let mut args = vec![0u8; 79];
        args.push(0x01);
        let cmd = build_razer_cmd(0x01, 0x00, 0x04, &args);
        assert_eq!(cmd[89], 0x50 ^ 0x04 ^ 0x01);
    }

    // -- `is_grid_absent` ----------------------------------------------

    #[test]
    fn not_found_and_permission_denied_are_absent_not_fatal() {
        assert!(is_grid_absent(&io::Error::from(io::ErrorKind::NotFound)));
        assert!(is_grid_absent(&io::Error::from(
            io::ErrorKind::PermissionDenied
        )));
    }

    #[test]
    fn enodev_and_eio_are_absent_not_fatal() {
        assert!(is_grid_absent(&io::Error::from_raw_os_error(19)));
        assert!(is_grid_absent(&io::Error::from_raw_os_error(5)));
    }

    #[test]
    fn an_unrelated_error_is_not_treated_as_absent() {
        assert!(!is_grid_absent(&io::Error::from_raw_os_error(22))); // EINVAL
    }

    // -- discovery: pure once the sysfs root is a parameter -----------

    #[test]
    fn discovery_returns_empty_when_the_sysfs_root_does_not_exist() {
        let found = discover_hidraw_under(Path::new("/nonexistent/acheron-analog-test/hidraw"));
        assert!(found.is_empty());
    }

    #[test]
    fn discovery_finds_both_interfaces_of_a_synthetic_matching_device() {
        let dir = tempfile::tempdir().unwrap();
        let hidraw_root = dir.path().join("hidraw");
        fs::create_dir_all(&hidraw_root).unwrap();

        for (hidraw_name, interface_number) in [("hidraw1", "01"), ("hidraw2", "02")] {
            let usb_device = dir.path().join("usbdev");
            fs::create_dir_all(&usb_device).unwrap();
            fs::write(usb_device.join("idVendor"), format!("{VENDOR_ID}\n")).unwrap();
            fs::write(usb_device.join("idProduct"), format!("{PRODUCT_ID}\n")).unwrap();

            let usb_interface = usb_device.join(format!("if{interface_number}"));
            fs::create_dir_all(&usb_interface).unwrap();
            fs::write(
                usb_interface.join("bInterfaceNumber"),
                format!("{interface_number}\n"),
            )
            .unwrap();

            let hid_dir = usb_interface.join("hid_device");
            fs::create_dir_all(&hid_dir).unwrap();

            let node_dir = hidraw_root.join(hidraw_name);
            fs::create_dir_all(&node_dir).unwrap();
            std::os::unix::fs::symlink(&hid_dir, node_dir.join("device")).unwrap();
        }

        let found = discover_hidraw_under(&hidraw_root);

        assert_eq!(found.len(), 2);
        assert!(found[&ANALOG_INTERFACE].ends_with("hidraw1"));
        assert!(found[&CONTROL_INTERFACE].ends_with("hidraw2"));
    }

    #[test]
    fn discovery_ignores_a_device_with_the_wrong_vendor_id() {
        let dir = tempfile::tempdir().unwrap();
        let hidraw_root = dir.path().join("hidraw");
        fs::create_dir_all(&hidraw_root).unwrap();

        let usb_device = dir.path().join("usbdev");
        fs::create_dir_all(&usb_device).unwrap();
        fs::write(usb_device.join("idVendor"), "9999\n").unwrap();
        fs::write(usb_device.join("idProduct"), format!("{PRODUCT_ID}\n")).unwrap();

        let usb_interface = usb_device.join("if01");
        fs::create_dir_all(&usb_interface).unwrap();
        fs::write(usb_interface.join("bInterfaceNumber"), "01\n").unwrap();

        let hid_dir = usb_interface.join("hid_device");
        fs::create_dir_all(&hid_dir).unwrap();

        let node_dir = hidraw_root.join("hidraw0");
        fs::create_dir_all(&node_dir).unwrap();
        std::os::unix::fs::symlink(&hid_dir, node_dir.join("device")).unwrap();

        assert!(discover_hidraw_under(&hidraw_root).is_empty());
    }

    // -- `relay_grid_blocking`: force-release on a dropped connection
    // (code-review finding on ticket 22) -----------------------------

    #[test]
    fn a_dropped_connection_force_releases_every_key_still_tracked_as_held() {
        use std::io::Write;
        use std::os::fd::FromRawFd;

        // A real pipe stands in for the hidraw fd — `relay_grid_blocking`
        // only needs something pollable/readable, and a pipe gives full,
        // hardware-free control over exactly when data or EOF arrives.
        let mut fds = [0i32; 2];
        assert_eq!(
            unsafe { libc::pipe(fds.as_mut_ptr()) },
            0,
            "pipe(2) must succeed"
        );
        let [read_fd, write_fd] = fds;
        let mut analog = unsafe { fs::File::from_raw_fd(read_fd) };
        let mut write_end = unsafe { fs::File::from_raw_fd(write_fd) };

        // Report byte 1 (keycap 1 / `Input::Grid(1, 1)`) pressed well past
        // the default 128 Actuation point; every other key at rest.
        let mut report = [0u8; 24];
        report[0] = ANALOG_REPORT_ID;
        report[1] = 200;
        write_end.write_all(&report).unwrap();

        let (tx, mut rx) = mpsc::channel(16);
        let (actuation_tx, mut actuation_rx) =
            watch::channel(Profile::default().resolved_actuation_points());
        let (depth_tx, _depth_rx) = watch::channel(HashMap::new());
        // A delay comfortably past this test's critical section (EOF must
        // be detected within one `REPORT_POLL_TIMEOUT` tick, ~8ms) but
        // still short enough to keep the suite fast — this test isn't
        // about repeat timing, just the force-release-on-error path.
        let schedule = RepeatSchedule::new(5_000, 1_000);

        let shutdown = AtomicBool::new(false);
        let handle = std::thread::spawn(move || {
            relay_grid_blocking(
                &mut analog,
                schedule,
                &tx,
                &mut actuation_rx,
                &depth_tx,
                &shutdown,
            )
        });

        let down = rx.blocking_recv().expect("the Down this report produces");
        assert_eq!(down.input, Input::Grid(1, 1));
        assert_eq!(down.state, EventState::Down);

        // Closing the write end makes the next `read()` on `analog` return
        // 0 bytes (EOF) — `relay_grid_blocking` treats that as
        // `UnexpectedEof` and must force-release every held key before
        // propagating it.
        drop(write_end);

        let up = rx
            .blocking_recv()
            .expect("the still-held key must be force-released before the error propagates");
        assert_eq!(up.input, Input::Grid(1, 1));
        assert_eq!(up.state, EventState::Up);

        let result = handle.join().unwrap();
        assert!(result.is_err());
        drop(actuation_tx);
    }
}
