Type: task
Status: open
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
