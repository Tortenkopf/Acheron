#!/usr/bin/env python3
"""
PROTOTYPE — throwaway code, not production. Standalone: imports nothing from
`daemon/` or `gui/`, stdlib only, and writes nothing into Acheron's config.

Answers: can we actually drive the three side **Status LED**s (orange / green /
blue, on/off only) on the real, connected Razer Tartarus Pro from Linux, via the
extended-matrix LED-effect frame documented in
`.scratch/tartarus-status-leds/research/tartarus-pro-status-leds.md` §3
(`command_class 0x0F`, `command_id 0x02` write / `0x82` read, LED ID `0x0B`,
effect = static, `VARSTORE`; channel bytes at args 6/7/8)?

Ticket:   .scratch/tartarus-status-leds/issues/01-prototype-status-led-controllability.md
Research:  .scratch/tartarus-status-leds/research/tartarus-pro-status-leds.md
Modelled on: prototype/13-analog-grid-capture/prototype.py (verified `hidraw`
discovery / CRC / HIDIOCSFEATURE against this same unit in tickets 13/16).

This is the effort's **KILL-GATE**. It is HITL — it needs the physical device and
a human watching the three side LEDs in real time. A written negative result on
criterion 1 (below) archives the whole effort.

The five criteria the ticket asks, and the command that settles each:

  1. Independent control — does each LED turn on and off independently, and do
     all combinations light correctly?
       -> `sequence` walks all 8 on/off combinations with a visual-confirm
          prompt between each. This is the kill-gate check.
  2. Read-back — does the `0x82` read frame report the channel values just set?
       -> every write command also issues a `0x82` read and prints args 6/7/8;
          `readback` does it on its own.
  3. On-device keymap switch — does the firmware re-assert its own
     keymap-indicator code and clobber a host-set state?
       -> `keymap-switch`: set all three, prompt for the on-device combo, read
          back + look again. Feeds the "re-assert hook" fog item.
  4. No adverse behaviour — no reset / re-enumeration across repeated LED writes
     (cross-check research §6's PR #2710 driver-mode caution on our own unit).
       -> after every feature write, discovery is re-run and any change to the
          device's presence / hidraw nodes is logged loudly. Watch `dmesg -w`.
  5. Is all-off reachable — does `(0, 0, 0)` actually leave all three dark, or
     does the firmware force at least one on?
       -> `off-test` (also the first and last step of `sequence`). Decides the
          shutdown-clear fog item.

Byte-level ambiguities to note on this unit/firmware (coordinate with ticket 02,
which runs the same ground from the source side):
  - `arg3` / `arg4`: CommandPost sends `00 01`, OpenRazer's helper `00 00`.
    Default here is CommandPost's (`--arg3` / `--arg4` to override).
  - `--txn`: research §3 puts the lighting frame on `transaction_id 0x1F`
    (Tartarus-Pro lighting id); `0x01` (Synapse capture) and `0xFF` (OpenRazer
    generic) are the alternatives to try if `0x1F` is ACKed-but-ignored.
  - `--effect-none`: whether `effect` id `0x00` / `data_size 0x06` is a cleaner
    "off" than a static frame with zero channels.
  - `--no-driver-mode`: whether the LED frame works at all without first
    entering driver/streaming mode (`0x00 0x04`, arg `0x03`).

Run (roughly in this order):

    python3 prototype.py selftest                 # no device, no root
    sudo python3 prototype.py probe               # read-only pre-state
    sudo python3 prototype.py set 010             # free-play: green only, once
    sudo python3 prototype.py readback            # just the 0x82 read
    sudo python3 prototype.py sequence --record .scratch/tartarus-status-leds/assets/01-sequence.jsonl
    sudo python3 prototype.py keymap-switch --record .scratch/tartarus-status-leds/assets/01-keymap.jsonl
    sudo python3 prototype.py off-test --record .scratch/tartarus-status-leds/assets/01-off.jsonl
    sudo python3 prototype.py relock             # restore normal mode (also a write)

`set` / `sequence` / `keymap-switch` / `off-test` enter driver mode first (unless
`--no-driver-mode`) and prompt for typed confirmation once (unless `--yes`),
exactly as prototype 13's `unlock` does — same risk, same unit.
"""

import argparse
import fcntl
import json
import os
import pathlib
import sys
import time

VENDOR_ID = "1532"
PRODUCT_ID = "0244"

CONTROL_INTERFACE = 2  # bInterfaceNumber that takes the razer_report feature report

RAZER_CMD_LEN = 91  # report-number byte + 90-byte razer_report

# ---------------------------------------------------------------------------
# ioctl numbers — Linux `_IOC()` from asm-generic/ioctl.h (2/8/8/14-bit
# dir/type/nr/size). nr 0x06 = HIDIOCSFEATURE (write request), 0x07 =
# HIDIOCGFEATURE (read response). Cross-checked in `selftest` against the
# constants the daemon's analog.rs pins (0xC05B4806 / 0xC05B4807 at size 91).
# ---------------------------------------------------------------------------

_IOC_WRITE = 1
_IOC_READ = 2


def _ioc(direction, type_char, nr, size):
    return (direction << 30) | (size << 16) | (ord(type_char) << 8) | nr


def hidiocsfeature(size):
    return _ioc(_IOC_WRITE | _IOC_READ, "H", 0x06, size)


def hidiocgfeature(size):
    return _ioc(_IOC_WRITE | _IOC_READ, "H", 0x07, size)


def ioctl_unsigned(fd, request, arg):
    """`fcntl.ioctl` with the >=2^31 request numbers HIDIOC* produces."""
    try:
        return fcntl.ioctl(fd, request, arg, True)
    except OverflowError:
        return fcntl.ioctl(fd, request - (1 << 32), arg, True)


# ---------------------------------------------------------------------------
# Frame construction — research §2/§3. `build_razer_cmd` mirrors prototype 13's
# byte for byte: report-number byte at [0], then the 90-byte struct, so struct
# byte N sits at buffer index N+1. CRC is XOR of struct bytes 2..87 (buffer
# indices 3..88); transaction_id at struct byte 1 (index 2) is excluded.
# ---------------------------------------------------------------------------


def build_razer_cmd(txn, command_class, command_id, args):
    buf = bytearray(RAZER_CMD_LEN)
    buf[2] = txn
    buf[6] = len(args)
    buf[7] = command_class
    buf[8] = command_id
    buf[9 : 9 + len(args)] = bytes(args)
    crc = 0
    for b in buf[3:89]:
        crc ^= b
    buf[89] = crc
    return bytes(buf)


# --- driver / streaming mode (optional; same frame the daemon + prototype 13
#     use). transaction_id 0x01 here deliberately — NOT the lighting frame's
#     0x1F. research §6: this can reset *some* Pro units; ours survived it in
#     tickets 13/16 and ships it for analog capture.
DRIVER_TXN = 0x01
CMD_CLASS_STANDARD = 0x00
CMD_SET_DEVICE_MODE = 0x04
MODE_DRIVER = 0x03
MODE_NORMAL = 0x00

DRIVER_MODE_CMD = build_razer_cmd(DRIVER_TXN, CMD_CLASS_STANDARD, CMD_SET_DEVICE_MODE, [MODE_DRIVER, 0x00])
NORMAL_MODE_CMD = build_razer_cmd(DRIVER_TXN, CMD_CLASS_STANDARD, CMD_SET_DEVICE_MODE, [MODE_NORMAL, 0x00])

# --- the status-LED frame (research §3)
LED_CMD_CLASS = 0x0F
LED_WRITE_ID = 0x02
LED_READ_ID = 0x82
LED_ID_SIDE_STRIPE = 0x0B
EFFECT_STATIC = 0x01
EFFECT_NONE = 0x00
VARSTORE = 0x01
NOSTORE = 0x00

CHANNEL_NAMES = ("orange", "green", "blue")
ON = 0xFF
OFF = 0x00

# response-struct offsets inside the 91-byte HIDIOCGFEATURE buffer (analog.rs)
RESP_STATUS = 1
RESP_CMD_CLASS = 7
RESP_CMD_ID = 8
RESP_ARGS = 9


def led_frame(channels, *, txn, arg3, arg4, store, effect_none=False):
    """Build the LED write/read frame.

    `channels` is a 3-tuple of 0x00/0xFF for (orange, green, blue). A static
    frame carries 9 argument bytes; `effect_none` carries 6 (research §3, note:
    the 6-byte form is documented as "plausible but untested").
    """
    if effect_none:
        args = [store, LED_ID_SIDE_STRIPE, EFFECT_NONE, arg3, arg4, 0x01]
    else:
        args = [store, LED_ID_SIDE_STRIPE, EFFECT_STATIC, arg3, arg4, 0x01, *channels]
    return args


def led_write_cmd(channels, *, txn, arg3, arg4, store, effect_none=False):
    return build_razer_cmd(
        txn, LED_CMD_CLASS, LED_WRITE_ID,
        led_frame(channels, txn=txn, arg3=arg3, arg4=arg4, store=store, effect_none=effect_none),
    )


def led_read_cmd(*, txn, arg3, arg4, store):
    return build_razer_cmd(
        txn, LED_CMD_CLASS, LED_READ_ID,
        led_frame((0, 0, 0), txn=txn, arg3=arg3, arg4=arg4, store=store),
    )


def parse_channel_spec(spec):
    """'010' / 'green' / 'orange+blue' / 'off' / 'all' -> (o, g, b) of 0/0xFF."""
    spec = spec.strip().lower()
    if spec in ("off", "none", "000"):
        return (OFF, OFF, OFF)
    if spec in ("all", "on", "111"):
        return (ON, ON, ON)
    if len(spec) == 3 and set(spec) <= {"0", "1"}:
        return tuple(ON if c == "1" else OFF for c in spec)
    wanted = {p for p in spec.replace("+", " ").replace(",", " ").split() if p}
    unknown = wanted - set(CHANNEL_NAMES)
    if unknown:
        raise ValueError(f"unknown channel(s): {', '.join(sorted(unknown))}")
    return tuple(ON if name in wanted else OFF for name in CHANNEL_NAMES)


def describe(channels):
    lit = [name for name, v in zip(CHANNEL_NAMES, channels) if v]
    return " + ".join(lit) if lit else "(all dark)"


# ---------------------------------------------------------------------------
# Device discovery — research §3.1. Match on bInterfaceNumber via sysfs. hidraw
# node numbers are not stable across a reset, so we re-discover rather than
# cache paths.
# ---------------------------------------------------------------------------


def read_attr(path):
    try:
        return pathlib.Path(path).read_text().strip()
    except (OSError, ValueError):
        return None


def read_bytes_attr(path):
    try:
        return pathlib.Path(path).read_bytes()
    except OSError:
        return None


def discover():
    """{bInterfaceNumber: {'hidraw': str, 'hid_id': str, 'sysfs_hid': Path}}."""
    found = {}
    for node in sorted(pathlib.Path("/sys/class/hidraw").glob("hidraw*")):
        try:
            hid_dir = (node / "device").resolve()
            usb_intf = hid_dir.parent
            usb_dev = usb_intf.parent
            if read_attr(usb_dev / "idVendor") != VENDOR_ID:
                continue
            if read_attr(usb_dev / "idProduct") != PRODUCT_ID:
                continue
            number = int(read_attr(usb_intf / "bInterfaceNumber"), 16)
        except (OSError, TypeError, ValueError):
            continue
        found[number] = {
            "hidraw": f"/dev/{node.name}",
            "hid_id": hid_dir.name,
            "sysfs_hid": hid_dir,
        }
    return found


def openrazer_state(interfaces):
    ctrl = interfaces.get(CONTROL_INTERFACE)
    if ctrl is None:
        return {}
    mode = read_bytes_attr(ctrl["sysfs_hid"] / "device_mode")
    return {
        "device_serial": read_attr(ctrl["sysfs_hid"] / "device_serial"),
        "firmware_version": read_attr(ctrl["sysfs_hid"] / "firmware_version"),
        "device_mode": None if mode is None else " ".join(f"{b:02x}" for b in mode),
    }


def require_device():
    interfaces = discover()
    if CONTROL_INTERFACE not in interfaces:
        sys.exit(
            f"Tartarus Pro ({VENDOR_ID}:{PRODUCT_ID}) control interface "
            f"{CONTROL_INTERFACE} not found. Is it plugged in?"
        )
    return interfaces


# ---------------------------------------------------------------------------
# Recorder — appends a JSONL log of every frame, readback and observation, for
# the raw evidence the ticket wants under the map's assets/.
# ---------------------------------------------------------------------------


class Recorder:
    def __init__(self, path):
        self.started = time.monotonic()
        self.fh = None
        if path:
            p = pathlib.Path(path)
            p.parent.mkdir(parents=True, exist_ok=True)
            self.fh = open(p, "a")
            self.emit("session", argv=sys.argv[1:], time=time.strftime("%Y-%m-%dT%H:%M:%S%z"))

    def emit(self, kind, **fields):
        rec = dict(kind=kind, t=round(time.monotonic() - self.started, 6), **fields)
        if self.fh:
            self.fh.write(json.dumps(rec) + "\n")
            self.fh.flush()
        return rec

    def close(self):
        if self.fh:
            self.fh.close()


# ---------------------------------------------------------------------------
# I/O — one send, one readback, and the criterion-4 presence check.
# ---------------------------------------------------------------------------


def send_feature(interfaces, buffer, label, rec):
    path = interfaces[CONTROL_INTERFACE]["hidraw"]
    fd = os.open(path, os.O_RDWR)
    try:
        ioctl_unsigned(fd, hidiocsfeature(len(buffer)), bytearray(buffer))
    finally:
        os.close(fd)
    line = f"  sent {label}: {buffer.hex(' ')}"
    print(line)
    rec.emit("frame_sent", label=label, hex=buffer.hex())
    return line


def read_feature(interfaces, request, label, rec):
    """SET the 0x82 request, then GET the 91-byte response. Returns (o, g, b)
    channel values or None if the response didn't echo our request."""
    path = interfaces[CONTROL_INTERFACE]["hidraw"]
    fd = os.open(path, os.O_RDWR)
    try:
        ioctl_unsigned(fd, hidiocsfeature(len(request)), bytearray(request))
        resp = bytearray(RAZER_CMD_LEN)
        for delay in (0.001, 0.003, 0.010):
            time.sleep(delay)
            resp = bytearray(RAZER_CMD_LEN)
            ioctl_unsigned(fd, hidiocgfeature(RAZER_CMD_LEN), resp)
            echoes = (
                resp[RESP_CMD_CLASS] == LED_CMD_CLASS
                and resp[RESP_CMD_ID] in (LED_READ_ID, LED_WRITE_ID)
                and resp[RESP_STATUS] in (0x00, 0x01, 0x02)
            )
            if echoes:
                break
    finally:
        os.close(fd)
    channels = tuple(resp[RESP_ARGS + 6 : RESP_ARGS + 9])
    rec.emit(
        "readback", label=label, hex=bytes(resp).hex(),
        status=resp[RESP_STATUS], cmd_class=resp[RESP_CMD_CLASS], cmd_id=resp[RESP_CMD_ID],
        channels=list(channels), echoes=echoes,
    )
    verdict = "echoed OK" if echoes else "DID NOT echo our request"
    print(
        f"  read {label}: status={resp[RESP_STATUS]:#04x} class={resp[RESP_CMD_CLASS]:#04x} "
        f"id={resp[RESP_CMD_ID]:#04x} ({verdict})"
    )
    print(
        f"           channels: orange={channels[0]:#04x} green={channels[1]:#04x} "
        f"blue={channels[2]:#04x}   full resp: {bytes(resp).hex(' ')}"
    )
    return channels if echoes else None


def presence_check(interfaces, rec, note):
    """Criterion 4: re-discover after a write, log any change loudly."""
    now = discover()
    before_nodes = {n: i["hidraw"] for n, i in interfaces.items()}
    after_nodes = {n: i["hidraw"] for n, i in now.items()}
    present = CONTROL_INTERFACE in now
    changed = before_nodes != after_nodes
    rec.emit("presence", note=note, present=present, changed=changed,
             before=before_nodes, after=after_nodes)
    if not present:
        print(f"  *** DEVICE VANISHED after {note} — control interface gone from the bus ***")
    elif changed:
        print(f"  *** hidraw nodes CHANGED after {note} (re-enumeration): {before_nodes} -> {after_nodes} ***")
    else:
        print(f"  device still present, nodes unchanged ({note})")
    return now if present else interfaces


# ---------------------------------------------------------------------------
# Shared setup
# ---------------------------------------------------------------------------


def print_state(interfaces, heading):
    state = openrazer_state(interfaces)
    print(f"{heading}:")
    for number, iface in sorted(interfaces.items()):
        print(f"  interface {number}: {iface['hidraw']}  {iface['hid_id']}")
    if state:
        print(f"  device_mode      : {state.get('device_mode')}   (00 00 = normal, 03 00 = driver)")
        print(f"  firmware_version : {state.get('firmware_version')}")
        print(f"  device_serial    : {state.get('device_serial')}")
    else:
        print("  (OpenRazer sysfs attributes unavailable — no device_mode readback)")
    return state


def confirm(word, args):
    if getattr(args, "yes", False):
        return True
    reply = input(f"\nType {word!r} to proceed, anything else to abort: ").strip()
    return reply == word


def enter_driver_mode(interfaces, args, rec):
    """Send set-device-mode 0x03 unless --no-driver-mode. Returns interfaces
    (possibly re-discovered if the device re-enumerated)."""
    if getattr(args, "no_driver_mode", False):
        state = openrazer_state(interfaces).get("device_mode")
        print(
            f"\n--no-driver-mode: not sending set-device-mode. Current device_mode = {state}\n"
            "  (either a prior step already entered driver mode, or this run tests whether\n"
            "   the LED frame works without it — check the readback / your eyes)."
        )
        rec.emit("driver_mode", entered=False, device_mode=state)
        return interfaces
    print(
        "\nAbout to enter driver/streaming mode (set-device-mode 0x03, transaction_id 0x01)\n"
        f"  target : {interfaces[CONTROL_INTERFACE]['hidraw']} (interface 2)\n"
        f"  bytes  : {DRIVER_MODE_CMD.hex(' ')}\n"
        "\nRisk (research §6): some Tartarus Pro firmware resets (a USB re-enumeration, not a\n"
        "brick) on set-device-mode. Our unit survived this in tickets 13/16 and ships it for\n"
        "analog capture. Watch `dmesg -w` in another terminal."
    )
    if not confirm("driver", args):
        sys.exit("aborted.")
    send_feature(interfaces, DRIVER_MODE_CMD, "set-device-mode 0x03 (driver)", rec)
    rec.emit("driver_mode", entered=True)
    time.sleep(0.05)
    interfaces = presence_check(interfaces, rec, "driver-mode enter")
    print_state(interfaces, "\nAfter entering driver mode")
    return interfaces


def led_args(args):
    return dict(
        txn=args.txn, arg3=args.arg3, arg4=args.arg4,
        store=NOSTORE if getattr(args, "nostore", False) else VARSTORE,
    )


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


def cmd_selftest(_args):
    checks = []

    def check(label, actual, expected):
        checks.append((actual == expected, label, actual, expected))

    check("HIDIOCSFEATURE(91)", hex(hidiocsfeature(91)), "0xc05b4806")
    check("HIDIOCGFEATURE(91)", hex(hidiocgfeature(91)), "0xc05b4807")

    # driver-mode frame — identical to prototype 13 / analog.rs
    check("driver-mode length", len(DRIVER_MODE_CMD), 91)
    check("driver-mode report number", DRIVER_MODE_CMD[0], 0x00)
    check("driver-mode transaction_id", DRIVER_MODE_CMD[2], 0x01)
    check("driver-mode data_size", DRIVER_MODE_CMD[6], 0x02)
    check("driver-mode command_class", DRIVER_MODE_CMD[7], 0x00)
    check("driver-mode command_id", DRIVER_MODE_CMD[8], 0x04)
    check("driver-mode arguments[0]", DRIVER_MODE_CMD[9], 0x03)
    check("driver-mode crc == 0x05", DRIVER_MODE_CMD[89], 0x02 ^ 0x04 ^ 0x03)
    check("normal-mode crc == 0x06", NORMAL_MODE_CMD[89], 0x02 ^ 0x04 ^ 0x00)

    # LED write frame — research §3, "light the green LED only" worked example
    green = led_write_cmd((OFF, ON, OFF), txn=0x1F, arg3=0x00, arg4=0x01, store=VARSTORE)
    check("LED green length", len(green), 91)
    check("LED green report number", green[0], 0x00)
    check("LED green transaction_id", green[2], 0x1F)
    check("LED green data_size", green[6], 0x09)
    check("LED green command_class", green[7], 0x0F)
    check("LED green command_id", green[8], 0x02)
    check("LED green arg0 VARSTORE", green[9], 0x01)
    check("LED green arg1 LED ID", green[10], 0x0B)
    check("LED green arg2 effect=static", green[11], 0x01)
    check("LED green arg3", green[12], 0x00)
    check("LED green arg4 (CommandPost)", green[13], 0x01)
    check("LED green arg5", green[14], 0x01)
    check("LED green arg6 orange=off", green[15], 0x00)
    check("LED green arg7 green=on", green[16], 0xFF)
    check("LED green arg8 blue=off", green[17], 0x00)
    check("LED green trailing byte 18", green[18], 0x00)
    # CRC by hand: XOR of struct bytes 2..87 = XOR of the non-zero args only.
    hand_crc = 0x09 ^ 0x0F ^ 0x02 ^ 0x01 ^ 0x0B ^ 0x01 ^ 0x00 ^ 0x01 ^ 0x01 ^ 0x00 ^ 0xFF ^ 0x00
    check("LED green crc (hand-computed)", green[89], hand_crc)
    check("LED green crc == 0xf0", green[89], 0xF0)
    check("LED green reserved byte", green[90], 0x00)

    all_off = led_write_cmd((OFF, OFF, OFF), txn=0x1F, arg3=0x00, arg4=0x01, store=VARSTORE)
    check("LED all-off crc == 0x0f", all_off[89], 0x0F)
    check("LED all-off channels zero", tuple(all_off[15:18]), (0, 0, 0))

    all_on = led_write_cmd((ON, ON, ON), txn=0x1F, arg3=0x00, arg4=0x01, store=VARSTORE)
    check("LED all-on channels 0xff", tuple(all_on[15:18]), (0xFF, 0xFF, 0xFF))
    check("LED all-on crc", all_on[89], 0x0F ^ 0xFF ^ 0xFF ^ 0xFF)

    read = led_read_cmd(txn=0x1F, arg3=0x00, arg4=0x01, store=VARSTORE)
    check("LED read command_id", read[8], 0x82)
    check("LED read data_size", read[6], 0x09)
    check("LED read channels zero", tuple(read[15:18]), (0, 0, 0))

    none = led_write_cmd((OFF, OFF, OFF), txn=0x1F, arg3=0x00, arg4=0x01, store=VARSTORE, effect_none=True)
    check("effect-none data_size == 0x06", none[6], 0x06)
    check("effect-none arg2 == 0x00", none[11], 0x00)

    check("arg4 override reaches the frame", led_write_cmd((0, 0, 0), txn=0x1F, arg3=0x00, arg4=0x00, store=VARSTORE)[13], 0x00)
    check("txn override reaches the frame", led_write_cmd((0, 0, 0), txn=0xFF, arg3=0, arg4=1, store=VARSTORE)[2], 0xFF)
    check("nostore override reaches the frame", led_write_cmd((0, 0, 0), txn=0x1F, arg3=0, arg4=1, store=NOSTORE)[9], 0x00)

    # spec parser
    check("parse '010'", parse_channel_spec("010"), (OFF, ON, OFF))
    check("parse '101'", parse_channel_spec("101"), (ON, OFF, ON))
    check("parse 'green'", parse_channel_spec("green"), (OFF, ON, OFF))
    check("parse 'orange+blue'", parse_channel_spec("orange+blue"), (ON, OFF, ON))
    check("parse 'off'", parse_channel_spec("off"), (OFF, OFF, OFF))
    check("parse 'all'", parse_channel_spec("all"), (ON, ON, ON))
    check("describe (1,0,1)", describe((ON, OFF, ON)), "orange + blue")
    check("describe (0,0,0)", describe((OFF, OFF, OFF)), "(all dark)")

    width = max(len(label) for _, label, _, _ in checks)
    for ok, label, actual, expected in checks:
        mark = "ok  " if ok else "FAIL"
        detail = f"{actual!r}" if ok else f"{actual!r} != expected {expected!r}"
        print(f"  {mark} {label:<{width}}  {detail}")
    failed = sum(1 for ok, *_ in checks if not ok)
    print(f"\n{len(checks) - failed}/{len(checks)} checks passed")
    if failed:
        sys.exit(f"{failed} check(s) FAILED — do not send anything to the device")
    print("\nExample frames (would be sent verbatim to interface 2):")
    print(f"  driver mode : {DRIVER_MODE_CMD.hex(' ')}")
    print(f"  green only  : {green.hex(' ')}")
    print(f"  all off     : {all_off.hex(' ')}")
    print(f"  read (0x82) : {read.hex(' ')}")
    return 0


def cmd_probe(_args):
    interfaces = require_device()
    print_state(interfaces, "Tartarus Pro")
    ctrl = interfaces[CONTROL_INTERFACE]
    try:
        os.close(os.open(ctrl["hidraw"], os.O_RDWR))
        print(f"\n  {ctrl['hidraw']} is R/W-open-able")
    except OSError as exc:
        print(f"\n  {ctrl['hidraw']} NOT R/W-open-able: {exc} — rerun under sudo")
    print("\n  Nothing was sent to the device.")
    return 0


def cmd_set(args):
    channels = parse_channel_spec(args.spec)
    interfaces = require_device()
    rec = Recorder(args.record)
    try:
        print_state(interfaces, "Tartarus Pro")
        interfaces = enter_driver_mode(interfaces, args, rec)
        print(f"\nSetting Status LEDs -> {describe(channels)}")
        cmd = led_write_cmd(channels, effect_none=args.effect_none, **led_args(args))
        send_feature(interfaces, cmd, f"LED write {describe(channels)}", rec)
        interfaces = presence_check(interfaces, rec, "LED write")
        if not args.no_readback:
            read_feature(interfaces, led_read_cmd(**led_args(args)), "after write", rec)
        rec.emit("expectation", channels=list(channels), described=describe(channels))
        print(f"\n>>> Look at the device. Expected: {describe(channels)}")
    finally:
        rec.close()
    return 0


def cmd_readback(args):
    interfaces = require_device()
    rec = Recorder(args.record)
    try:
        print_state(interfaces, "Tartarus Pro")
        # readback alone does not require driver mode; try it as-is first
        channels = read_feature(interfaces, led_read_cmd(**led_args(args)), "readback", rec)
        if channels is None:
            print("\n  No echo. Try `set` first (which enters driver mode), or a different --txn.")
    finally:
        rec.close()
    return 0


SEQUENCE = [
    (OFF, OFF, OFF),
    (ON, OFF, OFF),
    (OFF, ON, OFF),
    (OFF, OFF, ON),
    (ON, ON, OFF),
    (ON, OFF, ON),
    (OFF, ON, ON),
    (ON, ON, ON),
    (OFF, OFF, OFF),
]


def observe(expected, rec, step):
    print(f"\n  >>> EXPECTED: {describe(expected)}")
    seen = input("  What do you actually SEE? ('match', or e.g. 'orange+green', 'none'): ").strip()
    matched = seen.lower() in ("match", "m", "yes", "y", "")
    rec.emit("observation", step=step, expected=describe(expected),
             expected_raw=list(expected), seen=seen, matched=matched)
    return seen, matched


def cmd_sequence(args):
    interfaces = require_device()
    rec = Recorder(args.record)
    results = []
    try:
        print_state(interfaces, "Tartarus Pro")
        interfaces = enter_driver_mode(interfaces, args, rec)
        print("\n=== CRITERION 1 + 5: walk all 8 on/off combinations ===")
        for i, channels in enumerate(SEQUENCE):
            print(f"\n--- step {i + 1}/{len(SEQUENCE)}: {describe(channels)} ---")
            cmd = led_write_cmd(channels, effect_none=False, **led_args(args))
            send_feature(interfaces, cmd, f"LED {describe(channels)}", rec)
            interfaces = presence_check(interfaces, rec, f"step {i + 1}")
            read = read_feature(interfaces, led_read_cmd(**led_args(args)), f"step {i + 1}", rec)
            seen, matched = observe(channels, rec, i + 1)
            results.append((describe(channels), read, seen, matched))
        print("\n=== SEQUENCE SUMMARY ===")
        print(f"  {'expected':<22} {'readback (o,g,b)':<20} {'seen':<20} match")
        for expected, read, seen, matched in results:
            rb = "-" if read is None else ",".join(f"{v:02x}" for v in read)
            print(f"  {expected:<22} {rb:<20} {seen:<20} {'YES' if matched else 'NO'}")
        all_match = all(m for *_, m in results)
        print(f"\n  criterion 1 (independent control): {'PASS' if all_match else 'REVIEW — mismatches above'}")
        print(f"  criterion 5 (all-off reachable):   see steps 1 and {len(SEQUENCE)}")
        rec.emit("sequence_verdict", all_match=all_match)
    finally:
        rec.close()
    return 0


def cmd_keymap_switch(args):
    interfaces = require_device()
    rec = Recorder(args.record)
    try:
        print_state(interfaces, "Tartarus Pro")
        interfaces = enter_driver_mode(interfaces, args, rec)
        print("\n=== CRITERION 3: on-device keymap switch vs host-set LED state ===")
        target = (ON, ON, ON)
        send_feature(interfaces, led_write_cmd(target, **led_args(args)), "LED all-on", rec)
        interfaces = presence_check(interfaces, rec, "pre-switch write")
        before = read_feature(interfaces, led_read_cmd(**led_args(args)), "before keymap switch", rec)
        observe(target, rec, "before-switch")
        print(
            "\n  Now trigger the keypad's OWN on-device keymap / keymap-indicator switch\n"
            "  (per the Razer Tartarus Pro manual — the on-device combo that cycles the\n"
            "  keymap indicator, NOT an Acheron Profile switch). If this unit has no such\n"
            "  on-device combo, say so at the prompt."
        )
        input("  Press ENTER once you've done the on-device keymap switch (or to note there is none): ")
        after = read_feature(interfaces, led_read_cmd(**led_args(args)), "after keymap switch", rec)
        seen, _ = observe(target, rec, "after-switch")
        clobbered = before != after or seen.lower() not in ("match", "m", "yes", "y", "")
        rec.emit("keymap_verdict", before=list(before) if before else None,
                 after=list(after) if after else None, clobbered=clobbered)
        print(f"\n  readback before : {before}")
        print(f"  readback after  : {after}")
        print(f"  => firmware {'CLOBBERED the host-set state (re-assert hook needed)' if clobbered else 'left the host-set state intact'}")
    finally:
        rec.close()
    return 0


def cmd_off_test(args):
    interfaces = require_device()
    rec = Recorder(args.record)
    try:
        print_state(interfaces, "Tartarus Pro")
        interfaces = enter_driver_mode(interfaces, args, rec)
        print("\n=== CRITERION 5: is all-off a state the hardware accepts? ===")
        # light everything first so "off" is a visible transition
        send_feature(interfaces, led_write_cmd((ON, ON, ON), **led_args(args)), "LED all-on", rec)
        presence_check(interfaces, rec, "all-on")
        observe((ON, ON, ON), rec, "all-on")

        print("\n  -- static frame, channels (0,0,0) --")
        send_feature(interfaces, led_write_cmd((OFF, OFF, OFF), **led_args(args)), "LED static all-zero", rec)
        interfaces = presence_check(interfaces, rec, "static all-zero")
        read_feature(interfaces, led_read_cmd(**led_args(args)), "after static all-zero", rec)
        seen_static, static_ok = observe((OFF, OFF, OFF), rec, "static-off")

        seen_none, none_ok = "(not tried)", None
        if args.effect_none:
            print("\n  -- effect-none frame (effect id 0x00, data_size 0x06) --")
            send_feature(interfaces, led_write_cmd((ON, ON, ON), **led_args(args)), "LED all-on (reset)", rec)
            observe((ON, ON, ON), rec, "all-on-2")
            send_feature(
                interfaces,
                led_write_cmd((OFF, OFF, OFF), effect_none=True, **led_args(args)),
                "LED effect-none", rec,
            )
            interfaces = presence_check(interfaces, rec, "effect-none")
            read_feature(interfaces, led_read_cmd(**led_args(args)), "after effect-none", rec)
            seen_none, none_ok = observe((OFF, OFF, OFF), rec, "effect-none-off")

        rec.emit("off_verdict", static_ok=static_ok, none_ok=none_ok,
                 seen_static=seen_static, seen_none=seen_none)
        print("\n  === OFF-TEST RESULT ===")
        print(f"  static (0,0,0)   -> {'ALL DARK' if static_ok else 'NOT all dark: ' + seen_static}")
        if args.effect_none:
            print(f"  effect-none      -> {'ALL DARK' if none_ok else 'NOT all dark: ' + seen_none}")
        print(f"\n  => all-off {'IS' if static_ok else 'is NOT'} hardware-reachable via the static frame")
    finally:
        rec.close()
    return 0


def cmd_relock(args):
    interfaces = require_device()
    rec = Recorder(args.record)
    try:
        print("About to send set-device-mode 0x00 (normal). This is a write, not 'cleanup' —")
        print("research §5 implicates set-device-mode itself in the reset reports.")
        print(f"  bytes: {NORMAL_MODE_CMD.hex(' ')}")
        if not confirm("relock", args):
            sys.exit("aborted.")
        send_feature(interfaces, NORMAL_MODE_CMD, "set-device-mode 0x00 (normal)", rec)
        time.sleep(0.05)
        presence_check(interfaces, rec, "relock")
        print_state(discover(), "\nAfter")
    finally:
        rec.close()
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[1])
    sub = parser.add_subparsers(dest="command", required=True)

    def add(name, func, help_text, *, live=False, spec=False):
        p = sub.add_parser(name, help=help_text)
        p.set_defaults(func=func)
        if spec:
            p.add_argument("spec", help="channels: 010 | green | orange+blue | off | all")
        if live:
            p.add_argument("--record", metavar="FILE", help="append a JSONL evidence log")
            p.add_argument("--yes", action="store_true", help="skip typed confirmations")
            p.add_argument("--txn", type=lambda s: int(s, 0), default=0x1F,
                           help="transaction_id for the LED frame (default 0x1F)")
            p.add_argument("--arg3", type=lambda s: int(s, 0), default=0x00, help="frame arg3 (default 0x00)")
            p.add_argument("--arg4", type=lambda s: int(s, 0), default=0x01,
                           help="frame arg4 (default 0x01 = CommandPost; 0x00 = OpenRazer)")
            p.add_argument("--nostore", action="store_true", help="use NOSTORE (0x00) instead of VARSTORE")
            p.add_argument("--no-driver-mode", action="store_true",
                           help="do NOT enter driver mode first (tests the frame's dependency on it)")
            p.add_argument("--effect-none", action="store_true",
                           help="also/instead use effect-none (0x00, data_size 0x06) for 'off'")
        return p

    add("selftest", cmd_selftest, "check protocol constants; no device, no root")
    add("probe", cmd_probe, "read-only pre-state (sends nothing)")
    p = add("set", cmd_set, "set the LEDs once (RISKY: enters driver mode)", live=True, spec=True)
    p.add_argument("--no-readback", action="store_true", help="skip the 0x82 read after writing")
    add("readback", cmd_readback, "issue only the 0x82 read frame", live=True)
    add("sequence", cmd_sequence, "walk all 8 combinations w/ visual confirm (criteria 1, 5)", live=True)
    add("keymap-switch", cmd_keymap_switch, "on-device keymap switch vs host state (criterion 3)", live=True)
    add("off-test", cmd_off_test, "is all-off reachable? (criterion 5)", live=True)
    add("relock", cmd_relock, "send set-device-mode 0x00 (normal) — also a write", live=True)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
