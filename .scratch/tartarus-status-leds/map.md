Label: wayfinder:map

# Tartarus status-LED indicator

> **SPEC READY — this map is done (2026-09-02).** All five tickets are resolved. The
> destination — [`spec.md`](./spec.md) — is written, along with
> [ADR-0006](../../docs/adr/0006-status-leds-driven-from-dispatch-over-short-lived-hidraw-fds.md)
> and the `Status LED` / `Status LED assignment` `CONTEXT.md` entries. **Implementation is a
> separate, fresh effort**, not a resumption of this map. This map can be archived.

## Destination

A reviewed **`spec.md`** for a "Profile status-LED indicator" feature — every Profile carries
a defined on/off state for the three side **Status LED**s, asserted whenever that Profile
becomes active — **ready to hand to a separate implementation effort**.

The destination is the *spec*, not the build. The hardware prototype lives inside this map;
the daemon/GUI implementation does not.

**This is a gated effort.** [Ticket 01 (the prototype)](./issues/01-prototype-status-led-controllability.md)
is a true kill-gate: if the three Status LEDs turn out not to be host-controllable on the
real Tartarus Pro — or driver-mode / the LED frame does something adverse to the unit — the
effort is **archived with the negative result recorded** and no spec is written. Same
discipline the analog strand was held to (`tartarus-input-expansion` tickets 12/13): if it's
safe we do it, if it's risky we drop it.

## Notes

**This map plans, it does not execute** — resolving a ticket produces a decision (or, for the
prototype and research tickets, a finding). The one exception is [ticket 01](./issues/01-prototype-status-led-controllability.md),
which builds a *throwaway* prototype under `prototype/` — a feasibility test, not a step
toward the shipped feature.

**Grounding research** (done before this map existed):
[`research/tartarus-pro-status-leds.md`](./research/tartarus-pro-status-leds.md) — the common
wisdom that no open reimplementation cracked these LEDs is **wrong**. The Tartarus Pro drives
them through its extended-matrix effect command (`command_class 0x0F`, `command_id 0x02`,
**LED ID `0x0B`**, static effect) whose R/G/B argument bytes are three independent on/off
channels — one per fixed-colour LED (orange, green, blue). Implemented for real in OpenRazer
PR #2336 (closed unmerged) and shipped in CommandPost. The classic `0x03` profile-LED path
that everyone tried does nothing on the Pro. Acheron already speaks this exact transport —
`hidraw` Interface-2 feature reports, `build_razer_cmd`, CRC, `HIDIOCSFEATURE` — for analog
capture (`daemon/src/capture/analog.rs`), and already enters driver mode routinely, so the
prototype adds no daemon changes and carries no caution beyond what analog already lives with.

**Grounding facts found while charting (2026-09-02):**
- The user confirms from Windows/Synapse use that the three LEDs are exactly orange / green /
  blue and pure on/off — no adjustable brightness. The **orange LED is currently lit**.
  *(Charting assumed this was persisted Synapse state in onboard `VARSTORE` memory —
  [ticket 01](./issues/01-prototype-status-led-controllability.md) disproved that: neither
  VARSTORE nor NOSTORE host writes survive a re-enumeration; "orange only" is just the
  firmware's power-on keymap-indicator default.)* Acheron's Profile settings never live on
  the device.
- `Profile` (`daemon/src/config.rs:130`) is a plain `#[derive(Serialize, Deserialize)]` struct
  with `schema_version`-gated migrations already in place (currently `schema_version = 2`).
- `active_profile` / `active_profile()` live on `Config` (`config.rs:39/84`); the switch path
  is `edit::Edit::SwitchProfile` mutating `Config` directly (`config.rs:529`).
- `daemon/examples/device_info_probe.rs` already does an Interface-2 `hidraw` feature-report
  round-trip; `prototype/13-analog-grid-capture/prototype.py` is the verified Python harness
  the LED prototype should be modelled on.

**Decisions settled during the charting grilling — not to be re-litigated per ticket:**
- **Destination = gated spec** (Q1); **true kill-gate** on the prototype (Q2).
- **v1 model: three independent on/off toggles per Profile** (Q3). No custom colours, no
  brightness, no non-static effects — all out of scope (see below).
- **Every Profile has a defined Status-LED state** (Q4, Option A) — default `(off, off, off)`;
  switching Profiles is fully deterministic from the active Profile alone. Migration default
  for pre-existing Profiles is all-off.
- **Startup + reconnect:** the daemon asserts the active Profile's Status-LED state on launch
  **and on every device (re)connect** (Q6; ticket 01 made the reconnect assertion a hard
  requirement — the firmware reclaims the LEDs on every enumeration).
- **Shutdown:** clear all three LEDs on clean daemon exit — settled; ticket 01 confirmed
  all-off `(0,0,0)` is hardware-reachable (Q6/Q13 contingency did not trigger).
- **Layers never touch the Status LEDs** (Q10) — they track the Profile only. The user knows
  when they're holding the Mode key; the value of a static indicator is knowing which Profile
  is active.
- **No global opt-out setting** (Q11) — treated like analog depth: safe ⇒ do it, risky ⇒ drop
  the effort.
- **GUI control: three labelled colour toggles in a "Status LEDs" group on the Device
  Overview panel, near the Profile selector** (Q4/Q14). Specced in text — no UI prototype.
- **`config.toml`: a named table per Profile**, e.g.
  `status_leds = { orange = true, green = false, blue = false }`, with a `schema_version` bump
  and an all-off migration default (Q15).
- **Terminology (Q9)** — two reserved names, recorded here now, **added to `CONTEXT.md` when
  [ticket 05](./issues/05-write-status-led-spec.md) resolves** (same lazy discipline
  `tartarus-input-expansion` held Chord/Stepper to — a gated effort doesn't get glossary
  entries until its model is settled and it's known to survive):
  - **Status LED** — one of the three fixed-colour (orange, green, blue) on/off indicator LEDs
    on the device's left side. *Avoid:* "profile LED", Razer's "keymap indicator", "Chroma".
  - **Status LED assignment** — the per-Profile triple of on/off states, asserted on Profile
    switch.

**Skills to consult:** none — tickets 01–05 are all resolved and the spec is written. Nothing
left to decide or do on this map.

## Decisions so far

<!-- one line per closed ticket: enough to judge relevance, then zoom the link -->

- [Write the Status-LED spec](./issues/05-write-status-led-spec.md) — **done.** The
  destination is written: [`spec.md`](./spec.md) (feature summary, wire frame, daemon
  architecture, config schema, D-Bus surface, GUI, startup/shutdown, out-of-scope,
  vocabulary), [`docs/adr/0006-status-leds-driven-from-dispatch-over-short-lived-hidraw-fds.md`](../../docs/adr/0006-status-leds-driven-from-dispatch-over-short-lived-hidraw-fds.md)
  (refines ADR-0002), and the `Status LED` / `Status LED assignment` entries in `CONTEXT.md`.
  `.scratch/README.md` flipped to "spec ready". Nothing new decided — pure consolidation of
  tickets 01–04. **Implementation is a fresh effort.**

- [GUI ↔ daemon surface for Status LEDs](./issues/04-gui-daemon-surface-for-status-leds.md) —
  **settled** (HITL grilling). **Config:** new `StatusLeds { orange, green, blue }` struct
  (named, per ticket 03 §7) + `#[serde(default)] pub status_leds: StatusLeds` on `Profile`,
  serialized in full like `default_actuation`. **No `schema_version` bump** — the codebase has
  never bumped (`parse` hard-refuses a mismatch, no migration machinery); the all-off serde
  default *is* the migration, exactly as ticket 17/18/51/54 added their fields. New parse test
  mirrors `a_pre_ticket_17_...`. **No `config::validate` rule** (every bool-triple is valid).
  **D-Bus:** `Edit::SetStatusLeds { orange, green, blue }` / `SetStatusLeds(bbb)` — whole
  triple, one call, unconditional `Effect::AssertStatusLeds` (ticket 03 owns the effect +
  handler), no `target == active` gate, **no new signal** (no generic config-changed signal
  exists; siblings emit none). **`GetState()`: no addition** — config is the single source of
  truth; the sub-second orange-only connect transient is self-healing and there is no
  hardware-divergence path (ticket 01), unlike `capture_mode`. **Storage-mode knob: cut**
  (byte inert, no cached state). **GUI** (per user mockup
  `screenshots/Status LED location Mockup.png`): a "Status LEDs" group in `device_row`
  **between the thumbstick and Chords** — three vertically-stacked colour lozenges
  (orange/green/blue top-to-bottom, mirroring the device), **lit = full-saturation + border,
  unlit = desaturated + flat** (strong contrast, not a brightness shift), tooltip +
  `Gtk.Accessible` per lozenge (colour-blind safety), **no visible per-lozenge text**. Edits
  the **active** Profile (no "selected-for-editing" concept in Acheron — ticket 03 §5).
  State always from `config[...]["status_leds"]`, never a hardware read; a new Profile shows
  all-dark (default all-off ≡ "never set"); shown on **both Layers**, **Grid destination
  only**; renders the stored state even while the device is disconnected. `daemon_client.py` /
  `daemon_stub.py` / `wire.py` get the mechanical mirror; **`rules.py` gets nothing**.
  Corrects ticket 05's "schema_version bump + migration" / "GetState additions (if any)"
  bullets. No new `CONTEXT.md` term or ADR from this ticket (both land with ticket 05).
  No fog graduates; ticket 05 is now the sole remaining (and unblocked) ticket.

- [Daemon architecture for Status LEDs](./issues/03-daemon-architecture-for-status-leds.md) —
  **settled** (HITL grilling). Write primitive: `assert_status_leds(StatusLeds)` /
  `clear_status_leds()` in `analog.rs`, modelled on `relock()` — short-lived Interface-2
  hidraw fd, one `0x0F/0x02` static frame, **no driver mode, no read-back**. Writer: a
  dedicated **non-fatal `led` task** (sibling of `injector`, *not* in `main.rs`'s top-level
  `select!`), fed `watch<Option<StatusLeds>>` from dispatch — serialised `spawn_blocking`
  writes, `watch` coalesces switch bursts, owns the shutdown all-off write. Decider:
  **dispatch, sole owner** — `Config.active_profile().status_leds` is the authoritative
  triple (no cache); new unit `Effect::AssertStatusLeds` emitted by `Edit::SwitchProfile`
  **and** the ticket-04 set-edit (unconditionally — §5); `run_effects` + the `rx_connection`
  arm share a `push_status_leds(&config)` helper. Connect/startup/reconnect: assert on
  **every `connected == true`** from `rx_connection` (no flag — idempotent; the firmware
  reclaims the LEDs on every enumeration). Device absent ⇒ `Err` logged, non-fatal, never
  blocks a switch. Shutdown: `relock_and_exit` clears all-off before `relock()`. **No
  non-active-Profile write path exists** (all D-Bus edits are Profile-unscoped; GUI switches
  first) → no gate, and ticket 04's "active vs selected-for-editing" framing is dropped.
  `StatusLeds` must be a **named struct** (not `[bool; 3]`) so brightness/effect/backlight
  are additive later. No new `CONTEXT.md` term; **ADR-0006 warranted** (drafted in the
  Answer, filed by ticket 05). Future host-streamed animation would refine the `led` task's
  fd lifetime without disturbing the transport/ownership/no-driver-mode seams; all future
  lighting routes through the one `led` task.

- [Research: Status-LED wire protocol](./issues/02-research-status-led-wire-protocol.md) —
  implementation-ready ([write-up](./research/status-led-wire-protocol.md), all primary-source
  cited; annotated with ticket 01's hardware corrections). The frame is a standard
  extended-matrix static effect =
  `build_razer_cmd(0x1F, 0x0F, 0x02, &[0x00,0x0B,0x01,0x00,0x00,0x01, r,g,b])`, **no helper
  changes**, arg6/7/8 = R/G/B. Settled: **no driver-mode call needed** (LED frame is
  independent of Capture mode — send on a short-lived Interface-2 fd); the **daemon owns an
  authoritative RGB triple and re-sends the whole frame per change** (safe cross-device — and
  it must write unconditionally on every connect anyway); **off = static frame with channel
  byte `0x00`**, never `effect_none`; **arg4 = `0x00`** (CommandPost's `0x01` is inert);
  **arg0 = `0x00`** for intent-clarity (the storage byte turned out inert on our unit — see
  ticket 01 — so no config knob). Corrects three prose claims in the grounding file's §3
  (driver mode / read-back framing / PR #1577 — which contains no `0x0B` code; **PR #2336 is
  the sole real implementation and it's unmerged**).

- [Prototype: Status-LED controllability](./issues/01-prototype-status-led-controllability.md)
  — **KILL-GATE PASSED, the effort proceeds.** On the real unit (fw v1.2): all 8 on/off
  combinations lit exactly as commanded (crit 1 ✅), no reset / re-enumeration across ~25
  writes (crit 4 ✅), all-off `(0,0,0)` leaves all three dark (crit 5 ✅ → "clear on clean
  exit" stands). **Every byte ticket 02 derived from source verified on hardware** — the
  frame is correct; send `arg4 = 0x00`; `effect_none` does nothing (ACKs but no visual
  change) → off = static-zero. New facts: (a) **the Tartarus Pro has no host-independent
  on-device keymap switch** → the "re-assert on on-device keymap change" hook is **not
  needed** (crit 3); (b) **neither NOSTORE nor VARSTORE persists across a USB
  re-enumeration** — the firmware reclaims the LEDs to its "orange-only" default on every
  boot regardless → ticket 02's "stale *host* state on boot" concern is moot, but **the
  daemon must assert Status-LED state on startup AND on every device reconnect** (hard
  requirement for ticket 03); (c) **`arg0` NOSTORE/VARSTORE is cosmetic here** — both
  accepted, neither persists, settled read-back always echoes `0x01`; send `0x00` for intent
  but expect no observable difference, and **the storage-mode config knob ticket 02/04
  imagined is likely not worth exposing**; (d) **`0x82` read-back is trustworthy on fw v1.2**
  — directly tested the cold-read seed case ticket 02 warns about (4× after replug → true
  firmware state, not stale), so ticket 02's cross-device GET caution doesn't bite here
  (crit 2); spec keeps the authoritative-triple design regardless. Prototype:
  `prototype/01-status-leds/` (on `dev`, like ticket 13's harness); evidence:
  [`assets/01-RESULTS.md`](./assets/01-RESULTS.md).

## Not yet specified

<!-- in-scope fog; graduates to tickets as the frontier advances -->

- *(nothing outstanding — ticket 01 cleared the three items that were here: the on-device
  keymap-switch re-assert hook is not needed, all-off is reachable, and storage mode is
  NOSTORE. All three are folded into the ticket 01 decision line above and feed tickets
  03/04/05 directly.)*

## Out of scope

<!-- ruled beyond this destination; never graduates -->

- **The per-key Chroma backlight** (grid keys + mouse wheel) — Acheron already ignores it. It
  is primarily aesthetic; the Status LEDs are indicators. Razer routes both through Synapse's
  lighting settings, but they are different mechanisms (`command_id 0x03` linear matrix vs
  `command_id 0x02` LED effect) and different concerns.
- **LED brightness control** — the hardware is on/off only (user-confirmed).
- **Non-static LED effects** (breathing / pulse) — `effect` byte stays `0x01` (static).
- **Custom / arbitrary LED colours** — the three colours are fixed in hardware.
- **Automatic or per-application Profile/LED switching** — excluded by the definition of
  Profile (never switched automatically).
- **A bindable "set Status LED" Action** — the LEDs are Profile-driven only; a Binding cannot
  fire an LED change directly.
