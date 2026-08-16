# Research: Linux `hidraw` implementation plan for the Tartarus Pro analog grid-key signal

Ticket: [12-research-linux-analog-grid-key-protocol](../issues/12-research-linux-analog-grid-key-protocol.md)

Supersedes the Linux-feasibility parts of the earlier
[analog-pressure-sensitivity](../../tartarus-keybinder/research/analog-pressure-sensitivity.md)
research (written for the archived MVP map, where analog was correctly ruled out of scope).
That file's protocol facts still stand; this file replaces its "unverified on Linux,
extrapolated from a Windows-only project" framing with first-hand evidence from our own
hardware and from Linux-side sources that file never read.

## Bottom line

**Materially more certain than before, and implementation-ready.** Three things changed:

1. **Our own hardware corroborates the protocol independently of the Windows project.** The
   Tartarus Pro's own USB/HID descriptors — read directly off the connected unit — declare
   Report ID `0x06` with 23 payload bytes on Interface 1 (endpoint `0x82`, `wMaxPacketSize`
   `0x18` = 24 bytes = report ID + 23), and a 90-byte vendor Feature Report on Interface 2.
   That is exactly the shape `open-tartarus-driver` documented, arrived at from a completely
   different direction. The protocol is no longer single-sourced.
2. **The Razer control channel already round-trips real data from Linux on this unit, today.**
   OpenRazer's `razerkbd` is bound to all three interfaces and its Interface-2 `razer_report`
   query path returns this device's true serial (`PM2443F36300141`, matching what Synapse
   reported on Windows), firmware `v1.2`, and `device_mode = 00 00`. The transport is proven;
   only the specific *set*-device-mode command is unproven and risky.
3. **The reset risk is better characterised, and smaller than it first looks.** OpenRazer's
   shipped kernel source on this very machine carries a Tartarus-Pro-specific carve-out
   commented *"Tartarus Pro resets when it receives this command"*, and its Python daemon
   sets `DRIVER_MODE = False` for this device alone among all keyboards. But: "reset" means a
   USB re-enumeration (a firmware self-reboot), not a hang or a brick — the reported
   *loops* were loops only because the sender sat in the device-detection path, which is no
   longer true on our stack. It traces to a single contributor's report, uncorroborated by
   any user in the entire Tartarus Pro paper trail. And there is a concrete, testable
   hypothesis for why `open-tartarus-driver` never reproduced it: **they send a different
   `transaction_id`.** See §5.

Everything needed to write the prototype — exact 91-byte buffer, exact ioctl numbers, device
discovery, permissions — is below. No re-derivation from Windows source is needed at
prototype time.

One consequence that needs flagging before anything is built: there is good circumstantial
evidence that **driver mode makes the 20 grid keys stop emitting normal keycodes** (see
§6). If true, analog is not an additive feature — it changes the capture path Acheron's
entire existing Daemon depends on.

---

## 1. What was verified first-hand on our own hardware

All read-only. Nothing was written to the device; no `set_device_mode` command was sent.

### 1.1 USB interface / endpoint layout

From `/sys/bus/usb/devices/3-2:1.*` on the connected unit (`1532:0244`, `Razer Tartarus Pro`):

| Interface | `bInterfaceProtocol` | Endpoint | `wMaxPacketSize` | `hidraw` node (this boot) |
|---|---|---|---|---|
| 0 | `01` (Keyboard) | `0x81` | 8 | `hidraw1` |
| **1** | `01` (Keyboard) | **`0x82`** | **`0x18` = 24** | **`hidraw2`** |
| 2 | `02` (Mouse) | `0x83` | 8 | `hidraw3` |

This matches `open-tartarus-driver`'s table exactly, including its note that Interface 2
enumerates with a *mouse* protocol/usage despite being Razer's control channel.

Interface 1's 24-byte max packet is a direct physical corroboration of the analog report
size (1 report-ID byte + 23 payload bytes).

### 1.2 Report descriptors — Report `0x06` genuinely exists

`/sys/bus/hid/devices/0003:1532:0244.000C/report_descriptor` (Interface 1) declares five
top-level collections. Reports `0x01`/`0x02`/`0x03` are the ordinary keyboard, consumer and
system-control reports. Reports **`0x04`, `0x05` and `0x06`** are each an identical
vendor-shaped input report:

```
05 01 09 00 a1 01 85 06 09 03 15 00 26 ff 00 35 00 46 ff 00 75 08 95 17 81 00 c0
                     ^^ Report ID 0x06        logical 0..255      8 bits × 0x17 (23) count
```

23 bytes of `0x00`–`0xFF` data — enough for 20 per-key depth values plus 3 spare bytes,
exactly as `open-tartarus-driver` reports (`byte[1]..byte[20]` = keycaps 1–20).

**Why this matters beyond corroboration:** the Linux kernel drops any incoming report whose
ID is not in the parsed descriptor (`hid_get_report()` returns NULL → `__hid_input_report()`
bails before `hid_report_raw_event()`). Because `0x06` *is* declared, the kernel will accept
and forward it. This was a genuine open risk — the report could have been undeclared and
delivered only because Windows' stack is more permissive. It isn't.

`/sys/bus/hid/devices/0003:1532:0244.000D/report_descriptor` (Interface 2) ends with:

```
06 00 ff 09 02 15 00 25 01 75 08 95 5a b1 01
   ^^^^^ vendor page 0xFF00        8 bits × 0x5A (90)  Feature
```

A **90-byte, unnumbered Feature Report** — precisely `struct razer_report`. Unnumbered
matters: the `hidraw` report-number byte must be `0x00` (see §3).

### 1.3 The Interface-2 control channel already works from Linux on this unit

OpenRazer 3.12.4 (`razerkbd` DKMS module) is bound to all three interfaces. Reading its
sysfs attributes issues real `razer_report` control transfers to Interface 2 and parses the
replies:

| Attribute | Value read | What it proves |
|---|---|---|
| `device_serial` | `PM2443F36300141` | Identical to the serial Synapse reported on Windows (see [firmware note](../../tartarus-keybinder/research/firmware-version-of-our-tartarus-pro.md)) — a real, device-specific reply, not zeros |
| `firmware_version` | `v1.2` | Matches Synapse's `1.2.0.0`; latest available as of 2026-08-16 |
| `device_mode` | `00 00` | Device is currently in **normal mode**; the get-device-mode command (class `0x00`, cmd `0x84`) round-trips fine |

So: `razer_report` request/response over Interface 2 from Linux is a solved, working thing on
this exact unit and firmware. The only unknown is what the device does with `command_id 0x04`.

### 1.4 Environment facts the prototype will run in

- OpenRazer **3.12.4** installed via DKMS; `openrazer-daemon` **running** (systemd `--user`,
  PID 5318 at time of writing).
- `razerkbd` claims all three HID interfaces. This does **not** block `hidraw` (§4.1).
- `/dev/hidraw*` are `crw------- root root` — root-only by default (§4.3).
- The user is in `plugdev`; OpenRazer's udev rule sets `GROUP:="plugdev"` on the `usb`/`input`/
  `hid` subsystems, but **not** on `hidraw`.

---

## 2. The unlock command, extracted from `open-tartarus-driver`'s Rust source

Read from the actual source (`tartarus_driver/src/main.rs`), not just its `research.md`.

```rust
// main.rs:538 — builds an arbitrary razer_report, 91 bytes incl. leading report-ID 0 byte
fn build_razer_cmd(txn: u8, class: u8, cmd: u8, args: &[u8]) -> [u8; 91] {
    let mut buf = [0u8; 91];
    buf[2] = txn;              // transaction_id
    buf[6] = args.len() as u8; // data_size
    buf[7] = class;            // command_class
    buf[8] = cmd;              // command_id
    buf[9..9 + args.len()].copy_from_slice(args);
    let mut crc = 0u8;
    for b in &buf[3..89] { crc ^= *b; }
    buf[89] = crc;
    buf
}

// main.rs:660 — the entire lock/unlock mechanism
let cmd = build_razer_cmd(0x01, 0x00, 0x04, &[0x03, 0x00]);
ctrl.send_feature_report(&cmd)
```

`ctrl` is the Interface-2 handle, opened via `hidapi` by filtering the device list for
`usage_page == 0x0001 && usage == 0x0002` (main.rs:522). `send_feature_report` is `hidapi`'s
portable wrapper — on Windows `HidD_SetFeature`, **on Linux `ioctl(fd, HIDIOCSFEATURE(n), buf)`
with the identical buffer**. The Windows API question the prior research left open is moot:
the project never calls a platform API directly.

The CRC loop `&buf[3..89]` covers buffer indices 3–88, i.e. `razer_report` struct bytes 2–87 —
byte-for-byte the same range as OpenRazer's kernel `razer_calculate_crc()`
(`for(i = 2; i < 88; i++)`). Two independent implementations agree.

### The exact bytes

`razer_report` is 90 bytes; `hidraw`/`hidapi` prepend a report-number byte, giving **91**:

| Buf idx | Struct byte | Field | Unlock (mode 3) | Re-lock (mode 0) |
|---|---|---|---|---|
| 0 | — | `hidraw` report number | `0x00` (unnumbered) | `0x00` |
| 1 | 0 | `status` | `0x00` | `0x00` |
| 2 | 1 | `transaction_id` | **`0x01`** | `0x01` |
| 3–4 | 2–3 | `remaining_packets` (BE u16) | `0x0000` | `0x0000` |
| 5 | 4 | `protocol_type` | `0x00` | `0x00` |
| 6 | 5 | `data_size` | `0x02` | `0x02` |
| 7 | 6 | `command_class` | `0x00` | `0x00` |
| 8 | 7 | `command_id` (set device mode) | `0x04` | `0x04` |
| 9 | 8 | `arguments[0]` = mode | **`0x03`** | **`0x00`** |
| 10 | 9 | `arguments[1]` | `0x00` | `0x00` |
| 11–88 | 10–87 | `arguments[2..80]` | all `0x00` | all `0x00` |
| 89 | 88 | `crc` = XOR of buf[3..=88] | **`0x05`** | **`0x06`** |
| 90 | 89 | `reserved` | `0x00` | `0x00` |

CRC arithmetic, for checking an implementation by hand: the only non-zero bytes inside the
CRC range are `data_size`, `command_class`, `command_id` and `arguments[0..2]`, so unlock =
`0x02 ^ 0x04 ^ 0x03 = 0x05`, re-lock = `0x02 ^ 0x04 = 0x06`. Note `transaction_id` sits at
buf idx 2 and is **excluded** from the CRC — changing it does not change the CRC.

Behaviour after sending (per `open-tartarus-driver`, verified on their Windows hardware):
one-shot, not a polling relationship. A few ms later Interface 1 emits one all-zero "standby"
report, then a real report on every keypress, and keeps streaming even after the sending
process exits. No heartbeat.

---

## 3. Linux translation — exact calls

### 3.1 Device discovery (do **not** port the usage-based filter)

`open-tartarus-driver` picks interfaces by HID usage because Windows enumerates each
top-level collection separately. **This does not translate.** On Linux, `hidraw` exposes one
node per USB interface, and its usage is that of the *first* top-level collection — Interface
1's descriptor opens with `05 01 09 06` (Generic Desktop / **Keyboard**), so their
`!(usage_page == 0x01 && (usage == 0x02 || usage == 0x06))` filter would exclude the analog
interface outright. Match on **`bInterfaceNumber`** instead.

Node numbers are not stable across boots. Walk sysfs:

```
/sys/class/hidraw/hidrawN/device            -> .../3-2:1.1/0003:1532:0244.000C   (HID device)
/sys/class/hidraw/hidrawN/device/..         -> .../3-2:1.1                       (USB interface)
    bInterfaceNumber   -> "01"
/sys/class/hidraw/hidrawN/device/../..      -> .../3-2                           (USB device)
    idVendor  -> "1532"
    idProduct -> "0244"
```

Verified against all three nodes on the live device. Target Interface `01` for reads,
Interface `02` for the feature report.

### 3.2 The ioctls

`HIDIOCSFEATURE(len)` = `_IOC(_IOC_WRITE|_IOC_READ, 'H', 0x06, len)`;
`HIDIOCGFEATURE(len)` = the same with `nr = 0x07`. For `len = 91`:

```
HIDIOCSFEATURE(91) = 0xC05B4806
HIDIOCGFEATURE(91) = 0xC05B4807
```

`buf[0]` is the report number; the feature report is unnumbered, so it is `0x00` and the
kernel sends the remaining 90 bytes. The 91-byte buffer from §2 is passed verbatim.

Reference sketch — **do not run casually; sending this is the risky step (§5)**:

```python
import fcntl, os, struct

HIDIOCSFEATURE_91 = 0xC05B4806

def build(txn, cls, cmd, args):
    buf = bytearray(91)
    buf[2], buf[6], buf[7], buf[8] = txn, len(args), cls, cmd
    buf[9:9+len(args)] = args
    crc = 0
    for b in buf[3:89]:
        crc ^= b
    buf[89] = crc
    return bytes(buf)

unlock = build(0x01, 0x00, 0x04, b"\x03\x00")   # == re-lock with b"\x00\x00"

# Open the READ node first, so the one-shot standby report isn't missed.
analog = os.open("/dev/hidraw2", os.O_RDONLY)          # interface 1, resolved via sysfs
ctrl   = os.open("/dev/hidraw3", os.O_RDWR)            # interface 2, resolved via sysfs
fcntl.ioctl(ctrl, HIDIOCSFEATURE_91, unlock)

while True:
    r = os.read(analog, 64)          # kernel delivers one whole report per read
    if len(r) >= 21 and r[0] == 0x06:
        depths = list(r[1:21])       # keycap 1..20, 0x00-0xFF, identity mapping
```

Reads on `hidraw` are whole-report; a 64-byte buffer is comfortably larger than the 24-byte
report. Interface 1 also carries reports `0x01`–`0x05`, so the `r[0] == 0x06` filter is
required, not optional. Use `O_NONBLOCK` + `poll()` if the prototype needs a timeout.

A read-back via `HIDIOCGFEATURE` after a `get_device_mode` request (class `0x00`, cmd `0x84`,
`data_size 0x02`) is the cheap way to confirm the mode actually changed — but Razer firmware
needs a settle delay between the SET and the GET (OpenRazer uses ~600–1000 µs for this device
class). Reading OpenRazer's `device_mode` sysfs attribute does the same thing with the
timing already handled, and is the safer observation channel while the daemon is loaded.

---

## 4. Linux-specific wrinkles Windows doesn't have

### 4.1 OpenRazer's kernel driver claims all three interfaces — and that's fine

`razerkbd` binds all three HID devices, but `hidraw` access is unaffected:

- It calls `hid_hw_start(hdev, HID_CONNECT_DEFAULT)` (`razerkbd_driver.c:5518`), which
  includes `HID_CONNECT_HIDRAW` — so `HID_CLAIMED_HIDRAW` is set and
  `hid_report_raw_event()` feeds every input report to `hidraw` (`hid-core.c:2095`) *before*
  the input layer sees it.
- A bound driver can only starve `hidraw` by returning **negative** from `raw_event`
  (`hid-core.c:2167-2171`). For the Tartarus Pro, `razer_raw_event()` dispatches to
  `razer_raw_event_standard()`, which only rewrites reports that are 16/22/48 bytes **and**
  start with `0x04`; anything else falls through returning `0`. Our 24-byte report starting
  with `0x06` is untouched.

So the analog stream reaches `/dev/hidraw2` with OpenRazer loaded. No unbinding, no
`HIDIOCSFEATURE` conflict, no `usb_set_interface` fight. (Verified by reading both the
installed OpenRazer 3.12.4 source and current mainline `hid-core.c`.)

The daemon won't interfere either: `RazerTartarusPro` in
`openrazer_daemon/hardware/keyboards.py:216` sets `DRIVER_MODE = False`, overriding
`_MacroKeyboard`'s `DRIVER_MODE = True` — it is the only keyboard class in the tree that opts
out. OpenRazer will neither enable driver mode behind our back nor revert it if we do.

### 4.2 A ready-made unlock path exists — and it is the *wrong* one to use

OpenRazer exposes `device_mode` as a writable sysfs attribute on Interface 2
(`/sys/bus/hid/devices/0003:1532:0244.000D/device_mode`, `root:plugdev 0660`, so writable
without root here). Writing `\x03\x00` to it constructs exactly the same
`razer_chroma_standard_set_device_mode(0x03, 0x00)` report.

**Don't reach for it.** `razer_attr_write_device_mode()` hardcodes `transaction_id = 0xFF`,
and the kernel's internal `razer_set_device_mode()` uses `0x1F` for this device — both of
which are the variants implicated in the reset reports (§5). It is a one-line shortcut
straight into the known failure mode.

### 4.3 Permissions — and a v1.0 packaging consequence

`/dev/hidraw*` default to `0600 root:root`. OpenRazer's `99-razer.rules` sets
`GROUP:="plugdev"` for `SUBSYSTEM=="usb|input|hid"` but never touches `hidraw`, so nothing on
this machine grants user access today.

For the prototype (ticket 13): run it with `sudo`. Simplest, no persistent system change.

For a shipped feature, a udev rule is needed:

```udev
# /etc/udev/rules.d/99-acheron-tartarus-hidraw.rules
KERNEL=="hidraw*", SUBSYSTEM=="hidraw", ATTRS{idVendor}=="1532", ATTRS{idProduct}=="0244", \
    MODE="0660", GROUP="plugdev", TAG+="uaccess"
```

**This is a real consequence for the v1.0 destination, not a detail.** The map's
release-readiness argument rests on "the Daemon runs unprivileged and the GUI is pure
userspace, so nothing forces packaging complexity yet". Analog support breaks that: it
introduces the first install-time step needing root (dropping a rule into `/etc/udev/rules.d`
and reloading). Not fatal — `install.sh` can prompt for it — but the "no privileged install
step" property is spent the moment analog ships. Worth weighing when deciding whether analog
makes v1.0 or fast-follows.

### 4.4 Don't reuse the `hidapi` device-list filter

Covered in §3.1. If the prototype uses `hidapi` rather than raw ioctls, select by
`interface_number()`, never by `usage()`/`usage_page()`.

---

## 5. The firmware-reset risk — what is actually known

The prior research recorded this as "a reported firmware reset/reconnect loop on some units,
cross-referenced against an OpenRazer issue, not reproduced by `open-tartarus-driver`". The
picture is sharper now.

**It is first-party and in the shipped code on this machine.** OpenRazer PR
[#2710](https://github.com/openrazer/openrazer/pull/2710) ("Add support for Razer Tartarus Pro
(1532:0244) with stability fixes", merged 2026-03-14, the PR that produced the 3.12.4 support
we're running) is explicit — its two headline fixes are *both* about this command:

> **Bug fix 1: Daemon crash on device detection.** `_MacroKeyboard` sets `DRIVER_MODE = True`,
> which causes the daemon to send `set_device_mode(0x03, 0x00)` on detection. The Tartarus Pro
> firmware does not support this command and resets the device, causing an infinite reconnect
> loop. **Fix:** `DRIVER_MODE = False`.
>
> **Bug fix 2: Kernel probe crash.** The probe function unconditionally calls
> `razer_set_device_mode(dev, 0x00, 0x00)`... The Tartarus Pro firmware resets when it
> receives this command during probe. **Fix:** skip the call for the Tartarus Pro.

Maintainer `z3ntu`, in review, corrected the characterisation:

> "the comment 'does not support' is not quite right, **the firmware knows the command, but it
> just does bad stuff with it**."

Both carve-outs are in the source installed here (`razerkbd_driver.c:5501`,
`keyboards.py:216`). Note that **mode `0x00` (the "safe" re-lock) is implicated too** — the
kernel-probe crash was mode `0x00`, not `0x03`. There is no "just send mode 0 to recover"
escape hatch that is known-safe.

**Nothing narrows it to a firmware revision.** No comment on #2710 (issue or review threads)
mentions firmware versions, and no OpenRazer issue search turns up a Tartarus Pro
`device_mode` report with revision detail. `open-tartarus-driver`'s "possibly a firmware
revision difference" is that project's speculation, and it never states its own unit's
firmware. Our unit is **v1.2** (`1.2.0.0`), latest offered by Synapse as of 2026-08-16 — so
"newer firmware fixed it" is an untested guess in either direction.

### What "resets" actually means — and whether it's recoverable

No source describes the failure directly: there are no logs, no `dmesg` output, and no
symptom write-up anywhere in the public record. But the *shape* of the two bugs pins the
mechanism down tightly, because of what has to be true for either loop to form.

**"Reset" means the device drops off the USB bus and re-enumerates.** `razer_kbd_probe()` is
only ever called when the HID/USB core binds a newly-appeared interface. For sending a command
inside probe to cause an "infinite reconnect loop", the command must make the device
disappear from the bus and come back — at which point probe runs again, sends again, and so
on. Same for the daemon-side loop: `set_device_mode(0x03, 0x00)` is sent on device *detection*,
so a loop requires repeated detections. This is a firmware self-reboot — functionally the
same thing a physical replug does — not a hang.

**It is therefore almost certainly recoverable, and the loop reports are themselves the
evidence.** A device stuck in an infinite reconnect loop is, by definition, successfully
re-enumerating on every cycle. Nothing in the record describes a device that stopped
enumerating, needed a firmware recovery, or was bricked; the complaint is that it kept coming
back, over and over. Power-cycling is strictly a superset of what the reset already does to
itself.

**On our current software stack the loop cannot form at all.** Both loop-forming senders are
gone for this device in the OpenRazer 3.12.4 installed here: the kernel probe skips
`razer_set_device_mode()` for the Tartarus Pro (`razerkbd_driver.c:5501`), and the daemon's
`DRIVER_MODE = False` means it never sends driver mode on detection. So a one-shot manual
send from the prototype has nothing to re-trigger it — the predicted worst case is *one*
reset and *one* re-enumeration, not a loop. The loop was a property of where the send sat in
the lifecycle, not of the command.

Two genuine unknowns remain, and the prototype should record both:
- **What mode the device comes back in** after a reset. If the unlock survives the reset the
  behaviour is confusing but harmless; if it doesn't, mode 3 may be unreachable on this unit
  by this route.
- **Whether re-enumeration is clean**, i.e. whether the three `hidraw`/evdev nodes come back
  with the same properties (node numbers *will* change; that's expected and is why discovery
  goes through sysfs).

### How strong is the evidence, really?

Weaker than "it's in the shipped source" suggests, and worth stating plainly since the
carve-outs look authoritative:

- The **entire** public evidence base is PR #2710's own description, written by one
  contributor (`countgitmick`) reporting their own hardware testing, plus maintainer
  `z3ntu`'s one-line review response to it. z3ntu does not appear to own the device — in the
  same review he asks the contributor to confirm basic hardware facts about the profile LEDs
  — so the merge reflects trust in the contributor's testing, not independent reproduction.
- **No user ever reported this symptom.** Searched the whole Tartarus Pro paper trail:
  issue [#1039](https://github.com/openrazer/openrazer/issues/1039) (61 comments),
  issue [#1177](https://github.com/openrazer/openrazer/issues/1177),
  PR [#2336](https://github.com/openrazer/openrazer/pull/2336) (17 comments) and
  PR [#2622](https://github.com/openrazer/openrazer/pull/2622). Not one mention of a reset,
  reconnect, disconnect, loop or crash. The behaviour surfaces exactly once, in #2710.
- Against it: `open-tartarus-driver` sends this command on every startup and reports never
  reproducing the fault across extensive testing — with a different `transaction_id`, on
  Windows, on an unknown firmware.

So: one credible first-hand report that a maintainer found convincing enough to encode as a
permanent carve-out, one first-hand report of the opposite, and no corroboration either way.
That is enough to justify treating the command as risky and to approach it deliberately — it
is not enough to treat "this device resets on set-device-mode" as established fact.

### The transaction_id hypothesis — worth trying first

The three implementations do **not** send the same bytes:

| Source | `transaction_id` | Reported outcome on Tartarus Pro |
|---|---|---|
| `open-tartarus-driver` (`main.rs:660`) | **`0x01`** | Works; reset never observed in extensive testing |
| OpenRazer kernel `razer_set_device_mode()` | `0x1F` | Resets the device (probe crash, PR #2710) |
| OpenRazer sysfs `device_mode` write attr | `0xFF` | The daemon path that caused the reconnect loop |

`0x01` is what `open-tartarus-driver` captured Synapse itself sending. Razer firmware is known
to be `transaction_id`-sensitive across command classes — `open-tartarus-driver` found this
device's *lighting* commands need `0x1F` where sibling Tartarus models use `0xFF`, and its
`lighting.rs` explicitly warns that the `0x1F` lighting constant and the `0x01` device-mode
constant "are two different, independently-confirmed constants for two different command
classes, not a typo".

That makes a clean, testable story: **the resets OpenRazer hit may be a wrong-`transaction_id`
artefact rather than an inherent property of the set-device-mode command.** Unproven, and it
could equally be that `0x01` is incidental and OpenRazer's reporters had different units — but
it is the single highest-value thing for ticket 13 to try, and it costs nothing to prefer
`0x01`.

### Prior art the earlier research missed: this has been done on Linux

OpenRazer PR [#1868](https://github.com/openrazer/openrazer/pull/1868) ("Analog support",
**closed unmerged**) implements Razer analog driver mode on Linux for the Huntsman Mini
Analog — kernel-side, adding a `razer_raw_event_analog()` that reads the analog stream and
emits `ABS_*` events, plus an `analog_threshold` sysfs knob (default 128). It relies on the
device being put into driver mode via the standard `DRIVER_MODE = True` path.

So the earlier research's "nobody has verified this from Linux" is too strong: the
mode-`0x03`-unlocks-analog mechanism **has** been made to work on Linux, on a sibling Razer
analog device, with no reset trouble reported. That is a meaningfully better prior than a
Windows-only reference.

Two cautions on reading across from it:
- Its analog interface is also **Interface 1** (`bInterfaceNumber == 1`), matching ours.
- Its **payload layout is different**: sparse `(key, analog)` pairs at `data[1 + n*2]`,
  `data[2 + n*2]`, terminated by a zero key — not the Tartarus Pro's fixed 20-byte
  keycap-indexed array. Don't assume a shared format; the Tartarus Pro layout stands on
  `open-tartarus-driver`'s per-key empirical mapping plus our 23-byte descriptor.

---

## 6. The thing that most affects Acheron: driver mode probably silences the discrete keycodes

PR #1868's own summary describes the mechanism as *"driver mode support where keys **only**
emit their analog values"*. If the Tartarus Pro behaves the same way, then after the unlock
the 20 grid keys stop producing ordinary HID keycodes on the boot-keyboard interface — which
is precisely the evdev signal Acheron's Daemon captures for every existing feature.

Circumstantial support from `open-tartarus-driver`: its docs treat double input as a problem
**only** for the D-pad, wheel and middle click (which it suppresses via the Interception
kernel driver, and whose troubleshooting entry is entirely about Interception not loading).
The 20 analog keys — which it drives entirely from depth values via `SendInput` — are never
mentioned as producing duplicate input. If they still emitted native keycodes in mode 3, every
grid keypress would fire twice and would be the loudest bug in that project.

**This is an inference, not a verified fact**, and it is the single most consequential thing
ticket 13 should check — it is cheap to check (press a grid key after the unlock and watch the
existing evdev nodes with `evtest`) and it changes the shape of the feature completely:

- If keycodes survive → analog is additive. A second capture path alongside evdev, feeding a
  new Binding concept. Existing behaviour untouched.
- If keycodes disappear → analog is a **mode switch for the whole device**. Acheron would have
  to synthesise all 20 discrete keys from depth thresholds itself while analog is on, meaning
  every existing feature (Layers, Chords, Steppers, Trigger modes) has to keep working on top
  of a thresholded analog stream rather than evdev. That is a much larger change than "add a
  feature", and would strongly argue for analog fast-following v1.0 rather than blocking it.

---

## 7. Recommended procedure for the prototype (ticket 13)

In order, stopping at the first surprise:

1. **Before touching the device**, note that `openrazer-daemon` is running and will not fight
   us (`DRIVER_MODE = False`), and record the pre-state: `device_mode` (`00 00`),
   `firmware_version` (`v1.2`), and the three `hidraw` nodes resolved via sysfs. Confirm
   `device_mode` still reads `00 00` — that is the recovery check for every later step.
2. **Know what a reset would look like before triggering one.** Per §5, the expected worst
   case on this stack is a single USB disconnect + re-enumerate (a firmware self-reboot), not
   a hang and not a loop — nothing re-sends the command on reconnect. Watch `dmesg -w` and
   `udevadm monitor` during the send so a reset is *observed* rather than guessed at, and
   expect the `hidraw` node numbers to change if it happens. Have a replug available as the
   fallback, but it shouldn't be needed.
3. **Open the Interface-1 read fd before sending anything**, so the one-shot standby report
   isn't missed.
4. **Send the unlock with `transaction_id = 0x01`** (§2/§5) — not the sysfs shortcut, not
   `0x1F`/`0xFF`.
5. **Watch for the standby report** (all-zero depths, `r[0] == 0x06`) within a few ms. Its
   arrival is the cleanest positive signal that mode 3 took.
6. **Press one grid key at a time** and confirm the depth byte at the expected keycap index
   moves through intermediate values, not just 0/255. Record the actual mapping rather than
   trusting the identity mapping.
7. **Immediately check whether the grid keys still emit evdev keycodes** (§6) — `evtest` on
   the existing nodes, same keypress.
8. **Try to return to normal mode** (`arguments[0] = 0x00`, CRC `0x06`) and confirm
   `device_mode` reads `00 00` again. Note that mode `0x00` is itself implicated in the reset
   reports, so treat this step as risky too, not as cleanup.

Record whatever the unit actually does at each step — including a reset, which is a
publishable result in its own right, since no public source ties this behaviour to a specific
firmware revision.

---

## Sources

### First-hand (this machine, 2026-08-16)
- `/sys/bus/usb/devices/3-2:1.{0,1,2}` — interface numbers, protocols, endpoint addresses and
  `wMaxPacketSize`.
- `/sys/bus/hid/devices/0003:1532:0244.{000B,000C,000D}/report_descriptor` — Report `0x06`
  (23 bytes) on Interface 1; 90-byte unnumbered vendor Feature Report on Interface 2.
- `/sys/bus/hid/devices/0003:1532:0244.000D/{device_serial,firmware_version,device_mode}` —
  serial `PM2443F36300141`, firmware `v1.2`, device mode `00 00`.
- [Firmware version of our Tartarus Pro](../../tartarus-keybinder/research/firmware-version-of-our-tartarus-pro.md)
  — Synapse-reported `1.2.0.0`, no update offered as of 2026-08-16.

### OpenRazer 3.12.4, installed locally (`/usr/src/openrazer-driver-3.12.4`, `/usr/lib/python3/dist-packages/openrazer_daemon`)
- `driver/razercommon.{c,h}` — `struct razer_report` (90 bytes), `razer_calculate_crc()`
  (XOR of struct bytes 2–87), `razer_get_usb_response()`.
- `driver/razerchromacommon.c:25` — `razer_chroma_standard_set_device_mode()`
  (class `0x00`, cmd `0x04`, `data_size 0x02`).
- `driver/razerkbd_driver.c` — Tartarus Pro report index `0x02` / transaction id `0x1F`
  (`razer_get_report_params`, `razer_set_device_mode`); `device_mode` write attr forcing
  `0xFF`; the probe carve-out at :5501 with the *"Tartarus Pro resets when it receives this
  command"* comment; `razer_raw_event_standard()` pass-through behaviour;
  `hid_hw_start(HID_CONNECT_DEFAULT)` at :5518.
- `openrazer_daemon/hardware/keyboards.py:216` — `RazerTartarusPro.DRIVER_MODE = False`
  (the only such override among keyboards; `_MacroKeyboard` defaults to `True` at :19).
- `/usr/lib/udev/rules.d/99-razer.rules` — `plugdev` group set for `usb|input|hid`, not
  `hidraw`.

### Linux kernel
- [`drivers/hid/hid-core.c`](https://github.com/torvalds/linux/blob/master/drivers/hid/hid-core.c)
  — `hid_report_raw_event()` feeding `hidraw_report_event()` when `HID_CLAIMED_HIDRAW`
  (:2095); `__hid_input_report()` bailing only on a **negative** `raw_event` return (:2167)
  and dropping reports whose ID isn't in the parsed descriptor (:2160).

### `ultramonaka/open-tartarus-driver` (Windows-only, GPL-3.0)
- [`tartarus_driver/src/main.rs`](https://github.com/ultramonaka/open-tartarus-driver/blob/HEAD/tartarus_driver/src/main.rs)
  — `build_razer_cmd()` (:538), the unlock call with `transaction_id = 0x01` (:660),
  `open_razer_control_device()` usage-based Interface-2 filter (:522), the
  `ANALOG_REPORT_ID = 0x06` / `NUM_KEYS = 20` constants and identity keycap mapping (:192-201),
  and the read loop (:789).
- [`tartarus_driver/src/lighting.rs`](https://github.com/ultramonaka/open-tartarus-driver/blob/HEAD/tartarus_driver/src/lighting.rs)
  — the explicit note that lighting's `0x1F` and device-mode's `0x01` are two separately
  confirmed transaction ids.
- [`research.md`](https://github.com/ultramonaka/open-tartarus-driver/blob/HEAD/research.md)
  §1–2 — interface table, unlock field layout, the one-shot/standby behaviour, and the
  PR #2710 risk cross-reference.
- `README.md` / `USAGE.md` — double-input troubleshooting confined to D-pad/wheel/middle
  click (the §6 inference).

### OpenRazer upstream
- [PR #2710](https://github.com/openrazer/openrazer/pull/2710) — Tartarus Pro support, merged
  2026-03-14; both stability fixes and the maintainer's *"the firmware knows the command, but
  it just does bad stuff with it"* review comment. The sole public source for the reset
  behaviour.
- [Issue #1039](https://github.com/openrazer/openrazer/issues/1039) (61 comments),
  [Issue #1177](https://github.com/openrazer/openrazer/issues/1177),
  [PR #2336](https://github.com/openrazer/openrazer/pull/2336) (17 comments),
  [PR #2622](https://github.com/openrazer/openrazer/pull/2622) — searched for reset/reconnect/
  disconnect/loop/crash reports; **no hits**. Negative evidence for how widely the reset has
  actually been seen.
- [PR #1868](https://github.com/openrazer/openrazer/pull/1868) — unmerged Linux analog support
  for the Huntsman Mini Analog: Interface 1, `(key, analog)` pair layout, `analog_threshold`
  default 128, and the "keys only emit their analog values" framing.
