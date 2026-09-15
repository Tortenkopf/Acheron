Type: grilling
Blocked by: 03
Status: open

## Question

Design the Lighting tab and its D-Bus surface, the way
[`tartarus-status-leds/issues/04-gui-daemon-surface-for-status-leds.md`](../../tartarus-status-leds/issues/04-gui-daemon-surface-for-status-leds.md)
did for Status LEDs. Invoke `/grilling` and `/domain-modeling`.

- **Placement:** a third arm of `device_overview.py`'s `build_destination_switch`, alongside
  Grid and Library — keeps the Profile sidebar exactly as Grid does (per the user's original
  framing), not Library's Steppers/Macros sidebar swap.
- **Copy-from-Profile:** an explicit affordance to copy another Profile's whole Lighting
  assignment onto the one being edited (user-specified in charting) — one-shot copy, not a live
  link.
- **Fixed-effect picker:** the parameter surface per effect (colour(s), speed, direction) —
  sourced from ticket 01's byte tables, so this ticket is blocked on that data existing, not
  just on ticket 03's config shape.
- **Custom-layout painter:** reuse the Grid destination's physical-layout button component for
  the 20 grid keys + scroll wheel (paintable), with the Mode key and thumbstick rendered but
  inert (per Q6) — decide whether this is the *same* component in a "paint mode" or a
  purpose-built variant.
- **Brightness control** — widget shape once ticket 01 settles the value range.
- **D-Bus surface:** new `Edit` variant(s) (e.g. `SetLightingAssignment`), and the
  `wire.py` / `daemon_client.py` / `daemon_stub.py` / `rules.py` mirror — decide whether this is
  one whole-assignment call (like `SetStatusLeds`'s whole-triple shape) or split by mode.

## Answer

