Type: grilling
Blocked by: 07
Status: resolved

## Question

Lock the final v1.0 feature list for Acheron, reacting to the Synapse notes gathered in [Catalog Synapse's remap/macro feature set](./07-task-catalog-synapse-feature-set.md). Starting floor already known: the shipped MVP (Profiles, Layers, Keypress/Macro Actions, Fire-once/Hold-to-repeat/Toggle Trigger modes) plus the four features already ticketed on this map (Chord, mouse-button + full key output picker, Stepper, Profile-switch Action) plus the two polish tickets (context-menu cleanup, GUI sizing).

Settle at least:

- For each Synapse capability the notes surface that Acheron doesn't already have (ticketed or shipped): in scope for v1.0, explicitly out of scope, or fog for a later version?
- Does anything Synapse does reveal a gap in Acheron's *existing* four in-flight tickets (e.g. a Trigger-mode nuance Chord or Stepper hasn't accounted for) that should correct those tickets rather than spawn new ones?
- Write the settled list back into this map's Destination section as the concrete v1.0 feature boundary, replacing the "known floor, pending research" language.

## Answer

Grilled capability-by-capability against the Synapse catalog. Settled v1.0 feature boundary:

**In scope, required (v1.0 floor, unchanged from the map's prior known floor)**: MVP (Profiles, Layers, Keypress/Macro Actions, Fire-once/Hold-to-repeat/Toggle) + Chord Bindings (ticket 01, now with a thumbstick-diagonal worked example folded in) + mouse-button/full-keyboard output + picker (ticket 02, now widened to include multimedia/consumer-control keys and a canned-text-macro note) + Stepper (ticket 03) + Profile Switch (ticket 05) + the two polish tickets (04, 06).

**Newly in scope, required (v1.0 floor, new tickets)**:
- Reusable/named Macro entities — a real gap (today's Macro is inline-only); ticketed as [Design reusable, named Macro entities](./15-decide-reusable-macro-entities.md).

**Newly in scope, but explicitly non-blocking (may land in v1.0 if ready in time, otherwise fast-follow — see below)**:
- Grid-key analog capture/trigger points — an open-ended feasibility attempt, since no existing Linux tool does this at all. Ticketed as [Sharpen the Linux analog-capture protocol](./12-research-linux-analog-grid-key-protocol.md) (research) blocking [Standalone analog-capture prototype](./13-task-standalone-analog-capture-prototype.md) (task). If infeasible, this line of inquiry is dropped; everything past feasibility stays fog.
- Controller/Joystick output emulation (userspace-emulated, explicitly **not** a kernel-level virtual device — Synapse's anti-cheat rationale doesn't apply on Linux) — ticketed as [Design Controller/Joystick output emulation](./14-decide-controller-joystick-output-emulation.md). Macros firing controller/joystick buttons (not axis) is already settled as part of that ticket's scope.

**Explicitly out of scope for v1.0** (added to the map's Out of scope section):
- Shell-command execution as a Macro action — security-sensitive, no articulated use case.
- A Launch-a-program Action — user already judged it not worth having.
- An intrinsic Macro loop/repeat-forever primitive distinct from Toggle — confirmed real gap (Toggle restarts the whole Macro each cycle, no "run a setup sequence once then loop a suffix" primitive), but narrow enough to defer; Toggle already covers looping for the general case.
- Grid-key analog *trigger points* specifically (the depth-based actuation-point feature) beyond what the analog-capture strand above settles — this is fog past ticket 13, not separately ticketed.

**No new ticket needed, confirmed already covered**:
- Full keyboard symbol range — subsumed by ticket 02's picker scope.
- Canned-text macros — achievable for free once ticket 02's full keyboard range lands (a Macro of per-character Keypresses); ticket 02 carries a note to confirm this and judge whether a convenience "type this text" input is worth building.
- Thumbstick's four virtual diagonal bindings — free once Chord (ticket 01) lands, since the thumbstick's cardinal directions are already ordinary Inputs and Chord is generic over any 2+ simultaneous Inputs; also achievable today via a near-zero-delay Macro, independent of Chord. Worked example added to ticket 01, no new ticket.
- Mouse Wheel's Synapse-parity feature set — Acheron's Stepper (ticket 03) already exceeds what Synapse offers here (Synapse can't fire Controller/Joystick from the wheel; Acheron has no such restriction). No action needed.

**Release timing**: the two non-blocking strands (analog capture, Controller/Joystick) do not gate v1.0 — the map's destination is reached once the rest of the frontier (all required tickets above) is empty, regardless of whether either strand has landed. If either finishes first, it ships in v1.0; if not, it fast-follows as a later release.

