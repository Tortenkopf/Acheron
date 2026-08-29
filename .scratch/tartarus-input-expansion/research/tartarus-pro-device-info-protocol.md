# Research: reading firmware version and serial number from the Tartarus Pro over Linux `hidraw`

Ticket: [100-research-firmware-serial-read-protocol](../issues/100-research-firmware-serial-read-protocol.md)

Sibling of [the analog grid-key protocol note](./linux-analog-grid-key-protocol.md) — same
device, same Interface-2 `hidraw` control channel, same 90-byte `razer_report` frame and CRC.
That note covers the *write* side (the `set_device_mode` unlock). This one covers the two
*read* commands the About dialog (ticket 102) needs, so [ticket 101](./101-task-daemon-device-firmware-serial.md)
can implement the Daemon-side read without re-deriving anything.

## Bottom line

**Implementation-ready, and lower-risk than the analog unlock.** Both reads are the standard
Razer "get" commands that OpenRazer issues to *every* Razer keyboard, and both are already
confirmed working on our exact unit and firmware:

1. **The protocol is fully pinned from primary source.** Firmware = `command_class 0x00`,
   `command_id 0x81`, `data_size 0x02`; serial = `command_class 0x00`, `command_id 0x82`,
   `data_size 0x16`. Both come straight out of OpenRazer's
   `razer_chroma_standard_get_firmware_version()` / `razer_chroma_standard_get_serial()`
   (`razerchromacommon.c:67` / `:59`, installed locally at
   `/usr/src/openrazer-driver-3.12.4/driver/`). The frame, the CRC (XOR of struct bytes
   2..87, i.e. buffer indices 3..88, stored at index 89) and the Interface-2 transport are
   **byte-identical** to the analog unlock Acheron already sends — `build_razer_cmd` in
   `daemon/src/capture/analog.rs` builds the request unchanged; only `command_id`,
   `data_size` and the readback differ.

2. **The read already round-trips real data on our unit.** Ticket 12 / the analog note §1.3
   recorded that OpenRazer's Interface-2 query path returns this device's true serial
   (`PM2443F36300141`, matching Synapse on Windows) and firmware (`v1.2`). OpenRazer issues
   both of those reads with `transaction_id = 0xFF` (`razerkbd_driver.c:2044` and `:2067`,
   hardcoded, no Tartarus-Pro override) — so **`0xFF` is the empirically-confirmed value**
   for these two commands on our hardware, not a guess.

3. **The one genuinely new thing for Acheron is the readback.** Every `hidraw` call in the
   codebase and in `prototype/13-analog-grid-capture/prototype.py` so far is `HIDIOCSFEATURE`
   (write only). Reading firmware/serial needs a `HIDIOCGFEATURE` round-trip, which Acheron
   has never done. The ioctl number, buffer layout and offset handling are all worked out
   below (§3), but the SET→GET timing and the response-validation details are the parts
   ticket 101 should confirm live.

4. **Reset risk is negligible for these two commands** (§5). PR #2710's Tartarus-Pro reset
   concern is exclusively about `set_device_mode` (`command_id 0x04`, a *write*). The get
   commands change no device state, OpenRazer reads the serial on *every* device connect for
   every Razer keyboard, and there is no carve-out anywhere in OpenRazer for reading
   firmware/serial on the Tartarus Pro.

---

## 1. The two commands, from primary source

Both are "standard" (`command_class 0x00`) get commands. `command_id` has its top bit set
(`0x8_`), which in Razer's protocol marks direction Device→Host
(`razercommon.h`, `union command_id_union` / the struct comment: *"Get LED 0x80, Set LED
0x00"*).

| Read | `command_class` | `command_id` | `data_size` | OpenRazer builder |
|---|---|---|---|---|
| Firmware version | `0x00` | `0x81` | `0x02` (2) | `razer_chroma_standard_get_firmware_version()` — `razerchromacommon.c:67` |
| Serial number | `0x00` | `0x82` | `0x16` (22) | `razer_chroma_standard_get_serial()` — `razerchromacommon.c:59` |

`data_size` on a get command is the number of response bytes expected back in `arguments`
(the request itself carries no argument bytes — they are all `0x00`). Source: the
`struct razer_report` comment in `razercommon.h` — *"Data Size is the size of payload"* — and
`get_razer_report(command_class, command_id, data_size)` which sets these three fields and
nothing else.

GitHub (same content as the local v3.12.4 tree):
<https://github.com/openrazer/openrazer/blob/v3.12.4/driver/razerchromacommon.c>

---

## 2. Exact request buffer — all 91 bytes

Identical construction to `daemon/src/capture/analog.rs::build_razer_cmd` (a leading
`hidraw` report-number byte `0x00` + the 90-byte `struct razer_report`). Only the four
non-zero payload fields and the CRC change between the unlock and these reads.

### Firmware version request (`command_id 0x81`)

| Buf idx | Struct byte | Field | Value |
|---|---|---|---|
| 0 | — | `hidraw` report number | `0x00` |
| 1 | 0 | `status` | `0x00` |
| 2 | 1 | `transaction_id` | **`0xFF`** (primary — see §4) |
| 3–4 | 2–3 | `remaining_packets` (BE u16) | `0x0000` |
| 5 | 4 | `protocol_type` | `0x00` |
| 6 | 5 | `data_size` | **`0x02`** |
| 7 | 6 | `command_class` | `0x00` |
| 8 | 7 | `command_id` | **`0x81`** |
| 9–88 | 8–87 | `arguments[0..80]` | all `0x00` |
| 89 | 88 | `crc` = XOR of buf[3..=88] | **`0x83`** (`0x02 ^ 0x81`) |
| 90 | 89 | `reserved` | `0x00` |

### Serial number request (`command_id 0x82`)

Same buffer, with:

| Buf idx | Struct byte | Field | Value |
|---|---|---|---|
| 6 | 5 | `data_size` | **`0x16`** |
| 8 | 7 | `command_id` | **`0x82`** |
| 89 | 88 | `crc` = XOR of buf[3..=88] | **`0x94`** (`0x16 ^ 0x82`) |

CRC arithmetic by hand (the only non-zero bytes inside the CRC range `buf[3..=88]` are
`data_size`, `command_class`, `command_id`; `transaction_id` at buf idx 2 is **outside** the
range, exactly as the analog note §2 notes):

- firmware: `0x02 ^ 0x00 ^ 0x81 = 0x83`
- serial:   `0x16 ^ 0x00 ^ 0x82 = 0x94`

The CRC range is `for(i = 2; i < 88; i++)` over the struct in `razer_calculate_crc()`
(`razercommon.c`) — struct bytes 2..87 inclusive → buffer indices 3..88 inclusive → stored
at struct byte 88 / buffer index 89. This matches `analog.rs` (`buf[3..89]` folded, written
to `buf[89]`) and `prototype.py`'s `cmd_selftest`. Two independent implementations
(OpenRazer kernel, `open-tartarus-driver` `main.rs:576-578`) agree on the range.

> Note: OpenRazer's in-kernel loop upper bound is `i < 88`, so it XORs struct bytes 2..87
> and **excludes** byte 88 (the CRC slot itself) and byte 89 (reserved). The ticket's
> shorthand "XOR of bytes [2..88]" is the same range in half-open notation. Confirmed
> against `razercommon.c` `razer_calculate_crc()` directly.

---

## 3. Linux translation — exact calls

### 3.1 Which node

The Interface-2 `hidraw` node — the same one `analog.rs` opens for the unlock
(`CONTROL_INTERFACE: u8 = 2`, resolved via `/sys/class/hidraw` walk, matching on
`bInterfaceNumber == 02` under VID `1532` / PID `0244`). `analog.rs::discover_hidraw()` /
`relock()` already do exactly this discovery and open the node `read(true).write(true)`.

OpenRazer's `razer_get_report_params()` sets `report_index = response_index = 0x02` for the
Tartarus Pro (`razerkbd_driver.c:382-387`). That index **is** the USB interface number —
opening the Interface-2 `hidraw` node and issuing `HIDIOCSFEATURE`/`HIDIOCGFEATURE` on it
makes the kernel send the `SET_REPORT`/`GET_REPORT` control transfer with
`wIndex = bInterfaceNumber = 2` automatically (`usbhid_get_raw_report` /
`usbhid_set_raw_report` use `interface->desc.bInterfaceNumber`). So there is nothing extra to
set — picking the right node *is* picking `report_index 0x02`.

### 3.2 The ioctls

From `asm-generic/ioctl.h` `_IOC(dir, type, nr, size)` with `dir = _IOC_WRITE|_IOC_READ`,
`type = 'H'` (0x48), `nr = 0x06` for SET / `0x07` for GET, `size = 91`:

```
HIDIOCSFEATURE(91) = 0xC05B4806     // write request (unchanged from analog.rs)
HIDIOCGFEATURE(91) = 0xC05B4807     // read request  (nr 0x06 -> 0x07)
```

`analog.rs::hidiocsfeature()` already computes `0xC05B4806`; the GET variant is the same
`const fn` with the final `| 0x06` changed to `| 0x07`. `0xC05B4806` is checked against the
documented value in `analog.rs`'s own test (`hidiocsfeature_91_matches_the_documented_ioctl_number`);
`0xC05B4807` differs only in the low nibble.

### 3.3 The readback and its buffer offset — the one subtle point

`HIDIOCGFEATURE` is called with a **91-byte** buffer whose byte 0 is the requested report
number (`0x00`, unnumbered). The kernel's `usbhid_get_raw_report()`
(`drivers/hid/usbhid/hid-core.c`) does, for `report_number == 0`:

```c
if (report_number == 0x0) {
    /* Offset the return buffer by 1, so that the report ID
       will remain in byte 0. */
    buf++;
    count--;
    skipped_report_id = 1;
}
...
if (ret > 0 && skipped_report_id)
    ret++;
```

So the 90-byte response `struct razer_report` lands at **buffer indices 1..91**, byte 0 is
left as the `0x00` we put there, and the ioctl returns 91. This is **symmetric with the SET
buffer** — the same "`buf[0]` = report number, struct starts at `buf[1]`" layout in both
directions (`usbhid_set_raw_report()` does the identical `buf++; count--`). Source:
<https://github.com/torvalds/linux/blob/master/drivers/hid/usbhid/hid-core.c>

Response field offsets in the returned 91-byte buffer:

| Buf idx | Struct byte | Field | Use |
|---|---|---|---|
| 1 | 0 | `status` | `0x02` = success; see §4 for the tolerated values |
| 2 | 1 | `transaction_id` | echoed |
| 3–4 | 2–3 | `remaining_packets` | echoed (`0x0000`) |
| 6 | 5 | `data_size` | echoed request size (`0x02` / `0x16`) |
| 7 | 6 | `command_class` | echoed (`0x00`) — validate |
| 8 | 7 | `command_id` | echoed (`0x81` / `0x82`) — validate |
| 9.. | 8.. | `arguments[0..]` | the payload |
| 89 | 88 | `crc` | XOR of response struct bytes 2..87 |

### 3.4 Rendering

- **Firmware**: `format!("v{}.{}", buf[9], buf[10])` — `arguments[0]` major, `arguments[1]`
  minor, each a plain decimal byte. From
  `sprintf(buf, "v%d.%d\n", response.arguments[0], response.arguments[1])`
  (`razer_attr_read_firmware_version`, `razerkbd_driver.c:2070`). Our unit → `v1.2`.
- **Serial**: ASCII from `arguments[0..22]` (buffer indices 9..31), NUL-terminated. From
  `memcpy(&serial_string[0], &response.arguments[0], 22); serial_string[22] = '\0';`
  (`razer_attr_read_device_serial`, `razerkbd_driver.c:2048-2049`). OpenRazer copies a fixed
  22 bytes then forces a terminator at index 22, so the effective max is 22 chars. Our unit's
  serial `PM2443F36300141` is 15 chars; the remaining bytes are padding — OpenRazer does not
  document whether they are `0x00` or spaces, so ticket 101 should trim on the first `0x00`
  **and** `trim()` trailing whitespace to be safe. Treat non-ASCII / all-zero as "read
  failed" (mirrors `device_connected` going absent).

### 3.5 Reference sketch (untested against hardware — see §6)

```python
import fcntl, os

HIDIOCSFEATURE_91 = 0xC05B4806
HIDIOCGFEATURE_91 = 0xC05B4807

def build(txn, cls, cmd, data_size):
    buf = bytearray(91)
    buf[2], buf[6], buf[7], buf[8] = txn, data_size, cls, cmd
    crc = 0
    for b in buf[3:89]:
        crc ^= b
    buf[89] = crc
    return bytearray(buf)   # mutable: GETFEATURE writes back into it

def read_report(fd, txn, cmd, data_size):
    req = build(txn, 0x00, cmd, data_size)
    fcntl.ioctl(fd, HIDIOCSFEATURE_91, req)
    time.sleep(0.002)                      # settle (see §6)
    resp = bytearray(91)
    resp[0] = 0x00                         # requested report number
    fcntl.ioctl(fd, HIDIOCGFEATURE_91, resp)
    # resp[1..91] is the 90-byte struct; args start at resp[9]
    assert resp[7] == 0x00 and resp[8] == cmd, "class/id echo mismatch"
    return bytes(resp[9:9 + data_size])

ctrl = os.open("/dev/hidrawN", os.O_RDWR)  # interface 2, resolved via sysfs
fw   = read_report(ctrl, 0xFF, 0x81, 2)    # -> b"\x01\x02"  => "v1.2"
ser  = read_report(ctrl, 0xFF, 0x82, 22)   # -> b"PM2443F36300141\x00..."
```

---

## 4. Gap 1 — the `transaction_id`

### Primary candidate: `0xFF`

`razer_attr_read_firmware_version()` and `razer_attr_read_device_serial()` both hardcode
`request.transaction_id.id = 0xFF` (`razerkbd_driver.c:2067` and `:2044`) with **no
per-device switch** — unlike `razer_set_device_mode()` (`:503`), which has a big `switch
(device->usb_pid)` and gives the Tartarus Pro `0x1F`. The get-firmware / get-serial paths
have no such switch anywhere in the file.

This is not just "what OpenRazer sends" — it is **confirmed working on our exact unit**:
ticket 12 / the analog note §1.3 read `firmware_version` = `v1.2` and `device_serial` =
`PM2443F36300141` straight out of OpenRazer's sysfs on this machine, and that path sends
`0xFF`. So `0xFF` is the value with a positive hardware result behind it.

### Fallback: `0x1F`

If `0xFF` comes back with a class/id echo mismatch, a bad CRC, or `status` ∈
{`0x03` failure, `0x05` not-supported}, retry with `0x1F` — the Tartarus-Pro-specific
`transaction_id` OpenRazer uses for this device's *other* Interface-2 control commands
(`set_device_mode` at `:542`, and essentially all lighting/brightness/matrix commands, e.g.
`:1665`, `:2110`, `:2281`). `open-tartarus-driver`'s `research.md` calls the `0x1F`-vs-`0xFF`
split *"a frequent source of confusion when cross-referencing OpenRazer source"*.

### Not the unlock's `0x01`

The analog unlock uses `transaction_id = 0x01` (`open-tartarus-driver` `main.rs:685`,
captured from Synapse). There is **no evidence** that value applies to the standard get
commands — `open-tartarus-driver` never reads firmware or serial at all
(confirmed: no such code in `main.rs`), and its `0x01` is documented specifically as the
device-mode constant, distinct from its `0x1F` lighting constant (`lighting.rs`, per the
analog note §5). `0xFF` already has a hardware result; `0x01` would be a speculative third
option only worth trying if both `0xFF` and `0x1F` fail.

### `transaction_id` PID note

The ticket mentions "Tartarus V2 (PID 0x0208)". OpenRazer's `razerkbd_driver.h` actually
defines `USB_DEVICE_ID_RAZER_TARTARUS_V2 = 0x022B` and `USB_DEVICE_ID_RAZER_TARTARUS_CHROMA
= 0x0208` — `0x0208` is the *Chroma*, not the V2. The Tartarus Pro is `0x0244`. The V2
(`0x022B`) is grouped with the Pro in most Interface-2 command switches and also gets `0x1F`
for `set_device_mode` via the same case label, which is corroborating context for the `0x1F`
fallback but not itself about the get commands.

### **Ticket 101 must confirm on real hardware.** ###

The `0xFF` result above is via OpenRazer's `usb_control_msg` path. Acheron will read through
`HIDIOCGFEATURE` on the `hidraw` node — the same underlying USB control transfer, but a code
path Acheron has never exercised. Ticket 101's live check: send get-firmware with `0xFF`,
read back, verify `buf[7]==0x00 && buf[8]==0x81` and `buf[9..11] == [1, 2]` (matching the
known `v1.2`); if not, fall back to `0x1F`. The serial check is `buf[9..24]` ASCII ==
`PM2443F36300141`.

---

## 5. Gap 2 — SET→GET wait and response validation (OpenRazer)

### The wait

OpenRazer's `razer_send_control_msg()` (`razercommon.c`) issues the `SET_REPORT`, then
`usleep_range(wait_min, wait_max)` **before** `razer_get_usb_response()` issues the
`GET_REPORT`. For the Tartarus Pro, `razer_get_report_params()` sets
`wait_min = RAZER_BLACKWIDOW_CHROMA_WAIT_MIN_US = 600`,
`wait_max = ..._MAX_US = 800` (`razerkbd_driver.h:164-165`, `razerkbd_driver.c:385-386`).

So: **~600–800 µs** between the SET and the GET. There is no primary source for the minimum
that works from *userspace* `hidraw` on this device specifically (kernel timing and userspace
timing differ in scheduling jitter). Recommend ticket 101 use **≥ 1 ms**, and retry the GET
2–3 times with a short backoff (e.g. 1 ms, 3 ms, 10 ms) if the first readback fails
validation — cheap, and covers a slow first response after connect.

### Validation

`razer_send_payload()` (`razerkbd_driver.c:437`) checks, in order:

1. `razer_get_report()` returned 0 (i.e. the `GET_REPORT` returned exactly 90 bytes —
   `razer_get_usb_response()` sets `result = 1` and logs *"Invalid USB response"* otherwise).
2. **Echo match**: `response->remaining_packets == request->remaining_packets &&
   response->command_class == request->command_class &&
   response->command_id.id == request->command_id.id` — else `print_erroneous_report(...)`
   and return `-EIO`. This is the primary "is this the reply to my request" check.
3. **Status byte** (`response->status`, buffer index 1):
   - `0x01` `RAZER_CMD_BUSY` → **break (ignored)**. Not an error, not retried. The code
     comment even questions whether it should be an error. So OpenRazer treats a BUSY
     response as usable.
   - `0x02` `RAZER_CMD_SUCCESSFUL` → the implicit good path (falls through the switch).
   - `0x03` `RAZER_CMD_FAILURE` → `-EIO`.
   - `0x05` `RAZER_CMD_NOT_SUPPORTED` → `-EIO`.
   - `0x04` `RAZER_CMD_TIMEOUT` → `-EIO`.

Constants from `razercommon.h` (`RAZER_CMD_*`). There is **no CRC check on the response** in
OpenRazer's keyboard path — it validates by the class/id/packet echo and the status byte
only. Note `razer_get_usb_response()` will also rewrite the request's `transaction_id` to
`0xFF` if it was left `0x00` (`WARN_ON` + assignment) — so `0x00` is never actually sent;
Acheron must pick a real value.

**Recommendation for ticket 101**: after readback, require the class/id echo to match
(`buf[7] == 0x00`, `buf[8] == command_id`); treat `status` ∈ {`0x00`, `0x01`, `0x02`} as OK
and anything else as a failed read; do not bother checking the response CRC (OpenRazer
doesn't, and a get with a good echo is reliable in practice). A failed read → the
firmware/serial `GetState()` keys are simply absent, exactly like `device_connected`.

---

## 6. Gap 3 — does the read work in analog Capture mode, and what's the safe ordering

### Reasoning

- The reads go to **Interface 2**, the persistent control channel. The analog unlock is a
  transient one-shot feature-report write on the same interface (analog note §2: *"one-shot,
  not a polling relationship"*); it switches what **Interface 1** streams, and does not
  "hold" Interface 2 in any special state.
- OpenRazer reads `firmware_version` / `device_serial` regardless of device mode, and on our
  unit those reads succeed while the device sits in normal mode. Ticket 16 additionally
  showed `get_device_mode` (`command_id 0x84`, the same `command_class 0x00` family)
  round-trips in **both** modes.
- So the read almost certainly works in either mode. But there is no primary source that
  *specifically* confirms get-firmware/get-serial mid-analog-stream on the Tartarus Pro, and
  no reason to depend on it.

### Recommended ordering for ticket 101

**Read firmware and serial once, at device-connect, on a short-lived Interface-2 fd, before
(and independently of) any analog unlock.** Concretely:

- Do it on a code path that runs on **every** connect, not only when analog capture is
  attempted — even in forced-digital mode or when the analog unlock fails, the Interface-2
  `hidraw` node exists and is openable, and the About dialog still wants the fields. The
  natural shape is a one-shot helper that opens its own fd (like
  `analog.rs::relock()` already does: `discover_hidraw()` → open
  `CONTROL_INTERFACE` `read+write` → ioctl → drop), called from the supervisor's
  device-connect handling, result cached and pushed into `GetState()`.
- If it is instead folded into `grid_task_blocking`, put the two reads **immediately after
  the control fd is opened and before `send_unlock()`** (there is already a natural seam
  there — `read_repeat_schedule()` is called at that exact point). Never interleave a
  get with the `set_device_mode` write.
- The reads are idempotent and cheap; if the first attempt fails (device mid-enumeration,
  udev rule not yet applied), just retry on the next connection event rather than blocking.

---

## 7. Gap 4 — reset / reconnect risk from these two commands

**Verdict: negligible — materially lower than the already-low `set_device_mode` risk.**

- **These are reads.** `command_id 0x81` / `0x82` change no device state. PR #2710's entire
  reset concern is about `set_device_mode` (`command_id 0x04`), a *write* — both "bug fixes"
  in that PR (`DRIVER_MODE = False`; skip the probe-time `razer_set_device_mode()`) target
  that one command, and the probe carve-out is literally
  `if (idProduct != TARTARUS_PRO) razer_set_device_mode(dev, 0x00, 0x00);`
  (`razerkbd_driver.c:5500-5502`). Nothing analogous exists for the get commands.
- **OpenRazer reads the serial on every single connect** for every Razer keyboard. Our
  Tartarus Pro has been through that path many times (every `openrazer-daemon` start, every
  replug) with no reset in the record.
- **Confirmed benign on our unit**: `firmware_version` and `device_serial` both read clean
  via OpenRazer on this machine (ticket 12 / analog note §1.3). Nine-plus clean
  `set_device_mode` sends across tickets 13/16 never reset the device either — and those are
  the *risky* command; the reads are strictly safer.
- **Cross-check, same method as ticket 12**: searched the Tartarus Pro paper trail
  (OpenRazer issues [#1039](https://github.com/openrazer/openrazer/issues/1039),
  [#1177](https://github.com/openrazer/openrazer/issues/1177), PRs
  [#2336](https://github.com/openrazer/openrazer/pull/2336),
  [#2622](https://github.com/openrazer/openrazer/pull/2622),
  [#2710](https://github.com/openrazer/openrazer/pull/2710)) — not one report of a reset,
  disconnect or loop tied to reading firmware or serial, on this device or any other. Ticket
  12's conclusion stands: the only Tartarus-Pro reset evidence anywhere is PR #2710's
  single-contributor report about `set_device_mode`, plausibly a wrong-`transaction_id`
  artefact. It does not touch the read path.

The one residual, shared with the analog work: if a *wrong* `transaction_id` provokes odd
firmware behaviour on this device (the #2710 hypothesis), then trying `0x1F` after `0xFF`
is a second unusual value sent. Mitigation: `0xFF` is the confirmed-working value, so the
fallback should rarely fire; and ticket 101 should watch `dmesg -w` / `udevadm monitor`
during its first live read, exactly as ticket 13 did, and record a reset if one happens.

---

## 8. What could not be pinned to a primary source

1. **`HIDIOCGFEATURE` round-trip on the Tartarus Pro specifically.** `0xFF` firmware/serial
   reads are confirmed on our unit *via OpenRazer's `usb_control_msg` path*. The `hidraw`
   `HIDIOCGFEATURE` path is the same underlying USB control transfer but has never been run
   against this device by Acheron or (as far as the source shows) by `open-tartarus-driver`.
   High confidence it works; ticket 101 must verify.
2. **Minimum userspace SET→GET delay for this device.** OpenRazer's in-kernel value is
   600–800 µs; the safe userspace minimum is not documented anywhere for the Tartarus Pro.
   §5's "≥ 1 ms + retry" is a recommendation, not a sourced fact.
3. **Response `status`-byte behaviour on the Tartarus Pro for get commands.** OpenRazer
   checks it and tolerates BUSY, but whether this device populates it meaningfully (vs
   leaving `0x00`) for `0x81` / `0x82` is unconfirmed without hardware.
4. **Serial padding bytes.** OpenRazer copies a fixed 22 bytes and NUL-terminates at index
   22; it does not say whether unused trailing bytes are `0x00` or `0x20`. Ticket 101 should
   trim defensively (first `0x00`, then trailing whitespace).
5. **`open-tartarus-driver` behaviour** — it simply never reads firmware or serial, so it
   offers no cross-check for these commands (only for the shared frame/CRC, which it
   corroborates).

---

## Sources

### First-hand / prior tickets
- Ticket 12 / [analog grid-key protocol note](./linux-analog-grid-key-protocol.md) §1.3, §2,
  §5 — our unit's serial `PM2443F36300141` and firmware `v1.2` read via OpenRazer's
  Interface-2 path; the shared 90-byte frame, CRC range and `HIDIOCSFEATURE(91) =
  0xC05B4806`; the `transaction_id` split and reset-risk analysis.
- `daemon/src/capture/analog.rs` — `build_razer_cmd`, `hidiocsfeature`, `discover_hidraw`,
  `relock` (the fresh-fd Interface-2 open pattern), `CONTROL_INTERFACE = 2`.
- Tickets 13 / 16 raw captures (`assets/13-unlocked.jsonl`, `assets/16-driver-mode-facts.jsonl`)
  — `get_device_mode` round-trips in both modes; nine-plus clean `set_device_mode` sends, no
  reset.

### OpenRazer 3.12.4 (installed at `/usr/src/openrazer-driver-3.12.4/driver/`, = GitHub tag `v3.12.4`)
- `driver/razerchromacommon.c:59` `razer_chroma_standard_get_serial()` → `get_razer_report(0x00, 0x82, 0x16)`.
- `driver/razerchromacommon.c:67` `razer_chroma_standard_get_firmware_version()` → `get_razer_report(0x00, 0x81, 0x02)`.
- `driver/razercommon.h` — `struct razer_report` layout; `RAZER_CMD_*` status constants;
  `union command_id_union` direction bit; `RAZER_USB_REPORT_LEN 0x5A` (90).
- `driver/razercommon.c` — `razer_calculate_crc()` (`for i = 2; i < 88`), `razer_send_control_msg()`
  (`SET_REPORT`, `wValue 0x300`, `usleep_range(wait_min, wait_max)`), `razer_get_usb_response()`
  (`GET_REPORT`, `wValue 0x300`, `response_index`; rewrites `transaction_id 0x00 → 0xFF`).
- `driver/razerkbd_driver.c:338` `razer_get_report_params()` — Tartarus Pro (`case
  USB_DEVICE_ID_RAZER_TARTARUS_PRO`, `:382`) → `report_index = response_index = 0x02`,
  `wait_min/max = 600/800 µs` (`RAZER_BLACKWIDOW_CHROMA_WAIT_*`).
- `driver/razerkbd_driver.c:437` `razer_send_payload()` — CRC set on request; echo check
  (`remaining_packets` + `command_class` + `command_id.id`); `status` switch (BUSY ignored,
  FAILURE/NOT_SUPPORTED/TIMEOUT → `-EIO`); **no response-CRC check**.
- `driver/razerkbd_driver.c:2030` `razer_attr_read_device_serial()` — `transaction_id.id =
  0xFF` (`:2044`), `memcpy(serial_string, response.arguments[0], 22); serial_string[22] = '\0'`.
- `driver/razerkbd_driver.c:2060` `razer_attr_read_firmware_version()` — `transaction_id.id =
  0xFF` (`:2067`), `sprintf(buf, "v%d.%d\n", arguments[0], arguments[1])`.
- `driver/razerkbd_driver.c:503` `razer_set_device_mode()` — `switch (device->usb_pid)` with
  Tartarus Pro → `transaction_id 0x1F` (`:542`). No equivalent switch on the get paths.
- `driver/razerkbd_driver.c:5500` — probe carve-out: `set_device_mode(0x00,0x00)` skipped for
  the Tartarus Pro, comment *"Tartarus Pro resets when it receives this command"*. Applies to
  `command_id 0x04` only.
- `driver/razerkbd_driver.h` — `USB_DEVICE_ID_RAZER_TARTARUS_PRO 0x0244`,
  `USB_DEVICE_ID_RAZER_TARTARUS_V2 0x022B`, `USB_DEVICE_ID_RAZER_TARTARUS_CHROMA 0x0208`,
  `RAZER_BLACKWIDOW_CHROMA_WAIT_MIN_US 600` / `_MAX_US 800`.
- `openrazer_daemon/hardware/keyboards.py:211` `RazerTartarusPro` — `DRIVER_MODE = False`;
  `METHODS` list (lighting/brightness only; `get_firmware`/`get_serial` come from the
  always-on base `all.py` endpoints).
- `openrazer_daemon/dbus_services/dbus_methods/all.py:57` `get_firmware` — reads the
  `firmware_version` sysfs attr (which triggers the kernel read above).
- GitHub mirror: <https://github.com/openrazer/openrazer/blob/v3.12.4/driver/razerkbd_driver.c>,
  `.../razerchromacommon.c`, `.../razercommon.c`.

### Linux kernel
- `drivers/hid/usbhid/hid-core.c` — `usbhid_get_raw_report()` / `usbhid_set_raw_report()`:
  `report_number == 0` → `buf++; count--`, and for GET `ret++` on return, so the response
  struct sits at buffer offset 1 (symmetric with the SET buffer). `wIndex =
  interface->desc.bInterfaceNumber`. <https://github.com/torvalds/linux/blob/master/drivers/hid/usbhid/hid-core.c>
- `include/uapi/linux/hidraw.h` — `HIDIOCSFEATURE(len)` / `HIDIOCGFEATURE(len)` = `_IOC(RW,
  'H', 0x06|0x07, len)`.

### ultramonaka/open-tartarus-driver (Windows-only, GPL-3.0)
- [`tartarus_driver/src/main.rs`](https://github.com/ultramonaka/open-tartarus-driver/blob/HEAD/tartarus_driver/src/main.rs)
  — `build_razer_cmd()` (`:565`), CRC `for b in &buf[3..89]` (`:576-578`), the unlock send
  with `transaction_id 0x01` (`:685`). **No firmware or serial read anywhere in the file** —
  confirmed, so it corroborates only the shared frame/CRC.
- [`research.md`](https://github.com/ultramonaka/open-tartarus-driver/blob/HEAD/research.md)
  — the `0x1F` (Tartarus Pro lighting) vs `0xFF` (older Tartarus) `transaction_id` split and
  the "frequent source of confusion" note; the standard control-report structure; no get /
  firmware / serial content.

### OpenRazer upstream issue trail (negative evidence, searched for reset/reconnect/loop tied to reading firmware or serial — no hits)
- [PR #2710](https://github.com/openrazer/openrazer/pull/2710) (Tartarus Pro support + the
  `set_device_mode` reset carve-outs),
  [Issue #1039](https://github.com/openrazer/openrazer/issues/1039),
  [Issue #1177](https://github.com/openrazer/openrazer/issues/1177),
  [PR #2336](https://github.com/openrazer/openrazer/pull/2336),
  [PR #2622](https://github.com/openrazer/openrazer/pull/2622).
