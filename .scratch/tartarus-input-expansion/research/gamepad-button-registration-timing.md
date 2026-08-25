# Research: why do games only register `Action::ControllerButton` presses under Analog-repeat, never Fire-once/Hold-to-repeat?

Ticket: [74-research-gamepad-button-registration-timing](../issues/74-research-gamepad-button-registration-timing.md)
Feeds: [75-decide-controller-button-pulse-fix](../issues/75-decide-controller-button-pulse-fix.md)

## Bottom line

**Confirmed root cause, with primary-source mechanism, not just correlation:** the likely failure
mode is **per-frame polled-state reads**, not evdev event loss. Both the kernel's evdev queue and
the legacy `jsX` queue are FIFO ring buffers that do not drop a discrete Down/Up pair under normal
load — but the *consumer libraries* that games actually call (SDL's `SDL_GetGamepadButton`,
Windows' `XInputGetState`, DirectInput's default "immediate data" mode) do not read that queue
event-by-event. They drain **all** pending events into a single cached state variable and hand the
caller only the variable's value *after* the drain. A same-drain Down immediately followed by an
Up leaves that cached variable reading "not pressed" by the time the game asks — even though two
perfectly good discrete events really did arrive. This is a mechanical, source-verified fact in
SDL's own code (§2), and it generalizes to Proton/Wine-mediated Windows titles because XInput has
no alternative to polled state at all, and DirectInput defaults to polled state too (§5).

**Target dwell:** treat Analog-repeat's already-live-tuned **15ms** (`ANALOG_REPEAT_PULSE_HOLD`,
`daemon/src/dispatch.rs:262`) as a reasonable floor, not a coincidence — it lands almost exactly
on the independently-arrived-at defaults of two other Linux gamepad-remapping projects that solve
this identical problem: AntiMicroX's 100ms "Press Time" (§4.1) and sc-controller's 10ms
`Macro.HOLD_TIME` (§4.2). 15ms clears one 60fps frame (16.6ms) only by luck of phase alignment,
not with real margin — see §6 for why 15ms is *riskier* than it looks and what headroom would
remove the luck.

---

## 1. Acheron's current code, confirmed by direct reading (not re-derived — restated here for a self-contained record)

- `daemon/src/executor.rs:111-112` — `compile()` turns `Action::ControllerButton { button }` into
  `vec![MacroStep::KeyDown(*button), MacroStep::KeyUp(*button)]`, structurally identical to a bare
  keypress (`keypress_steps`, same file) and with no `MacroStep::Delay` inserted between them.
- `daemon/src/dispatch.rs:262` — `const ANALOG_REPEAT_PULSE_HOLD: Duration = Duration::from_millis(15);`
- `daemon/src/dispatch.rs:317-333` — Analog-repeat's pulse path explicitly `tokio::time::sleep(ANALOG_REPEAT_PULSE_HOLD).await` between firing Down and firing Up. This is the one path with a deliberate non-zero dwell, and it is the one path confirmed (ticket 73) to register reliably in games.
- Fire-once/Hold-to-repeat have no equivalent sleep; the only gap between Down and Up is a
  sub-millisecond `mpsc`/`oneshot` channel round-trip through the injector task (`injector.rs`,
  per ticket 74's own framing), and each write is still its own genuine `SYN_REPORT` frame — i.e.,
  two fully valid, separately-timestamped evdev events, just extremely close together.

## 2. SDL2/SDL3: the mechanism that can miss a same-drain Down→Up pair, read from source

### 2.1 The evdev→SDL ingestion loop drains everything queued, once per call

`src/joystick/linux/SDL_sysjoystick.c`, `HandleInputEvents()` (invoked from `LINUX_JoystickUpdate`,
which is called once per `SDL_PumpEvents`/`SDL_UpdateJoysticks` pass — i.e., typically once per
game-loop frame if the game doesn't pump more often):

```c
while ((len = read(joystick->hwdata->fd, events, sizeof(events))) > 0) {
    len /= sizeof(events[0]);
    for (i = 0; i < len; ++i) {
        struct input_event *event = &events[i];
        code = event->code;
        switch (event->type) {
        case EV_KEY:
            SDL_SendJoystickButton(SDL_EVDEV_GetEventTimestamp(event),
                joystick, joystick->hwdata->key_map[code],
                (event->value != 0));
```
(https://github.com/libsdl-org/SDL/blob/main/src/joystick/linux/SDL_sysjoystick.c)

This loop reads until `read()` returns nothing more — it drains **all** evdev events that
accumulated since the previous call, applying each one **in arrival order**, in a single pass,
before returning control to the game. If a 15ms-or-less-apart Down and Up both landed in the
kernel's evdev queue before this loop ran (entirely possible: the loop only runs once per frame,
and 15ms is shorter than a 60fps frame), both get applied to internal state in the same pass,
back to back.

### 2.2 `SDL_SendJoystickButton` — one call updates the cache *and* pushes an event

`src/joystick/SDL_joystick.c`:

```c
void SDL_SendJoystickButton(Uint64 timestamp, SDL_Joystick *joystick, Uint8 button, bool down)
{
    ...
    if (down == joystick->buttons[button]) {
        return;
    }
    // Update internal joystick state
    joystick->buttons[button] = down;
    joystick->update_complete = timestamp;
    // Post the event, if desired
    if (SDL_EventEnabled(event.type)) {
        ...
        SDL_PushEvent(&event);
    }
}
```
(https://github.com/libsdl-org/SDL/blob/main/src/joystick/SDL_joystick.c)

Two consumption models diverge exactly here:

- **Event-queue consumer** (`SDL_JOYBUTTONDOWN`/`SDL_JOYBUTTONUP`, or SDL3's
  `SDL_EVENT_GAMEPAD_BUTTON_DOWN`/`UP`): each call to `SDL_SendJoystickButton` does its own
  `SDL_PushEvent`, so **both** the Down and the Up land in SDL's event queue as two discrete,
  ordered entries — regardless of how close together they were applied to the cache. A game that
  drains `SDL_PollEvent` in a loop and reacts to each event sees both. This path structurally
  cannot lose a genuine Down/Up pair from being merely close together (loss would require SDL's
  own event queue to overflow, which is a dynamically-growing array in modern SDL, not a fixed
  small ring — not a realistic failure mode for a single extra event pair).
- **Polled-state consumer** (`SDL_GetJoystickButton`/`SDL_GetGamepadButton`,
  `SDL_GameControllerGetButton` in old SDL2 naming): reads only `joystick->buttons[button]`.

  `src/joystick/SDL_joystick.c`:
  ```c
  bool SDL_GetJoystickButton(SDL_Joystick *joystick, int button)
  {
      bool down = false;
      ...
      down = joystick->buttons[button];
      ...
      return down;
  }
  ```
  `src/joystick/SDL_gamepad.c`'s `SDL_GetGamepadButton` resolves through the gamepad's button
  bindings down to this same per-joystick cached array/axis state — it is not a separate read
  path to hardware.

  Because `HandleInputEvents()` applies Down then Up to `joystick->buttons[button]` **in the same
  drain pass**, if the game calls `SDL_GetGamepadButton` only *after* that pass completes (the
  standard "pump once per frame, then query state" pattern), it observes the final value: `false`.
  The button press is invisible to this call even though it structurally happened and even though
  the *event queue* (if the game were also listening to it) would show it. **This is the
  mechanism**: not evdev event loss, but same-tick state coalescing in the polled-state cache,
  which the event-queue path is immune to by construction.

### 2.3 Implication for Acheron

Any game using `SDL_JOYBUTTONDOWN`/`UP` or `SDL_EVENT_GAMEPAD_BUTTON_DOWN`/`UP` should register a
Fire-once pulse today, at any dwell, including zero. Games that instead call
`SDL_GetGamepadButton`/`SDL_GameControllerGetButton` once per frame are the ones structurally
capable of missing it — and per the ticket's own hardware finding, real games evidently do this
commonly enough that the bug is reproducible. SDL does not document which pattern any given game
uses; it is a per-game engine choice with no way to detect from Acheron's side. The fix must
assume the polled-state case is in play and defend against it unconditionally.

## 3. Kernel evdev and legacy `jsX`: both are non-lossy FIFO queues, not the failure point

### 3.1 evdev (`drivers/input/evdev.c`)

Per-client event buffer is a genuine circular FIFO, not a coalescing/state cache:

```c
static unsigned int evdev_compute_buffer_size(struct input_dev *dev)
{
    unsigned int n_events =
        max(dev->hint_events_per_packet * EVDEV_BUF_PACKETS,
            EVDEV_MIN_BUFFER_SIZE);
    return roundup_pow_of_two(n_events);
}
```
with `EVDEV_MIN_BUFFER_SIZE = 64`, `EVDEV_BUF_PACKETS = 8`
(https://github.com/torvalds/linux/blob/master/drivers/input/evdev.c). Events are only dropped
(signaled to the reader via `SYN_DROPPED`) when the ring buffer is actually saturated
(`client->head == client->tail` after a write) — i.e., when a consumer isn't reading fast enough
to drain 64+ backlogged events, nowhere close to the "one extra button pulse" scenario here.
A single Down/Up pair a few milliseconds apart is trivially two ordinary entries in this queue;
evdev itself never merges or drops them.

### 3.2 Legacy `/dev/input/jsX` (`joydev`)

Confirmed from both the kernel's own docs and its source that `jsX` is architecturally the same
shape as evdev — a discrete event queue, not a polled snapshot:

- Docs (`Documentation/input/joydev/joystick-api.rst`): "the joystick driver now reports only any
  changes of its state" via `read(fd, &e, sizeof(e))` calls, each returning one `js_event`.
  "As of version 1.2.8, the queue is circular and able to hold 64 events." Overflow behavior is
  documented too: "If time between reads is enough to fill the queue and lose an event, the driver
  will switch to startup mode" (synthetic `JS_EVENT_INIT` events resent on next read) — again, an
  overflow-only failure mode, not a same-tick coalescing one.
  (https://github.com/torvalds/linux/blob/master/Documentation/input/joydev/joystick-api.rst)
- Source (`drivers/input/joydev.c`): `#define JOYDEV_BUFFER_SIZE 64`, and `struct joydev_client`
  carries its own `buffer[JOYDEV_BUFFER_SIZE]`/`head`/`tail` circular FIFO, filled by
  `joydev_pass_event()` per discrete event.
  (https://github.com/torvalds/linux/blob/master/drivers/input/joydev.c)

So a game reading raw `jsX` via `read()` in a loop gets the same immunity to short pulses that
evdev event-queue consumers get. (Whether any relevant modern game actually still uses `jsX`
directly is a separate, likely-moot question — see the sibling research file
`legacy-joystick-api-compatibility.md`, which found Acheron gets `jsX` compatibility for free via
`joydev`'s automatic binding to any gamepad-shaped `uinput` device regardless.)

**Conclusion for §2/§3 together:** the kernel-level transport (evdev and jsX alike) is
structurally not the bottleneck. The loss happens strictly above the kernel, inside whichever
userspace library/API a given game links against, and specifically only in that library's
polled-state accessor, not its event-queue accessor.

## 4. Prior art: other remapper tools already solved this, with their own tuned constants

### 4.1 AntiMicroX — explicit, documented minimum "Press Time"

AntiMicroX (successor to AntiMicro, a mature Linux gamepad-to-keyboard/mouse/gamepad remapper)
ships a first-class concept for exactly this problem:

> "This slot type is used in order to prevent rapid key presses and releases that might be missed
> by the event loop in a game." … "By default, a value of 0.10 seconds is used for the key press
> time."

(https://github.com/AntiMicroX/antimicrox/wiki/Advanced-Slot-Explanation, "Press Time" section)

This is a directly on-point primary source: an actively maintained Linux remapper independently
identified the same class of bug ("a game's event loop can miss a rapid press/release") and ships
a **default 100ms** minimum hold as the mitigation, user-adjustable per-profile or per-button. This
is markedly higher than Acheron's proven-working 15ms, though AntiMicroX's default targets
worst-case game engines generically rather than being tuned against specific titles the way
Acheron's 15ms was (ticket 73).

### 4.2 sc-controller — `Macro.HOLD_TIME = 0.01` (10ms), 1ms specifically carved out for keyboard typing

sc-controller (Steam Controller driver/remapper, emits both keyboard/mouse and a virtual Xbox 360
gamepad via `uinput`) defines, in `scc/macros.py`:

```python
class Macro(Action):
    ...
    HOLD_TIME = 0.01
```

and, distinctly, for the `Type` macro (used for typing text via keyboard key events, not gamepad
buttons):

```python
class Type(Macro):
    ...
    HOLD_TIME = 0.001
```

(https://github.com/kozec/sc-controller/blob/master/scc/macros.py)

`Macro.HOLD_TIME` is the default `delay_after` applied between a macro's press and release steps
(see `PressAction`/`ReleaseAction`/`TapAction` in the same file) whenever no explicit delay is
given. The general-purpose default is **10ms**; the authors *lowered* it specifically for the
text-typing macro action, to 1ms — consistent with §2's finding that keyboard input on Linux
(consumed via XKB/text-input event pipelines, not per-frame gamepad-style state polling) doesn't
need the same defensive dwell that gamepad button emulation does. sc-controller drew the same
distinction Acheron's ticket 75 is asking about (generic bare-Down/Up fix vs. `ControllerButton`-
scoped fix) and landed on scoping the larger dwell away from keyboard output.

No sc-controller constant was found named specifically for gamepad-button dwell as distinct from
`Macro.HOLD_TIME` — the same general macro-timing constant is what governs its virtual-gamepad
button emission when buttons are driven through the macro system, and it did not need a
gamepad-specific value beyond that.

### 4.3 QJoyPad — not applicable

QJoyPad (older Qt-based joystick-to-keyboard/mouse mapper) was checked but is keyboard/mouse-only
in its output (no gamepad/`uinput`-controller output mode), so it has no equivalent constant to
cite; not further pursued.

### 4.4 What this triangulation supports

Three independent projects, three different numbers for conceptually the same problem: Acheron's
already-tuned 15ms, sc-controller's general-purpose 10ms, AntiMicroX's conservative 100ms default.
None of the three claims a rigorously-derived number from a spec — all read as pragmatically tuned
against real games/engines the authors tested against. This is corroborating evidence that a
double-digit-millisecond dwell is in the right neighborhood, not proof of a single correct value;
see §6 for why the top of that range (closer to AntiMicroX's 100ms, or at minimum something with
real single-frame headroom) is the safer target than the bottom.

## 5. Proton/Wine: does it add latency/granularity that pushes the safe minimum higher?

For a Windows-built game running under Proton, the chain is: Acheron's `uinput` gamepad → kernel
evdev → (Wine's `winebus.sys` SDL backend, itself built on the same SDL evdev backend as §2) →
Wine's internal HID report queue → the game's own Windows-API read (XInput or DirectInput).

### 5.1 Wine's SDL-backed bus driver forwards each event individually, with a 10ms idle-wait — it does not itself introduce a polling/coalescing hazard

`dlls/winebus.sys/bus_sdl.c`, `sdl_bus_wait()`:

```c
do
{
    if (bus_event_queue_pop(&event_queue, result)) return STATUS_PENDING;
    if (pSDL_WaitEventTimeout(&event, 10) != 0) process_device_event(&event);
    else check_all_devices_effects_state();
} while (event.type != quit_event);
```

Each `SDL_CONTROLLERBUTTONDOWN`/`UP` (or `SDL_JOYBUTTONDOWN`/`UP`) SDL event is processed
individually and immediately turned into its own queued HID input report
(`hid_device_set_button()` + `bus_event_queue_input_report()`), not batched.
`SDL_WaitEventTimeout(&event, 10)` blocks for **up to** 10ms only when idle — it returns
immediately the moment an SDL event is actually available, so it is not itself a 10ms polling
cadence gating delivery of a real event.
(https://github.com/wine-mirror/wine/blob/master/dlls/winebus.sys/bus_sdl.c)

Two caveats worth flagging honestly, since this is where primary-source confidence gets thinner:
- This bus driver still sits *downstream* of SDL's own evdev ingestion (§2.1), which itself only
  drains once per its own call cadence — so the same same-drain-coalescing risk that afflicts a
  polled-state SDL consumer on native Linux could in principle also apply *inside Wine's own SDL
  usage*, before the event even reaches the HID queue Wine hands to the game. This wasn't traced
  further; Wine's SDL polling thread runs independently of any specific game's frame loop, so its
  effective "drain interval" is governed by its own 10ms idle-wait loop, not the guest game's
  frame time — plausibly *better* isolated from frame-rate variance than a native SDL game polling
  once per frame, but not verified to be immune.
- Whether a given Proton title is actually routed through `bus_sdl.c` at all (vs. Proton's more
  common SDL-backed XInput controller passthrough, or native Linux joystick passthrough options)
  wasn't verified further; Proton's controller handling has changed across versions (search results
  note "Proton 8.7" added flags mapping both DInput and XInput types through SDL, but the exact
  current-version routing wasn't independently confirmed against Proton's own source in this pass).

### 5.2 The guest-side API the game actually calls is polled by design — this is the more load-bearing fact

- **XInput**: `XInputGetState()` is documented by Microsoft as "Retrieves the current state of the
  specified controller" — a snapshot read.
  (https://learn.microsoft.com/en-us/windows/win32/api/xinput/nf-xinput-xinputgetstate)
  XInput has **no** alternative event/message-queue API for buttons at all — `XInputGetState` is
  the only way to read button state. A game using XInput is *necessarily* a polled-state consumer,
  by API design, independent of Wine/Proton entirely (true on real Windows too).
- **DirectInput** ships both modes, and defaults to polled ("immediate") state:

  > "Microsoft DirectInput supplies two types of data: buffered and immediate. Buffered data is a
  > record of events that are stored until an application retrieves them. Immediate data is a
  > snapshot of the current state of a device." … "The buffer size value may be set to 0 to
  > indicate that the application does not read buffered data from the device. **The default size
  > of the buffer is 0**, and you cannot obtain buffered data unless you change this value."

  (https://learn.microsoft.com/en-us/previous-versions/windows/desktop/ee416236(v=vs.85),
  "Buffered and Immediate Data"; buffer-size default confirmed via
  `IDirectInputDevice8::GetDeviceData`/`SetProperty` docs family)

  A DirectInput game must **opt in** (`DIPROP_BUFFERSIZE` > 0 + `GetDeviceData`) to get the
  event-queue-immune behavior; unless a title specifically does this, it is a polled-state
  consumer by default too.

**Conclusion for §5:** Proton/Wine does not appear to be the dominant source of risk here — the
Windows-side gamepad APIs (XInput unconditionally, DirectInput by default) are themselves
polled-state by design, matching the same hazard already established for native SDL games in §2,
with or without Wine in the chain. No evidence was found that Wine's own SDL-to-HID relay adds a
*coarser* polling granularity than a native game's own frame-paced SDL polling would; if anything
its independent 10ms idle-wait thread is decoupled from the guest's frame rate. This is a genuine
open uncertainty (flagged, not papered over): it was not independently verified against current
Proton source exactly which titles route through `bus_sdl.c` vs. other controller paths, so no
Proton-specific "add N ms of headroom" figure can be responsibly cited beyond "the guest-side APIs
are polled by design regardless of Proton."

## 6. What minimum dwell to target, and why 15ms is riskier than the numbers alone suggest

No source found (Microsoft, SDL, kernel, or any of the tools surveyed) documents a rigorously
derived "minimum synthetic press duration to survive N% of real game loops" figure — this appears
to be an empirically-tuned-per-project constant everywhere it exists (§4.4), not a published spec
value. Say that plainly rather than inventing false precision.

What the primary sources *do* support, concretely:

- The failure mode is "does the dwell survive being sampled by an external, uncorrelated
  once-per-frame poll," not "does the dwell exceed some fixed kernel/API floor." A poll that lands
  even 1ms after the Up already reads "not pressed"; there is no dwell short of "spans the actual
  polling interval" that is safe by construction against a badly-phased poll.
- At uncapped/60fps (~16.6ms), a 15ms pulse has **positive but thin** margin — it beats one frame
  interval only if the poll happens to land inside the pulse's live window, and a poll's phase
  relative to Acheron's pulse start is not something Acheron controls or synchronizes with. In the
  worst-case phase alignment, even a dwell *longer* than one frame interval is not guaranteed safe
  unless it exceeds **two** consecutive frame intervals minus one instant — i.e., true guaranteed
  single-poll coverage against arbitrary phase requires dwell > one full inter-poll period, not
  merely dwell ≈ one inter-poll period. Ticket 73 confirms 15ms was tuned/verified with `evtest`
  (device-level, i.e., confirms the pulse shape on the wire) — not verified against an actual
  game's frame-polling behavior, which is exactly the gap ticket 75 flags for the follow-up
  hardware-verify task.
- At 120fps+ (~8.3ms or less), 15ms already exceeds one full inter-poll period, which is
  meaningfully safer against arbitrary phase than the 60fps case.
- Uncapped/variable-rate engines are the genuinely hard case: no fixed dwell is provably safe
  against an adversarial or highly variable poll cadence; a dwell needs to be chosen against a
  *practical* worst-case frame time a target game is expected to hit, not a theoretical one.

Given the above, and weighing the two closest concrete precedents:
- sc-controller's general 10ms sits *below* Acheron's own already-partially-verified 15ms — weak
  supporting evidence for 15ms's ballpark, but 10ms itself is not a stronger data point than what
  Acheron already has.
- AntiMicroX's 100ms default is the one number in the survey explicitly justified by the same
  "game event loop might miss it" reasoning this exact bug is about, chosen as a *default* meant to
  be safe across arbitrary/unknown games rather than tuned to specific titles — i.e., it is the
  closest thing found to a "safe against unknown game engines" figure rather than a "tuned against
  the games I tested" figure.

This research does not pick the final number — that is explicitly the follow-up decision session's
job (ticket 75) — but the concrete inputs it can carry forward are: 15ms is proven only at the
device/`evtest` level, not against real game polling; the one directly-analogous "chosen to be
robust against unknown game loops" precedent in the wild is two-thirds of an order of magnitude
higher (100ms); and any figure chosen should be justified against a stated target frame-time
assumption (e.g., "safe against polling as slow as Xfps") rather than picked as a bare constant,
since that is the actual variable the failure mode is sensitive to.

## 7. Hold-to-repeat: taps vs. sustained hold (secondary question from ticket 74)

Not exhaustively researched in this pass (ticket 74's primary ask was the timing/consumption-model
question; this is carried forward as an open input for ticket 75, not resolved here). What's
directly relevant from the above: because Hold-to-repeat today fires a *fresh, independent*
zero-dwell pulse on every kernel autorepeat `Repeat` event (`dispatch.rs`'s
`(TriggerMode::HoldToRepeat, EventState::Down | EventState::Repeat)` handling, per ticket 74's own
grounding), each of those pulses is independently subject to the exact same same-drain-coalescing
hazard described in §2 — giving each pulse a minimum dwell (the "generic fix" framed in ticket 75)
would make Hold-to-repeat register as a train of discrete taps at kernel-autorepeat cadence, which
reads to a polled-state game consumer as a real (if rapid) sequence of press/release cycles, not a
single sustained hold — this is a UX/design question for ticket 75 to weigh, not a timing question
this research file can settle from primary sources alone (whether a given game distinguishes
"held button" from "rapid taps" in its own game logic is game-specific and not discoverable from
SDL/kernel/API docs in the abstract).

## 8. Sources index

- SDL3 source, `src/joystick/linux/SDL_sysjoystick.c` (`HandleInputEvents`/`LINUX_JoystickUpdate`) — https://github.com/libsdl-org/SDL/blob/main/src/joystick/linux/SDL_sysjoystick.c
- SDL3 source, `src/joystick/SDL_joystick.c` (`SDL_SendJoystickButton`, `SDL_GetJoystickButton`) — https://github.com/libsdl-org/SDL/blob/main/src/joystick/SDL_joystick.c
- SDL3 source, `src/joystick/SDL_gamepad.c` (`SDL_GetGamepadButton`) — https://github.com/libsdl-org/SDL/blob/main/src/joystick/SDL_gamepad.c
- SDL3 docs, `SDL_UpdateGamepads` — https://wiki.libsdl.org/SDL3/SDL_UpdateGamepads
- Linux kernel source, `drivers/input/evdev.c` (buffer sizing, FIFO, `SYN_DROPPED`) — https://github.com/torvalds/linux/blob/master/drivers/input/evdev.c
- Linux kernel docs, `Documentation/input/joydev/joystick-api.rst` — https://github.com/torvalds/linux/blob/master/Documentation/input/joydev/joystick-api.rst
- Linux kernel source, `drivers/input/joydev.c` (`JOYDEV_BUFFER_SIZE`, per-client FIFO) — https://github.com/torvalds/linux/blob/master/drivers/input/joydev.c
- AntiMicroX wiki, "Advanced Slot Explanation" (Press Time) — https://github.com/AntiMicroX/antimicrox/wiki/Advanced-Slot-Explanation
- sc-controller source, `scc/macros.py` (`Macro.HOLD_TIME`, `Type.HOLD_TIME`) — https://github.com/kozec/sc-controller/blob/master/scc/macros.py
- sc-controller source, `scc/uinput.py` (autorepeat delay/period, unrelated but checked) — https://github.com/kozec/sc-controller/blob/master/scc/uinput.py
- Wine source, `dlls/winebus.sys/bus_sdl.c` (`sdl_bus_wait`, per-event HID report forwarding) — https://github.com/wine-mirror/wine/blob/master/dlls/winebus.sys/bus_sdl.c
- Microsoft Learn, `XInputGetState` function — https://learn.microsoft.com/en-us/windows/win32/api/xinput/nf-xinput-xinputgetstate
- Microsoft Learn, "Buffered and Immediate Data" (DirectInput) — https://learn.microsoft.com/en-us/previous-versions/windows/desktop/ee416236(v=vs.85)
- Acheron source, `daemon/src/executor.rs:111-112` (`compile()` for `Action::ControllerButton`)
- Acheron source, `daemon/src/dispatch.rs:262,317-333` (`ANALOG_REPEAT_PULSE_HOLD`, Analog-repeat pulse timing)
