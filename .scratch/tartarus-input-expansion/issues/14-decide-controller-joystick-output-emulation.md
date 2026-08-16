Type: grilling

## Question

Design **Controller/Joystick output emulation** as a new Acheron capability, surfaced by the Synapse catalog ([Catalog Synapse's remap/macro feature set](./07-task-catalog-synapse-feature-set.md)) and locked in scope (non-blocking for v1.0) by [Lock the v1.0 feature list](./08-decide-v1-feature-list.md). Synapse emulates a full kernel-level virtual controller partly to circumvent anti-cheat tooling on Windows — that rationale doesn't transfer to Linux, and it's explicitly not wanted here. A **userspace-emulated** virtual gamepad (a second `uinput` device advertising `EV_ABS`/`BTN_*` gamepad codes, analogous to how the existing keyboard `uinput` device already advertises the full `EV_KEY` range) is fine and is the intended shape.

This is the first ticket of an open-ended strand — expect research and prototype tickets to graduate from it once the design questions below narrow the shape; don't try to spec the whole implementation in this session.

Settle at least:

- **Minimal viable mapping**: which physical Inputs map to which virtual controller elements? Synapse's Joystick mode offers 24 bindable buttons plus X/Y/Z analog axis output (with single-step increment/decrement, not just full-deflection) and four digital diagonal directions. Acheron doesn't need to match this 1:1 — decide what subset is worth building given the Tartarus Pro's actual physical control count (20 grid keys, Mode key, thumbstick, wheel).
- **Axis output without analog input**: per the map's Notes (grounding facts from charting), Acheron can simulate a fixed-% or steppable virtual axis position from ordinary digital (press/release) Inputs even with no analog *capture* — decide whether that's the v1.0-viable approach (independent of whether [the analog-capture research/prototype strand](./12-research-linux-analog-grid-key-protocol.md) ever pans out), or whether axis output should be gated on real analog input existing.
- **New Action target, not a new Binding shape**: decide whether "fire a controller button/axis" is a new `Action` kind (alongside Keypress/Macro/Profile-Switch), reusing the existing Binding/Trigger-mode machinery, or something structurally different.
- **Macros firing controller/joystick buttons**: already settled during charting — **yes**, Macros should be allowed to include controller/joystick button steps (not axis — axis was explicitly ruled too complex for a Macro step). Build this in rather than re-litigating it.
- **Device advertising**: does the virtual gamepad need to be a distinct `uinput` device from the existing keyboard one, and if so, does it need `js0`/`event*` joystick-API compatibility (older `/dev/input/jsX` interface) for older game engines, or is a modern `evdev`-only gamepad sufficient for Linux gaming today? This is likely a `research` question to graduate.
- **GUI surface**: how a user assigns "controller button N" or "axis step +1" as an Action — likely its own picker, related to but distinct from the key/mouse-button picker in [Finalize mouse-button + key output and picker](./02-decide-mouse-button-output-and-picker.md).
