# Research: does Acheron's virtual gamepad need legacy `/dev/input/jsX` (`joydev`) compatibility?

Ticket: [37-research-legacy-joystick-api-compatibility](../issues/37-research-legacy-joystick-api-compatibility.md)
Spawned from: [Design Controller/Joystick output emulation](../issues/14-decide-controller-joystick-output-emulation.md), Q4 ("Device advertising")

## Bottom line

**Skip it — and there is nothing to skip.** `jsX` exposure is not a userspace feature a `uinput`
device author opts into; it is a side effect the kernel's `joydev` module (`CONFIG_INPUT_JOYDEV`)
produces automatically for *any* registered `input_dev` — hardware or `uinput`-created — that
advertises the same `EV_KEY`/`EV_ABS` gamepad bits (`BTN_GAMEPAD`, `BTN_JOYSTICK`, `ABS_X`, …)
Acheron's virtual gamepad already needs to advertise for the modern `evdev` path to work at all.
There is no separate ioctl, capability flag, or code path to add. Build the `evdev`/`uinput`
gamepad per the Linux Gamepad Specification and move on — the `jsX` question resolves itself.

## 1. The kernel's own position: `joydev` is legacy, evdev is the encouraged path

The canonical kernel documentation for the `js` API states this in its own words, not as a
secondary summary:

> "This document describes legacy `js` interface. Newer clients are encouraged to switch to the
> generic event (`evdev`) interface." — and — "The 0.x joystick driver API is quite limited and
> its usage is deprecated. The driver offers backward compatibility, though."

Source: [kernel.org, `input/joydev/joystick-api.html`](https://www.kernel.org/doc/html/latest/input/joydev/joystick-api.html)
(canonical source file: [`Documentation/input/joydev/joystick-api.rst`](https://github.com/torvalds/linux/blob/master/Documentation/input/joydev/joystick-api.rst)).

The modern replacement Acheron is already targeting — the [Linux Gamepad Specification](https://www.kernel.org/doc/html/latest/input/gamepad.html)
(`Documentation/input/gamepad.rst`) — defines the `BTN_GAMEPAD`/`BTN_*`/`ABS_*` code set a
compliant gamepad should advertise ("All gamepads that follow the protocol described here map
`BTN_GAMEPAD`. This is an alias for `BTN_SOUTH`/`BTN_A`.") and says nothing whatsoever about
`joydev`/`jsX` — the spec is `evdev`-native by design, with no compatibility clause referencing
the legacy interface.

## 2. `jsX` exposure is automatic for any gamepad-shaped `input_dev`, uinput included

This is the load-bearing fact for Q4 ("downside to also advertising jsX defensively"). Read
directly from the kernel's `joydev` driver source
([`drivers/input/joydev.c`](https://github.com/torvalds/linux/blob/master/drivers/input/joydev.c)):

```c
static bool joydev_match(struct input_handler *handler, struct input_dev *dev)
{
	/* Disable blacklisted devices */
	if (joydev_dev_is_blacklisted(dev))
		return false;

	/* Avoid absolute mice */
	if (joydev_dev_is_absolute_mouse(dev))
		return false;

	return true;
}

static const struct input_device_id joydev_ids[] = {
	{ .flags = INPUT_DEVICE_ID_MATCH_EVBIT | INPUT_DEVICE_ID_MATCH_ABSBIT,
	  .evbit = { BIT_MASK(EV_ABS) }, .absbit = { BIT_MASK(ABS_X) } },
	/* … ABS_Z, ABS_WHEEL, ABS_THROTTLE … */
	{ .flags = INPUT_DEVICE_ID_MATCH_EVBIT | INPUT_DEVICE_ID_MATCH_KEYBIT,
	  .evbit = { BIT_MASK(EV_KEY) },
	  .keybit = {[BIT_WORD(BTN_JOYSTICK)] = BIT_MASK(BTN_JOYSTICK) } },
	/* … BTN_GAMEPAD, BTN_TRIGGER_HAPPY … */
```

`input_register_handler`/the input core runs every newly-registered `input_dev` — including one
created purely in userspace via `/dev/uinput`, which is exactly how Acheron's existing keyboard
device and the planned gamepad device are built — against this ID table. Any device that matches
(and isn't blacklisted, or classified as an "absolute mouse") gets a `/dev/input/jsX` node
attached automatically by the kernel, with **no opt-in, no extra ioctl, and no code on the
uinput-creator's side.** The Acheron daemon's own `input/uinput` crate usage never has visibility
into or control over this — it's a consequence of `CONFIG_INPUT_JOYDEV` being loaded, not of
anything Acheron does.

Practical corollary: since a gamepad-classifying `uinput` device automatically produces both an
`eventX` node *and* a `jsX` node whenever `joydev` is loaded, Q4's "also advertise jsX
defensively" isn't a decision Acheron gets to make in either direction — there is no capability
bit that means "evdev but not joydev." The only lever that exists is *not* advertising
`BTN_GAMEPAD`/`ABS_X`-class bits at all, which would break the feature's own purpose.

`CONFIG_INPUT_JOYDEV` itself is a mainstream option shipped (as a module, autoloaded by udev on
first match) across desktop kernels — Ubuntu, Fedora, Arch, and SteamOS all carry it; it is not
an exotic or opt-in kernel patch. Where it happens to be absent (a minimal/embedded kernel a user
built themselves), `jsX` simply doesn't appear for *any* device, hardware or virtual alike — not
a regression specific to Acheron.

## 3. The modern stack already covers detection — checked against SDL's actual source, not docs summaries

SDL2/SDL3 is the layer nearly every native Linux game, and every Proton title routing through
Steam's `SDL_GameControllerDB`/gamepad mapping, ultimately calls into. Read directly from SDL's
Linux joystick backend
([`src/joystick/linux/SDL_sysjoystick.c`](https://github.com/libsdl-org/SDL/blob/main/src/joystick/linux/SDL_sysjoystick.c)):

```c
static bool SDL_classic_joysticks = false;
/* … */
SDL_classic_joysticks = SDL_GetHintBoolean(SDL_HINT_JOYSTICK_LINUX_CLASSIC, false);

static bool IsJoystickJSNode(const char *node) { /* matches "js<N>" */ }
static bool IsJoystickEventNode(const char *node) { /* matches "event<N>" */ }

static bool IsJoystickDeviceNode(const char *node)
{
    if (SDL_classic_joysticks) {
        return IsJoystickJSNode(node);
    } else {
        return IsJoystickEventNode(node);
    }
}
```

`SDL_HINT_JOYSTICK_LINUX_CLASSIC` (added SDL 3.2.0) defaults to `false` — SDL scans
`/dev/input/eventX` via `libudev` by default and only falls back to `jsX` if an application (or
user, via env var) explicitly opts in to the legacy path. SDL's own device-classification helper
(`GuessDeviceClass`/`SDL_EVDEV_GuessDeviceClass`) queries `EVIOCGBIT` for `EV_KEY`/`EV_ABS`/
`EV_REL` bits — the same `evdev` capability introspection Acheron's planned device already
supports by construction — and gates full "gamepad" classification on `has_key[BTN_GAMEPAD]`,
matching the Linux Gamepad Specification directly. No `jsX` involvement in the default path.

**Godot** (a major native-Linux, non-SDL-in-earlier-versions engine) is independent corroboration
in the opposite direction: its own pre-4.5 Linux joystick driver
([`platform/linuxbsd/joypad_linux.cpp`](https://github.com/godotengine/godot), checked at the
`4.3-stable` tag) scans `/dev/input` for `event*` nodes and explicitly defines and checks against
`ignore_str = "/dev/input/js"` to skip `jsX` nodes outright, using `EVIOCGBIT`/`EVIOCGNAME`/
`EVIOCGID` exclusively. Godot didn't merely fail to need `jsX` — its own driver actively filtered
it out to avoid double-counting the same physical device via both interfaces. And as of Godot
4.5 (2025), the engine dropped its bespoke Linux driver entirely and moved to SDL3 for gamepad
input on Windows/macOS/Linux alike, further consolidating the ecosystem onto the same
`evdev`+SDL-mapping-database path Acheron is targeting.

No current, credible source surfaced in this research describes any actively-maintained engine
or framework (Godot, Unity via SDL, Unreal via SDL, or SDL-based indies) that probes only
`/dev/input/jsX` in 2026. Every primary source found points the other way: `jsX`-only scanning is
something you must explicitly opt back into (`SDL_HINT_JOYSTICK_LINUX_CLASSIC=1`), not the
default or a name any current engine defaults to.

## 4. Steam Input specifically

Checked against Steamworks' own documentation and Valve's public udev rules — neither describes
any `jsX`/`joydev` dependency:

- The Steamworks ["Steam Input Gamepad Emulation - Best Practices"](https://partner.steamgames.com/doc/features/steam_controller/steam_input_gamepad_emulation_bestpractices)
  page (this is how Steam Input feeds games that don't natively support it — an emulated
  Xbox-360-shaped virtual controller, the same category of thing Acheron is building for its own
  purpose) states: *"On Windows the Steam Overlay will hook traditional gamepad input APIs…
  inject an emulated Xbox controller device. On macOS and Linux emulated controller input is
  provided by a driver."* It does not name `jsX`, `joydev`, VID/PID requirements, or any
  special uinput convention a virtual-device author must follow — the mechanism is left as an
  implementation detail, not a documented contract third parties (like Acheron) must match.
- Valve's own [`steam-devices` udev rules](https://github.com/ValveSoftware/steam-devices/blob/master/60-steam-input.rules)
  grant `hidraw`/`uinput` access by explicit vendor/product ID match (e.g.
  `ATTRS{idVendor}=="054c", ATTRS{idProduct}=="05c4"` for a DualShock 4) — this is Steam Input's
  *raw-HID ingestion* path for devices it drives directly, unrelated to how it recognizes a
  third-party `uinput` gamepad already exposing standard `evdev` gamepad semantics. Nothing in
  that file references `jsX`.
- Independent, technically detailed reporting on Steam Deck-era Linux controller plumbing
  (a GNOME/`libmanette` maintainer's ["Steam Deck, HID, and libmanette adventures"](https://nyaa.place/blog/steam-deck-hid-and-libmanette-adventures/))
  describes the ecosystem's trajectory as evdev-plus-raw-`hidraw` for cases needing more detail
  than evdev exposes (Steam Deck's own controller uses `hidraw` directly for that reason) —
  `joydev`/`jsX` is not mentioned anywhere in that piece, consistent with it being irrelevant to
  current engineering discussion in this space.
- Steam also ships a user-facing "Generic Gamepad Configuration Support" setting that lets
  unrecognized (no bespoke Steam mapping) controllers be bound through Steam Input — this is a
  detection/mapping-database concern (does Steam have a curated mapping for this exact
  VID/PID?), not an API-choice concern; it operates on evdev-visible devices either way and has
  no bearing on `jsX`.

Nothing found suggests Steam Input needs anything beyond a standard `evdev`/`uinput` gamepad
advertising the Linux Gamepad Specification's codes to recognize and remap Acheron's planned
device.

## 5. Answering the ticket's four questions directly

1. **Does the modern stack already cover detection without `jsX`?** Yes — confirmed against
   SDL's actual source (default `evdev`, `jsX` opt-in only) and the kernel's own gamepad spec,
   which is `evdev`-native and silent on `joydev`.
2. **Is there a meaningful population of current engines/games that only probe `jsX`?** No
   evidence found. The two concrete engine sources checked (SDL, Godot) both default to
   `evdev`-only, and Godot's own driver went out of its way to *exclude* `jsX` nodes rather than
   rely on them.
3. **Does Steam Input need anything beyond a standard `evdev` `uinput` gamepad?** No documented
   requirement found in Steamworks docs or Valve's own udev rules; the udev rules that do exist
   are VID/PID-scoped `hidraw` grants for real hardware Steam drives directly, not a contract for
   third-party virtual gamepads.
4. **Is there a downside to also advertising `jsX` defensively, vs. deferring?** The question
   doesn't apply as posed — there is no separate "advertise `jsX`" action available to Acheron at
   the `uinput` layer. `jsX` exposure is an automatic kernel-side consequence of advertising the
   same `BTN_GAMEPAD`/`ABS_*`-class bits the `evdev` path already requires, gated only by whether
   `CONFIG_INPUT_JOYDEV` happens to be loaded on the user's system (true by default on every
   mainstream desktop distro). Acheron cannot meaningfully "skip" or "include" it independent of
   building the `evdev` gamepad in the first place.

## Recommendation

**Build the virtual gamepad as a standard `evdev`/`uinput` device following the Linux Gamepad
Specification (`BTN_GAMEPAD`/`BTN_*`/`ABS_*`), and do not spend any implementation effort on
`/dev/input/jsX` compatibility.** This is not a "skip and revisit if a gap surfaces" hedge — it's
closer to "there is no jsX-specific work item to schedule, defer, or revisit," because:

- the kernel automatically exposes a `jsX` node for the device anyway, for free, on any desktop
  system with `CONFIG_INPUT_JOYDEV` loaded (the near-universal default), with zero code from
  Acheron either way;
- the kernel's own documentation calls the API legacy/deprecated and points integrators at
  `evdev`;
- SDL2/SDL3 (the layer nearly all native Linux and Proton/Steam-Play gamepad detection ultimately
  runs through) defaults to `evdev`-only scanning, with the legacy path requiring an explicit,
  non-default opt-in;
- Steam Input's own documentation and udev-rule conventions show no dependency on `jsX` for
  recognizing a virtual gamepad.

If a real, specific game or engine is later found (via user bug report) that fails to detect
Acheron's virtual controller, re-open this as a targeted compatibility ticket at that point — but
budget zero engineering time against this risk now; ticket 14 should treat Q4 as closed.
