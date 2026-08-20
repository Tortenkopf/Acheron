Type: research
Status: resolved

## Question

Spawned from [Design Controller/Joystick output emulation](./14-decide-controller-joystick-output-emulation.md)'s Q4: does Acheron's planned virtual gamepad `uinput` device need legacy `/dev/input/jsX` joystick-API compatibility, alongside (or instead of) relying purely on the modern `evdev`/SDL gamepad-detection path?

Settle with evidence, not general impressions:

- Does the modern Linux gamepad stack (kernel `uinput` advertising standard `BTN_GAMEPAD`/`BTN_*`/`ABS_*` codes, consumed via `evdev` and SDL2/SDL3's gamepad mapping database) already cover current Linux-native and Proton/Steam Play game detection without also exposing the legacy `js0`-style joystick API (`CONFIG_INPUT_JOYDEV`, `/dev/input/jsX`)?
- Is there a meaningful population of current engines/frameworks/games (native Linux or under Proton) that still only probe `/dev/input/jsX` and miss a device that's `evdev`-only?
- Does Steam Input specifically (which many Proton titles route controller input through regardless of the game's own detection) need anything beyond a standard `evdev` `uinput` gamepad to recognize and remap the device?
- Is there any downside (compatibility risk, code cost, `uinput` capability conflicts) to *also* advertising jsX compatibility defensively, versus the cost of skipping it and only adding it later if a real gap surfaces?

Write findings to `.scratch/tartarus-input-expansion/research/legacy-joystick-api-compatibility.md`. Conclude with a clear recommendation: skip jsX, include it, or defer-until-evidence-of-a-real-gap — ticket 14 needs a usable answer, not just a literature dump.

## Answer

Build the virtual gamepad as a standard `evdev`/`uinput` device per the Linux Gamepad
Specification and spend zero effort on `/dev/input/jsX`. Checked against primary sources
(kernel.org's `joydev`/gamepad docs, the kernel's own `joydev.c` match table, SDL's actual Linux
joystick source, Godot's driver history, and Steamworks/Valve docs): the kernel's `joydev`
module already attaches `/dev/input/jsX` automatically to *any* registered `input_dev` —
`uinput`-created ones included — that advertises the same `BTN_GAMEPAD`/`ABS_X`-class bits the
`evdev` path needs anyway, with zero opt-in code required, on any desktop kernel with
`CONFIG_INPUT_JOYDEV` loaded (the near-universal default). SDL2/SDL3 (the layer nearly all
native-Linux and Proton/Steam-Play gamepad detection ultimately runs through) defaults to
`evdev`-only scanning; Godot's own pre-SDL Linux driver actively *excluded* `jsX` nodes; and
neither Steamworks' gamepad-emulation docs nor Valve's own udev rules describe any `jsX`
dependency for Steam Input to recognize a virtual controller. There is no "skip vs. include"
trade-off to make — `jsX` exposure isn't a feature Acheron implements, it's a free kernel-side
side effect of building the `evdev` gamepad, so ticket 14's Q4 is closed rather than deferred.
See [the research](../research/legacy-joystick-api-compatibility.md) for full citations.
