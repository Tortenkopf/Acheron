Type: grilling
Blocked by: 01, 02
Status: open

## Question

Design the daemon-side shape for Lighting, the way
[`tartarus-status-leds/issues/03-daemon-architecture-for-status-leds.md`](../../tartarus-status-leds/issues/03-daemon-architecture-for-status-leds.md)
did for Status LEDs. Invoke `/grilling` and `/domain-modeling`.

- **Writer task:** does Lighting extend the existing `led` task (widening its scope beyond the
  three Status LEDs) or get a sibling task? Both still write occasional one-shot frames on the
  same Interface-2 hidraw transport, and ADR-0006 already establishes "the device has a single
  control channel and frames must not interleave — a second parallel writer is disallowed," so
  whichever shape is chosen must keep Status-LED writes and Lighting writes serialised through
  one path.
- **Config type:** a `LightingAssignment` struct/enum on `Profile` — mutually-exclusive Fixed
  effect vs. Custom layout (Q4), off default (Q8), brightness (Q5), Profile-only (Q7, no Layer
  field). Named type per the same reasoning `StatusLeds` used ("so brightness/effect/backlight
  are additive later") — decide the concrete shape (e.g. an enum `LightingMode { FixedEffect {
  effect, params }, CustomLayout { colours: [Colour; 21] } }` plus a sibling `brightness: u8`
  and `on: bool`, or fold "off" into the enum as a variant — settle which).
- **`config.toml` shape** and whether this needs a `schema_version` bump or a plain `#[serde(default)]`
  migration (precedent: `StatusLeds` needed neither).
- **Dispatch wiring:** an `Effect::AssertLighting`-style unit, mirroring `Effect::AssertStatusLeds`
  — emitted on Profile switch and on whatever the ticket-04 set-edit turns out to be, plus
  startup/reconnect/shutdown assertion timing (near-certain to mirror Status LEDs exactly —
  assert on every `connected == true`, clear-or-off on clean exit — but confirm rather than
  assume, since the *frame itself* is more expensive here: a Custom-layout write is a full
  21-colour payload, not a 3-bit triple).

## Answer

