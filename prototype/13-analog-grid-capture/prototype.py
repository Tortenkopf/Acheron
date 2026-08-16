#!/usr/bin/env python3
"""
PROTOTYPE — throwaway code, not production. Standalone: imports nothing from
`daemon/` or `gui/`, stdlib only, and writes nothing into Acheron's config.

Answers: can we actually read live per-grid-key analog depth values off the
real, connected Tartarus Pro from Linux via `hidraw`?
Ticket: .scratch/tartarus-input-expansion/issues/13-task-standalone-analog-capture-prototype.md
Plan:   .scratch/tartarus-input-expansion/research/linux-analog-grid-key-protocol.md

The four questions the ticket asks, and where each gets answered:

  1. Does the documented unlock actually put the device into streaming mode?
     -> `unlock` logs the standby report (all-zero depths, report 0x06) if it
        arrives, and re-reads `device_mode` before and after.
  2. Do report 0x06 reads carry plausible *analog* depth, not just on/off?
     -> the live view tracks, per keycap, min/max and the set of distinct
        depths seen. "DISTINCT DEPTHS" in the header is the headline number:
        2 means on/off, dozens means real analog.
  3. Any deviation from the documented byte layout / report IDs on this unit?
     -> the live view shows the 3 documented-as-spare trailing bytes, counts
        every other report ID seen on interface 1, and flags any report whose
        length isn't the documented 24.
  4. Is the firmware-reset risk reproduced?
     -> the loop watches for the device vanishing off the bus (POLLERR/ENODEV),
        timestamps it, then re-discovers and reports which mode it came back in.

Plus the §6 question the research calls the most consequential one: does driver
mode silence the grid keys' ordinary evdev keycodes? The live view reads the
device's three evdev nodes *in the same process*, so one keypress shows both
channels side by side — no second terminal, no `evtest`.

Ticket 16 extends it with the driver-mode facts ticket 13 left open:

  5. Does driver mode silence anything *other* than the 20 grid keys?
     -> the NON-GRID INPUTS panel tracks the Mode key, the four thumbstick
        directions and the three wheel events individually. Report 0x06 has no
        channel for any of them, so if driver mode silences these too the whole
        analog strand loses the Layer key. This is the invalidating question.
  6. Does the mode survive a power cycle / suspend-resume?
     -> `survive` keeps monitoring across the device leaving the bus: it
        re-discovers, re-opens, and logs which mode it came back in, so one
        process and one JSONL span the whole unplug/replug or suspend cycle.
  7. Is byte n really keycap n?
     -> `mapping` walks the 20 keycaps in a fixed *non-reading* order (ticket
        13 only ever saw them pressed in layout order) and records which byte
        actually moved for each.

Run (in the order of the research's §7 procedure):

    python3 prototype.py selftest              # no device, no root
    sudo python3 prototype.py probe            # read-only pre-state
    sudo python3 prototype.py listen           # watch both channels, send nothing
    sudo python3 prototype.py unlock --dry-run # print the bytes, send nothing
    sudo python3 prototype.py unlock           # THE RISKY STEP (§5)
    sudo python3 prototype.py mapping          # byte -> keycap, out of order
    sudo python3 prototype.py survive          # hold across unplug/suspend
    sudo python3 prototype.py relock           # also risky; not "cleanup"

`unlock`/`relock` prompt for typed confirmation unless given --yes. Press ESC
to leave the live view (not 'q' — 'q' is grid key 07), or 'm' to re-read
`device_mode` into the log. Add --record FILE to append a JSONL log of every
report, evdev event and mapping observation.
"""

import argparse
import errno
import fcntl
import json
import os
import pathlib
import re
import select
import struct
import sys
import termios
import time

VENDOR_ID = "1532"
PRODUCT_ID = "0244"

ANALOG_INTERFACE = 1  # streams report 0x06
CONTROL_INTERFACE = 2  # takes the razer_report feature report

ANALOG_REPORT_ID = 0x06
ANALOG_REPORT_LEN = 24  # report ID + 23 payload bytes
NUM_KEYS = 20

# ---------------------------------------------------------------------------
# Protocol — research §2/§3. Every constant here is cross-checked by `selftest`
# against a value the research derived independently (by hand, or from
# OpenRazer's kernel source), because getting these wrong means sending
# arbitrary bytes to firmware that may reboot itself in response.
# ---------------------------------------------------------------------------

_IOC_WRITE = 1
_IOC_READ = 2


def _ioc(direction, type_char, nr, size):
    """Linux `_IOC()` from asm-generic/ioctl.h (2/8/8/14-bit dir/type/nr/size)."""
    return (direction << 30) | (size << 16) | (ord(type_char) << 8) | nr


def hidiocsfeature(size):
    return _ioc(_IOC_WRITE | _IOC_READ, "H", 0x06, size)


def build_razer_cmd(txn, command_class, command_id, args):
    """The 91-byte buffer `hidraw` wants: report-number byte + 90-byte razer_report.

    Mirrors `open-tartarus-driver`'s `build_razer_cmd` (main.rs:538) byte for
    byte. CRC is the XOR of buffer indices 3..88 inclusive, which is struct
    bytes 2..87 — the same range as OpenRazer's `razer_calculate_crc()`.
    Note `transaction_id` sits at index 2 and is *excluded* from the CRC.
    """
    buf = bytearray(91)
    buf[2] = txn
    buf[6] = len(args)
    buf[7] = command_class
    buf[8] = command_id
    buf[9 : 9 + len(args)] = args
    crc = 0
    for b in buf[3:89]:
        crc ^= b
    buf[89] = crc
    return bytes(buf)


# transaction_id 0x01 is what open-tartarus-driver captured Synapse sending and
# is the one variant never reported to reset the device. Do NOT substitute
# OpenRazer's 0x1F/0xFF, and do NOT use the `device_mode` sysfs shortcut, which
# hardcodes 0xFF (research §4.2, §5).
TRANSACTION_ID = 0x01
CMD_CLASS_STANDARD = 0x00
CMD_SET_DEVICE_MODE = 0x04
CMD_GET_DEVICE_MODE = 0x84

MODE_DRIVER = 0x03  # unlocks the analog stream
MODE_NORMAL = 0x00  # re-lock; itself implicated in the reset reports (§5)

UNLOCK_CMD = build_razer_cmd(
    TRANSACTION_ID, CMD_CLASS_STANDARD, CMD_SET_DEVICE_MODE, bytes([MODE_DRIVER, 0x00])
)
RELOCK_CMD = build_razer_cmd(
    TRANSACTION_ID, CMD_CLASS_STANDARD, CMD_SET_DEVICE_MODE, bytes([MODE_NORMAL, 0x00])
)


def ioctl_unsigned(fd, request, arg):
    """`fcntl.ioctl` with the >=2^31 request numbers HIDIOC* produces.

    Older CPython insists the request fit a signed int; pass the two's
    complement form if the unsigned one is rejected.
    """
    try:
        return fcntl.ioctl(fd, request, arg)
    except OverflowError:
        return fcntl.ioctl(fd, request - (1 << 32), arg)


# ---------------------------------------------------------------------------
# Device discovery — research §3.1. Match on bInterfaceNumber via sysfs, never
# on HID usage (interface 1's first collection reports as a plain Keyboard, so
# the Windows-side usage filter would exclude the analog interface outright).
# hidraw node numbers are not stable across boots, or across a reset.
# ---------------------------------------------------------------------------


class Interface:
    def __init__(self, number, hid_id, hidraw, sysfs_hid, event_nodes):
        self.number = number
        self.hid_id = hid_id
        self.hidraw = hidraw
        self.sysfs_hid = sysfs_hid
        self.event_nodes = event_nodes

    def __repr__(self):
        return f"<Interface {self.number} {self.hidraw} {self.hid_id}>"


def read_attr(path):
    try:
        return pathlib.Path(path).read_text().strip()
    except (OSError, ValueError):  # ValueError: binary attributes aren't UTF-8
        return None


def read_bytes_attr(path):
    try:
        return pathlib.Path(path).read_bytes()
    except OSError:
        return None


def discover():
    """Map bInterfaceNumber -> Interface for the connected Tartarus Pro.

    Returns {} if the device isn't on the bus (which is also how a reset is
    confirmed after the fact).
    """
    found = {}
    for node in sorted(pathlib.Path("/sys/class/hidraw").glob("hidraw*")):
        # This runs in a loop against a device that is actively re-enumerating,
        # so any node may be half-created or already gone: a partially written
        # attribute reads back as '' and would raise from int(). Skipping the
        # node and re-polling is always the right answer.
        try:
            hid_dir = (node / "device").resolve()
            usb_intf = hid_dir.parent
            usb_dev = usb_intf.parent
            if read_attr(usb_dev / "idVendor") != VENDOR_ID:
                continue
            if read_attr(usb_dev / "idProduct") != PRODUCT_ID:
                continue
            number = int(read_attr(usb_intf / "bInterfaceNumber"), 16)
            events = sorted(
                str(p.name) for p in hid_dir.glob("input/input*/event*") if p.is_dir()
            )
        except (OSError, TypeError, ValueError):
            continue
        found[number] = Interface(
            number=number,
            hid_id=hid_dir.name,
            hidraw=f"/dev/{node.name}",
            sysfs_hid=hid_dir,
            event_nodes=[f"/dev/input/{e}" for e in events],
        )
    return found


def openrazer_state(interfaces):
    """Read OpenRazer's sysfs view of the device: the safe observation channel.

    Reading `device_mode` here issues a real get-device-mode control transfer
    with the settle delay already handled by the kernel driver — cheaper and
    safer than sending our own GET after every SET (research §3.2).
    """
    ctrl = interfaces.get(CONTROL_INTERFACE)
    if ctrl is None:
        return {}
    # device_mode is two *raw* bytes, not text — `00 00` normal, `03 00` driver.
    mode = read_bytes_attr(ctrl.sysfs_hid / "device_mode")
    return {
        "device_serial": read_attr(ctrl.sysfs_hid / "device_serial"),
        "firmware_version": read_attr(ctrl.sysfs_hid / "firmware_version"),
        "device_mode": None if mode is None else " ".join(f"{b:02x}" for b in mode),
    }


# ---------------------------------------------------------------------------
# Physical layout (layout.md + daemon/src/input.rs). The keycap-index -> key
# correspondence below is the *hypothesis* under test, not a fact: report byte
# n is documented as keycap n, and we assume keycap numbering runs in reading
# order with keycap 20 as the thumb key. Step 6 of the research procedure is
# precisely to check that empirically, so the live view labels each byte index
# with its hypothesised key and lets the human confirm or refute it.
# ---------------------------------------------------------------------------

GRID_KEY_NAMES = [
    "KEY_1", "KEY_2", "KEY_3", "KEY_4", "KEY_5",
    "KEY_TAB", "KEY_Q", "KEY_W", "KEY_E", "KEY_R",
    "KEY_CAPSLOCK", "KEY_A", "KEY_S", "KEY_D", "KEY_F",
    "KEY_LEFTSHIFT", "KEY_Z", "KEY_X", "KEY_C", "KEY_SPACE",
]

EV_SYN, EV_KEY, EV_REL, EV_MSC, EV_LED = 0x00, 0x01, 0x02, 0x04, 0x11
# EV_MSC/MSC_SCAN fires alongside every key event and EV_LED is keyboard-LED
# chatter — both are pure noise for the one question here (does a grid key
# still emit a keycode?), so they never reach the display.
EV_IGNORED = (EV_SYN, EV_MSC, EV_LED)

# evdev codes for everything this device emits, so the evdev panel is readable
# without a second tool. Grid keys are on interface 1, mode key/thumbstick on
# interface 0, middle click on interface 2.
EVDEV_NAMES = {
    2: "KEY_1", 3: "KEY_2", 4: "KEY_3", 5: "KEY_4", 6: "KEY_5",
    15: "KEY_TAB", 16: "KEY_Q", 17: "KEY_W", 18: "KEY_E", 19: "KEY_R",
    58: "KEY_CAPSLOCK", 30: "KEY_A", 31: "KEY_S", 32: "KEY_D", 33: "KEY_F",
    42: "KEY_LEFTSHIFT", 44: "KEY_Z", 45: "KEY_X", 46: "KEY_C", 57: "KEY_SPACE",
    56: "KEY_LEFTALT", 103: "KEY_UP", 108: "KEY_DOWN", 105: "KEY_LEFT",
    106: "KEY_RIGHT", 274: "BTN_MIDDLE",
}

GRID_KEY_CODES = {code for code, name in EVDEV_NAMES.items() if name in GRID_KEY_NAMES}

REL_WHEEL = 8  # the wheel's EV_REL axis; sign gives the direction

# Every Input on this device that report 0x06 does NOT carry — it has 20 depth
# bytes for the 20 keycaps and nothing else. Each entry is the evdev signature
# that identifies it, keyed by the Input name `daemon/src/input.rs` uses.
# Ticket 16's invalidating question is whether driver mode silences these too:
# if it does, analog-primary capture loses the Mode key (and with it Layers)
# and the thumbstick, with no analog channel to recover them from.
NON_GRID_INPUTS = [
    ("mode_key", 56, None),
    ("thumbstick_up", 103, None),
    ("thumbstick_down", 108, None),
    ("thumbstick_left", 105, None),
    ("thumbstick_right", 106, None),
    ("wheel_middle", 274, None),
    ("wheel_scroll_up", REL_WHEEL, +1),
    ("wheel_scroll_down", REL_WHEEL, -1),
]


def non_grid_input_for(etype, code, value):
    """evdev event -> the non-grid Input name it represents, or None.

    Key events count on press only (value 1); wheel ticks are EV_REL and carry
    their direction in the sign of the value.
    """
    for name, expected_code, sign in NON_GRID_INPUTS:
        if sign is None:
            if etype == EV_KEY and code == expected_code and value == 1:
                return name
        elif etype == EV_REL and code == expected_code and value * sign > 0:
            return name
    return None


def keycap_label(index):
    """1-based report byte index -> "NN r{row}c{col} KEY_X" under the hypothesis."""
    row, col = (index - 1) // 5 + 1, (index - 1) % 5 + 1
    return f"{index:02d} r{row}c{col} {GRID_KEY_NAMES[index - 1]}"


# ---------------------------------------------------------------------------
# Live view
# ---------------------------------------------------------------------------

INPUT_EVENT = struct.Struct("llHHi")  # timeval + type + code + value = 24 bytes

# The thumbstick types arrow keys, and an arrow key *is* an ESC sequence on a
# tty (Left is `ESC [ D`) — so a bare "did I see 0x1b" quit check exits the
# moment the human presses the very Input ticket 16 exists to test. Strip whole
# CSI/SS3 sequences first; only what survives can be a real keystroke.
CSI_SEQUENCE = re.compile(rb"\x1b[\[O][0-9;?]*[ -/]*[@-~]")


def stdin_commands(data):
    """Raw stdin bytes -> the set of commands the human actually typed."""
    typed = CSI_SEQUENCE.sub(b"", data)
    commands = set()
    if b"\x1b" in typed:
        commands.add("quit")
    if b"m" in typed:
        commands.add("mode")
    return commands


class KeyStats:
    def __init__(self):
        self.current = 0
        self.min = None
        self.max = 0
        self.values = set()
        self.updates = 0

    def observe(self, depth):
        self.current = depth
        self.updates += 1
        self.values.add(depth)
        self.max = max(self.max, depth)
        self.min = depth if self.min is None else min(self.min, depth)

    @property
    def intermediate(self):
        """Distinct depths that are neither 0 nor 255 — the analog evidence."""
        return len(self.values - {0, 255})


class Monitor:
    """Reads the analog hidraw node and the device's evdev nodes together."""

    def __init__(self, interfaces, record=None, ui=True, observer=None):
        self.interfaces = interfaces
        self.record = record
        self.ui = ui
        # An optional guided-procedure driver (see MappingObserver): gets every
        # depth vector, contributes its own panel, and can end the run.
        self.observer = observer
        self.started = time.monotonic()
        self.keys = [KeyStats() for _ in range(NUM_KEYS)]
        self.report_counts = {}
        self.spare_bytes = None
        self.last_report = None
        self.anomalies = []
        self.log_lines = []
        self.evdev_counts = {}
        self.evdev_recent = []
        self.grid_keycodes_seen = 0
        self.non_grid_seen = {name: 0 for name, _, _ in NON_GRID_INPUTS}
        self.attachments = 0
        self.phases = []
        self._stdin_tail = b""
        self._stdin_tail_at = 0.0
        self.waiting_until = None
        self.analog_reports = 0
        self.first_report_at = None
        self.device_lost_at = None
        self.analog_fd = None
        self.evdev_fds = {}
        self._dirty = True
        self._last_draw = 0.0

    # -- lifecycle ----------------------------------------------------------

    def open_analog(self):
        """Open the interface-1 read fd. Call BEFORE sending the unlock, so the
        one-shot standby report can't be missed (research §7 step 3)."""
        path = self.interfaces[ANALOG_INTERFACE].hidraw
        self.analog_fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
        self.attachments += 1
        self.log(f"opened {path} (interface 1, analog)")

    def open_evdev(self):
        for number, interface in sorted(self.interfaces.items()):
            for path in interface.event_nodes:
                try:
                    self.evdev_fds[os.open(path, os.O_RDONLY | os.O_NONBLOCK)] = (
                        number,
                        path,
                    )
                    self.evdev_counts[path] = 0
                except OSError as exc:
                    self.log(f"could not open {path}: {exc}")

    def close(self):
        for fd in list(self.evdev_fds):
            os.close(fd)
        self.evdev_fds.clear()
        if self.analog_fd is not None:
            os.close(self.analog_fd)
            self.analog_fd = None

    # -- recording ----------------------------------------------------------

    def log(self, message):
        stamp = time.monotonic() - self.started
        self.log_lines.append(f"[{stamp:7.3f}s] {message}")
        self.emit({"kind": "log", "message": message})
        self._dirty = True
        if not self.ui:
            print(self.log_lines[-1], flush=True)

    def emit(self, payload):
        if self.record is None:
            return
        payload = dict(payload, t=round(time.monotonic() - self.started, 6))
        self.record.write(json.dumps(payload) + "\n")
        self.record.flush()

    # -- reading ------------------------------------------------------------

    def drain(self, fd, size, consume):
        """Read from `fd` until it would block, handing each chunk to `consume`.

        Returns False once the device is gone — an empty read, ENODEV or EIO,
        all of which mean the same thing here: it left the bus.
        """
        while True:
            try:
                chunk = os.read(fd, size)
            except BlockingIOError:
                return True
            except OSError as exc:
                if exc.errno in (errno.ENODEV, errno.EIO):
                    return False
                raise
            if not chunk:
                return False
            consume(chunk)

    def handle_analog(self):
        # hidraw reads are whole-report, so one read is one report; 64 bytes is
        # comfortably larger than the documented 24.
        return self.drain(self.analog_fd, 64, self.on_report)

    def on_report(self, data):
        report_id = data[0]
        self.report_counts[report_id] = self.report_counts.get(report_id, 0) + 1
        if report_id != ANALOG_REPORT_ID:
            self._dirty = True
            return

        self.analog_reports += 1
        self.last_report = data
        if self.first_report_at is None:
            self.first_report_at = time.monotonic() - self.started
            zeros = all(b == 0 for b in data[1:])
            self.log(
                f"first report 0x06 after {self.first_report_at:.3f}s, "
                f"{len(data)} bytes, {'all-zero (standby)' if zeros else 'NON-ZERO'}"
            )
        if len(data) != ANALOG_REPORT_LEN:
            self.note_anomaly(f"report 0x06 was {len(data)} bytes, expected 24")
        for index in range(1, min(NUM_KEYS, len(data) - 1) + 1):
            self.keys[index - 1].observe(data[index])
        if self.observer is not None:
            self.observer.on_depths(tuple(data[1 : NUM_KEYS + 1]), self)
        spare = tuple(data[NUM_KEYS + 1 :])
        if spare and spare != self.spare_bytes:
            self.spare_bytes = spare
            if any(spare):
                self.note_anomaly(
                    "documented-spare trailing bytes are non-zero: "
                    + " ".join(f"{b:02x}" for b in spare)
                )
        self.emit({"kind": "analog", "report": data.hex()})
        self._dirty = True

    def note_anomaly(self, message):
        if message not in self.anomalies:
            self.anomalies.append(message)
            self.log("DEVIATION: " + message)

    def read_stdin(self, fd):
        """Tty bytes -> commands, holding back a trailing lone ESC.

        A read can land mid-sequence, splitting the `ESC` from the `[D` that
        makes it an arrow key. A trailing ESC is therefore parked rather than
        acted on, and only `expired_escape` promotes it to a real quit.
        """
        data = self._stdin_tail + os.read(fd, 256)
        self._stdin_tail = b""
        if data.endswith(b"\x1b"):
            data, self._stdin_tail = data[:-1], b"\x1b"
            self._stdin_tail_at = time.monotonic()
        return stdin_commands(data)

    def expired_escape(self):
        """True once a parked ESC has gone unfollowed long enough to be real."""
        return bool(self._stdin_tail) and time.monotonic() - self._stdin_tail_at > 0.3

    def handle_evdev(self, fd):
        number, path = self.evdev_fds[fd]

        def consume(chunk):
            for offset in range(0, len(chunk) - INPUT_EVENT.size + 1, INPUT_EVENT.size):
                _, _, etype, code, value = INPUT_EVENT.unpack_from(chunk, offset)
                if etype in EV_IGNORED:
                    continue
                self.on_evdev(number, path, etype, code, value)

        return self.drain(fd, INPUT_EVENT.size * 64, consume)

    def on_evdev(self, number, path, etype, code, value):
        if etype == EV_KEY:
            name = EVDEV_NAMES.get(code, f"code {code}")
            action = {0: "release", 1: "press", 2: "repeat"}.get(value, str(value))
            label = f"if{number} {name} {action}"
            if code in GRID_KEY_CODES and value == 1:
                self.grid_keycodes_seen += 1
        elif etype == EV_REL:
            label = f"if{number} REL axis {code} {value:+d}"
        else:
            label = f"if{number} type {etype} code {code} value {value}"
        non_grid = non_grid_input_for(etype, code, value)
        if non_grid is not None:
            self.non_grid_seen[non_grid] += 1
        self.evdev_counts[path] = self.evdev_counts.get(path, 0) + 1
        self.evdev_recent.append(label)
        del self.evdev_recent[:-6]
        self.emit(
            {
                "kind": "evdev",
                "interface": number,
                "type": etype,
                "code": code,
                "value": value,
                "name": EVDEV_NAMES.get(code) if etype == EV_KEY else None,
                "input": non_grid,
            }
        )
        self._dirty = True

    # -- loop ---------------------------------------------------------------

    def run(self, deadline=None):
        """Poll until ESC, Ctrl-C, `deadline` passes, the observer finishes, or
        the device vanishes. `deadline` is an absolute `time.monotonic()`, so a
        run that re-attaches across a power cycle keeps one overall budget."""
        poller = select.poll()
        poller.register(self.analog_fd, select.POLLIN)
        for fd in self.evdev_fds:
            poller.register(fd, select.POLLIN)
        stdin_fd = sys.stdin.fileno() if sys.stdin.isatty() else None
        if stdin_fd is not None:
            poller.register(stdin_fd, select.POLLIN)

        while True:
            if deadline is not None and time.monotonic() >= deadline:
                return "duration elapsed"
            if self.observer is not None and self.observer.done():
                return "procedure complete"
            if self.expired_escape():
                return "ESC"
            timeout = 200 if deadline is None else min(
                200, max(0, (deadline - time.monotonic()) * 1000)
            )
            for fd, event in poller.poll(timeout):
                if event & (select.POLLERR | select.POLLHUP | select.POLLNVAL):
                    if fd == self.analog_fd or fd in self.evdev_fds:
                        self.on_device_lost()
                        return "device disappeared"
                if not event & select.POLLIN:
                    continue
                if fd == stdin_fd:
                    commands = self.read_stdin(fd)
                    if "quit" in commands:
                        return "ESC"
                    if "mode" in commands:
                        self.read_device_mode()
                elif fd == self.analog_fd:
                    if not self.handle_analog():
                        self.on_device_lost()
                        return "device disappeared"
                elif fd in self.evdev_fds:
                    if not self.handle_evdev(fd):
                        self.on_device_lost()
                        return "device disappeared"
            self.draw()

    def on_device_lost(self):
        self.device_lost_at = time.monotonic() - self.started
        self.log("*** DEVICE LEFT THE BUS (unplug, suspend, or the §5 reset) ***")

    def read_device_mode(self):
        """Log the current `device_mode` — the only reliable way to tell which
        mode the device came back in after a power cycle or a suspend."""
        state = openrazer_state(self.interfaces)
        mode = state.get("device_mode")
        self.log(f"device_mode = {mode}   (00 00 = normal, 03 00 = driver)")
        self.emit({"kind": "mode", "device_mode": mode})
        return mode

    def reattach(self, timeout=60.0):
        """Wait for the device to come back on the bus and re-open both channels.

        hidraw node numbers are not stable across a re-enumeration, so this
        re-runs discovery rather than reusing the old paths. Stats accumulated
        before the disconnect are deliberately kept: the whole point is to
        compare one process's before and after."""
        self.log(f"waiting up to {timeout:.0f}s for the device to come back...")
        self.waiting_until = time.monotonic() + timeout
        try:
            while time.monotonic() < self.waiting_until:
                self.draw(force=True)
                interfaces = discover()
                if {ANALOG_INTERFACE, CONTROL_INTERFACE} <= set(interfaces):
                    time.sleep(1.0)  # let OpenRazer bind before querying device_mode
                    self.interfaces = discover()
                    self.evdev_counts = {}
                    self.open_analog()
                    self.open_evdev()
                    self.log(f"re-attached after {time.monotonic() - self.started:.1f}s")
                    self.read_device_mode()
                    self.begin_phase(f"after reconnect #{self.attachments - 1}")
                    return True
                time.sleep(0.5)
            self.log("device did NOT come back")
        except KeyboardInterrupt:
            self.log("gave up waiting (Ctrl-C)")
        except OSError as exc:
            self.log(f"re-open failed: {exc}")
        finally:
            self.waiting_until = None
        return False

    # -- phases -------------------------------------------------------------

    def begin_phase(self, label):
        """Snapshot the counters so the summary can report per-phase deltas.

        Every question ticket 16 asks is a before/after comparison inside one
        process — before and after a power cycle, a suspend, a re-lock — so the
        running totals alone would answer none of them."""
        self.phases.append(
            {
                "label": label,
                "at": round(time.monotonic() - self.started, 3),
                "analog": self.analog_reports,
                "grid": self.grid_keycodes_seen,
                "non_grid": dict(self.non_grid_seen),
            }
        )
        self.log(f"--- phase: {label} ---")
        self.emit({"kind": "phase", "label": label})

    def phase_deltas(self):
        """[(label, analog reports, grid presses, {input: count}) per phase]."""
        out = []
        boundaries = self.phases + [
            {
                "analog": self.analog_reports,
                "grid": self.grid_keycodes_seen,
                "non_grid": dict(self.non_grid_seen),
            }
        ]
        for start, end in zip(boundaries, boundaries[1:]):
            out.append(
                (
                    start["label"],
                    end["analog"] - start["analog"],
                    end["grid"] - start["grid"],
                    {
                        name: end["non_grid"][name] - start["non_grid"][name]
                        for name, _, _ in NON_GRID_INPUTS
                    },
                )
            )
        return out

    # -- rendering ----------------------------------------------------------

    def draw(self, force=False):
        if not self.ui:
            return
        now = time.monotonic()
        if not force and (not self._dirty or now - self._last_draw < 0.04):
            return
        self._dirty = False
        self._last_draw = now
        sys.stdout.write("\x1b[H\x1b[J" + self.render())
        sys.stdout.flush()

    def movement(self):
        """(every depth seen anywhere, [(byte index, stats) that ever left 0]).

        The two headline numbers both come from here: how many distinct depth
        values the device produced at all, and which keycaps actually moved.
        """
        distinct = set()
        for key in self.keys:
            distinct |= key.values
        touched = [(i + 1, k) for i, k in enumerate(self.keys) if k.values - {0}]
        return distinct, touched

    def non_grid_cells(self):
        """One cell per non-grid Input, counted since the current phase began.

        Counting since the phase (not since the process started) is what makes
        the panel readable while it is running: after a phase mark it is empty
        again, so the human presses each one and watches them light up.
        """
        base = self.phases[-1]["non_grid"] if self.phases else None
        cells = []
        for name, _, _ in NON_GRID_INPUTS:
            count = self.non_grid_seen[name] - (base[name] if base else 0)
            cells.append(f"{'OK ' if count else '.. '}{name} {count}")
        return cells

    def render(self):
        distinct, touched = self.movement()

        out = []
        if self.waiting_until is not None:
            # Loud, and at the top: the first live run of this looked to the
            # human like the tool had simply exited.
            left = max(0.0, self.waiting_until - time.monotonic())
            out.append("  " + "*" * 68)
            out.append(f"  *  DEVICE IS OFF THE BUS — waiting {left:3.0f}s more for it to return")
            out.append("  *  plug it back in (or resume from suspend) — this re-attaches itself")
            out.append("  *  and keeps this session's counters, so before/after compare directly")
            out.append("  " + "*" * 68)
            out.append("")
        out.append(
            f"  ANALOG CAPTURE   elapsed {time.monotonic() - self.started:6.1f}s"
            f"   reports 0x06: {self.analog_reports}"
        )
        out.append(
            f"  DISTINCT DEPTHS ACROSS ALL KEYS: {len(distinct)}"
            "   (2 = on/off only; many = real analog)"
        )
        out.append(
            f"  GRID KEYCODES ON EVDEV SINCE START: {self.grid_keycodes_seen}"
            "   (0 after unlock = driver mode silences them, §6)"
        )
        out.append("")
        out.append("  Depth per keycap (hypothesised report-byte -> key mapping):")
        for row in range(4):
            cells = []
            for col in range(5):
                index = row * 5 + col
                stats = self.keys[index]
                filled = round(stats.current / 255 * 6)
                bar = "█" * filled + "░" * (6 - filled)
                cells.append(f"{index + 1:02d} {bar} {stats.current:3d}")
            out.append("   " + "  ".join(cells))
        out.append("")

        if touched:
            out.append("  Keycaps that moved:")
            out.append("    byte  key                   min  max  distinct  intermediate")
            for index, stats in touched:
                out.append(
                    f"    {index:02d}    {keycap_label(index)[3:]:<20}  "
                    f"{stats.min:3d}  {stats.max:3d}  {len(stats.values):8d}  "
                    f"{stats.intermediate:12d}"
                )
            out.append("")

        if self.report_counts:
            counts = "  ".join(
                f"0x{rid:02x}:{n}" for rid, n in sorted(self.report_counts.items())
            )
            out.append(f"  Report IDs seen on interface 1: {counts}")
        if self.spare_bytes is not None:
            out.append(
                "  Trailing bytes 21-23 (documented spare): "
                + " ".join(f"{b:02x}" for b in self.spare_bytes)
            )
        if self.last_report is not None:
            out.append("  Last raw report: " + self.last_report.hex(" "))
        out.append("")

        out.append("  evdev (same keypress, other channel):")
        for path, count in sorted(self.evdev_counts.items()):
            out.append(f"    {path:<24} {count} events")
        for line in self.evdev_recent:
            out.append(f"      {line}")
        out.append("")

        out.append("  NON-GRID INPUTS — report 0x06 has no channel for any of these:")
        out.append("   " + "  ".join(self.non_grid_cells()))
        out.append("")

        if self.observer is not None:
            out.extend(self.observer.panel())
            out.append("")

        if self.anomalies:
            out.append("  DEVIATIONS FROM THE DOCUMENTED LAYOUT:")
            for line in self.anomalies:
                out.append(f"    - {line}")
            out.append("")

        out.append("  Log:")
        for line in self.log_lines[-8:]:
            out.append(f"    {line}")
        out.append("")
        out.append("  Press a grid key harder/softer. ESC or Ctrl-C to stop.")
        return "\n".join(out) + "\n"

    def summary(self):
        distinct, touched = self.movement()
        lines = [
            "",
            "=== RESULT ===",
            f"report 0x06 count           : {self.analog_reports}",
            f"first 0x06 arrived after    : "
            + ("never" if self.first_report_at is None else f"{self.first_report_at:.3f}s"),
            f"distinct depths, all keys   : {len(distinct)}",
            f"keycaps that moved          : {len(touched)}",
            f"grid keycodes seen on evdev : {self.grid_keycodes_seen}",
            f"report IDs on interface 1   : "
            + (
                "  ".join(f"0x{r:02x}:{n}" for r, n in sorted(self.report_counts.items()))
                or "none"
            ),
            f"device left the bus         : "
            + ("no" if self.device_lost_at is None else f"yes, at {self.device_lost_at:.3f}s"),
        ]
        for index, stats in touched:
            lines.append(
                f"  byte {index:02d} {keycap_label(index)[3:]:<20} "
                f"min {stats.min:3d} max {stats.max:3d} "
                f"distinct {len(stats.values):3d} intermediate {stats.intermediate:3d}"
            )

        deltas = self.phase_deltas()
        if deltas:
            width = max(12, max(len(label) for label, *_ in deltas) + 2)
            lines.append("")
            lines.append(
                "events per phase — a 0 only means silenced if it was actually pressed:"
            )
            lines.append("  " + f"{'channel':<20}" + "".join(f"{l:>{width}}" for l, *_ in deltas))

            def row(name, values):
                lines.append("  " + f"{name:<20}" + "".join(f"{v:>{width}}" for v in values))

            row("report 0x06", [a for _, a, _, _ in deltas])
            row("grid keycodes", [g for _, _, g, _ in deltas])
            for name, _, _ in NON_GRID_INPUTS:
                row(name, [ng[name] for *_, ng in deltas])

        if self.observer is not None:
            lines.append("")
            lines.extend(self.observer.summary())
        if self.anomalies:
            lines.append("deviations:")
            lines.extend(f"  - {a}" for a in self.anomalies)
        return "\n".join(lines)


# ---------------------------------------------------------------------------
# Guided byte -> keycap confirmation (ticket 16)
# ---------------------------------------------------------------------------

# A fixed permutation of the 20 keycaps with no fixed point and no adjacent
# pair, checked by `selftest`. Ticket 13 inferred `byte n = keycap n` from two
# runs where the keys happened to be pressed in layout order — which is exactly
# the ordering under which a non-identity mapping would still look monotonic.
# Pressing them in this order instead makes the inference falsifiable.
MAPPING_ORDER = [13, 20, 14, 17, 9, 1, 15, 6, 11, 19, 2, 4, 8, 5, 12, 10, 3, 7, 16, 18]

PRESS_DEPTH = 40  # a keycap is "pressed" once its depth passes this
RELEASE_DEPTH = 5  # ...and released once every depth is back under this


class MappingObserver:
    """Walks the 20 keycaps in `MAPPING_ORDER`, recording which byte moved.

    One keycap at a time: a prompt stays up until exactly one byte index rises
    past `PRESS_DEPTH` and everything falls back under `RELEASE_DEPTH`. Two
    bytes moving at once means two keys were touched, which proves nothing —
    so that attempt is discarded and the same keycap is prompted again.
    """

    def __init__(self):
        self.pending = list(MAPPING_ORDER)
        self.observed = {}
        self.moved = set()
        self.holding = False
        self.note = ""
        self.retries = 0

    @property
    def target(self):
        return self.pending[0] if self.pending else None

    def done(self):
        return not self.pending

    def announce(self, monitor):
        """Log the current prompt, so `--no-ui` runs are still followable."""
        if self.target is not None:
            monitor.log(
                f"press and release keycap {self.target:02d} "
                f"({keycap_label(self.target)[3:]})  [{len(self.observed)}/20 done]"
            )

    def on_depths(self, depths, monitor):
        if self.done():
            return
        above = {i + 1 for i, depth in enumerate(depths) if depth >= PRESS_DEPTH}
        if above:
            self.holding = True
            self.moved |= above
        elif self.holding and all(depth <= RELEASE_DEPTH for depth in depths):
            self.resolve(monitor)

    def resolve(self, monitor):
        target, moved = self.target, sorted(self.moved)
        self.holding, self.moved = False, set()
        if len(moved) != 1:
            self.retries += 1
            self.note = f"{len(moved)} bytes moved at once ({moved}) — one key at a time"
            monitor.log(f"keycap {target:02d}: ambiguous, retrying ({self.note})")
            return
        byte = moved[0]
        clash = next((k for k, b in self.observed.items() if b == byte), None)
        self.observed[target] = byte
        self.pending.pop(0)
        verdict = "identity" if byte == target else "*** NOT THE IDENTITY ***"
        if clash is not None:
            verdict += f" *** byte {byte:02d} already claimed by keycap {clash:02d} ***"
        self.note = ""
        monitor.log(f"keycap {target:02d} -> report byte {byte:02d}   {verdict}")
        monitor.emit({"kind": "mapping", "keycap": target, "byte": byte})
        self.announce(monitor)

    def cell(self, keycap):
        if keycap == self.target:
            return f"[{keycap:02d}]   "
        if keycap in self.observed:
            byte = self.observed[keycap]
            return f" {keycap:02d}>{byte:02d}" + (" " if byte == keycap else "!")
        return f" {keycap:02d} .. "

    def panel(self):
        lines = ["  BYTE -> KEYCAP, in a deliberately non-reading order:"]
        for row in range(4):
            lines.append("   " + " ".join(self.cell(row * 5 + col + 1) for col in range(5)))
        lines.append("   (keycap numbers are layout.md's; 20 is the thumb key)")
        if self.target is not None:
            lines.append(
                f"  >>> press and release keycap {self.target:02d} "
                f"({keycap_label(self.target)[3:]}) — {len(self.observed)}/20 done"
            )
        else:
            lines.append("  >>> all 20 recorded — ESC to finish")
        if self.note:
            lines.append(f"      {self.note}")
        return lines

    def summary(self):
        lines = [f"byte -> keycap mapping ({len(self.observed)}/20 recorded, "
                 f"{self.retries} ambiguous attempts discarded):"]
        mismatches = [(k, b) for k, b in sorted(self.observed.items()) if k != b]
        for keycap, byte in sorted(self.observed.items()):
            flag = "" if keycap == byte else "   <- NOT the identity"
            lines.append(f"  keycap {keycap:02d} {keycap_label(keycap)[3:]:<20} byte {byte:02d}{flag}")
        if len(self.observed) < NUM_KEYS:
            lines.append("  INCOMPLETE — not every keycap was recorded, verdict withheld")
        elif len(set(self.observed.values())) != len(self.observed):
            # Checked before the mismatch verdict: a duplicate means a keycap was
            # mis-pressed or a byte serves two keys, and either way the run is
            # void rather than evidence of a non-identity mapping.
            lines.append("  VERDICT: VOID — two keycaps resolved to the same byte, re-run")
        elif mismatches:
            lines.append(f"  VERDICT: NOT the identity — {len(mismatches)} keycap(s) differ")
        else:
            lines.append("  VERDICT: byte n == keycap n, confirmed per-key out of order")
        return lines


class RawTerminal:
    """Stop the device's own keystrokes echoing over the live view.

    Grid keys type '1','q','a','z'... into whatever terminal this runs in. Turn
    off ECHO/ICANON for the duration so the display stays readable, and so a
    single ESC read can end the run.
    """

    def __init__(self):
        self.fd = None
        self.saved = None

    def __enter__(self):
        if sys.stdin.isatty():
            self.fd = sys.stdin.fileno()
            self.saved = termios.tcgetattr(self.fd)
            new = termios.tcgetattr(self.fd)
            new[3] &= ~(termios.ECHO | termios.ICANON)
            new[6][termios.VMIN] = 0
            new[6][termios.VTIME] = 0
            termios.tcsetattr(self.fd, termios.TCSANOW, new)
        return self

    def __exit__(self, *exc):
        if self.saved is not None:
            termios.tcflush(self.fd, termios.TCIFLUSH)
            termios.tcsetattr(self.fd, termios.TCSANOW, self.saved)
        return False


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


def require_device():
    interfaces = discover()
    missing = {ANALOG_INTERFACE, CONTROL_INTERFACE} - set(interfaces)
    if missing:
        sys.exit(
            f"Tartarus Pro ({VENDOR_ID}:{PRODUCT_ID}) interfaces {sorted(missing)} not found. "
            "Is it plugged in?"
        )
    return interfaces


def print_state(interfaces, heading):
    state = openrazer_state(interfaces)
    print(f"{heading}:")
    for number, interface in sorted(interfaces.items()):
        events = ", ".join(interface.event_nodes) or "none"
        print(f"  interface {number}: {interface.hidraw}  {interface.hid_id}  evdev: {events}")
    if state:
        print(f"  device_mode      : {state.get('device_mode')}   (00 00 = normal)")
        print(f"  firmware_version : {state.get('firmware_version')}")
        print(f"  device_serial    : {state.get('device_serial')}")
    else:
        print("  (OpenRazer sysfs attributes unavailable — no device_mode readback)")
    return state


def cmd_selftest(_args):
    """Check every protocol constant against a value the research derived
    independently. Runs without the device and without root."""
    checks = []

    def check(label, actual, expected):
        ok = actual == expected
        checks.append((ok, label, actual, expected))

    check("HIDIOCSFEATURE(91)", hex(hidiocsfeature(91)), "0xc05b4806")
    check("unlock length", len(UNLOCK_CMD), 91)
    check("unlock report number", UNLOCK_CMD[0], 0x00)
    check("unlock transaction_id", UNLOCK_CMD[2], 0x01)
    check("unlock data_size", UNLOCK_CMD[6], 0x02)
    check("unlock command_class", UNLOCK_CMD[7], 0x00)
    check("unlock command_id", UNLOCK_CMD[8], 0x04)
    check("unlock arguments[0]", UNLOCK_CMD[9], 0x03)
    # Hand-computed in research §2: the only non-zero bytes in the CRC range
    # are data_size, command_class, command_id and arguments[0..2].
    check("unlock crc", UNLOCK_CMD[89], 0x02 ^ 0x04 ^ 0x03)
    check("unlock crc == 0x05", UNLOCK_CMD[89], 0x05)
    check("relock arguments[0]", RELOCK_CMD[9], 0x00)
    check("relock crc == 0x06", RELOCK_CMD[89], 0x06)
    check("crc excludes transaction_id", build_razer_cmd(0xFF, 0x00, 0x04, b"\x03\x00")[89], 0x05)
    # Pins the CRC range's far end: with 80 arguments the last one lands on
    # buffer index 88, the final byte inside the range. Both our real commands
    # leave that byte zero, so nothing else here would catch an off-by-one.
    check(
        "crc includes buf[88]",
        build_razer_cmd(0x01, 0x00, 0x04, bytes(79) + b"\x01")[89],
        0x50 ^ 0x04 ^ 0x01,
    )
    check("only byte 2 differs by txn", build_razer_cmd(0xFF, 0x00, 0x04, b"\x03\x00")[3:], UNLOCK_CMD[3:])
    check("everything else is zero", set(UNLOCK_CMD) - {0x00, 0x01, 0x02, 0x03, 0x04, 0x05}, set())
    check("input_event size", INPUT_EVENT.size, 24)
    check("keycap 1 label", keycap_label(1), "01 r1c1 KEY_1")
    check("keycap 20 label", keycap_label(20), "20 r4c5 KEY_SPACE")
    check("grid key codes", len(GRID_KEY_CODES), 20)
    # The non-grid Inputs must be exactly daemon/src/input.rs's Input enum minus
    # the 20 Grid variants — miss one and the invalidating question goes unasked.
    check(
        "non-grid Inputs",
        [name for name, _, _ in NON_GRID_INPUTS],
        ["mode_key", "thumbstick_up", "thumbstick_down", "thumbstick_left",
         "thumbstick_right", "wheel_middle", "wheel_scroll_up", "wheel_scroll_down"],
    )
    check("mode key is KEY_LEFTALT", non_grid_input_for(EV_KEY, 56, 1), "mode_key")
    check("release is not a press", non_grid_input_for(EV_KEY, 56, 0), None)
    check("wheel up is REL_WHEEL +", non_grid_input_for(EV_REL, REL_WHEEL, 1), "wheel_scroll_up")
    check("wheel down is REL_WHEEL -", non_grid_input_for(EV_REL, REL_WHEEL, -1), "wheel_scroll_down")
    check("grid keys are not non-grid", non_grid_input_for(EV_KEY, 30, 1), None)
    # The thumbstick's arrow keys reach the tty as escape sequences. Quitting on
    # them aborted the first live run of this ticket, one Input in.
    for arrow, final in (("up", b"A"), ("down", b"B"), ("right", b"C"), ("left", b"D")):
        check(f"thumbstick {arrow} is not ESC", stdin_commands(b"\x1b[" + final), set())
    check("SS3 arrows are not ESC either", stdin_commands(b"\x1bOD"), set())
    check("grid keys type harmlessly", stdin_commands(b"1qaz \t"), set())
    check("a lone ESC still quits", stdin_commands(b"\x1b"), {"quit"})
    check("ESC after an arrow quits", stdin_commands(b"\x1b[D\x1b"), {"quit"})
    check("'m' reads device_mode", stdin_commands(b"m"), {"mode"})
    # A mapping order that shares a fixed point with reading order would let the
    # identity hypothesis pass on a key it never actually tested.
    check("mapping order is a permutation", sorted(MAPPING_ORDER), list(range(1, 21)))
    check(
        "mapping order has no fixed point",
        [n for i, n in enumerate(MAPPING_ORDER) if n == i + 1],
        [],
    )
    check(
        "mapping order is not adjacent",
        [n for i, n in enumerate(MAPPING_ORDER) if abs(n - (i + 1)) == 1],
        [],
    )

    width = max(len(label) for _, label, _, _ in checks)
    for ok, label, actual, expected in checks:
        mark = "ok  " if ok else "FAIL"
        detail = f"{actual!r}" if ok else f"{actual!r} != expected {expected!r}"
        print(f"  {mark} {label:<{width}}  {detail}")
    failed = sum(1 for ok, *_ in checks if not ok)
    print(f"\n{len(checks) - failed}/{len(checks)} checks passed")
    if failed:
        sys.exit(f"{failed} check(s) FAILED — do not send anything to the device")
    print("\nUnlock buffer (would be sent verbatim to interface 2):")
    print("  " + UNLOCK_CMD.hex(" "))
    return 0


def cmd_probe(_args):
    """Read-only pre-state. Sends nothing to the device (research §7 step 1)."""
    interfaces = require_device()
    print_state(interfaces, "Tartarus Pro")
    analog = interfaces[ANALOG_INTERFACE]
    print(f"\n  interface 1 report descriptor: {analog.sysfs_hid / 'report_descriptor'}")
    raw = read_bytes_attr(analog.sysfs_hid / "report_descriptor")
    if raw is None:
        print("    unreadable")
    else:
        # The kernel drops any report whose ID isn't in the parsed descriptor,
        # so 0x85 0x06 ("Report ID 6") being present is a precondition (§1.2).
        declared = bytes([0x85, ANALOG_REPORT_ID]) in raw
        print(
            f"    {len(raw)} bytes; report id 0x06 declared: "
            f"{'yes' if declared else 'NO — the kernel would drop the stream'}"
        )
    for path in (interfaces[ANALOG_INTERFACE].hidraw, interfaces[CONTROL_INTERFACE].hidraw):
        try:
            os.close(os.open(path, os.O_RDONLY))
            print(f"    {path} readable")
        except OSError as exc:
            print(f"    {path} NOT readable: {exc} — rerun under sudo")
    return 0


def run_monitor(interfaces, args, before_loop=None, observer=None, phase="initial"):
    """Open both channels, run the live view, print the verdict.

    With `--survive`, a device that leaves the bus is not the end of the run:
    the monitor re-discovers it, re-attaches, and starts a new phase, so one
    process spans the whole unplug/replug or suspend/resume cycle.
    """
    record = open(args.record, "a") if args.record else None
    monitor = Monitor(interfaces, record=record, ui=not args.no_ui, observer=observer)
    deadline = None if args.duration is None else time.monotonic() + args.duration
    survive = getattr(args, "survive", False)
    try:
        with RawTerminal():
            monitor.open_analog()  # always before any write to the device
            monitor.open_evdev()
            monitor.begin_phase(phase)
            if before_loop is not None:
                before_loop(monitor)
            while True:
                monitor.draw(force=True)
                try:
                    reason = monitor.run(deadline=deadline)
                except KeyboardInterrupt:
                    reason = "Ctrl-C"
                monitor.close()
                if not (survive and reason == "device disappeared"):
                    break
                if not monitor.reattach():
                    reason = "device did not come back"
                    break
    finally:
        monitor.close()
        if record is not None:
            record.close()
    print(monitor.summary())
    print(f"\nstopped: {reason}")
    if monitor.device_lost_at is not None and not survive:
        report_reenumeration(interfaces)
    return monitor


def report_reenumeration(before):
    """After a reset, record whether it comes back and in which mode (§5)."""
    print("\nwaiting up to 20s for the device to re-enumerate...")
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        interfaces = discover()
        if {ANALOG_INTERFACE, CONTROL_INTERFACE} <= set(interfaces):
            time.sleep(1.0)  # let OpenRazer bind before querying it
            print_state(discover(), "came back as")
            old = {n: i.hidraw for n, i in before.items()}
            new = {n: i.hidraw for n, i in discover().items()}
            print(f"  hidraw nodes {'changed' if old != new else 'unchanged'}: {old} -> {new}")
            return
        time.sleep(0.5)
    print("  DID NOT COME BACK within 20s — replug the device")


def cmd_listen(args):
    """Read-only monitor of both channels, in whichever mode the device is in.

    The §7 step-1 baseline, and also how ticket 16 inspects state a *previous*
    process left behind: after a SIGKILL, after a power cycle, after a suspend.
    `device_mode` is printed above, so what the keys do can be read against it.
    """
    interfaces = require_device()
    print_state(interfaces, "Tartarus Pro")
    print("\nNothing will be sent to the device. Press every Input — grid keys, the Mode")
    print("key, all four thumbstick directions, the wheel up/down and the wheel click.\n")
    run_monitor(interfaces, args, phase="listen")
    return 0


def send_feature(interfaces, buffer, label):
    path = interfaces[CONTROL_INTERFACE].hidraw
    fd = os.open(path, os.O_RDWR)
    try:
        ioctl_unsigned(fd, hidiocsfeature(len(buffer)), buffer)
    finally:
        os.close(fd)
    return f"sent {label} to {path} via HIDIOCSFEATURE({len(buffer)})"


def confirm(word, args):
    if args.yes:
        return True
    print(f"\nType {word!r} to send it, anything else to abort: ", end="", flush=True)
    return sys.stdin.readline().strip() == word


def cmd_set_mode(args, buffer, mode_name, word):
    interfaces = require_device()
    print(f"About to send set-device-mode {mode_name} with transaction_id 0x01.")
    print(f"  target : {interfaces[CONTROL_INTERFACE].hidraw} (interface 2)")
    print(f"  ioctl  : HIDIOCSFEATURE(91) = {hidiocsfeature(91):#x}")
    print(f"  bytes  : {buffer.hex(' ')}")
    print(
        "\nRisk (research §5): this firmware is reported to reset — a USB re-enumeration,\n"
        "not a hang or a brick — on set-device-mode. The reconnect *loop* cannot form on\n"
        "this stack (kernel probe skips this device, daemon DRIVER_MODE=False), so the\n"
        "expected worst case is one reset. Watch `dmesg -w` in another terminal."
    )
    if args.dry_run:
        print("\n--dry-run: nothing sent.")
        return 0
    print_state(interfaces, "\nBefore")
    if not confirm(word, args):
        print("aborted.")
        return 1

    def send(monitor):
        monitor.log(send_feature(interfaces, buffer, f"set-device-mode {mode_name}"))

    monitor = run_monitor(interfaces, args, before_loop=send)
    if monitor.device_lost_at is None:
        print_state(discover(), "\nAfter")
    return 0


def cmd_unlock(args):
    return cmd_set_mode(args, UNLOCK_CMD, "0x03 (driver/analog)", "unlock")


def cmd_relock(args):
    return cmd_set_mode(args, RELOCK_CMD, "0x00 (normal)", "relock")


def cmd_mode(_args):
    print_state(require_device(), "Tartarus Pro")
    return 0


def cmd_mapping(args):
    """Confirm `byte n = keycap n` per key, out of order. Sends nothing.

    Requires the device to already be in driver mode — run `unlock` first and
    ESC out of it. That the mode outlives that process is itself worth noting.
    """
    interfaces = require_device()
    state = print_state(interfaces, "Tartarus Pro")
    if (state.get("device_mode") or "").split()[:1] != ["03"]:
        sys.exit(
            f"device_mode is {state.get('device_mode')!r}, not '03 00' — there is no analog\n"
            "stream to map. Run `unlock` first, ESC out of it, then re-run `mapping`."
        )
    print("\nPress each prompted keycap one at a time, firmly, then release it.")
    print("Nothing is sent to the device. ESC aborts; re-run to start over.\n")
    observer = MappingObserver()
    run_monitor(
        interfaces,
        args,
        before_loop=observer.announce,
        observer=observer,
        phase="mapping",
    )
    return 0


def cmd_survive(args):
    """Hold both channels across the device leaving the bus. Sends nothing.

    Covers the power-cycle and suspend/resume questions in one command: it
    keeps monitoring, re-attaches when the device returns, logs which mode it
    came back in, and reports the before/after event counts side by side.
    """
    args.survive = True
    interfaces = require_device()
    print_state(interfaces, "Tartarus Pro")
    print(
        "\nNothing will be sent to the device. Press every Input now to establish the\n"
        "'before' row, then unplug/replug it (or `systemctl suspend` from another\n"
        "terminal), and press them all again once it is back.\n"
        "ESC to finish; 'm' at any time re-reads device_mode into the log.\n"
    )
    run_monitor(interfaces, args, phase="before")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[1])
    sub = parser.add_subparsers(dest="command", required=True)

    def add(name, func, help_text, live=False, risky=False):
        p = sub.add_parser(name, help=help_text)
        p.set_defaults(func=func)
        if live:
            p.add_argument("--record", metavar="FILE", help="append JSONL capture log")
            p.add_argument("--duration", type=float, help="stop after N seconds")
            p.add_argument("--no-ui", action="store_true", help="plain log, no live view")
            p.add_argument(
                "--survive",
                action="store_true",
                help="re-attach instead of exiting if the device leaves the bus",
            )
        if risky:
            p.add_argument("--yes", action="store_true", help="skip the typed confirmation")
            p.add_argument("--dry-run", action="store_true", help="print the bytes, send nothing")
        return p

    add("selftest", cmd_selftest, "check protocol constants; no device, no root")
    add("probe", cmd_probe, "read-only pre-state (sends nothing)")
    add("mode", cmd_mode, "read device_mode and re-check discovery")
    add("listen", cmd_listen, "watch both channels, send nothing", live=True)
    add("mapping", cmd_mapping, "confirm byte -> keycap out of order", live=True)
    add("survive", cmd_survive, "hold across unplug/replug or suspend", live=True)
    add("unlock", cmd_unlock, "send mode 0x03, then watch (RISKY)", live=True, risky=True)
    add("relock", cmd_relock, "send mode 0x00, then watch (RISKY)", live=True, risky=True)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
