Type: grilling
Blocked by: 01

## Question

Decide the **daemon-side architecture** for asserting a Profile's Status-LED assignment.
Grilling + domain-modeling against the real code and the [ticket 01](./01-prototype-status-led-controllability.md)
prototype result. Decisions only — no build.

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
- **Startup assertion.** Where in daemon boot does the active Profile's Status-LED state get
  asserted, relative to device-connect and capture-source start?
- **Device absent / disconnected.** LED writes no-op when no Tartarus Pro is connected
  (mirror analog's handling); on reconnect, re-assert the active Profile's state — where does
  that hook sit (the existing `connection_tx` reconnect path)?
- **State ownership.** Research §3: one frame drives all three channels, so a partial update
  needs the other two channels. Acheron always writes the full triple from the active
  Profile, so no cached `led_state` is strictly needed — confirm, or decide to cache.
- **Editing a non-active Profile's assignment.** Changing `status_leds` on a Profile that
  isn't active must *not* touch the hardware; only a switch to it (or already being on it)
  asserts. Confirm the write is gated on "is this the active Profile".
- **Re-assert hook (conditional).** If [ticket 01](./01-prototype-status-led-controllability.md)
  criterion 3 shows the firmware clobbers host state on an on-device keymap switch, decide
  whether/how the daemon detects that and re-asserts (hook an event, or poll). If ticket 01
  shows no clobber, this drops.

Also: does `CONTEXT.md` need any new runtime term (e.g. for the LED-writing component), and
is an ADR warranted for the hidraw-ownership choice?

Output: `## Answer` with the settled architecture; append the gist to the map's Decisions so
far; graduate or close the relevant Not-yet-specified item.
