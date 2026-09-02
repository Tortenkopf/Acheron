Type: task
Blocked by: 02, 03, 04
Status: resolved (Charon, 2026-09-02)

## Question

Write **`.scratch/tartarus-status-leds/spec.md`** — the effort's destination — from the
decisions and findings resolved across this map. This is a writing task: everything it needs
is settled by the time it's unblocked; nothing new is decided here.

The spec is the hand-off to a separate implementation effort. It should cover:

- **Feature summary** — every Profile carries a Status-LED assignment (three fixed-colour
  on/off toggles); the daemon asserts it whenever that Profile becomes active, on startup,
  **and on every device (re)connect** (ticket 01: the firmware reclaims the LEDs on every
  enumeration — nothing persists host-side).
- **The wire frame** — the exact bytes, from [ticket 02](./02-research-status-led-wire-protocol.md)
  and verified on hardware by [ticket 01](./01-prototype-status-led-controllability.md):
  `build_razer_cmd(0x1F, 0x0F, 0x02, &[0x00, 0x0B, 0x01, 0x00, 0x00, 0x01, r, g, b])`; off =
  same frame with the channel byte(s) `0x00` (**not** `effect_none`); `arg0 = 0x00`, no
  storage-mode knob (the byte is inert); **no driver-mode call**; `0x82` read-back reliable on
  fw v1.2 but the design keeps an authoritative triple regardless.
- **Daemon architecture** — from [ticket 03](./03-daemon-architecture-for-status-leds.md):
  hidraw ownership across both capture modes, the Profile-switch hook point, startup +
  reconnect assertion, device-absent handling. (No on-device-keymap re-assert hook — ticket 01
  confirmed the Tartarus Pro has no such switch.)
- **Config schema** — from [ticket 04](./04-gui-daemon-surface-for-status-leds.md): the
  `[profiles.<name>.status_leds]` table, the `StatusLeds` struct + `Profile.status_leds`
  field, **additive `#[serde(default)]` — no `schema_version` bump**, all-off serde default
  as the migration (ticket 04 §1 corrects the earlier "bump + migration" language).
- **D-Bus surface** — from [ticket 04](./04-gui-daemon-surface-for-status-leds.md): the new
  `Edit::SetStatusLeds { orange, green, blue }` / `SetStatusLeds(bbb)` method, whole-triple,
  unconditional `Effect::AssertStatusLeds`. **No `GetState()` addition and no new signal**
  (ticket 04 §3).
- **GUI** — from [ticket 04](./04-gui-daemon-surface-for-status-leds.md) §5: the "Status LEDs"
  group in `device_row` between the thumbstick and Chords (per the user's mockup,
  `screenshots/Status LED location Mockup.png`) — three vertically-stacked colour lozenges,
  lit/unlit from the active Profile's stored triple, Grid-destination-only, shown on both
  Layers, edits the active Profile. Plus the `daemon_client.py` / `daemon_stub.py` /
  `wire.py` mirror (ticket 04 §6); nothing in `rules.py`.
- **Startup / shutdown behaviour** — assert on startup and every reconnect;
  **clear all three LEDs on clean daemon exit** ([ticket 01](./01-prototype-status-led-controllability.md)
  criterion 5: all-off `(0,0,0)` is reachable — the Q6/Q13 contingency does not trigger).
- **Out of scope** — carry the map's Out-of-scope list into the spec so the implementation
  effort inherits the boundary.
- **CONTEXT.md** — note the `Status LED` / `Status LED assignment` glossary entries (added
  during charting) so the implementation effort keeps the vocabulary.

Also: add the effort's spec to `.scratch/README.md`'s line for this effort, and flip the map
+ this effort to a state that says "spec ready, implementation is a fresh effort".

**If [ticket 01](./01-prototype-status-led-controllability.md) killed the effort, this ticket
is closed unresolved** — no spec — and the map is archived with the negative result.

## Comments

**2026-09-02 (Charon) — [ticket 03](./03-daemon-architecture-for-status-leds.md) resolved.**
Two additions to this ticket's scope, following the map's lazy discipline (glossary + ADRs
land when the spec lands):

- **File `docs/adr/0006-status-leds-driven-from-dispatch-over-short-lived-hidraw-fds.md`**
  (or similar slug — scan `docs/adr/` for the next number) alongside the spec. Full drafted
  text is in [ticket 03](./03-daemon-architecture-for-status-leds.md)'s Answer §9. It
  refines ADR-0002. The spec's "Daemon architecture" section then *references* it rather
  than re-arguing the hidraw-ownership / no-driver-mode choice.
- **Add the `Status LED` / `Status LED assignment` `CONTEXT.md` entries** (reserved in the
  map's Notes) as part of this ticket, not a follow-up.

**2026-09-02 (Charon) — [ticket 04](./04-gui-daemon-surface-for-status-leds.md) resolved;
this ticket is now the sole remaining frontier ticket.** Its Answer settles the full
config schema, D-Bus surface, and GUI control — nothing new to decide here, this stays a
pure writing task. Scope bullets above updated to match. Key corrections it makes:
`schema_version` is **not** bumped (additive `#[serde(default)]` field, all-off serde
default *is* the migration — the codebase has never bumped and `parse` hard-refuses a
mismatch); `GetState()` gets **no** new field and there is **no** new signal; the GUI group
lives in the device area (mockup), not the Profile sidebar.

## Answer

**Done — the spec is written.** Pure consolidation of tickets 01–04; nothing new decided.
Three files created, two updated:

- **[`.scratch/tartarus-status-leds/spec.md`](../spec.md)** (`Status: ready-for-agent`) —
  modelled on `tartarus-keybinder/spec.md`. Sections: Problem Statement, Solution, 10 User
  Stories, Implementation Decisions (wire frame / daemon architecture / config schema /
  D-Bus surface / GUI / startup-shutdown / domain vocabulary), Testing Decisions (the `led`
  task seam + the `DaemonStub` seam), Out of Scope (map list carried through, plus the two
  knobs tickets 01/04 cut), Further Notes (ADR pointer, retroactive corrections, ticket-03
  §8 future-proofing, prior art). Every byte of the frame, every struct/field/method name,
  every hook point is quoted from the resolved tickets with links back.
- **[`docs/adr/0006-status-leds-driven-from-dispatch-over-short-lived-hidraw-fds.md`](../../../docs/adr/0006-status-leds-driven-from-dispatch-over-short-lived-hidraw-fds.md)**
  — from ticket 03 §9's drafted text, formatted as an ADR (title matches filename, prose +
  "Considered and rejected"). Refines ADR-0002.
- **`CONTEXT.md`** — added `Status LED` and `Status LED assignment` at the end of the
  `### Configuration` glossary section, with `_Avoid_` lines in the file's house style
  (rules out "profile LED", "keymap indicator", "Chroma", "LED profile").
- **`.scratch/README.md`** — the effort's line rewritten from "active, frontier: ticket 05"
  to "**spec ready**", linking `spec.md` and ADR-0006, stating implementation is a fresh
  effort and this map can be archived.
- **`map.md`** — a `SPEC READY — this map is done` banner under the title; ticket 05's
  entry prepended to Decisions so far.

**No fog graduates** (Not yet specified was already empty), **nothing new ruled out of
scope**, **no new tickets** — ticket 05 was the last one. The map is complete. Implementation
picks up from `spec.md` as a separate effort.
