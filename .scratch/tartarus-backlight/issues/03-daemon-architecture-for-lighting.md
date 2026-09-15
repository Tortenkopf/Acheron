Type: grilling
Blocked by: 01, 02
Status: resolved (Charon, 2026-09-15)

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

**The settled daemon architecture.** Grilled + code-checked against `daemon/src/` on `dev`
(HITL, Charon, 2026-09-15). Decisions only — no build. Feeds
[ticket 04](./04-gui-daemon-surface-for-lighting.md) (GUI/D-Bus surface, now the frontier) and
[ticket 05](./05-write-lighting-spec.md) (the spec).

### 0. Scope correction — ripple/ripple-random confirmed out of scope

Ticket 01 found ripple is not a device-side command: OpenRazer fakes it with a ~25 Hz host
thread streaming `matrix_custom_frame`/`matrix_effect_custom` writes — no
`matrix_effect_ripple` sysfs file or driver command exists at all. That mechanism is exactly
what the map's own "Out of scope" section already excludes ("Host-streamed per-frame
Custom-layout animation"). Closing this as a scope clarification, not a fresh inclusion: no
persistent-fd/render-loop mode, no host-side effect thread, in this effort. `FixedEffect` (§2)
carries no ripple variant.

### 1. The writer — `led` task extended, not a sibling

Per ADR-0006's own forward note ("All lighting frames — now and future — route through the
one `led` task; the device has a single control channel and frames must not interleave"),
Lighting's writer lives in the existing `led` task (`daemon/src/led.rs`), not a new task.

- `daemon/src/led.rs`'s existing `watch::Sender<Option<StatusLeds>>` plumbing
  (`main.rs:109-110,164`, `dispatch.rs:103-108,158-163`) is untouched.
- A second, independent channel is added: `lighting_tx: watch::Sender<Option<LightingState>>`
  / `lighting_rx`, created in `main.rs` alongside `led_tx`/`led_rx` and handed to both
  `led::spawn` and `DispatchState`, mirroring `led_tx`'s wiring exactly.
- `led::run` (`led.rs:61`) grows a second parameter, `mut lighting_rx:
  watch::Receiver<Option<LightingState>>`, and its `while rx.changed().await` loop becomes a
  `loop { tokio::select! { ... } }` over both receivers — each arm still does
  `borrow_and_update()` → `spawn_blocking` → **awaited** before the loop repeats, so a write
  triggered by either channel fully completes before the next `select!` poll starts. This is
  the serialization primitive: two receivers, one task, one write in flight at a time,
  regardless of which channel woke it.
- Rejected: one combined channel (`watch::Sender<Option<LedState>>` bundling both
  `StatusLeds` and `LightingState`). It would force every Status-LED-only push
  (`push_status_leds`) to also resupply the current Lighting value (or vice versa), widening
  code and tests that today only know about `StatusLeds` for no serialization benefit the
  two-channel/one-task shape doesn't already give.
- `AssertLeds`-style trait (`led.rs:35-37`) gets a Lighting-shaped sibling (`assert_lighting`
  or a second trait method) so tests can substitute a recorder exactly like the existing
  `Recorder`/`RecordingSink` pattern.
- `capture/analog.rs` gets `assert_lighting(state: LightingState) -> io::Result<()>`, a
  standalone function modelled on `assert_status_leds` (`analog.rs:554`) — `discover_hidraw()`
  → open Interface 2 → one `HIDIOCSFEATURE` per §2 frame(s) → drop the fd. A `CustomLayout`
  assert is the write-then-arm two-step ticket 01 documented
  (`matrix_custom_frame` then `matrix_effect_custom`) — still one short-lived fd, two
  sequential `HIDIOCSFEATURE` calls before it's dropped, not two separate task iterations.

### 2. Config type — a sum type, typed against ticket 01's wire facts now

```rust
/// CONTEXT.md: Lighting assignment (ticket 05 files the term). Mutually
/// exclusive with itself by construction (Q4 from charting) — Off is its
/// own variant, not a bool, so asserting Off means sending the firmware's
/// own "none" effect frame (command_id 0x02, effect 0x00): one concept, not
/// a config-level no-op distinct from a device-level one.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LightingAssignment {
    #[default]
    Off,
    FixedEffect { effect: FixedEffect },
    CustomLayout { colours: [Colour; 21] },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FixedEffect {
    Static { colour: Colour },
    Spectrum,
    Reactive { colour: Colour },
    Wave { direction: Direction },
    Breath { style: BreathStyle },
    Starlight { style: BreathStyle },
}

/// random | single(colour) | dual(colour, colour) — shared by Breath and
/// Starlight, which ticket 01 found take an identical style split.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "style", rename_all = "snake_case")]
pub enum BreathStyle {
    Random,
    Single { colour: Colour },
    Dual { first: Colour, second: Colour },
}

/// Named struct, never a tuple or hex string — same reasoning `StatusLeds`
/// documents (config.rs:474-478): keeps the type additive-friendly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Colour { pub r: u8, pub g: u8, pub b: u8 }
```

Tagging style (`#[serde(tag = "type", rename_all = "snake_case")]`) matches this file's
existing precedent for data-carrying enums (`Action`, config.rs:650). Typed fully now because
these are wire-level facts ticket 01 already established (Breath/Starlight's random/single/dual
split, Wave's direction, Static/Reactive's single colour, Spectrum's none) — not open
decisions. What remains open is which of these fields the GUI exposes as editable vs. fixes to
a default, which stays [ticket 04](./04-gui-daemon-surface-for-lighting.md)'s call, now
unblocked.

`brightness: u8` lands as a sibling field on `Profile`, alongside `lighting:
LightingAssignment` — not nested in the enum, since it applies uniformly regardless of which
variant is active (one more byte on the same one-shot frame, exactly like Status LEDs'
`Default` reasoning documented at config.rs:474-478 for "additive change" but here present
from day one since ticket 02 confirmed the `0x00`–`0xFF` range).

### 3. `config.toml` — plain `#[serde(default)]`, no version bump

`Profile` (config.rs:169's `status_leds` neighbourhood) gets two new
`#[serde(default)]` fields, `lighting: LightingAssignment` and `brightness: u8`, following
every prior additive Profile field (`status_leds`, `chords_base`, `axis_base`, `deep_stages`)
exactly. A pre-feature `config.toml` parses with every Profile at
`LightingAssignment::Off`/`brightness: 0` — the serde default is the migration, no
`SCHEMA_VERSION` bump (config.rs:1101-1110).

### 4. Dispatch wiring — `Effect::AssertLighting`, mirrors `AssertStatusLeds` exactly

- A new unit variant `Effect::AssertLighting` in `edit.rs`'s `Effect` enum, alongside
  `AssertStatusLeds` (edit.rs:439).
- `edit::plan`'s `SwitchProfile` arm (edit.rs:615) appends it unconditionally alongside
  `AssertStatusLeds` — order between the two is irrelevant, same independence argument.
- The ticket-04 set-Lighting edit (name TBD by that ticket) appends it unconditionally too,
  mirroring `SetStatusLeds` (edit.rs:830-840) — no `target == active` gate needed, by the same
  structural argument ADR-0006/ticket-03-for-status-leds §5 already established (every
  mutating D-Bus method is Profile-unscoped; the GUI always edits the active Profile).
- `run_effects` (dispatch.rs:770-774's `AssertStatusLeds` arm) gets a sibling arm calling a new
  `DispatchState::push_lighting(&self, config: &Config)` helper (mirroring `push_status_leds`,
  dispatch.rs:158-163) — reads `config.active_profile().lighting` and `.brightness`, sends
  `Some(LightingState { assignment, brightness })` on `self.lighting_tx`.
- **No cached `lighting_state` in `DispatchState`** — `Config` stays the sole authoritative
  source, same as Status LEDs (no partial-update problem: every assert sends the full
  assignment + brightness).

### 5. Startup + (re)connect assertion — exact parity with Status LEDs

`dispatch::run`'s `rx_connection` arm (dispatch.rs:1021's `push_status_leds` call site) gets a
sibling `state.push_lighting(&config)` call, same trigger (every message where `connected ==
true`, no transition-only gating), same reasoning: idempotent, cheap relative to the ~1-2ms
`discover_hidraw` cost either way, and `watch` coalescing already bounds a same-tick burst to
one write per channel. A `CustomLayout` assert being a full 21-colour payload instead of 3 bits
doesn't change this — still one `HIDIOCSFEATURE` ioctl (two for the write-then-arm two-step),
not a hot path.

### 6. No shutdown clear — asymmetric with Status LEDs, and deliberately so

New fact from ticket 01's research not previously on the map: every backlight effect-select
frame carries `VARSTORE` (`arg0 = 0x01`), meaning the firmware persists the asserted effect in
its own storage — unlike Status LEDs' frame, which uses `NOSTORE` and which the firmware always
reclaims to its orange-only default on reconnect (the reason Status LEDs' re-assertion is
mandatory and its `clear_status_leds()` exit-time clear is meaningful, `main.rs:210`).

Lighting gets **no** equivalent `clear_lighting()` call in `relock_and_exit` (`main.rs:201`).
Backlight is cosmetic, not a daemon-health signal the way Status LEDs are treated; since
`VARSTORE` already makes the effect outlive the daemon process, a clear-on-exit write would
only actively erase the user's chosen look from the device's own storage for no benefit — the
next connect or daemon startup re-asserts the active Profile's Lighting regardless (§5). This
is a conscious asymmetry with Status LEDs, not an oversight — worth a line in ticket 05's ADR
so a future reader doesn't "fix" it into false symmetry.

### 7. Domain — deferred to ticket 05, same lazy discipline as Status LEDs

No `CONTEXT.md` edits or ADR filed by this ticket. [Ticket 05](./05-write-lighting-spec.md)
files: the **Lighting assignment** / **Fixed effect** / **Custom layout** glossary entries, and
a new ADR (extending ADR-0002/ADR-0006) covering the two-channel/one-task plumbing (§1) and the
VARSTORE-driven shutdown asymmetry (§6) — the two choices a future reader would most need
"why" for.

