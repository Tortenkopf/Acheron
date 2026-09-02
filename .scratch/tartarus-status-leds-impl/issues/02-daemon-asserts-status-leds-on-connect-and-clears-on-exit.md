# 02 — Daemon asserts the active Profile's Status LEDs on connect, startup, and clean exit

**What to build:** The Daemon physically drives the three Status LEDs to match the
active Profile's stored assignment. It asserts that assignment on Daemon startup,
on **every** device (re)connect (the firmware reclaims the LEDs to its orange-only
default on every USB enumeration — this is a hard requirement, not an
optimisation), and clears all three LEDs on a clean Daemon exit. A user who
hand-edits `[profiles.<name>.status_leds]` and plugs the device in — or restarts
the Daemon — sees exactly that Profile's LEDs light; stopping Acheron leaves no
stale indicator lit. There is no D-Bus surface and no GUI in this ticket, and no
`Effect`/`Edit` plumbing yet — a Profile *switch* does not re-assert until the
next ticket; only the connect edge and shutdown do.

Source of truth: [`spec.md`](../../tartarus-status-leds/spec.md) §"Daemon
architecture", §"Startup / shutdown behaviour", and
[ADR-0006](../../../docs/adr/0006-status-leds-driven-from-dispatch-over-short-lived-hidraw-fds.md).

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] Two standalone functions in the daemon's `capture/analog` module, siblings
      of `relock()` / `read_device_info()`, **no changes to `build_razer_cmd` or
      `discover_hidraw`**:
  - `assert_status_leds(leds: StatusLeds) -> io::Result<()>` — discover the
    Interface-2 control node, open it read+write, send **one**
    `build_razer_cmd(0x1F, 0x0F, 0x02, &[0x00, 0x0B, 0x01, 0x00, 0x00, 0x01, r, g, b])`
    frame via `HIDIOCSFEATURE` (orange←`r`, green←`g`, blue←`b`; `0xFF` = on,
    `0x00` = off), drop the fd. No read-back, no retry loop, no driver-mode call,
    no unlock.
  - `clear_status_leds() -> io::Result<()>` — the same frame with `r = g = b = 0x00`.
  - Device absent ⇒ `Err(io::ErrorKind::NotFound)`, exactly like `relock()`.
  - Rationale for the exact bytes and "no driver mode" is settled in
    `spec.md` §"The wire frame" and verified on hardware (charting ticket 01) —
    do not re-derive.
- [ ] `main.rs` creates a `tokio::sync::watch::channel::<Option<StatusLeds>>(None)`
      and `tokio::spawn`s a dedicated **non-fatal** `led` task with the receiver.
      It is **not** a branch in `main.rs`'s top-level `tokio::select!` — an LED
      write failure must never exit the process.
- [ ] The `led` task loop: `rx.changed().await` → `borrow_and_update()` the latest
      `Option<StatusLeds>` → if `Some`, `spawn_blocking` the `assert_status_leds`
      call and await it (serialising writes within the task) → on `Err`, log once
      (device absent = `NotFound`, harmless) and keep running. `watch` semantics
      coalesce a burst to the final triple — no queue of stale writes, no
      out-of-order landing.
- [ ] The `watch::Sender` is threaded into the dispatch task and held on its
      runtime-state struct. A private dispatch helper `push_status_leds(&self,
      config: &Config)` reads `config.active_profile().status_leds` and sends
      `Some(leds)` on the channel. `Config` is the sole authoritative triple —
      **no cached `led_state`** in dispatch state.
- [ ] In `dispatch::run`'s `rx_connection` select arm, **after**
      `handle_connection_change(...)`, call `push_status_leds(&config)` on **every**
      message where `connected == true` — no flag, no dependence on the connection
      *transition* (dispatch's `device_connected` starts optimistically `true` and
      `handle_connection_change` early-returns on an unchanged bool, so hanging the
      assert off transition detection would miss the startup assert). The write is
      idempotent on the hardware; a redundant `true` costs one ioctl.
- [ ] **No pre-loop assertion** in `dispatch::run`'s init block — the device may
      not be present yet, and the connect edge covers present-at-startup.
- [ ] The SIGTERM/SIGINT shutdown path (`relock_and_exit`) sends the all-off frame
      via `clear_status_leds()` **before** `relock()` — best-effort,
      log-and-continue. **Not** on the supervisor's swap-away-from-analog
      `relock()` (the Daemon is still running there and the LEDs must keep showing
      the active Profile). Only a clean Daemon exit clears.
- [ ] Works identically in Analog and Digital Capture mode —
      `assert_status_leds` opens its own Interface-2 fd regardless of what capture
      is doing, and never sends a driver-mode command.
- [ ] Tests exercise the `led`-task seam (the `watch<Option<StatusLeds>>`
      channel), not the `HIDIOCSFEATURE` write: a `Some(triple)` on the channel
      drives exactly one assert with that triple; every `connected == true` from
      the connection channel re-pushes the active Profile's triple; a burst
      coalesces to the final triple. The frame bytes are already hardware-verified
      (charting ticket 01) and cross-checked by the prototype's `selftest`.
