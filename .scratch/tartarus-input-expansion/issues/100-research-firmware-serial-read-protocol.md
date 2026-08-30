Type: research
Status: resolved
Assignee: Charon (research subagent, kicked off 2026-08-29)

## Question

Produce an implementation-ready protocol note for reading the **firmware version** and
**serial number** from the connected Tartarus Pro, so [ticket 101](./101-task-daemon-device-firmware-serial.md)
can build the Daemon-side read directly instead of re-deriving it. The About dialog (ticket
102) shows both fields.

**The desk research is largely done already** (charting pass, 2026-08-29) — this ticket
consolidates it into a write-up and closes the remaining gaps against primary sources:

- Both reads use the **standard Razer control report** on the **Interface-2 `hidraw` control
  channel that Acheron already opens for the analog unlock** (`daemon/src/capture/analog.rs`)
  — no new device node, no new permissions beyond the udev rule ticket 23 already installs.
- Report structure (from `ultramonaka/open-tartarus-driver` `research.md` and OpenRazer
  `razercommon.c`): status `[0]=0x00`, transaction_id `[1]`, remaining-packets `[2:4]` BE u16
  = 0, protocol_type `[4]=0x00`, data_size `[5]`, command_class `[6]`, command_id `[7]`,
  args `[8..]`, CRC `[88]` = XOR of bytes `[2..88]`, reserved `[89]=0x00`. Sent via
  `HIDIOCSFEATURE`; response read back via `HIDIOCGFEATURE` after a short delay.
- **Firmware version**: `command_class 0x00`, `command_id 0x81`, `data_size 0x02`. Response:
  `args[0].args[1]` → render as `vX.Y` (OpenRazer `razer_attr_read_firmware_version`).
- **Serial number**: `command_class 0x00`, `command_id 0x82`, `data_size 0x16`. Response:
  22 bytes of ASCII from `args[0..22]`, NUL-terminate (OpenRazer `razer_attr_read_device_serial`).

Close these before writing the note:

- **The exact `transaction_id` for the Tartarus Pro.** The analog unlock used `0x01`;
  OpenRazer's generic keyboard path uses `0xFF`; some Razer devices use `0x1F`/`0x3F`. Check
  OpenRazer's **Tartarus Pro / Tartarus V2-specific** driver entries and device matrix for
  the `transaction_id` those devices register, and read `ultramonaka/open-tartarus-driver`'s
  actual source (not just `research.md`) for what it sends. State a primary candidate and a
  fallback, and mark clearly that ticket 101 must confirm it on the real device.
- **Response-read timing / retry**: how long OpenRazer waits between `SETFEATURE` and
  `GETFEATURE`, and how it validates the response (does it echo class/id? a status byte?).
- **Does the read work while the device is in analog Capture mode** (post-unlock), or must
  it happen before the mode switch? The control channel is Interface 2 and the unlock is a
  transient command, so it likely works either way — confirm against the sources and note
  the safest ordering for ticket 101 (probably: read firmware/serial right after opening the
  control node, before sending the analog unlock).
- Any Tartarus-Pro-specific **reset/reconnect risk** from these two commands, cross-checked
  the same way ticket 12 checked the unlock command (that investigation found the risk traces
  to a single uncorroborated report and a likely wrong-`transaction_id` artefact).

Output: a new write-up alongside
[the analog protocol note](../research/linux-analog-grid-key-protocol.md), e.g.
`../research/tartarus-pro-device-info-protocol.md` — exact byte buffers, ioctl constants,
offsets. Does not touch Acheron's codebase.

## Answer

Full write-up: [tartarus-pro-device-info-protocol.md](../research/tartarus-pro-device-info-protocol.md).
Implementation-ready; every claim cited to primary source (OpenRazer 3.12.4 driver installed
locally, the Linux kernel `usbhid` source, and `open-tartarus-driver`'s Rust).

**The two commands** (from OpenRazer `razerchromacommon.c`):

| Read | `command_class` | `command_id` | `data_size` | request CRC |
|---|---|---|---|---|
| Firmware | `0x00` | `0x81` | `0x02` | `0x83` |
| Serial | `0x00` | `0x82` | `0x16` | `0x94` |

Request buffer is `analog.rs::build_razer_cmd` **unchanged** — same 91-byte layout, same
leading `0x00` report byte, same CRC = XOR of buf indices 3..=88 stored at index 89
(confirmed against `razer_calculate_crc()` `for i = 2; i < 88`, and against
`open-tartarus-driver` `main.rs:576`). Only `data_size` (buf[6]), `command_id` (buf[8]) and
the CRC differ from the unlock. Sent on the **Interface-2** `hidraw` node Acheron already
discovers/opens (`CONTROL_INTERFACE = 2`; that node choice *is* OpenRazer's
`report_index 0x02`).

**Readback** — new for Acheron (all existing `hidraw` calls are SET-only):
`HIDIOCGFEATURE(91) = 0xC05B4807` (`analog.rs::hidiocsfeature` with the final `| 0x06`
changed to `| 0x07`). Pass a 91-byte buffer, `buf[0] = 0x00`. The kernel's
`usbhid_get_raw_report` does `buf++; count--` for report number 0 and `ret++` on return, so
the 90-byte response struct lands at **buf[1..91]** — symmetric with the SET buffer.
Response fields: `status` buf[1], `command_class` echo buf[7], `command_id` echo buf[8],
`arguments[0..]` from **buf[9]**.

**Render**: firmware = `format!("v{}.{}", buf[9], buf[10])` (decimal bytes → our unit `v1.2`);
serial = ASCII `buf[9..31]`, trim on first `0x00` then trailing whitespace (our unit
`PM24XXXXXXXXXXX`).

**Gap 1 — `transaction_id`: primary `0xFF`, fallback `0x1F`.** OpenRazer's
`razer_attr_read_firmware_version` / `razer_attr_read_device_serial` both hardcode `0xFF`
with *no* per-device switch (unlike `razer_set_device_mode`, which gives the Pro `0x1F`), and
`0xFF` is already confirmed working on our exact unit — ticket 12 read `v1.2` /
`PM24XXXXXXXXXXX` out of OpenRazer's sysfs, which sends `0xFF`. Fallback `0x1F` is the
Tartarus-Pro-specific id OpenRazer uses for this device's other Interface-2 commands. Not the
unlock's `0x01` (no evidence it applies to standard get commands; `open-tartarus-driver`
never reads these). **Ticket 101 must confirm live** — our `0xFF` result is via OpenRazer's
`usb_control_msg`; the `hidraw` `HIDIOCGFEATURE` path (same USB transfer underneath) has
never been run against this device. (PID note: OpenRazer's `TARTARUS_V2` is `0x022B`;
`0x0208` is the *Chroma*.)

**Gap 2 — wait + validation.** OpenRazer waits `usleep_range(600, 800)` µs between SET and
GET (`RAZER_BLACKWIDOW_CHROMA_WAIT_*`, in-kernel). Use **≥ 1 ms** from userspace + 2–3
retries with backoff. `razer_send_payload` validates by **echo match** (`remaining_packets` +
`command_class` + `command_id`) then the **status byte**: BUSY `0x01` is *ignored/accepted*,
FAILURE `0x03` / TIMEOUT `0x04` / NOT_SUPPORTED `0x05` → error. **No response-CRC check.**
Recommend ticket 101 require the class/id echo, accept status ∈ {0x00, 0x01, 0x02}, skip the
CRC.

**Gap 3 — ordering: read at device-connect, before the analog unlock, on a short-lived
Interface-2 fd** (the `analog.rs::relock()` fresh-fd pattern), on a path that runs on *every*
connect regardless of capture mode. The reads almost certainly work in Capture mode too
(Interface 2 is persistent; ticket 16 showed `get_device_mode` round-trips in both modes) but
nothing should depend on it. Never interleave a get with the `set_device_mode` write.

**Gap 4 — reset risk: negligible.** These are reads; they change no device state. PR #2710's
reset concern is exclusively `set_device_mode` (`command_id 0x04`) and its carve-out is
`command_id`-specific. OpenRazer reads the serial on *every* connect for every Razer keyboard;
both reads are confirmed benign on our unit. Searched the Tartarus Pro issue trail (OpenRazer
#1039/#1177/#2336/#2622/#2710) — zero reports of any reset tied to reading firmware/serial.

**Could not pin to a primary source**: (1) that `HIDIOCGFEATURE` specifically works on the
Tartarus Pro (high confidence, same USB transfer as the confirmed OpenRazer path, but
untested by Acheron); (2) the minimum userspace SET→GET delay for this device; (3) whether
this device populates the response `status` byte meaningfully for `0x81`/`0x82`; (4) whether
unused serial bytes are `0x00` or spaces. All four are live-check items for ticket 101.
