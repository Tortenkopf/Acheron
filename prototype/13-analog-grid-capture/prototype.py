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

Run (in the order of the research's §7 procedure):

    python3 prototype.py selftest              # no device, no root
    sudo python3 prototype.py probe            # read-only pre-state
    sudo python3 prototype.py listen           # baseline: normal mode
    sudo python3 prototype.py unlock --dry-run # print the bytes, send nothing
    sudo python3 prototype.py unlock           # THE RISKY STEP (§5)
    sudo python3 prototype.py relock           # also risky; not "cleanup"

`unlock`/`relock` prompt for typed confirmation unless given --yes. Press ESC
to leave the live view (not 'q' — 'q' is grid key 07). Add --record FILE to
append a JSONL log of every report and evdev event.
"""

import argparse
import errno
import fcntl
import json
import os
import pathlib
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
        hid_dir = (node / "device").resolve()
        usb_intf = hid_dir.parent
        usb_dev = usb_intf.parent
        if read_attr(usb_dev / "idVendor") != VENDOR_ID:
            continue
        if read_attr(usb_dev / "idProduct") != PRODUCT_ID:
            continue
        number_text = read_attr(usb_intf / "bInterfaceNumber")
        if number_text is None:
            continue
        events = sorted(
            str(p.name) for p in hid_dir.glob("input/input*/event*") if p.is_dir()
        )
        found[int(number_text, 16)] = Interface(
            number=int(number_text, 16),
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


def keycap_label(index):
    """1-based report byte index -> "NN r{row}c{col} KEY_X" under the hypothesis."""
    row, col = (index - 1) // 5 + 1, (index - 1) % 5 + 1
    return f"{index:02d} r{row}c{col} {GRID_KEY_NAMES[index - 1]}"


# ---------------------------------------------------------------------------
# Live view
# ---------------------------------------------------------------------------

EV_SYN, EV_KEY, EV_REL, EV_MSC, EV_LED = 0x00, 0x01, 0x02, 0x04, 0x11
# EV_MSC/MSC_SCAN fires alongside every key event and EV_LED is keyboard-LED
# chatter — both are pure noise for the one question here (does a grid key
# still emit a keycode?), so they never reach the display.
EV_IGNORED = (EV_SYN, EV_MSC, EV_LED)
INPUT_EVENT = struct.Struct("llHHi")  # timeval + type + code + value = 24 bytes


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

    def __init__(self, interfaces, record=None, ui=True):
        self.interfaces = interfaces
        self.record = record
        self.ui = ui
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
        self.evdev_counts[path] = self.evdev_counts.get(path, 0) + 1
        self.evdev_recent.append(label)
        del self.evdev_recent[:-6]
        self.emit(
            {"kind": "evdev", "interface": number, "type": etype, "code": code, "value": value}
        )
        self._dirty = True

    # -- loop ---------------------------------------------------------------

    def run(self, duration=None):
        """Poll until ESC, Ctrl-C, `duration` elapses, or the device vanishes."""
        poller = select.poll()
        poller.register(self.analog_fd, select.POLLIN)
        for fd in self.evdev_fds:
            poller.register(fd, select.POLLIN)
        stdin_fd = sys.stdin.fileno() if sys.stdin.isatty() else None
        if stdin_fd is not None:
            poller.register(stdin_fd, select.POLLIN)

        deadline = None if duration is None else time.monotonic() + duration
        while True:
            if deadline is not None and time.monotonic() >= deadline:
                return "duration elapsed"
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
                    if b"\x1b" in os.read(fd, 256):
                        return "ESC"
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
        self.log("*** DEVICE DISAPPEARED FROM THE BUS — this is the reset (§5) ***")

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

    def render(self):
        distinct, touched = self.movement()

        out = []
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
        if self.anomalies:
            lines.append("deviations:")
            lines.extend(f"  - {a}" for a in self.anomalies)
        return "\n".join(lines)


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


def run_monitor(interfaces, args, before_loop=None):
    record = open(args.record, "a") if args.record else None
    monitor = Monitor(interfaces, record=record, ui=not args.no_ui)
    try:
        monitor.open_analog()  # always before any write to the device
        monitor.open_evdev()
        if before_loop is not None:
            before_loop(monitor)
        with RawTerminal():
            monitor.draw(force=True)
            try:
                reason = monitor.run(duration=args.duration)
            except KeyboardInterrupt:
                reason = "Ctrl-C"
    finally:
        monitor.close()
        if record is not None:
            record.close()
    print(monitor.summary())
    print(f"\nstopped: {reason}")
    if monitor.device_lost_at is not None:
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
    """Baseline in normal mode: no writes, nothing unlocked (§7 step 1-3)."""
    interfaces = require_device()
    print_state(interfaces, "Tartarus Pro (normal mode baseline)")
    print("\nNothing will be sent to the device. Press grid keys: evdev keycodes")
    print("should appear and report 0x06 should NOT.\n")
    run_monitor(interfaces, args)
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
        if risky:
            p.add_argument("--yes", action="store_true", help="skip the typed confirmation")
            p.add_argument("--dry-run", action="store_true", help="print the bytes, send nothing")
        return p

    add("selftest", cmd_selftest, "check protocol constants; no device, no root")
    add("probe", cmd_probe, "read-only pre-state (sends nothing)")
    add("mode", cmd_mode, "read device_mode and re-check discovery")
    add("listen", cmd_listen, "watch both channels in normal mode", live=True)
    add("unlock", cmd_unlock, "send mode 0x03, then watch (RISKY)", live=True, risky=True)
    add("relock", cmd_relock, "send mode 0x00, then watch (RISKY)", live=True, risky=True)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
