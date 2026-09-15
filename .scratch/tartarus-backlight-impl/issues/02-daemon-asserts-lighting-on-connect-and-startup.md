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

**Status:** done

- [x] `assert_lighting(state: LightingState) -> io::Result<()>` in `capture/analog.rs`,
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
- [x] `main.rs` creates a second `tokio::sync::watch::channel::<Option<LightingState>>(None)`
      (`lighting_tx`/`lighting_rx`) alongside the existing `led_tx`/`led_rx`, and
      hands both to `led::spawn`.
- [x] `led::run`'s loop becomes `loop { tokio::select! { ... } }` over both
      receivers. Each arm still `borrow_and_update()`s → `spawn_blocking`s the
      write → **awaits** it before the loop repeats — one write in flight at a
      time across both channels, regardless of which one woke it. The existing
      `StatusLeds` arm is otherwise untouched.
- [x] The `lighting_tx` sender is threaded into the dispatch task's runtime-state
      struct alongside `led_tx`. A private dispatch helper `push_lighting(&self,
      config: &Config)` reads `config.active_profile().lighting` and `.brightness`
      and sends `Some(LightingState { assignment, brightness })` on
      `self.lighting_tx`. `Config` is the sole authoritative source — **no cached
      `lighting_state`** in dispatch state.
- [x] In `dispatch::run`'s `rx_connection` select arm, after
      `handle_connection_change(...)`, call `push_lighting(&config)` on **every**
      message where `connected == true` — same trigger and reasoning as
      `push_status_leds`, called alongside it. No flag, no dependence on the
      connection *transition*.
- [x] **No pre-loop assertion** in `dispatch::run`'s init block — the connect edge
      covers present-at-startup, same as Status LEDs.
- [x] **No change to `relock_and_exit`** — it keeps clearing Status LEDs but gets
      no Lighting equivalent. This is the deliberate VARSTORE-driven asymmetry;
      do not add a `clear_lighting()` call.
- [x] Works identically in Analog and Digital Capture mode — `assert_lighting`
      opens its own Interface-2 fd regardless of what capture is doing, and never
      sends a driver-mode command.
- [x] Tests exercise the `led`-task seam (the second `watch<Option<LightingState>>`
      channel), not the `HIDIOCSFEATURE` write: a `Some(state)` on the channel
      drives exactly one assert with that state; every `connected == true` from
      the connection channel re-pushes the active Profile's Lighting; a burst
      coalesces to the final state; a dedicated test confirms both channels'
      writes are serialised (no interleaved partial frames) when both fire in the
      same tick. The frame bytes are already hardware-verified (charting tickets
      01–02); they are not re-verified here.

## Comments

Implemented as specced. `assert_lighting` lands in `capture/analog.rs` beside
`assert_status_leds`, translating `research/backlight-wire-protocol.md`'s
byte tables into `effect_args`/`custom_frame_args`/`custom_effect_arm_args`/
`brightness_args`, all funnelled through one `send_lighting_cmd` helper
(`LIGHTING_TXN`/`LIGHTING_CMD_CLASS` shared, only `cmd_id`/`args` vary per
call). `led::run` becomes a `tokio::select!` over `led_rx`/`lighting_rx`,
each with its own `open`/`closed` liveness flag so one channel closing can't
silence the other. `dispatch::push_lighting` mirrors `push_status_leds`
exactly and is called alongside it in the `rx_connection` arm's `connected ==
true` branch — no Profile-switch/`SetLighting` plumbing yet, per the ticket.

Code review (forked) flagged nine items; six were fixed, three declined:
- **Fixed:** moved `LightingState` from `capture/analog.rs` into
  `config.rs` beside `LightingAssignment`/`StatusLeds` — it was defined in
  the hardware-writer module and imported upward into `dispatch.rs`/
  `led.rs`/`main.rs`, inverted from `StatusLeds`'s precedent (defined beside
  `Profile`, consumed by the writer). Added pure frame-byte tests for every
  effect/tail/custom-frame/brightness builder (mirroring
  `status_led_frame_matches_the_hardware_verified_bytes`'s existing role —
  these pin the Rust translation of the already-hardware-verified byte
  table, not a re-verification against hardware). Extracted
  `send_lighting_cmd` to stop the `LIGHTING_TXN`/`LIGHTING_CMD_CLASS`
  envelope from being repeated at five call sites. Extracted
  `style_count_and_colours` to deduplicate Breath's/Starlight's colour-tail
  encoding. Made `custom_frame_args`'s `row_index` byte an explicit `= 0x00`
  rather than relying on zero-init. Added an `eprintln!` when either `led`
  channel closes unexpectedly (never happens in production; previously
  silent).
- **Declined:** deduplicating the two `tokio::select!` arms in `led.rs` —
  the ticket says the existing `StatusLeds` arm stays "otherwise untouched,"
  and the duplication is two call sites, not a real recurring pattern yet.
  The double `discover_hidraw()` walk when both channels fire on the same
  connect event — this is the ticket's own explicit design (`assert_lighting`
  "modelled on `assert_status_leds`," each with its own independent fd), and
  spec.md's "Startup / shutdown behaviour" section already accepts the
  `~1-2ms` discovery cost per assert as "not a hot path."
- **Not a bug:** the reviewer's claim that a failed write mid-`assert_lighting`
  "silently skips" the brightness write — it doesn't skip silently, it
  returns `Err` immediately via `?`, the same fail-fast, no-retry pattern
  every other write in this file (`relock`, `assert_status_leds`, the
  Custom-layout two-step itself) already uses. Tightened the doc comment's
  wording to make this explicit instead.

Daemon: 604 tests green (`cargo test`, `cargo clippy --all-targets -- -D
warnings`, `cargo fmt --check`). GUI unaffected by this ticket (no config
schema or D-Bus surface changes) — 569 tests still green
(`gui/.venv/bin/pytest gui/tests`).
