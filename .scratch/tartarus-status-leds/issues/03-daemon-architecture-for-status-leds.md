Type: grilling
Blocked by: 01

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
