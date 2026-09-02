Type: grilling
Blocked by: 01
Status: resolved (Charon, 2026-09-02)

## Question

Decide the **daemon-side architecture** for asserting a Profile's Status-LED assignment.
Grilling + domain-modeling against the real code and the [ticket 01](./01-prototype-status-led-controllability.md)
prototype result. Decisions only — no build.

**Settled inputs from [ticket 01](./01-prototype-status-led-controllability.md) / [ticket 02](./02-research-status-led-wire-protocol.md)** —
don't re-open:
- Frame = `build_razer_cmd(0x1F, 0x0F, 0x02, &[0x00, 0x0B, 0x01, 0x00, 0x00, 0x01, r, g, b])`,
  no helper change; off = same frame with the channel byte(s) `0x00`; **no driver-mode call**.
- **Nothing persists on the device** (neither storage byte survives re-enumeration; the
  firmware always reclaims the LEDs to orange-only). So asserting on daemon startup **and on
  every device (re)connect** is a **hard requirement**, not an optimisation.
- **No host-independent on-device keymap switch exists** on the Tartarus Pro → the
  "re-assert after an on-device keymap change" hook is **not needed** (was the last open
  bullet below; now closed).
- `0x82` read-back is reliable on our fw v1.2 but the design should still not depend on it
  cross-device — the daemon writes unconditionally on connect anyway.

Settle at least:

- **Who owns the Interface-2 `hidraw` handle for the LED write?** In Analog capture mode
  `capture::analog` already holds an Interface-2 handle for the unlock/control channel; in
  Digital mode nothing opens `hidraw` at all. Does the LED writer open its own short-lived
  handle per write, share the capture layer's, or does a new small owner (an "led" module)
  hold one for the daemon's lifetime? The write must work identically in **both** capture
  modes (a Status LED has nothing to do with grid capture).
- **Where does the write hook into the Profile-switch path?** `edit::Edit::SwitchProfile`
  mutates `Config` directly (`config.rs:529`). Is asserting the LEDs a new `edit::Effect`
  variant (like `ReconcileStepperCursor`), a side effect in the dispatch task, or something
  else? It must fire on *every* route to a new active Profile — GUI `SetActiveProfile`, a
  `SwitchProfile` Action, and daemon startup.
- **Startup + (re)connect assertion.** Where in daemon boot does the active Profile's
  Status-LED state get asserted, relative to device-connect and capture-source start? And the
  same assertion must fire on every reconnect (`connection_tx` path) — ticket 01 proved the
  firmware reclaims the LEDs on every enumeration, so this is not optional.
- **Device absent / disconnected.** LED writes no-op when no Tartarus Pro is connected
  (mirror analog's handling).
- **State ownership.** One frame drives all three channels, so a partial update needs the
  other two channels. Acheron always writes the full triple from the active Profile, so no
  cached `led_state` is strictly needed — confirm, or decide to cache (ticket 02's write-up
  keeps an authoritative triple as the safe cross-device choice).
- **Editing a non-active Profile's assignment.** Changing `status_leds` on a Profile that
  isn't active must *not* touch the hardware; only a switch to it (or already being on it)
  asserts. Confirm the write is gated on "is this the active Profile".

Also: does `CONTEXT.md` need any new runtime term (e.g. for the LED-writing component), and
is an ADR warranted for the hidraw-ownership choice?

Output: `## Answer` with the settled architecture; append the gist to the map's Decisions so
far; graduate or close the relevant Not-yet-specified item.

## Answer

**The settled daemon architecture.** Grilled + code-checked against `daemon/src/` on `dev`
(HITL, Charon, 2026-09-02). Decisions only — no build. Everything here feeds
[ticket 05](./05-write-status-led-spec.md) (the spec) and refines
[ticket 04](./04-gui-daemon-surface-for-status-leds.md) (which now becomes the frontier).

### 1. The write primitive — a standalone function, modelled on `relock()`

Add to `daemon/src/capture/analog.rs`, as siblings of the existing `relock()` (`:402`) and
`read_device_info()` (`:553`):

- `pub fn assert_status_leds(leds: StatusLeds) -> io::Result<()>` — `discover_hidraw()` →
  open `CONTROL_INTERFACE` (Interface 2) read+write → one
  `build_razer_cmd(0x1F, 0x0F, 0x02, &[0x00, 0x0B, 0x01, 0x00, 0x00, 0x01, r, g, b])`
  via `HIDIOCSFEATURE` → drop the fd. `r/g/b` are `0xFF`/`0x00` per channel. **No read-back,
  no retry loop** (unlike `feature_exchange` — we never GET), **no driver-mode call**, no
  unlock. One SET ioctl on a freshly-opened node.
- `pub fn clear_status_leds() -> io::Result<()>` — the same frame with `r = g = b = 0x00`
  (**not** `effect_none` — ticket 01 confirmed it ACKs but does nothing).

Rejected: a new `capture/led.rs` module — it would force `discover_hidraw` /
`build_razer_cmd` / `hidiocsfeature` to `pub(crate)` for no real separation gain. `relock()`
already establishes "a standalone control-channel write that isn't really capture" living in
`analog.rs`.

**What "short-lived Interface-2 fd" and "no driver mode" mean, and why** (for the spec /
ADR-0006):

- *Interface 2* is the Tartarus Pro's control `/dev/hidrawN` node — it takes Razer vendor
  feature reports (unlock, relock, device-info, lighting, actuation config). "Short-lived"
  vs "long-lived" is only the **fd lifetime**: the analog grid task holds an Interface-2
  handle continuously because it polls depth many times a second; `relock()` /
  `read_device_info()` open-send-close per call because they are occasional one-shots. The
  LED write is occasional (Profile switch, connect) and one-shot (one frame, no GET), so it
  takes the `relock()` shape. The payoff is **total independence from the capture layer**:
  in Digital capture mode nothing holds an Interface-2 handle at all, so a "share the
  capture handle" design would have nothing to share half the time and would have to thread
  an `Arc<Mutex<File>>` / channel across the supervisor↔dispatch boundary and handle
  "capture just swapped modes and dropped the handle" races. Short-lived sidesteps all of
  it.
- *Driver mode* is a **firmware state** (`command_class 0x00, command_id 0x04, arg 0x03`),
  unrelated to fds — it tells the device "a driver is in control, stop emitting normal HID
  events." Ticket 02 §5 + ticket 01 established the `0x0F/0x02` lighting frame works
  regardless: OpenRazer runs the Pro with `DRIVER_MODE = False` permanently (the Pro
  *resets itself* on the normal→driver transition — the exact risk this effort is cautious
  about), CommandPost sends no mode command, and ticket 01 lit the LEDs on hardware with
  `device_mode = 00 00`. So the LED writer **never** sends `0x00/0x04`.

### 2. The writer — a dedicated non-fatal `led` task

A new task, sibling of `injector`, owning the actual device writes:

- `main.rs` creates a `tokio::sync::watch::channel::<Option<StatusLeds>>(None)` and
  `tokio::spawn`s the `led` task with the receiver. It is **not** a branch in `main.rs`'s
  top-level `tokio::select!` — an LED write failure must never exit the process (contrast
  capture/injector/dispatch). Failures are logged once, the task keeps running.
- The task loop: `rx.changed().await` → `borrow_and_update()` the latest `Option<StatusLeds>`
  → if `Some`, `tokio::task::spawn_blocking(move || analog::assert_status_leds(leds))` and
  await *that* (serialising writes within the task) → on `Err`, log once (device absent =
  `NotFound`, harmless). `watch` semantics mean a burst of Profile switches coalesces to the
  final triple — no queue of stale writes.
- The task also performs the **shutdown all-off** write (see §6).

Chosen over (b) a detached `tokio::spawn(spawn_blocking)` per assert straight from dispatch
(a fast A→B→A switch burst could land three writes out of order) and (c) awaiting
`spawn_blocking` inline on the dispatch loop (~1–2 ms stall per switch/connect, and
`discover_hidraw` walks sysfs). The dedicated task is the same shape as `injector` (a
separate task dispatch pushes to) and `actuation_tx` / `depth_tx` (a `watch` channel of
latest state) — it inherits ordering, burst-coalescing, and off-loop execution for free.

### 3. The decider — dispatch, sole owner

Dispatch is the only component that decides *what* the LEDs should show; it owns `Config`,
and the supervisor stays entirely LED-agnostic.

- **`Config` is the authoritative triple.** No cached `led_state` in `DispatchState`.
  `config.active_profile().status_leds` *is* the authoritative triple; every assert sends
  the full frame read from it. Ticket 02's "keep an authoritative triple" concern (partial
  updates needing the other two channels' last values) does not arise — there are no partial
  writes, and there is no non-active-Profile write path (§5).
- **`Effect::AssertStatusLeds`** — a new **unit** variant in `daemon/src/edit.rs`'s `Effect`
  enum, alongside `ReconcileStepperCursor` / `AnnounceProfileChange`. `edit::plan` appends it
  from **two** arms:
  - `Edit::SwitchProfile` — appended to its existing effect list
    (`StopAllToggles`, `RepublishActuation`, `ResetAxisOutputs`, `StopAllAnalogRepeats`,
    `AnnounceProfileChange`). Order among them is irrelevant — LEDs are independent of
    Toggles/axes/Analog-repeat. Put it last.
  - the ticket-04 set-Status-LEDs edit — **unconditionally** (see §5).
- **`run_effects`** handles `AssertStatusLeds` by calling a private `DispatchState` helper,
  `push_status_leds(&self, config: &Config)`, which reads
  `config.active_profile().status_leds` and `self.led_tx.send(Some(leds))`. (`led_tx` is the
  `watch::Sender` handed in from `main.rs`, held on `DispatchState`.)

### 4. Startup + (re)connect assertion

The firmware reclaims the LEDs to its orange-only power-on default on **every** USB
enumeration (ticket 01) — so re-asserting on connect is a hard requirement, and startup is
just "the first connect."

- In `dispatch::run`'s `rx_connection` select arm, **after** `handle_connection_change(...)`,
  call `state.push_status_leds(&config)` on **every message where `connected == true`**.
  No flag, no dependence on the connection *transition*: `DispatchState.device_connected`
  starts optimistically `true` (ticket 20) and `handle_connection_change` early-returns when
  the bool is unchanged, so hanging the assert off transition detection would miss the
  startup assert. Asserting on every `true` is safe — the write is idempotent on the
  hardware (ticket 01: 25+ writes, no adverse behaviour), the `CaptureSource` contract
  already says dispatch only observes genuine value changes (`capture/mod.rs:69`), and a
  stray redundant `true` costs one ~1 ms ioctl. `handle_connection_change` keeps its
  early-return for the `DeviceConnectionChanged` *signal* only.
- This path does **not** go through `Effect` — it reads `Config` and calls `push_status_leds`
  directly. `run_effects` (Profile switch) and this arm (connect) are the two call sites of
  the one helper.
- No pre-loop assertion in `dispatch::run`'s init block — the device may not be present yet,
  and the connect edge covers the present-at-startup case (the supervisor's fresh capture
  attempt publishes presence `false`→`true`, and even a first-message `true` is asserted).
- Works identically in Analog and Digital capture mode: `assert_status_leds` opens its own
  Interface-2 fd regardless of what capture is doing.

### 5. No non-active-Profile write path — confirmed, no gate needed

Every mutating D-Bus method is **Profile-unscoped** (`set_binding(input, layer, …)`,
`set_actuation_point(…)`, `set_mode_key_role(role)` — none take a Profile name); `edit::plan`
applies them all to `active_profile`. The GUI's Device Overview renders only the active
Profile, and clicking another Profile in the sidebar calls `switch_profile` **first**
(`gui/acheron_gui/device_overview.py:187–219`). So a Status-LEDs edit is *structurally*
always an edit to the active Profile — exactly like `SetActuationPoint`, which
unconditionally emits `RepublishActuation`. The ticket-04 edit therefore emits
`Effect::AssertStatusLeds` **unconditionally**; no `target == active` check. Q3's
"read `Config` each write" is the belt-and-braces backstop.

→ **Note for [ticket 04](./04-gui-daemon-surface-for-status-leds.md):** its question text's
"whether it edits the *active* Profile or the *currently-selected-for-editing* Profile"
framing should be **dropped** — there is no such distinction in Acheron. The GUI control
edits the active Profile, like every other per-Profile control on Device Overview.

### 6. Device-absent handling + shutdown clear

- **Device absent:** `assert_status_leds` returns `Err` (`discover_hidraw` finds no
  Interface 2 → `io::ErrorKind::NotFound`), identical to `relock()`. The `led` task logs it
  once and carries on. A Profile switch with no device connected persists `config.toml`
  normally, the LED write no-ops, and the next connect edge asserts the (now current) active
  Profile's triple.
- **Shutdown clear:** `main.rs::relock_and_exit` (the clean SIGTERM/SIGINT path) sends the
  all-off frame **before** `analog::relock()`, best-effort with the same log-and-continue
  treatment. Implementation: `relock_and_exit` calls `analog::clear_status_leds()` directly
  (it is already a synchronous `!`-returning fn doing exactly this kind of best-effort
  send). **Not** on the supervisor's swap-away-from-analog `relock()` — there the daemon is
  still running and the device still present, so the LEDs must keep showing the active
  Profile. Only a clean daemon exit clears. (Ticket 01 confirmed all-off `(0,0,0)` is
  hardware-reachable — the Q6/Q13 contingency does not trigger.)

### 7. State type — a named struct, from day one

`StatusLeds { orange: bool, green: bool, blue: bool }` (exact field names / `#[serde]` /
`config.toml` shape are [ticket 04](./04-gui-daemon-surface-for-status-leds.md)'s call —
charting Q15 sketched `status_leds = { orange = true, green = false, blue = false }`). The
architectural requirement here: it is a **named struct**, never a bare `[bool; 3]` or a
tuple, so that a later brightness byte, effect/speed params, or a sibling `backlight`
sub-struct is an *additive* change — the `watch<Option<StatusLeds>>` channel type and the
`Effect::AssertStatusLeds` variant do not churn.

### 8. Future-proofing — what a richer lighting feature would and wouldn't disturb

The per-key Chroma backlight and non-static Status-LED effects are **out of scope** (map).
Recorded here so a future effort inherits the seam instead of fighting it:

| "more complex" | impact on this architecture |
|---|---|
| more on/off channels / a 4th indicator | wider struct; nothing else moves |
| brightness instead of on/off | the frame already carries a full `0x00`–`0xFF` byte per channel; `StatusLeds` fields become `u8`-shaped; still one fire-and-forget frame |
| firmware-driven effects (breathing/spectrum/reactive) | still **one** short-lived write — the device firmware animates it; effect-id byte changes from `0x01`, arg3/arg4 carry speed/direction; payload widens to `{effect, speed, colour}`. No architecture impact. |
| host-streamed animation (daemon pushes an RGB frame every ~33 ms) | the only straining case: the `led` task grows from "write on change" to "hold a persistent fd + render loop". But the seams survive — `dispatch (decides) → watch channel → led task → Interface-2 hidraw` is unchanged; a persistent fd is a *refinement* of "Interface-2 hidraw, no driver mode", not a reversal. |

**Binding constraint on any future lighting work:** the device has a single control channel
and lighting frames must not interleave — any future lighting surface routes through the
**same `led` task**, never a second parallel writer opening Interface 2 independently.

### 9. Domain — no new `CONTEXT.md` term; ADR-0006 warranted

- **No new glossary term** for the writer component — `CONTEXT.md` is a glossary, not an
  implementation record. The map's reserved `Status LED` / `Status LED assignment` entries
  still land when [ticket 05](./05-write-status-led-spec.md) resolves.
- **ADR-0006 is warranted** and should be filed by [ticket 05](./05-write-status-led-spec.md)
  alongside the spec + glossary entries (same lazy discipline). It directly refines ADR-0002
  ("OpenRazer remains available if lighting integration is ever wanted later") and answers
  the question a future reader of the capture layer will ask — "why doesn't the LED write
  share the capture handle?". Drafted text:

  > **# Status LEDs are driven from the dispatch task over short-lived Interface-2 hidraw fds**
  >
  > The three side Status LEDs are set with a single Razer extended-matrix static-effect
  > feature report (`command_class 0x0F`, `command_id 0x02`, LED id `0x0B`) on the Tartarus
  > Pro's Interface-2 control node. We drive them from a dedicated non-fatal `led` task fed
  > by the dispatch task (the sole `Config` owner and Profile-switch decider), writing on a
  > freshly-opened, immediately-closed hidraw fd — **not** the analog capture layer's
  > long-lived handle (nothing holds one in Digital capture mode) — and **without** entering
  > driver mode (the `0x0F/0x02` frame works regardless, and the normal→driver transition
  > resets this device — ticket 02/01 research). Considered and rejected: sharing the
  > capture handle (couples LED writes to capture mode and the supervisor↔dispatch
  > boundary), and letting the supervisor write on connect (it would need Profile state it
  > doesn't own). The fd *lifetime* is a consequence of today's occasional one-shot writes;
  > a future host-streamed-animation feature would revisit the lifetime without disturbing
  > the transport, the ownership, or the no-driver-mode choice. All lighting frames — now
  > and future — route through the one `led` task; the device has a single control channel
  > and frames must not interleave.
