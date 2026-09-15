#!/usr/bin/env python3
"""
PROTOTYPE — throwaway code, not production. Standalone: imports nothing from
`daemon/` or `gui/`, stdlib only, and writes nothing into Acheron's config.

Hardware-verification tool for
`.scratch/tartarus-backlight/issues/02-hardware-verification.md` — this
effort is **ungated** (map: no kill-gate), so unlike
`prototype/01-status-leds/prototype.py` this is a plain verification pass,
not a criterion-1-kills-the-effort gate.

Byte layouts come from
`.scratch/tartarus-backlight/research/backlight-wire-protocol.md` (source:
vendored OpenRazer 3.12.4 driver tree), not re-derived here. Every command
targets `BACKLIGHT_LED = 0x05` (or `ZERO_LED = 0x00` for brightness, or the
hardcoded `0x00`/`0x00` custom-effect-arm frame) — never
`SIDE_STRIPE_LED = 0x0B`, which is the Status LEDs' unrelated feature.

This tool deliberately never touches `device_mode` (no driver-mode enter/
exit): research found no Tartarus Pro backlight command needs it, and the
Status LED prototype already confirmed on this exact unit that
`set_device_mode` — not steady-state feature writes — is the operation
research implicates in reset reports (ADR-0006). Every subcommand below is a
single one-shot HIDIOCSFEATURE write followed by a presence re-check; no
`input()` prompts block on a human in *this* process, because the human here
is the agent's chat conversation partner, not this script's stdin — the
agent runs one subcommand at a time and asks what was observed in the
conversation.

Commands:

    python3 prototype.py selftest                    # no device, no root
    python3 prototype.py probe                        # read-only pre-state
    python3 prototype.py custom-cols "0-19:00ff00,20:ff0000" --record F
    python3 prototype.py custom-fill ff00ff --record F
    python3 prototype.py effect static --color ff0000 --record F
    python3 prototype.py effect breathing-single --color 00ff00 --txn 0x3f --record F
    python3 prototype.py effect wave --direction 1 --record F
    python3 prototype.py effect none --record F
    python3 prototype.py brightness get --record F
    python3 prototype.py brightness set 128 --record F
"""

import argparse
import fcntl
import json
import os
import pathlib
import re
import sys
import time

VENDOR_ID = "1532"
PRODUCT_ID = "0244"
CONTROL_INTERFACE = 2  # bInterfaceNumber that takes the razer_report feature report
RAZER_CMD_LEN = 91  # report-number byte + 90-byte razer_report

_IOC_WRITE = 1
_IOC_READ = 2


def _ioc(direction, type_char, nr, size):
    return (direction << 30) | (size << 16) | (ord(type_char) << 8) | nr


def hidiocsfeature(size):
    return _ioc(_IOC_WRITE | _IOC_READ, "H", 0x06, size)


def hidiocgfeature(size):
    return _ioc(_IOC_WRITE | _IOC_READ, "H", 0x07, size)


def ioctl_unsigned(fd, request, arg):
    try:
        return fcntl.ioctl(fd, request, arg, True)
    except OverflowError:
        return fcntl.ioctl(fd, request - (1 << 32), arg, True)


# ---------------------------------------------------------------------------
# Frame construction — identical struct shape to prototype 01 (research
# §1: shared with the Status LED frame anatomy, ADR-0006).
# ---------------------------------------------------------------------------


def build_razer_cmd(txn, command_class, command_id, args, data_size=None):
    buf = bytearray(RAZER_CMD_LEN)
    buf[2] = txn
    buf[6] = len(args) if data_size is None else data_size
    buf[7] = command_class
    buf[8] = command_id
    buf[9 : 9 + len(args)] = bytes(args)
    crc = 0
    for b in buf[3:89]:
        crc ^= b
    buf[89] = crc
    return bytes(buf)


LED_CMD_CLASS = 0x0F
CMD_SET = 0x02
CMD_CUSTOM_FRAME = 0x03
CMD_BRIGHTNESS_SET = 0x04
CMD_BRIGHTNESS_GET = 0x84

BACKLIGHT_LED = 0x05
ZERO_LED = 0x00
VARSTORE = 0x01
NOSTORE = 0x00

TXN_DEFAULT = 0x1F
TXN_BREATH_QUIRK = 0x3F  # research §7 — what the driver actually transmits

NUM_COLS = 21  # research §10.4 — MATRIX_DIMS = [1, 21]; column N -> physical
# cell is exactly what this ticket's sweep settles.

RESP_STATUS = 1
RESP_CMD_CLASS = 7
RESP_CMD_ID = 8
RESP_ARGS = 9


def rgb(hexstr):
    hexstr = hexstr.strip().lstrip("#")
    if not re.fullmatch(r"[0-9a-fA-F]{6}", hexstr):
        raise ValueError(f"not a 6-hex-digit colour: {hexstr!r}")
    return tuple(int(hexstr[i : i + 2], 16) for i in (0, 2, 4))


# --- effect-select frames (research §2-8, §12 summary table) ---------------


def effect_static(color):
    return build_razer_cmd(
        TXN_DEFAULT, LED_CMD_CLASS, CMD_SET,
        [VARSTORE, BACKLIGHT_LED, 0x01, 0x00, 0x00, 0x01, *color],
    )


def effect_none():
    return build_razer_cmd(
        TXN_DEFAULT, LED_CMD_CLASS, CMD_SET,
        [VARSTORE, BACKLIGHT_LED, 0x00, 0x00, 0x00, 0x00],
    )


def effect_spectrum():
    return build_razer_cmd(
        TXN_DEFAULT, LED_CMD_CLASS, CMD_SET,
        [VARSTORE, BACKLIGHT_LED, 0x03, 0x00, 0x00, 0x00],
    )


def effect_wave(direction):
    return build_razer_cmd(
        TXN_DEFAULT, LED_CMD_CLASS, CMD_SET,
        [VARSTORE, BACKLIGHT_LED, 0x04, direction, 0x28, 0x00],
    )


def effect_reactive(color, speed=0x02):
    return build_razer_cmd(
        TXN_DEFAULT, LED_CMD_CLASS, CMD_SET,
        [VARSTORE, BACKLIGHT_LED, 0x05, 0x00, speed, 0x01, *color],
    )


def effect_breathing(variant, colors, txn):
    """variant: 'random' | 'single' | 'dual'. `txn` lets the caller try both
    0x1F and the driver's actual 0x3F (research §7 — flagged as a probable
    copy/paste bug, needs hardware to say which one(s) the firmware honours)."""
    if variant == "random":
        args = [VARSTORE, BACKLIGHT_LED, 0x02, 0x00, 0x00, 0x00]
    elif variant == "single":
        args = [VARSTORE, BACKLIGHT_LED, 0x02, 0x01, 0x00, 0x01, *colors[0]]
    elif variant == "dual":
        args = [VARSTORE, BACKLIGHT_LED, 0x02, 0x02, 0x00, 0x02, *colors[0], *colors[1]]
    else:
        raise ValueError(variant)
    return build_razer_cmd(txn, LED_CMD_CLASS, CMD_SET, args)


def effect_starlight(variant, colors, speed=0x02):
    if variant == "random":
        args = [VARSTORE, BACKLIGHT_LED, 0x07, 0x00, speed, 0x00]
    elif variant == "single":
        args = [VARSTORE, BACKLIGHT_LED, 0x07, 0x00, speed, 0x01, *colors[0]]
    elif variant == "dual":
        args = [VARSTORE, BACKLIGHT_LED, 0x07, 0x00, speed, 0x02, *colors[0], *colors[1]]
    else:
        raise ValueError(variant)
    return build_razer_cmd(TXN_DEFAULT, LED_CMD_CLASS, CMD_SET, args)


EFFECTS = {
    "none": lambda a: effect_none(),
    "spectrum": lambda a: effect_spectrum(),
    "static": lambda a: effect_static(rgb(a.color)),
    "wave": lambda a: effect_wave(a.direction),
    "reactive": lambda a: effect_reactive(rgb(a.color), a.speed),
    "breathing-random": lambda a: effect_breathing("random", [], a.txn),
    "breathing-single": lambda a: effect_breathing("single", [rgb(a.color)], a.txn),
    "breathing-dual": lambda a: effect_breathing("dual", [rgb(a.color), rgb(a.color2)], a.txn),
    "starlight-random": lambda a: effect_starlight("random", [], a.speed),
    "starlight-single": lambda a: effect_starlight("single", [rgb(a.color)], a.speed),
    "starlight-dual": lambda a: effect_starlight("dual", [rgb(a.color), rgb(a.color2)], a.speed),
}


# --- custom-frame two-step (research §10) -----------------------------------


def custom_frame_write(row_colors):
    """`row_colors`: list of 21 (r,g,b) tuples, column 0..20. research §10.1:
    data_size fixed 0x47 (71), a0/a1 unused (no varstore/led_id concept),
    a2=row(0x00), a3=start_col(0x00), a4=stop_col(0x14), a5.. = RGB triples."""
    assert len(row_colors) == NUM_COLS, f"expected {NUM_COLS} columns, got {len(row_colors)}"
    args = [0x00, 0x00, 0x00, 0x00, NUM_COLS - 1]
    for r, g, b in row_colors:
        args += [r, g, b]
    return build_razer_cmd(TXN_DEFAULT, LED_CMD_CLASS, CMD_CUSTOM_FRAME, args, data_size=0x47)


def custom_effect_arm():
    """research §10.2: variable_storage and led_id hardcoded 0x00 for every
    device — NOT VARSTORE/BACKLIGHT_LED. data_size 0x0C (12)."""
    args = [0x00, 0x00, 0x08] + [0x00] * 9
    return build_razer_cmd(TXN_DEFAULT, LED_CMD_CLASS, CMD_SET, args, data_size=0x0C)


def parse_col_spec(spec):
    """'0-19:00ff00,20:ff0000' -> list of 21 (r,g,b), default black. Ranges
    are inclusive, comma-separated, colour is 6 hex digits."""
    cols = [(0, 0, 0)] * NUM_COLS
    for chunk in spec.split(","):
        chunk = chunk.strip()
        if not chunk:
            continue
        rng, _, colorhex = chunk.partition(":")
        color = rgb(colorhex)
        if "-" in rng:
            lo, hi = (int(x) for x in rng.split("-"))
        else:
            lo = hi = int(rng)
        for i in range(lo, hi + 1):
            if not (0 <= i < NUM_COLS):
                raise ValueError(f"column {i} out of range 0..{NUM_COLS - 1}")
            cols[i] = color
    return cols


# --- brightness (research §11) ----------------------------------------------


def brightness_set_cmd(value):
    return build_razer_cmd(TXN_DEFAULT, LED_CMD_CLASS, CMD_BRIGHTNESS_SET, [VARSTORE, ZERO_LED, value])


def brightness_get_cmd():
    # research §11: get_razer_report(0x0F, 0x84, 0x03) — data_size 3 even
    # though only 2 argument bytes (storage, led_id) are meaningful.
    return build_razer_cmd(TXN_DEFAULT, LED_CMD_CLASS, CMD_BRIGHTNESS_GET, [VARSTORE, ZERO_LED], data_size=0x03)


# ---------------------------------------------------------------------------
# Device discovery (identical to prototype 01)
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
        found[number] = {"hidraw": f"/dev/{node.name}", "hid_id": hid_dir.name, "sysfs_hid": hid_dir}
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
        sys.exit(f"Tartarus Pro ({VENDOR_ID}:{PRODUCT_ID}) control interface {CONTROL_INTERFACE} not found.")
    return interfaces


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


def send_feature(interfaces, buffer, label, rec):
    path = interfaces[CONTROL_INTERFACE]["hidraw"]
    fd = os.open(path, os.O_RDWR)
    try:
        ioctl_unsigned(fd, hidiocsfeature(len(buffer)), bytearray(buffer))
    finally:
        os.close(fd)
    print(f"  sent {label}: {buffer.hex(' ')}")
    rec.emit("frame_sent", label=label, hex=buffer.hex())


def read_feature(interfaces, request, label, rec):
    path = interfaces[CONTROL_INTERFACE]["hidraw"]
    fd = os.open(path, os.O_RDWR)
    try:
        ioctl_unsigned(fd, hidiocsfeature(len(request)), bytearray(request))
        resp = bytearray(RAZER_CMD_LEN)
        for delay in (0.001, 0.003, 0.010, 0.030):
            time.sleep(delay)
            resp = bytearray(RAZER_CMD_LEN)
            ioctl_unsigned(fd, hidiocgfeature(RAZER_CMD_LEN), resp)
            echoes = resp[RESP_STATUS] in (0x00, 0x01, 0x02)
            if echoes:
                break
    finally:
        os.close(fd)
    rec.emit(
        "readback", label=label, hex=bytes(resp).hex(), status=resp[RESP_STATUS],
        cmd_class=resp[RESP_CMD_CLASS], cmd_id=resp[RESP_CMD_ID], args=list(resp[RESP_ARGS : RESP_ARGS + 12]),
    )
    print(
        f"  read {label}: status={resp[RESP_STATUS]:#04x} class={resp[RESP_CMD_CLASS]:#04x} "
        f"id={resp[RESP_CMD_ID]:#04x} args={bytes(resp[RESP_ARGS:RESP_ARGS + 12]).hex(' ')}"
    )
    return resp


def presence_check(interfaces, rec, note):
    now = discover()
    before_nodes = {n: i["hidraw"] for n, i in interfaces.items()}
    after_nodes = {n: i["hidraw"] for n, i in now.items()}
    present = CONTROL_INTERFACE in now
    changed = before_nodes != after_nodes
    rec.emit("presence", note=note, present=present, changed=changed, before=before_nodes, after=after_nodes)
    if not present:
        print(f"  *** DEVICE VANISHED after {note} — control interface gone from the bus ***")
    elif changed:
        print(f"  *** hidraw nodes CHANGED after {note} (re-enumeration): {before_nodes} -> {after_nodes} ***")
    else:
        print(f"  device still present, nodes unchanged ({note})")
    return now if present else interfaces


def print_state(interfaces, heading):
    state = openrazer_state(interfaces)
    print(f"{heading}:")
    for number, iface in sorted(interfaces.items()):
        print(f"  interface {number}: {iface['hidraw']}  {iface['hid_id']}")
    if state:
        print(f"  device_mode      : {state.get('device_mode')}   (00 00 = normal — this tool never changes it)")
        print(f"  firmware_version : {state.get('firmware_version')}")
        print(f"  device_serial    : {state.get('device_serial')}")
    return state


# ---------------------------------------------------------------------------
# Commands
# ---------------------------------------------------------------------------


def cmd_selftest(_args):
    checks = []

    def check(label, actual, expected):
        checks.append((actual == expected, label, actual, expected))

    check("HIDIOCSFEATURE(91)", hex(hidiocsfeature(91)), "0xc05b4806")
    check("HIDIOCGFEATURE(91)", hex(hidiocgfeature(91)), "0xc05b4807")

    static = effect_static((0xFF, 0x00, 0x00))
    check("static len", len(static), 91)
    check("static txn", static[2], 0x1F)
    check("static data_size", static[6], 0x09)
    check("static cmd_class", static[7], 0x0F)
    check("static cmd_id", static[8], 0x02)
    check("static a0 VARSTORE", static[9], 0x01)
    check("static a1 BACKLIGHT_LED", static[10], 0x05)
    check("static a2 effect", static[11], 0x01)
    check("static rgb", tuple(static[15:18]), (0xFF, 0x00, 0x00))

    none = effect_none()
    check("none data_size", none[6], 0x06)
    check("none a2", none[11], 0x00)

    wave = effect_wave(0x01)
    check("wave data_size", wave[6], 0x06)
    check("wave a2 effect", wave[11], 0x04)
    check("wave a3 direction", wave[12], 0x01)
    check("wave a4 speed fixed", wave[13], 0x28)

    breath_quirk = effect_breathing("single", [(0, 0xFF, 0)], TXN_BREATH_QUIRK)
    check("breath single txn (driver-actual)", breath_quirk[2], 0x3F)
    check("breath single data_size", breath_quirk[6], 0x09)
    check("breath single a3", breath_quirk[12], 0x01)
    check("breath single a5 colour count", breath_quirk[14], 0x01)

    breath_std = effect_breathing("single", [(0, 0xFF, 0)], TXN_DEFAULT)
    check("breath single txn (spec-standard, needs hw)", breath_std[2], 0x1F)

    frame = custom_frame_write([(1, 2, 3)] * NUM_COLS)
    check("custom-frame len", len(frame), 91)
    check("custom-frame txn", frame[2], 0x1F)
    check("custom-frame data_size fixed 0x47", frame[6], 0x47)
    check("custom-frame cmd_id", frame[8], 0x03)
    check("custom-frame a0 unused", frame[9], 0x00)
    check("custom-frame a1 unused", frame[10], 0x00)
    check("custom-frame a2 row", frame[11], 0x00)
    check("custom-frame a3 start_col", frame[12], 0x00)
    check("custom-frame a4 stop_col", frame[13], 0x14)
    check("custom-frame first column rgb", tuple(frame[14:17]), (1, 2, 3))

    arm = custom_effect_arm()
    check("arm data_size", arm[6], 0x0C)
    check("arm cmd_id", arm[8], 0x02)
    check("arm a0 hardcoded 0x00 (not VARSTORE)", arm[9], 0x00)
    check("arm a1 hardcoded 0x00 (not BACKLIGHT_LED)", arm[10], 0x00)
    check("arm a2 effect custom", arm[11], 0x08)

    bset = brightness_set_cmd(128)
    check("brightness set data_size", bset[6], 0x03)
    check("brightness set cmd_id", bset[8], 0x04)
    check("brightness set a1 ZERO_LED (not BACKLIGHT_LED)", bset[10], 0x00)
    check("brightness set a2 value", bset[11], 128)

    bget = brightness_get_cmd()
    check("brightness get cmd_id", bget[8], 0x84)
    check("brightness get data_size", bget[6], 0x03)

    cols = parse_col_spec("0-19:00ff00,20:ff0000")
    check("col spec length", len(cols), NUM_COLS)
    check("col spec range fill", cols[0], (0, 0xFF, 0))
    check("col spec range fill end", cols[19], (0, 0xFF, 0))
    check("col spec single override", cols[20], (0xFF, 0, 0))

    width = max(len(label) for _, label, _, _ in checks)
    for ok, label, actual, expected in checks:
        mark = "ok  " if ok else "FAIL"
        detail = f"{actual!r}" if ok else f"{actual!r} != expected {expected!r}"
        print(f"  {mark} {label:<{width}}  {detail}")
    failed = sum(1 for ok, *_ in checks if not ok)
    print(f"\n{len(checks) - failed}/{len(checks)} checks passed")
    if failed:
        sys.exit(f"{failed} check(s) FAILED — do not send anything to the device")
    return 0


def cmd_probe(_args):
    interfaces = require_device()
    print_state(interfaces, "Tartarus Pro")
    ctrl = interfaces[CONTROL_INTERFACE]
    try:
        os.close(os.open(ctrl["hidraw"], os.O_RDWR))
        print(f"\n  {ctrl['hidraw']} is R/W-open-able")
    except OSError as exc:
        print(f"\n  {ctrl['hidraw']} NOT R/W-open-able: {exc}")
    print("\n  Nothing was sent to the device.")
    return 0


def cmd_effect(args):
    interfaces = require_device()
    rec = Recorder(args.record)
    try:
        print_state(interfaces, "Tartarus Pro")
        cmd = EFFECTS[args.name](args)
        send_feature(interfaces, cmd, f"effect {args.name}", rec)
        presence_check(interfaces, rec, f"effect {args.name}")
        print(f"\n>>> Look at the device. Effect requested: {args.name}")
    finally:
        rec.close()
    return 0


def cmd_custom_fill(args):
    interfaces = require_device()
    rec = Recorder(args.record)
    try:
        print_state(interfaces, "Tartarus Pro")
        color = rgb(args.color)
        send_feature(interfaces, custom_frame_write([color] * NUM_COLS), "custom-frame fill", rec)
        presence_check(interfaces, rec, "custom-frame write")
        send_feature(interfaces, custom_effect_arm(), "custom-effect arm", rec)
        presence_check(interfaces, rec, "custom-effect arm")
        print(f"\n>>> Look at the device. Expected: solid #{args.color} across the Custom layout.")
    finally:
        rec.close()
    return 0


def cmd_custom_cols(args):
    interfaces = require_device()
    rec = Recorder(args.record)
    try:
        print_state(interfaces, "Tartarus Pro")
        cols = parse_col_spec(args.spec)
        send_feature(interfaces, custom_frame_write(cols), f"custom-frame cols {args.spec}", rec)
        presence_check(interfaces, rec, "custom-frame write")
        send_feature(interfaces, custom_effect_arm(), "custom-effect arm", rec)
        presence_check(interfaces, rec, "custom-effect arm")
        print(f"\n>>> Look at the device. Column spec sent: {args.spec}")
        print("    (column 0..20, left-to-right in wire order — physical mapping is what this settles)")
    finally:
        rec.close()
    return 0


def cmd_brightness(args):
    interfaces = require_device()
    rec = Recorder(args.record)
    try:
        print_state(interfaces, "Tartarus Pro")
        if args.action == "get":
            read_feature(interfaces, brightness_get_cmd(), "brightness get", rec)
        else:
            send_feature(interfaces, brightness_set_cmd(args.value), f"brightness set {args.value}", rec)
            presence_check(interfaces, rec, "brightness set")
            read_feature(interfaces, brightness_get_cmd(), "brightness get (after set)", rec)
    finally:
        rec.close()
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[1])
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("selftest", help="check protocol constants; no device")
    sub.add_parser("probe", help="read-only pre-state (sends nothing)")

    p = sub.add_parser("effect", help="send a Fixed-effect select frame")
    p.add_argument("name", choices=sorted(EFFECTS))
    p.add_argument("--color", default="ff0000")
    p.add_argument("--color2", default="0000ff")
    p.add_argument("--direction", type=lambda s: int(s, 0), default=1)
    p.add_argument("--speed", type=lambda s: int(s, 0), default=2)
    p.add_argument("--txn", type=lambda s: int(s, 0), default=TXN_BREATH_QUIRK,
                   help="breathing only: transaction_id (default 0x3f, what the driver actually sends)")
    p.add_argument("--record", metavar="FILE")
    p.set_defaults(func=cmd_effect)

    p = sub.add_parser("custom-fill", help="Custom-layout: one solid colour across all 21 columns")
    p.add_argument("color")
    p.add_argument("--record", metavar="FILE")
    p.set_defaults(func=cmd_custom_fill)

    p = sub.add_parser("custom-cols", help="Custom-layout: per-column-range colour spec, e.g. '0-9:ff0000,10-20:0000ff'")
    p.add_argument("spec")
    p.add_argument("--record", metavar="FILE")
    p.set_defaults(func=cmd_custom_cols)

    p = sub.add_parser("brightness", help="get or set brightness (ZERO_LED)")
    p.add_argument("action", choices=("get", "set"))
    p.add_argument("value", nargs="?", type=lambda s: int(s, 0), default=None)
    p.add_argument("--record", metavar="FILE")
    p.set_defaults(func=cmd_brightness)

    args = parser.parse_args(argv)
    if args.command == "brightness" and args.action == "set" and args.value is None:
        parser.error("brightness set requires a value 0-255")
    func = {"selftest": cmd_selftest, "probe": cmd_probe}.get(args.command, getattr(args, "func", None))
    return func(args)


if __name__ == "__main__":
    sys.exit(main())
