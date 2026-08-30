Type: task
Status: resolved
Assignee: Charon
Blocked by: 100

## Question

Make the Daemon read the connected Tartarus Pro's **firmware version** and **serial number**
and expose them to clients, so the About dialog (ticket 102) can show them. Build directly
against [ticket 100](./100-research-firmware-serial-read-protocol.md)'s protocol note.

Settled in the charting grilling (2026-08-29) — do not re-litigate:

- **Read once, at device-connect.** The Daemon queries firmware + serial at the same
  lifecycle point it already opens the Interface-2 control node for the analog unlock
  (`daemon/src/capture/analog.rs` / `capture::supervisor`), caches the two strings, and
  re-reads on every fresh reconnect (a reconnect may be a physically different unit).
- **Surface via `GetState()`**, not a new method. Add two **optional** string keys —
  `firmware_version` and `serial_number` — to `state_to_dict` (`daemon/src/dbus/wire.rs`),
  **present when known, absent when the device is disconnected or the read failed**. This
  mirrors how `device_connected` / `capture_mode` already flow; the About dialog reads them
  straight from the state snapshot it already holds.
- No dedicated `GetDeviceInfo()` call — the data never changes within a connection.

Implement:

- The `hidraw` `HIDIOCSFEATURE`/`HIDIOCGFEATURE` exchange per ticket 100's note (report
  struct, CRC = XOR of `[2..88]`, `command_id 0x81` firmware → `vX.Y`, `command_id 0x82`
  serial → 22 ASCII bytes). Keep the byte-buffer construction and response parsing **pure
  and unit-tested** separately from the I/O, the same discipline tickets 22/18 used for the
  analog capture logic.
- Ordering: read firmware/serial **before** sending the analog unlock (ticket 100 confirms
  the safest order), so a digital-only / `force_digital` session still gets device info —
  the control channel is Interface 2, independent of Capture mode.
- Failure is non-fatal and silent: a failed or timed-out read leaves both keys absent, the
  Daemon logs it once and carries on (same posture as a missing udev rule degrading to
  Digital).
- Thread the two new keys through `GetState()`'s consumers: `daemon/src/dbus/mod.rs`,
  `gui/acheron_gui/daemon_client.py`, `daemon_stub.py`, and `app.py` — and give
  `daemon_stub.py` plausible fake values so the GUI/About dialog can be developed and
  screenshot-tested without a device.

**HITL — needs the real Tartarus Pro.** Ticket 100's `transaction_id` and response timing
are stated as candidates, not certainties; this session confirms them live. Follow the
`daemon/examples/analog_probe.rs` pattern for a throwaway probe (`examples/device_info_probe.rs`)
to nail the exchange down before wiring it into the capture path. Verify: correct firmware +
serial for the connected unit, both keys absent after an unplug, correct values again after
replug, and no reset/reconnect disturbance across repeated reads. Daemon + GUI suites green.

`config.toml` must be left byte-identical and the daily-driver daemon restored, per the
map's standing hardware-testing discipline.

## Answer

Built against [ticket 100](./100-research-firmware-serial-read-protocol.md)'s note and
**verified end-to-end on the real Tartarus Pro this session** — no separate verify ticket
needed (unlike tickets 22→24 / 26→27, the hardware run happened here). Every ticket-100
live-check item landed on the first attempt.

### What was built

- **`daemon/src/capture/analog.rs`** — the device-info read, pure/impure split the same way
  the capture logic is:
  - `hidiocsfeature`/`hidiocgfeature` refactored onto one `hidioc_feature(nr, size)` helper
    (`0x06` vs `0x07`); `hidiocgfeature(91) == 0xC05B4807` pinned by its own test.
  - Pure, unit-tested: `response_echoes` (class/id echo + tolerated status `0x00..=0x02`, no
    response-CRC — matches OpenRazer), `parse_firmware` (`v{args[0]}.{args[1]}`), `parse_serial`
    (22 ASCII bytes, trim at first NUL then trailing whitespace, reject non-ASCII/empty). The
    request buffers reuse `build_razer_cmd` unchanged — `firmware_request_matches_the_researched_bytes`
    / `serial_request_matches_the_researched_bytes` confirm `data_size`/`command_id`/CRC
    (`0x83` / `0x94`).
  - I/O: `feature_exchange` (one SET→GET with `[1, 3, 10]`ms backoff retries on the GET),
    `read_device_info()` — opens a fresh short-lived Interface-2 fd (the `relock()` pattern),
    tries `transaction_id 0xFF` then `0x1F`, logs which one worked. `pub` so the probe can call it.
    `DeviceInfo { firmware_version, serial_number }`.
- **`daemon/src/capture/supervisor.rs`** — on the connection edge into `connected` (once per
  connection, re-armed on disconnect so a physically different unit is re-read), spawns
  `spawn_blocking(analog::read_device_info)` and pushes `Some(info)` to dispatch over a new
  `device_info_tx` channel; pushes `None` on disconnect. Independent of `mode`, so
  forced-digital / analog-failed sessions still populate it. A failed read logs once and
  leaves the keys absent.
- **`daemon/src/command.rs` / `dispatch.rs` / `dbus/wire.rs`** — `State` gains
  `firmware_version: Option<String>` / `serial_number: Option<String>`; dispatch caches the
  latest `rx_device_info` value and folds it into `GetState()`; `state_to_dict` emits the two
  keys **only when `Some`** — absent otherwise, which the keyed dict (ticket 25) makes safe.
  No new D-Bus method, no signal (the About dialog reads a snapshot on open; the data never
  changes within a connection). `main.rs` wires the channel.
- **`daemon/examples/device_info_probe.rs`** — throwaway HITL probe (read-only, never sends
  `set_device_mode`, no relock needed), `cargo run --example device_info_probe [iterations]`.
- **`gui/acheron_gui/daemon_stub.py`** — `_firmware_version = "v1.2"` / `_serial_number =
  "PM2443F36300141"`, in `get_state()` only while `_device_connected` (so
  `simulate_device_disconnected` exercises the absent-key path for ticket 102's screenshots).
- **`daemon_client.py` / `app.py`** — no change needed: `get_state()` returns the raw keyed
  dict, new optional keys pass straight through. `app.py`'s own consumption is ticket 102's
  job (the About dialog fetches its own `GetState()` snapshot).

### Live verification (real Tartarus Pro, this session)

| Check | Result |
|---|---|
| `transaction_id` | **`0xFF` (primary) — no fallback**. `0x1F` never needed. |
| firmware / serial | **`v1.2` / `PM2443F36300141`** — exact match to research §4. |
| exchange timing | ~11–12ms for both reads together (well under budget). |
| reset / re-enumeration | **none** — 13+ probe reads plus daemon reads; USB `power/connected_duration` climbed monotonically throughout. Confirms research §7 (these are reads, negligible risk). |
| works in analog Capture mode | **yes** — probe ran fine alongside the running daily-driver daemon (device already unlocked); our daemon read it with `capture_mode=analog`. Research gap 3 settled: no ordering dependence. |
| `GetState()` keys present when connected | `"firmware_version" s "v1.2"`, `"serial_number" s "PM2443F36300141"`. |
| both keys absent after unplug | **yes** — `device_connected=false`, both keys gone from `GetState()`. |
| correct values after replug | **yes** — re-read fired on reconnect (`daemon.log` shows a second "read device info … transaction_id 0xff"). |
| suites | daemon **380 passed / 0 failed** (new: analog device-info parsing/ioctl tests, a dispatch `GetState()` device-info round-trip test, a `wire.rs` absent-keys test), GUI **337 passed / 0 failed** (new: stub absent-when-disconnected test). `cargo clippy --all-targets` clean, `cargo fmt` applied. |
| hardware discipline | `config.toml` **byte-identical** (`md5 2a6249ee…` before and after); daily-driver `systemd --user` daemon stopped for the test and restarted. |

### Notes for ticket 102 (About dialog)

- Now **unblocked** (99 and 101 both resolved).
- Read the keys straight off a `GetState()` snapshot; show "Not connected" (or "—") for
  whichever is absent. Absent is the honest state — there is deliberately no optimistic
  placeholder.
- The stub already returns plausible values while "connected" and drops them on
  `simulate_device_disconnected`, so both dialog states are screenshot-testable with no device.
- Minor, accepted: during a `SetForceDigital` capture-source swap the supervisor briefly
  forwards `connected=false` then `true` (pre-existing behaviour — `device_connected` already
  flickers there), so the two keys blink absent→present and the device-info read re-runs once.
  Harmless (idempotent ~11ms read, no device-state change); only visible if the About dialog
  is open at the exact moment of a force-digital toggle.
