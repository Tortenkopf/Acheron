// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

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
use crate::config::{
    ActuationPoint, BreathStyle, Colour, FixedEffect, LightingAssignment, LightingState,
    StatusLeds, WaveDirection,
};
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

/// The Status-LED write (`.scratch/tartarus-status-leds/spec.md` §"The wire
/// frame"; ADR-0006) — a standard Razer extended-matrix static-effect
/// command aimed at the dedicated side-stripe LED id, verified byte-for-byte
/// on hardware by charting ticket 01 and cross-checked by
/// `prototype/01-status-leds/prototype.py`'s `selftest`. Do not re-derive.
/// `transaction_id 0x1F` is the Tartarus-Pro-specific id (as the lighting
/// frame uses — *not* the unlock's `0x01`); `command_class 0x0F`,
/// `command_id 0x02` (write). Independent of Capture mode: the `0x0F/0x02`
/// frame works with `device_mode = 00 00`, so this never sends a driver-mode
/// command (the normal→driver transition is the reset risk this effort
/// avoids).
const STATUS_LED_TXN: u8 = 0x1F;
const STATUS_LED_CMD_CLASS: u8 = 0x0F;
const STATUS_LED_CMD_ID: u8 = 0x02;
/// LED id `SIDE_STRIPE_LED`.
const STATUS_LED_ID: u8 = 0x0B;
/// Effect id: static. Never `effect_none` (`0x00`) — it ACKs but does
/// nothing to these fixed-colour LEDs (charting ticket 01).
const STATUS_LED_EFFECT_STATIC: u8 = 0x01;
/// One fixed-colour LED per channel byte: `0xFF` on, `0x00` off.
const STATUS_LED_ON: u8 = 0xFF;
const STATUS_LED_OFF: u8 = 0x00;

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

// ---------------------------------------------------------------------------
// Device-info reads (firmware version + serial number) — ticket 100/101.
// Standard Razer "get" commands on the same Interface-2 control channel and
// the same 90-byte frame/CRC as the unlock; only `command_id`, `data_size`
// and the readback differ. Full derivation:
// `.scratch/tartarus-input-expansion/research/tartarus-pro-device-info-protocol.md`.
// ---------------------------------------------------------------------------

/// OpenRazer's `razer_attr_read_firmware_version` / `_device_serial` both
/// hardcode `transaction_id 0xFF` with no per-device switch, and `0xFF` is
/// the value already confirmed reading `v1.2` / `PM2443F36300141` off our
/// own unit via OpenRazer's sysfs (research §4). `0x1F` — the
/// Tartarus-Pro-specific id used for `set_device_mode` and lighting — is the
/// fallback, tried only if `0xFF`'s response fails validation. Not the
/// unlock's `0x01`: no evidence it applies to standard get commands.
const DEVICE_INFO_TXN_PRIMARY: u8 = 0xFF;
const DEVICE_INFO_TXN_FALLBACK: u8 = 0x1F;

const CMD_GET_FIRMWARE: u8 = 0x81;
const CMD_GET_SERIAL: u8 = 0x82;
const FIRMWARE_DATA_SIZE: usize = 2;
const SERIAL_DATA_SIZE: usize = 22;

/// OpenRazer waits 600–800µs in-kernel between the `SET_REPORT` and the
/// `GET_REPORT` (`RAZER_BLACKWIDOW_CHROMA_WAIT_*`). From userspace, research
/// §5 recommends ≥1ms plus a couple of backed-off retries in case the first
/// GET right after connect is early — these are the per-attempt sleeps.
const DEVICE_INFO_SETGET_DELAYS_MS: [u64; 3] = [1, 3, 10];

/// Response-struct offsets inside the 91-byte buffer `HIDIOCGFEATURE` fills
/// (research §3.3): byte 0 stays the report number we wrote and the 90-byte
/// `razer_report` lands at indices 1..91, so `status` is index 1, the
/// class/id echo is 7/8, and `arguments` start at index 9 — symmetric with
/// the SET buffer `build_razer_cmd` produces.
const RESP_STATUS: usize = 1;
const RESP_CMD_CLASS: usize = 7;
const RESP_CMD_ID: usize = 8;
const RESP_ARGS: usize = 9;

/// Linux's `_IOC(dir, type, nr, size)` (`asm-generic/ioctl.h`), specialized
/// to the HID feature-report ioctls exactly as `prototype.py`'s
/// `hidiocsfeature` computes it. `nr` `0x06` is `HIDIOCSFEATURE` (write the
/// request), `0x07` is `HIDIOCGFEATURE` (read the response back) — the two
/// differ only in that nibble (research `tartarus-pro-device-info-protocol.md`
/// §3.2). Checked against the documented `0xC05B4806` / `0xC05B4807` for
/// `size = 91` by the two `hidioc*feature_91_matches_the_documented_ioctl_number`
/// tests below.
const fn hidioc_feature(nr: u64, size: u32) -> u64 {
    const IOC_WRITE: u64 = 1;
    const IOC_READ: u64 = 2;
    ((IOC_WRITE | IOC_READ) << 30) | ((size as u64) << 16) | (b'H' as u64) << 8 | nr
}

const fn hidiocsfeature(size: u32) -> u64 {
    hidioc_feature(0x06, size)
}

const fn hidiocgfeature(size: u32) -> u64 {
    hidioc_feature(0x07, size)
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

    /// The offset from the start of a hold at which repeat number `fired`
    /// falls due — `delay_ms + fired * period_ms`, the same due time
    /// `repeat_due` checks against. A self-driven autorepeat emitter (the
    /// Toggle sustained-hold loop, spec-kernel-shaped-repeat.md §5.2) sleeps
    /// until `started + due_offset(fired)` before consulting `repeat_due` /
    /// `advance_fired`; `capture::analog`'s own grid loop doesn't need it
    /// because it is already woken by the incoming report stream.
    pub fn due_offset(&self, fired: u32) -> Duration {
        Duration::from_millis(
            u64::from(self.delay_ms) + u64::from(fired) * u64::from(self.period_ms),
        )
    }

    /// The `fired` count to advance to after emitting one Repeat at
    /// `held_for` — normally `fired + 1`, but when the loop stalled long
    /// enough that several repeats' due times slipped past (a full
    /// `event_rx` back-pressuring `blocking_send`, or a scheduler hiccup),
    /// it jumps straight to the count real elapsed time calls for, so the
    /// caller emits exactly one Repeat now and the next falls due a full
    /// `period_ms` later — not a sub-millisecond catch-up burst of the
    /// missed ones. Mirrors the kernel's `input_repeat_key`, which re-arms
    /// its timer from the current instant and never bursts (ticket 06).
    pub fn advance_fired(&self, held_for: Duration, fired: u32) -> u32 {
        let held_ms = held_for.as_millis();
        let delay_ms = u128::from(self.delay_ms);
        let elapsed_repeats = held_ms
            .checked_sub(delay_ms)
            .map(|since_delay| since_delay / u128::from(self.period_ms) + 1)
            .unwrap_or(0);
        let caught_up = u32::try_from(elapsed_repeats).unwrap_or(u32::MAX);
        caught_up.max(fired + 1)
    }

    /// The same live envelope with its `delay_ms` warm-up collapsed to a
    /// single `period_ms` — what Analog-repeat's hold-solid phase runs at
    /// (`analog_repeat::run_analog_repeat_loop`, spec-kernel-shaped-repeat.md
    /// §5.3): no `REP_DELAY` gap before the first `value=2` (the top of a
    /// hand-driven tapping ramp, not a fresh press), just the steady
    /// `period_ms` cadence from the outset. `due_offset` / `repeat_due` /
    /// `advance_fired` then apply unchanged — the missed-deadline clamp (§5.4)
    /// included — so this needs no siblings of its own.
    pub fn without_warmup(&self) -> RepeatSchedule {
        RepeatSchedule {
            delay_ms: self.period_ms,
            period_ms: self.period_ms,
        }
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

/// Resolves the live kernel-autorepeat envelope once at Daemon startup for
/// the self-driven `value=2` emitters — the single-key Toggle hold
/// (spec-kernel-shaped-repeat.md §5.2) and Analog-repeat's hold-solid phase
/// (§5.3): a `spawn_blocking` wrapper around `read_repeat_schedule` so the device
/// open/ioctl never runs on an async task's own thread — the exact
/// discipline `executor::resolve_toggle_lap_target` follows for `target_lap`.
/// An inline per-Toggle-press blocking read breaks the `tokio::time::pause()`
/// test harness (ticket 68's finding), so the resolved value is threaded
/// down as a plain `RepeatSchedule` instead. A `spawn_blocking` panic (never
/// observed, only theoretically possible) falls back to the kernel-default
/// pair, same as a failed device read.
pub async fn resolve_toggle_autorepeat_schedule() -> RepeatSchedule {
    tokio::task::spawn_blocking(read_repeat_schedule)
        .await
        .unwrap_or_else(|_| RepeatSchedule::new(DEFAULT_REPEAT_DELAY_MS, DEFAULT_REPEAT_PERIOD_MS))
}

/// (Re)discover the Interface-2 control node and open it read+write on a
/// fresh fd — the shared prelude of every standalone control-channel
/// operation (`relock()`, `read_device_info()`, the Status-LED writes).
/// Device absent ⇒ `Err(io::ErrorKind::NotFound)`. The grid task
/// (`grid_task_blocking`) opens its own inline because it needs the analog
/// node alongside and folds the failure into its absence-retry bucket.
fn open_control_fd() -> io::Result<fs::File> {
    let interfaces = discover_hidraw();
    let control_path = interfaces
        .get(&CONTROL_INTERFACE)
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(control_path)
}

/// Send one already-built 91-byte command frame as a `HIDIOCSFEATURE`
/// `SET_REPORT` on an open control fd — the write half shared by
/// `send_unlock` / `send_relock` / the Status-LED frame. (`feature_exchange`
/// does its own SET+GET pair with retries and doesn't route through here.)
fn send_feature(control: &fs::File, buf: &mut [u8; RAZER_CMD_LEN]) -> io::Result<()> {
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

fn send_unlock(control: &fs::File) -> io::Result<()> {
    send_feature(control, &mut unlock_cmd())
}

fn send_relock(control: &fs::File) -> io::Result<()> {
    send_feature(control, &mut relock_cmd())
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
    send_relock(&open_control_fd()?)
}

// ---------------------------------------------------------------------------
// Status-LED writes (`.scratch/tartarus-status-leds/`, ADR-0006). Siblings of
// `relock()` / `read_device_info()`: one `HIDIOCSFEATURE` on a freshly-opened,
// immediately-closed Interface-2 control fd. No read-back, no retry loop, no
// driver-mode call, no unlock. `build_razer_cmd` / `discover_hidraw` unchanged.
// ---------------------------------------------------------------------------

/// The nine argument bytes of the Status-LED static-effect frame:
/// `[storage, LED id, effect, arg3, arg4, colour count, r, g, b]`
/// (`spec.md` §"The wire frame"). `arg0` (storage) is inert on this
/// firmware — `0x00` for intent-clarity; `arg3`/`arg4` are unused for the
/// static effect; the colour count is `1`.
fn status_led_args(r: u8, g: u8, b: u8) -> [u8; 9] {
    [
        0x00,
        STATUS_LED_ID,
        STATUS_LED_EFFECT_STATIC,
        0x00,
        0x00,
        0x01,
        r,
        g,
        b,
    ]
}

/// Open a fresh Interface-2 control fd, send one Status-LED frame via
/// `HIDIOCSFEATURE`, drop the fd. Device absent ⇒ `Err(io::ErrorKind::NotFound)`,
/// exactly like `relock()`.
fn send_status_leds(r: u8, g: u8, b: u8) -> io::Result<()> {
    send_feature(
        &open_control_fd()?,
        &mut build_razer_cmd(
            STATUS_LED_TXN,
            STATUS_LED_CMD_CLASS,
            STATUS_LED_CMD_ID,
            &status_led_args(r, g, b),
        ),
    )
}

/// Physically drive the three side Status LEDs to `leds` (CONTEXT.md: Status
/// LED assignment). Orange←`r`, green←`g`, blue←`b`; each channel `0xFF` on
/// / `0x00` off. Called by the `led` task (`crate::led`) on a `spawn_blocking`
/// thread whenever dispatch pushes a new triple — Profile switch, device
/// (re)connect, Daemon startup. Works identically in Analog and Digital
/// Capture mode: it opens its own short-lived Interface-2 fd regardless of
/// what capture is doing, and never sends a driver-mode command.
pub fn assert_status_leds(leds: StatusLeds) -> io::Result<()> {
    let channel = |on| if on { STATUS_LED_ON } else { STATUS_LED_OFF };
    send_status_leds(
        channel(leds.orange),
        channel(leds.green),
        channel(leds.blue),
    )
}

/// Clear all three Status LEDs — the same frame with every channel byte
/// `0x00`. Sent on a clean Daemon exit (`main.rs::relock_and_exit`, before
/// `relock()`) so a stopped Acheron leaves no stale indicator lit. All-off
/// `(0, 0, 0)` is hardware-reachable (charting ticket 01, criterion 5).
pub fn clear_status_leds() -> io::Result<()> {
    send_status_leds(STATUS_LED_OFF, STATUS_LED_OFF, STATUS_LED_OFF)
}

// ---------------------------------------------------------------------------
// Lighting writes (`tartarus-backlight`, ADR-0012). Siblings of the
// Status-LED writes above: one freshly-opened, immediately-closed
// Interface-2 control fd per assert, no read-back, no retry loop, no
// driver-mode call, no unlock. Byte layout is settled by
// `research/backlight-wire-protocol.md` and verified on hardware (charting
// ticket 02) — not re-derived here. Unlike Status LEDs there is no
// shutdown-clear equivalent (ADR-0012: every frame below carries VARSTORE,
// so the firmware persists the asserted effect device-side).
// ---------------------------------------------------------------------------

/// The storage-mode byte every Lighting effect-select frame carries — unlike
/// the Status-LED frame's inert storage byte, this one actually matters
/// (VARSTORE = firmware-persisted). The custom-frame write and the
/// custom-effect arm frame both hardcode their equivalent byte to `0x00`
/// instead (spec.md §"Custom layout").
const VARSTORE: u8 = 0x01;
/// The backlight's dedicated LED id, distinct from the Status LEDs'
/// `STATUS_LED_ID` (`SIDE_STRIPE_LED`).
const BACKLIGHT_LED: u8 = 0x05;
/// Brightness targets this LED id instead of `BACKLIGHT_LED` — a
/// Tartarus-Pro/V2-specific quirk (spec.md §"Brightness").
const ZERO_LED: u8 = 0x00;
/// Every Lighting effect-select frame uses `transaction_id 0x1F`, including
/// Breath — the driver source's own frame actually transmits `0x3F` for
/// Breath (a copy/paste bug, spec.md §"The wire frames" / research §7), but
/// ticket 02 confirmed `0x1F` works on hardware too, so the spec uses it
/// uniformly for consistency with every other effect.
const LIGHTING_TXN: u8 = 0x1F;
const LIGHTING_CMD_CLASS: u8 = 0x0F;
const LIGHTING_EFFECT_CMD_ID: u8 = 0x02;
const LIGHTING_CUSTOM_FRAME_CMD_ID: u8 = 0x03;
const LIGHTING_BRIGHTNESS_CMD_ID: u8 = 0x04;

const EFFECT_NONE: u8 = 0x00;
const EFFECT_STATIC: u8 = 0x01;
const EFFECT_BREATH: u8 = 0x02;
const EFFECT_SPECTRUM: u8 = 0x03;
const EFFECT_WAVE: u8 = 0x04;
const EFFECT_REACTIVE: u8 = 0x05;
const EFFECT_STARLIGHT: u8 = 0x07;
const EFFECT_CUSTOM: u8 = 0x08;

const WAVE_RIGHT: u8 = 0x01;
const WAVE_LEFT: u8 = 0x02;
/// The fixed wave speed byte every Tartarus Pro wave frame carries (lower =
/// faster per the driver's own comment) — not user-configurable.
const WAVE_SPEED: u8 = 0x28;

/// The Pro's `1×21` matrix: 20 grid keys + the scroll wheel (spec.md
/// §"Column addressing" — `LightingAssignment::CustomLayout`'s `colours`
/// array is already in this column order; `assert_lighting` never remaps
/// it). `stop_col` for a full-row write is `MATRIX_COLUMNS - 1`.
const MATRIX_COLUMNS: usize = 21;
/// `matrix_custom_frame`'s `data_size` is a fixed `0x47` (71) regardless of
/// actual row length — 5 header bytes plus up to 63 bytes of RGB data,
/// zero-padded (spec.md §"Custom layout" / research §10.1).
const CUSTOM_FRAME_DATA_SIZE: usize = 71;
const CUSTOM_EFFECT_ARM_DATA_SIZE: usize = 12;

fn wave_direction_byte(direction: WaveDirection) -> u8 {
    match direction {
        WaveDirection::Right => WAVE_RIGHT,
        WaveDirection::Left => WAVE_LEFT,
    }
}

/// The style-count byte (0/1/2) plus 0/1/2 colours, shared by Breath's and
/// Starlight's tail (spec.md §"The wire frames" / research §7-§8) — the only
/// piece the two effects actually share; `a3`/`a4` differ per caller
/// (`breath_tail`/`starlight_tail` below), so those stay separate.
fn style_count_and_colours(style: &BreathStyle) -> (u8, Vec<u8>) {
    match style {
        BreathStyle::Random => (0x00, vec![]),
        BreathStyle::Single { colour } => (0x01, vec![colour.r, colour.g, colour.b]),
        BreathStyle::Dual { first, second } => (
            0x02,
            vec![first.r, first.g, first.b, second.r, second.g, second.b],
        ),
    }
}

/// Breath's `arg3`/`arg4`/`arg5` tail — `a3` repeats the style count, `a4` is
/// always `0x00`.
fn breath_tail(style: &BreathStyle) -> Vec<u8> {
    let (count, colours) = style_count_and_colours(style);
    let mut tail = vec![count, 0x00, count];
    tail.extend(colours);
    tail
}

/// Starlight's tail — `a3` is always `0x00`, `a4` carries the speed byte.
fn starlight_tail(style: &BreathStyle, speed: u8) -> Vec<u8> {
    let (count, colours) = style_count_and_colours(style);
    let mut tail = vec![0x00, speed, count];
    tail.extend(colours);
    tail
}

/// `effect id` + argument tail for one `FixedEffect`, per spec.md §"The wire
/// frames" table.
fn fixed_effect_id_and_tail(effect: &FixedEffect) -> (u8, Vec<u8>) {
    match effect {
        FixedEffect::Static { colour } => (
            EFFECT_STATIC,
            vec![0x00, 0x00, 0x01, colour.r, colour.g, colour.b],
        ),
        FixedEffect::Spectrum => (EFFECT_SPECTRUM, vec![0x00, 0x00, 0x00]),
        FixedEffect::Wave { direction } => (
            EFFECT_WAVE,
            vec![wave_direction_byte(*direction), WAVE_SPEED, 0x00],
        ),
        FixedEffect::Reactive { colour, speed } => (
            EFFECT_REACTIVE,
            vec![0x00, *speed, 0x01, colour.r, colour.g, colour.b],
        ),
        FixedEffect::Breath { style } => (EFFECT_BREATH, breath_tail(style)),
        FixedEffect::Starlight { style, speed } => {
            (EFFECT_STARLIGHT, starlight_tail(style, *speed))
        }
    }
}

/// The full argument list for one effect-select frame (`command_id 0x02`):
/// `[VARSTORE, BACKLIGHT_LED, effect id, ...tail]`.
fn effect_args(effect_id: u8, tail: &[u8]) -> Vec<u8> {
    let mut args = vec![VARSTORE, BACKLIGHT_LED, effect_id];
    args.extend_from_slice(tail);
    args
}

/// `matrix_custom_frame`'s fixed 71-byte argument list: `[0x00, 0x00,
/// row_index, start_col, stop_col, RGB×21, ...zero padding]` (spec.md
/// §"Custom layout" / research §10.1). `a0`/`a1` are left zero — this
/// command carries no `variable_storage`/`led_id` concept.
fn custom_frame_args(colours: &[Colour; MATRIX_COLUMNS]) -> [u8; CUSTOM_FRAME_DATA_SIZE] {
    let mut args = [0u8; CUSTOM_FRAME_DATA_SIZE];
    args[2] = 0x00; // row_index — the Pro's matrix has only row 0
    args[3] = 0x00; // start_col
    args[4] = (MATRIX_COLUMNS - 1) as u8; // stop_col
    for (i, colour) in colours.iter().enumerate() {
        let offset = 5 + i * 3;
        args[offset] = colour.r;
        args[offset + 1] = colour.g;
        args[offset + 2] = colour.b;
    }
    args
}

/// `matrix_effect_custom`'s argument list — the generic "display whatever's
/// staged" trigger. `variable_storage`/`led_id` are hardcoded `0x00` here,
/// not `VARSTORE`/`BACKLIGHT_LED` (spec.md §"Custom layout" / research
/// §10.2).
fn custom_effect_arm_args() -> [u8; CUSTOM_EFFECT_ARM_DATA_SIZE] {
    let mut args = [0u8; CUSTOM_EFFECT_ARM_DATA_SIZE];
    args[2] = EFFECT_CUSTOM;
    args
}

fn brightness_args(brightness: u8) -> [u8; 3] {
    [VARSTORE, ZERO_LED, brightness]
}

/// One `HIDIOCSFEATURE` write on `control` carrying a Lighting frame — every
/// Lighting command shares `LIGHTING_TXN`/`LIGHTING_CMD_CLASS`, so callers
/// only ever vary `cmd_id`/`args`. Shared by every arm of `assert_lighting`
/// so a future change to the shared envelope (e.g. the Breath
/// `transaction_id` quirk noted on `LIGHTING_TXN`) can't be applied to only
/// some call sites by accident.
fn send_lighting_cmd(control: &fs::File, cmd_id: u8, args: &[u8]) -> io::Result<()> {
    send_feature(
        control,
        &mut build_razer_cmd(LIGHTING_TXN, LIGHTING_CMD_CLASS, cmd_id, args),
    )
}

/// Physically drive the backlight to `state` (CONTEXT.md: Lighting
/// assignment). Discovers a fresh Interface-2 control fd, sends one
/// effect-select write (`Off`/`FixedEffect`) or the two-step
/// custom-frame-write-then-arm (`CustomLayout`) for `state.assignment`, then
/// sends a brightness write for `state.brightness` — every variant above
/// ends with one, so the brightness byte always accompanies whichever
/// assignment was asserted — all on the same short-lived fd, then drops it.
/// No read-back, no retry loop, no driver-mode call, no unlock — like every
/// other write in this file, a failed step aborts the whole assert rather
/// than skipping ahead to the next one. Device absent ⇒
/// `Err(io::ErrorKind::NotFound)`, exactly like `relock()`/
/// `assert_status_leds`. Called by the `led` task (`crate::led`) on a
/// `spawn_blocking` thread whenever dispatch pushes a new `LightingState` —
/// Daemon startup and every device (re)connect (Profile switch and
/// `SetLighting` follow in a later ticket). Works identically in Analog and
/// Digital Capture mode: this opens its own Interface-2 fd regardless of
/// what capture is doing, and never sends a driver-mode command.
pub fn assert_lighting(state: LightingState) -> io::Result<()> {
    let control = open_control_fd()?;
    match &state.assignment {
        LightingAssignment::Off => {
            send_lighting_cmd(
                &control,
                LIGHTING_EFFECT_CMD_ID,
                &effect_args(EFFECT_NONE, &[0x00, 0x00, 0x00]),
            )?;
        }
        LightingAssignment::FixedEffect { effect } => {
            let (effect_id, tail) = fixed_effect_id_and_tail(effect);
            send_lighting_cmd(
                &control,
                LIGHTING_EFFECT_CMD_ID,
                &effect_args(effect_id, &tail),
            )?;
        }
        LightingAssignment::CustomLayout { colours } => {
            send_lighting_cmd(
                &control,
                LIGHTING_CUSTOM_FRAME_CMD_ID,
                &custom_frame_args(colours),
            )?;
            send_lighting_cmd(&control, LIGHTING_EFFECT_CMD_ID, &custom_effect_arm_args())?;
        }
    }
    send_lighting_cmd(
        &control,
        LIGHTING_BRIGHTNESS_CMD_ID,
        &brightness_args(state.brightness),
    )
}

// ---------------------------------------------------------------------------
// Device-info read — firmware version + serial number (ticket 100/101).
//
// Pure buffer/response handling (`build_razer_cmd` above, `response_echoes`/
// `parse_firmware`/`parse_serial` below) is kept separate from the I/O
// (`feature_exchange`/`read_device_info`) and unit-tested on its own, the
// same discipline tickets 22/18 used for the capture logic.
// ---------------------------------------------------------------------------

/// The Tartarus Pro's firmware version (`vX.Y`) and serial number, read once
/// per connection over the Interface-2 control channel. Surfaced as two
/// *optional* `GetState()` keys for the About dialog (ticket 102) — present
/// when known, absent when the device is disconnected or the read failed,
/// mirroring `device_connected`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub firmware_version: String,
    pub serial_number: String,
}

/// True when a `HIDIOCGFEATURE` response buffer is the reply to our own
/// get-`command_id` request: the class/id echo matches and the `status`
/// byte is one OpenRazer tolerates (research §5 — `0x00` unset, `0x01`
/// BUSY, `0x02` SUCCESS; FAILURE `0x03` / TIMEOUT `0x04` / NOT_SUPPORTED
/// `0x05` are rejected). No response-CRC check — OpenRazer's own keyboard
/// path doesn't do one either.
fn response_echoes(resp: &[u8], command_id: u8) -> bool {
    resp.len() > RESP_ARGS
        && resp[RESP_CMD_CLASS] == CMD_CLASS_STANDARD
        && resp[RESP_CMD_ID] == command_id
        && matches!(resp[RESP_STATUS], 0x00..=0x02)
}

/// `arguments[0]` major, `arguments[1]` minor, each a plain decimal byte →
/// `vX.Y` (research §3.4, OpenRazer `razer_attr_read_firmware_version`).
/// `None` if the buffer isn't a valid firmware reply.
fn parse_firmware(resp: &[u8]) -> Option<String> {
    if !response_echoes(resp, CMD_GET_FIRMWARE) || resp.len() < RESP_ARGS + FIRMWARE_DATA_SIZE {
        return None;
    }
    Some(format!("v{}.{}", resp[RESP_ARGS], resp[RESP_ARGS + 1]))
}

/// 22 bytes of ASCII from `arguments[0..22]`, trimmed at the first NUL then
/// of trailing whitespace — OpenRazer copies a fixed 22 bytes and the
/// padding is undocumented, so trim defensively (research §3.4 / §8.4). A
/// non-ASCII or empty result is treated as a failed read (mirrors
/// `device_connected` going absent).
fn parse_serial(resp: &[u8]) -> Option<String> {
    if !response_echoes(resp, CMD_GET_SERIAL) || resp.len() < RESP_ARGS + SERIAL_DATA_SIZE {
        return None;
    }
    let raw = &resp[RESP_ARGS..RESP_ARGS + SERIAL_DATA_SIZE];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let text = std::str::from_utf8(&raw[..end]).ok()?.trim();
    if text.is_empty() || !text.is_ascii() {
        return None;
    }
    Some(text.to_string())
}

/// One `SET_REPORT`-then-`GET_REPORT` feature-report exchange on an already
/// open Interface-2 control fd: writes the get-`command_id` request (no
/// argument bytes — `data_size` is the *expected response* size), waits, and
/// reads the 91-byte response back, retrying the GET a few times with
/// backoff (research §5) since the first read right after connect can be
/// early. Returns the raw response buffer for the pure parsers to validate.
fn feature_exchange(
    control: &fs::File,
    txn: u8,
    command_id: u8,
    data_size: usize,
) -> io::Result<[u8; RAZER_CMD_LEN]> {
    const ZEROS: [u8; SERIAL_DATA_SIZE] = [0u8; SERIAL_DATA_SIZE];
    let mut req = build_razer_cmd(txn, CMD_CLASS_STANDARD, command_id, &ZEROS[..data_size]);
    let set = unsafe {
        libc::ioctl(
            control.as_raw_fd(),
            hidiocsfeature(RAZER_CMD_LEN as u32) as libc::c_ulong,
            req.as_mut_ptr(),
        )
    };
    if set < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut last_err = io::Error::new(
        io::ErrorKind::InvalidData,
        "device-info response never passed echo validation",
    );
    for delay_ms in DEVICE_INFO_SETGET_DELAYS_MS {
        std::thread::sleep(Duration::from_millis(delay_ms));
        let mut resp = [0u8; RAZER_CMD_LEN];
        let got = unsafe {
            libc::ioctl(
                control.as_raw_fd(),
                hidiocgfeature(RAZER_CMD_LEN as u32) as libc::c_ulong,
                resp.as_mut_ptr(),
            )
        };
        if got < 0 {
            last_err = io::Error::last_os_error();
            continue;
        }
        if response_echoes(&resp, command_id) {
            return Ok(resp);
        }
    }
    Err(last_err)
}

fn read_device_info_with(control: &fs::File, txn: u8) -> io::Result<DeviceInfo> {
    let invalid = |what: &str| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("device-info {what} response failed to parse"),
        )
    };
    let firmware = feature_exchange(control, txn, CMD_GET_FIRMWARE, FIRMWARE_DATA_SIZE)?;
    let firmware_version = parse_firmware(&firmware).ok_or_else(|| invalid("firmware"))?;
    let serial = feature_exchange(control, txn, CMD_GET_SERIAL, SERIAL_DATA_SIZE)?;
    let serial_number = parse_serial(&serial).ok_or_else(|| invalid("serial"))?;
    Ok(DeviceInfo {
        firmware_version,
        serial_number,
    })
}

/// Reads the connected Tartarus Pro's firmware version and serial number
/// over a freshly-opened, short-lived Interface-2 control fd (the
/// `relock()` fresh-fd pattern) — independent of Capture mode and of any
/// running grid task, so a forced-digital or analog-failed session still
/// gets device info (ticket 101). `capture::supervisor` calls this on every
/// device connect. Tries the primary `transaction_id` first, then the
/// fallback if the primary's responses don't validate (research §4).
///
/// These are reads — they change no device state — so the reset risk is
/// negligible (research §7). A failure here is non-fatal to the Daemon: the
/// caller logs it once and leaves both `GetState()` keys absent.
pub fn read_device_info() -> io::Result<DeviceInfo> {
    let control = open_control_fd()?;

    let mut last_err = io::Error::new(
        io::ErrorKind::InvalidData,
        "no device-info response validated",
    );
    for txn in [DEVICE_INFO_TXN_PRIMARY, DEVICE_INFO_TXN_FALLBACK] {
        match read_device_info_with(&control, txn) {
            Ok(info) => {
                eprintln!(
                    "acheron-daemon: read device info over Interface 2 with transaction_id {txn:#04x}: \
                     firmware {}, serial {}",
                    info.firmware_version, info.serial_number
                );
                return Ok(info);
            }
            Err(err) => last_err = err,
        }
    }
    Err(last_err)
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
            let Some(state) = hold else { continue };
            let held_for = state.started.elapsed();
            if schedule.repeat_due(held_for, state.fired) {
                // Advance past any repeats whose due time slipped by while
                // the loop was stalled, rather than `+= 1` and firing a
                // bunched catch-up burst on the next ticks (ticket 06).
                state.fired = schedule.advance_fired(held_for, state.fired);
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

    #[test]
    fn advance_fired_steps_by_one_when_on_schedule() {
        let schedule = RepeatSchedule::new(250, 33);
        // First repeat, due right at the delay: 0 -> 1.
        assert_eq!(schedule.advance_fired(Duration::from_millis(250), 0), 1);
        // Second, exactly one period later: 1 -> 2.
        assert_eq!(schedule.advance_fired(Duration::from_millis(283), 1), 2);
        // A tick that fires a hair past due still only steps by one.
        assert_eq!(schedule.advance_fired(Duration::from_millis(300), 1), 2);
    }

    #[test]
    fn advance_fired_skips_missed_repeats_after_a_stall_and_the_next_is_a_full_period_out() {
        let schedule = RepeatSchedule::new(250, 33);
        // Held 5s but only the initial repeat emitted (fired == 1): the loop
        // stalled through ~143 due times. `repeat_due` is true...
        assert!(schedule.repeat_due(Duration::from_millis(5_000), 1));
        // ...and advancing jumps straight to the count real elapsed time
        // calls for — (5000 - 250) / 33 + 1 == 144 — so exactly one Repeat
        // is emitted now, not a burst of the 143 missed ones.
        let fired = schedule.advance_fired(Duration::from_millis(5_000), 1);
        assert_eq!(fired, (5_000 - 250) / 33 + 1);
        // The next repeat is due one full period later, not immediately.
        assert!(!schedule.repeat_due(Duration::from_millis(5_000), fired));
        assert!(!schedule.repeat_due(Duration::from_millis(5_001), fired));
        let next_due = 250 + u64::from(fired) * 33;
        assert!(!schedule.repeat_due(Duration::from_millis(next_due - 1), fired));
        assert!(schedule.repeat_due(Duration::from_millis(next_due), fired));
    }

    #[test]
    fn advance_fired_never_regresses_the_count() {
        let schedule = RepeatSchedule::new(250, 33);
        // A spurious call before the first repeat is even due still moves
        // forward rather than backward.
        assert_eq!(schedule.advance_fired(Duration::from_millis(0), 4), 5);
    }

    #[test]
    fn due_offset_is_the_full_delay_then_one_period_per_fired_repeat() {
        let schedule = RepeatSchedule::new(250, 33);
        // The first repeat (fired == 0) falls due a full delay after the
        // press — a Toggle-held key looks exactly like a physically held one.
        assert_eq!(schedule.due_offset(0), Duration::from_millis(250));
        assert_eq!(schedule.due_offset(1), Duration::from_millis(283));
        assert_eq!(schedule.due_offset(5), Duration::from_millis(250 + 5 * 33));
        // `due_offset(fired)` is exactly the boundary `repeat_due` checks.
        for fired in 0..10 {
            let at = schedule.due_offset(fired);
            assert!(schedule.repeat_due(at, fired));
            assert!(!schedule.repeat_due(at - Duration::from_millis(1), fired));
        }
    }

    // -- without_warmup: Analog-repeat hold-solid (§5.3/§5.4) -------------

    #[test]
    fn without_warmup_collapses_the_delay_to_a_single_period() {
        let steady = RepeatSchedule::new(250, 33).without_warmup();
        // The first repeat (fired == 0) falls due one plain period in — no
        // `delay_ms` gap (contrast `due_offset(0)` == 250ms on the source).
        assert_eq!(steady.due_offset(0), Duration::from_millis(33));
        assert_eq!(steady.due_offset(1), Duration::from_millis(66));
        assert_eq!(steady.due_offset(5), Duration::from_millis(6 * 33));
        // The whole `due_offset` / `repeat_due` contract still holds on it.
        for fired in 0..10 {
            let at = steady.due_offset(fired);
            assert!(steady.repeat_due(at, fired));
            assert!(!steady.repeat_due(at - Duration::from_millis(1), fired));
        }
    }

    #[test]
    fn without_warmup_still_clamps_missed_deadlines_without_bursting() {
        let steady = RepeatSchedule::new(250, 33).without_warmup();
        // On schedule: one step per period elapsed.
        assert_eq!(steady.advance_fired(Duration::from_millis(33), 0), 1);
        assert_eq!(steady.advance_fired(Duration::from_millis(66), 1), 2);
        // Stalled 5s having emitted only the first repeat: jump straight to
        // the elapsed-time count — one repeat now, not a burst of the missed
        // ~150 — and the next is due within a period, never immediately.
        let fired = steady.advance_fired(Duration::from_millis(5_000), 1);
        assert_eq!(fired, (5_000 - 33) / 33 + 1);
        let next_due = steady.due_offset(fired);
        assert!(!steady.repeat_due(Duration::from_millis(5_000), fired));
        assert!(!steady.repeat_due(next_due - Duration::from_millis(1), fired));
        assert!(steady.repeat_due(next_due, fired));
        // Never regresses on a spurious early call.
        assert_eq!(steady.advance_fired(Duration::from_millis(0), 4), 5);
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
    fn hidiocgfeature_91_matches_the_documented_ioctl_number() {
        // Research §3.2: the read variant differs from `HIDIOCSFEATURE` only
        // in the direction nr (`0x07` vs `0x06`).
        assert_eq!(hidiocgfeature(91), 0xC05B4807);
    }

    // -- device-info requests: same frame/CRC as the unlock, per research §2 --

    #[test]
    fn firmware_request_matches_the_researched_bytes() {
        let cmd = build_razer_cmd(
            DEVICE_INFO_TXN_PRIMARY,
            CMD_CLASS_STANDARD,
            CMD_GET_FIRMWARE,
            &[0u8; FIRMWARE_DATA_SIZE],
        );
        assert_eq!(cmd[2], 0xFF, "transaction_id");
        assert_eq!(cmd[6], 0x02, "data_size");
        assert_eq!(cmd[7], 0x00, "command_class");
        assert_eq!(cmd[8], 0x81, "command_id");
        assert_eq!(
            cmd[89], 0x83,
            "crc = data_size ^ command_class ^ command_id"
        );
    }

    #[test]
    fn serial_request_matches_the_researched_bytes() {
        let cmd = build_razer_cmd(
            DEVICE_INFO_TXN_PRIMARY,
            CMD_CLASS_STANDARD,
            CMD_GET_SERIAL,
            &[0u8; SERIAL_DATA_SIZE],
        );
        assert_eq!(cmd[2], 0xFF, "transaction_id");
        assert_eq!(cmd[6], 0x16, "data_size");
        assert_eq!(cmd[7], 0x00, "command_class");
        assert_eq!(cmd[8], 0x82, "command_id");
        assert_eq!(
            cmd[89], 0x94,
            "crc = data_size ^ command_class ^ command_id"
        );
    }

    // -- Status-LED frame: ported, not recomputed (spec.md §"The wire frame",
    // verified on hardware by charting ticket 01 / `prototype.py` selftest) --

    #[test]
    fn status_led_frame_matches_the_hardware_verified_bytes() {
        // Orange on, green + blue off.
        let cmd = build_razer_cmd(
            STATUS_LED_TXN,
            STATUS_LED_CMD_CLASS,
            STATUS_LED_CMD_ID,
            &status_led_args(STATUS_LED_ON, STATUS_LED_OFF, STATUS_LED_OFF),
        );
        assert_eq!(cmd[2], 0x1F, "transaction_id");
        assert_eq!(cmd[6], 0x09, "data_size");
        assert_eq!(cmd[7], 0x0F, "command_class");
        assert_eq!(cmd[8], 0x02, "command_id");
        assert_eq!(cmd[9], 0x00, "arg0 = storage (inert, 0x00 for clarity)");
        assert_eq!(cmd[10], 0x0B, "arg1 = SIDE_STRIPE_LED id");
        assert_eq!(cmd[11], 0x01, "arg2 = static effect");
        assert_eq!(cmd[14], 0x01, "arg5 = colour count");
        assert_eq!((cmd[15], cmd[16], cmd[17]), (0xFF, 0x00, 0x00), "r/g/b");
    }

    #[test]
    fn status_led_all_off_frame_crc_matches_the_prototype_selftest() {
        let off = build_razer_cmd(
            STATUS_LED_TXN,
            STATUS_LED_CMD_CLASS,
            STATUS_LED_CMD_ID,
            &status_led_args(STATUS_LED_OFF, STATUS_LED_OFF, STATUS_LED_OFF),
        );
        // `prototype/01-status-leds/prototype.py` selftest: "LED all-off crc == 0x0f".
        assert_eq!(off[89], 0x0F, "all-off crc");
    }

    // -- Lighting frames: byte tables ported from
    // `research/backlight-wire-protocol.md`, verified on hardware by charting
    // ticket 02 (not re-verified here) — these tests only pin the Rust
    // translation of that already-verified table, the same role
    // `status_led_frame_matches_the_hardware_verified_bytes` plays above. --

    fn colour(r: u8, g: u8, b: u8) -> Colour {
        Colour { r, g, b }
    }

    #[test]
    fn off_frame_sends_the_none_effect_with_no_colour() {
        let args = effect_args(EFFECT_NONE, &[0x00, 0x00, 0x00]);
        assert_eq!(
            args,
            vec![VARSTORE, BACKLIGHT_LED, EFFECT_NONE, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn static_frame_carries_one_colour() {
        let (effect_id, tail) = fixed_effect_id_and_tail(&FixedEffect::Static {
            colour: colour(0xFF, 0x10, 0x20),
        });
        assert_eq!(effect_id, EFFECT_STATIC);
        let args = effect_args(effect_id, &tail);
        assert_eq!(
            args,
            vec![
                VARSTORE,
                BACKLIGHT_LED,
                EFFECT_STATIC,
                0x00,
                0x00,
                0x01,
                0xFF,
                0x10,
                0x20
            ]
        );
    }

    #[test]
    fn spectrum_frame_carries_no_parameters() {
        let (effect_id, tail) = fixed_effect_id_and_tail(&FixedEffect::Spectrum);
        assert_eq!(effect_id, EFFECT_SPECTRUM);
        assert_eq!(tail, vec![0x00, 0x00, 0x00]);
    }

    #[test]
    fn wave_frame_encodes_direction_and_the_fixed_speed_byte() {
        let (effect_id, right) = fixed_effect_id_and_tail(&FixedEffect::Wave {
            direction: WaveDirection::Right,
        });
        assert_eq!(effect_id, EFFECT_WAVE);
        assert_eq!(right, vec![0x01, WAVE_SPEED, 0x00], "right = 0x01");

        let (_, left) = fixed_effect_id_and_tail(&FixedEffect::Wave {
            direction: WaveDirection::Left,
        });
        assert_eq!(left, vec![0x02, WAVE_SPEED, 0x00], "left = 0x02");
    }

    #[test]
    fn reactive_frame_carries_speed_and_one_colour() {
        let (effect_id, tail) = fixed_effect_id_and_tail(&FixedEffect::Reactive {
            colour: colour(0x01, 0x02, 0x03),
            speed: 0x04,
        });
        assert_eq!(effect_id, EFFECT_REACTIVE);
        assert_eq!(tail, vec![0x00, 0x04, 0x01, 0x01, 0x02, 0x03]);
    }

    #[test]
    fn breath_random_carries_no_colour_and_repeats_the_zero_count_at_a3_and_a5() {
        let (effect_id, tail) = fixed_effect_id_and_tail(&FixedEffect::Breath {
            style: BreathStyle::Random,
        });
        assert_eq!(effect_id, EFFECT_BREATH);
        assert_eq!(tail, vec![0x00, 0x00, 0x00]);
    }

    #[test]
    fn breath_single_repeats_the_one_count_at_a3_and_a5_around_a4_zero() {
        let (_, tail) = fixed_effect_id_and_tail(&FixedEffect::Breath {
            style: BreathStyle::Single {
                colour: colour(0x11, 0x22, 0x33),
            },
        });
        assert_eq!(tail, vec![0x01, 0x00, 0x01, 0x11, 0x22, 0x33]);
    }

    #[test]
    fn breath_dual_repeats_the_two_count_and_carries_both_colours() {
        let (_, tail) = fixed_effect_id_and_tail(&FixedEffect::Breath {
            style: BreathStyle::Dual {
                first: colour(0x01, 0x02, 0x03),
                second: colour(0x04, 0x05, 0x06),
            },
        });
        assert_eq!(
            tail,
            vec![0x02, 0x00, 0x02, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06]
        );
    }

    #[test]
    fn starlight_random_fixes_a3_zero_and_carries_the_speed_byte_at_a4() {
        let (effect_id, tail) = fixed_effect_id_and_tail(&FixedEffect::Starlight {
            style: BreathStyle::Random,
            speed: 0x02,
        });
        assert_eq!(effect_id, EFFECT_STARLIGHT);
        assert_eq!(tail, vec![0x00, 0x02, 0x00]);
    }

    #[test]
    fn starlight_dual_carries_speed_and_both_colours() {
        let (_, tail) = fixed_effect_id_and_tail(&FixedEffect::Starlight {
            style: BreathStyle::Dual {
                first: colour(0x0A, 0x0B, 0x0C),
                second: colour(0x0D, 0x0E, 0x0F),
            },
            speed: 0x03,
        });
        assert_eq!(
            tail,
            vec![0x00, 0x03, 0x02, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F]
        );
    }

    #[test]
    fn custom_frame_args_place_row_and_column_headers_then_21_rgb_triples() {
        let mut colours = [colour(0x00, 0x00, 0x00); MATRIX_COLUMNS];
        colours[0] = colour(0xAA, 0xBB, 0xCC);
        colours[20] = colour(0x11, 0x22, 0x33);
        let args = custom_frame_args(&colours);

        assert_eq!(
            args.len(),
            CUSTOM_FRAME_DATA_SIZE,
            "fixed 71-byte data_size"
        );
        assert_eq!(args[2], 0x00, "row_index");
        assert_eq!(args[3], 0x00, "start_col");
        assert_eq!(args[4], 0x14, "stop_col = 20 (21 columns, 0-indexed)");
        assert_eq!((args[5], args[6], args[7]), (0xAA, 0xBB, 0xCC), "column 0");
        assert_eq!(
            (args[65], args[66], args[67]),
            (0x11, 0x22, 0x33),
            "column 20"
        );
        assert!(args[68..].iter().all(|&b| b == 0x00), "tail zero-padded");
    }

    #[test]
    fn custom_effect_arm_args_are_a_bare_trigger_with_no_varstore_or_led_id() {
        let args = custom_effect_arm_args();
        assert_eq!(args.len(), CUSTOM_EFFECT_ARM_DATA_SIZE);
        assert_eq!(args[0], 0x00, "a0 is not VARSTORE here");
        assert_eq!(args[1], 0x00, "a1 is not BACKLIGHT_LED here");
        assert_eq!(args[2], EFFECT_CUSTOM);
        assert!(args[3..].iter().all(|&b| b == 0x00));
    }

    #[test]
    fn brightness_args_target_zero_led_not_backlight_led() {
        assert_eq!(brightness_args(0x80), [VARSTORE, ZERO_LED, 0x80]);
    }

    #[test]
    fn a_fixed_effect_frame_carries_the_lighting_txn_and_command_class() {
        let (effect_id, tail) = fixed_effect_id_and_tail(&FixedEffect::Spectrum);
        let cmd = build_razer_cmd(
            LIGHTING_TXN,
            LIGHTING_CMD_CLASS,
            LIGHTING_EFFECT_CMD_ID,
            &effect_args(effect_id, &tail),
        );
        assert_eq!(cmd[2], 0x1F, "transaction_id");
        assert_eq!(cmd[6], 0x06, "data_size");
        assert_eq!(cmd[7], 0x0F, "command_class");
        assert_eq!(cmd[8], 0x02, "command_id");
        assert_eq!(cmd[10], BACKLIGHT_LED, "arg1 = BACKLIGHT_LED id");
    }

    // -- pure response parsing (research §3.3/§3.4/§5) --------------------

    /// A synthetic 91-byte `HIDIOCGFEATURE` response: byte 0 the report
    /// number, then the 90-byte struct at 1.., `status` at 1, class/id echo
    /// at 7/8, `arguments` from 9.
    fn response(status: u8, command_class: u8, command_id: u8, args: &[u8]) -> [u8; RAZER_CMD_LEN] {
        let mut resp = [0u8; RAZER_CMD_LEN];
        resp[RESP_STATUS] = status;
        resp[RESP_CMD_CLASS] = command_class;
        resp[RESP_CMD_ID] = command_id;
        resp[RESP_ARGS..RESP_ARGS + args.len()].copy_from_slice(args);
        resp
    }

    #[test]
    fn parse_firmware_renders_the_two_argument_bytes_as_vmajor_dot_minor() {
        let resp = response(0x02, 0x00, CMD_GET_FIRMWARE, &[1, 2]);
        assert_eq!(parse_firmware(&resp).as_deref(), Some("v1.2"));
    }

    #[test]
    fn parse_firmware_accepts_the_busy_and_unset_status_bytes() {
        for status in [0x00, 0x01, 0x02] {
            let resp = response(status, 0x00, CMD_GET_FIRMWARE, &[3, 4]);
            assert_eq!(parse_firmware(&resp).as_deref(), Some("v3.4"));
        }
    }

    #[test]
    fn parse_firmware_rejects_a_failure_status_a_wrong_id_echo_and_a_wrong_class() {
        assert!(parse_firmware(&response(0x03, 0x00, CMD_GET_FIRMWARE, &[1, 2])).is_none());
        assert!(parse_firmware(&response(0x02, 0x00, CMD_GET_SERIAL, &[1, 2])).is_none());
        assert!(parse_firmware(&response(0x02, 0x07, CMD_GET_FIRMWARE, &[1, 2])).is_none());
    }

    #[test]
    fn parse_serial_extracts_ascii_and_trims_nul_padding() {
        let mut args = [0u8; SERIAL_DATA_SIZE];
        args[..15].copy_from_slice(b"PM2443F36300141");
        let resp = response(0x02, 0x00, CMD_GET_SERIAL, &args);
        assert_eq!(parse_serial(&resp).as_deref(), Some("PM2443F36300141"));
    }

    #[test]
    fn parse_serial_also_trims_trailing_whitespace_padding() {
        let mut args = [b' '; SERIAL_DATA_SIZE];
        args[..6].copy_from_slice(b"PM2443");
        let resp = response(0x02, 0x00, CMD_GET_SERIAL, &args);
        assert_eq!(parse_serial(&resp).as_deref(), Some("PM2443"));
    }

    #[test]
    fn parse_serial_rejects_an_all_zero_or_non_ascii_payload() {
        assert!(
            parse_serial(&response(
                0x02,
                0x00,
                CMD_GET_SERIAL,
                &[0u8; SERIAL_DATA_SIZE]
            ))
            .is_none()
        );
        assert!(
            parse_serial(&response(
                0x02,
                0x00,
                CMD_GET_SERIAL,
                &[0xFF; SERIAL_DATA_SIZE]
            ))
            .is_none()
        );
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
