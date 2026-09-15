# Lighting shares the `led` task's second channel; VARSTORE means no shutdown clear

The Tartarus Pro's RGB backlight matrix (20 grid keys + scroll wheel) is driven by the same
extended-matrix command family the Status LEDs use (`command_class 0x0F`), a different LED id
(`BACKLIGHT_LED = 0x05` vs `SIDE_STRIPE_LED = 0x0B`) on the same Interface-2 control node. Per
ADR-0006's own forward note ("all lighting frames — now and future — route through the one
`led` task; the device has a single control channel and frames must not interleave"), Lighting's
writer extends the existing `led` task rather than spawning a sibling: a second
`watch<Option<LightingState>>` channel is added alongside the existing
`watch<Option<StatusLeds>>` one, and `led::run`'s loop becomes a `tokio::select!` over both
receivers — each arm still fully awaits its `spawn_blocking` write before the next poll, so one
task with one write in flight at a time, regardless of which channel woke it. This keeps
Status-LED and Lighting writes serialised through the single control channel ADR-0006 already
established must not interleave.

Considered and rejected:

- **One combined channel** (`watch<Option<LedState>>` bundling `StatusLeds` and
  `LightingState`). Would force every Status-LED-only push to also resupply the current
  Lighting value (or vice versa), widening code and tests that today only know about one field —
  for no serialization benefit the two-channel/one-task shape doesn't already give. Two
  receivers polled by one task already keep writes serialised.
- **A sibling `lighting` task.** The same rejection ADR-0006 already gave a hypothetical second
  writer: the device has one control channel, and a second independent writer opening
  Interface 2 would race the `led` task's Status-LED writes.

**Lighting gets no shutdown-clear, unlike Status LEDs — a deliberate asymmetry, not an
oversight.** Every backlight effect-select frame carries `VARSTORE` (`arg0 = 0x01`), meaning the
firmware persists the asserted effect device-side. Status LEDs' frame uses `NOSTORE`, and the
firmware reclaims those LEDs to their orange-only default on every USB enumeration — the reason
Status LEDs' re-assertion is mandatory and its exit-time all-off clear is meaningful. Lighting's
persistence means the opposite: a clear-on-exit write would only actively erase the user's
chosen look from the device's own storage, for no benefit, since the next connect or Daemon
startup re-asserts the active Profile's Lighting regardless. `relock_and_exit` therefore calls
`analog::clear_status_leds()` but has no Lighting equivalent. A future reader should not "fix"
this into false symmetry with Status LEDs.

Lighting is still re-asserted on every `connected == true`, exactly like Status LEDs — VARSTORE
persistence describes what the firmware remembers on its own, not a substitute for the Daemon's
own re-assertion discipline, which exists to guarantee the device matches `config.toml` after
any possible desync (a config edit made while disconnected, a Profile switch that never reached
the device, etc.), regardless of whether the firmware happens to have retained the prior value.

This refines ADR-0006, which itself refines ADR-0002 ("OpenRazer remains available if lighting
integration is ever wanted later"): lighting integration is now wanted, and — like Status LEDs
before it — done directly over hidraw on the same transport Acheron already uses for analog
capture, not via OpenRazer.
