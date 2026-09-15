# 02 — Daemon asserts the active Profile's Lighting on connect and Daemon startup

**What to build:** The Daemon physically drives the backlight to match the active
Profile's stored `LightingAssignment` + `brightness`. It asserts that state on
Daemon startup and on **every** device (re)connect, sharing the existing `led`
task's control channel with Status LEDs so the two never interleave a write. A
user who hand-edits `[profiles.<name>.lighting]` and plugs the device in — or
restarts the Daemon — sees exactly that Profile's backlight light. Unlike Status
LEDs, there is **no** shutdown clear: backlight effect-select frames are VARSTORE
(firmware-persisted), so a clean Daemon exit leaves the last-asserted effect in
place. There is no D-Bus surface and no GUI in this ticket, and no `Effect`/`Edit`
plumbing yet — a Profile *switch* does not re-assert until the next ticket; only
the connect edge and startup do.

Source of truth: [`spec.md`](../../tartarus-backlight/spec.md) §"Daemon architecture"
and §"Startup / shutdown behaviour", and
[ADR-0012](../../../docs/adr/0012-lighting-shares-the-led-task-no-varstore-shutdown-clear.md).

**Blocked by:** 01

**Status:** ready-for-agent

- [ ] `assert_lighting(state: LightingState) -> io::Result<()>` in `capture/analog.rs`,
      a standalone function modelled on `assert_status_leds` — discover the
      Interface-2 control node, open it, then:
  - `Off` / `FixedEffect` → one `HIDIOCSFEATURE` write, the effect-select frame
    per the spec's per-effect byte table (`BACKLIGHT_LED = 0x05`, `VARSTORE`,
    `transaction_id 0x1F` uniformly, including Breath).
  - `CustomLayout` → the two-step write-then-arm: `matrix_custom_frame`
    (`command_id 0x03`) carrying all 21 RGB triples, then `matrix_effect_custom`
    (`command_id 0x02`, effect `0x08`) to arm it — both writes on the same
    short-lived fd, not two separate task iterations.
  - Then a second, always-sent `HIDIOCSFEATURE` write for brightness
    (`command_id 0x04`, `arg0 = VARSTORE`, `arg1 = ZERO_LED (0x00)`,
    `arg2 = brightness`) — regardless of which assignment branch ran above.
  - Drop the fd. No read-back, no retry loop, no driver-mode call, no unlock.
    Device absent ⇒ `Err(io::ErrorKind::NotFound)`, exactly like `relock()`. Byte
    layout is settled by `research/backlight-wire-protocol.md` and verified on
    hardware (charting ticket 02) — do not re-derive.
- [ ] `main.rs` creates a second `tokio::sync::watch::channel::<Option<LightingState>>(None)`
      (`lighting_tx`/`lighting_rx`) alongside the existing `led_tx`/`led_rx`, and
      hands both to `led::spawn`.
- [ ] `led::run`'s loop becomes `loop { tokio::select! { ... } }` over both
      receivers. Each arm still `borrow_and_update()`s → `spawn_blocking`s the
      write → **awaits** it before the loop repeats — one write in flight at a
      time across both channels, regardless of which one woke it. The existing
      `StatusLeds` arm is otherwise untouched.
- [ ] The `lighting_tx` sender is threaded into the dispatch task's runtime-state
      struct alongside `led_tx`. A private dispatch helper `push_lighting(&self,
      config: &Config)` reads `config.active_profile().lighting` and `.brightness`
      and sends `Some(LightingState { assignment, brightness })` on
      `self.lighting_tx`. `Config` is the sole authoritative source — **no cached
      `lighting_state`** in dispatch state.
- [ ] In `dispatch::run`'s `rx_connection` select arm, after
      `handle_connection_change(...)`, call `push_lighting(&config)` on **every**
      message where `connected == true` — same trigger and reasoning as
      `push_status_leds`, called alongside it. No flag, no dependence on the
      connection *transition*.
- [ ] **No pre-loop assertion** in `dispatch::run`'s init block — the connect edge
      covers present-at-startup, same as Status LEDs.
- [ ] **No change to `relock_and_exit`** — it keeps clearing Status LEDs but gets
      no Lighting equivalent. This is the deliberate VARSTORE-driven asymmetry;
      do not add a `clear_lighting()` call.
- [ ] Works identically in Analog and Digital Capture mode — `assert_lighting`
      opens its own Interface-2 fd regardless of what capture is doing, and never
      sends a driver-mode command.
- [ ] Tests exercise the `led`-task seam (the second `watch<Option<LightingState>>`
      channel), not the `HIDIOCSFEATURE` write: a `Some(state)` on the channel
      drives exactly one assert with that state; every `connected == true` from
      the connection channel re-pushes the active Profile's Lighting; a burst
      coalesces to the final state; a dedicated test confirms both channels'
      writes are serialised (no interleaved partial frames) when both fire in the
      same tick. The frame bytes are already hardware-verified (charting tickets
      01–02); they are not re-verified here.
