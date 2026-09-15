Label: wayfinder:map
Status: archived — destination reached 2026-09-15; all five tickets resolved, spec.md handed off. Implementation continues in a fresh, non-wayfinder effort (`tartarus-backlight-impl/`), not a resumption of this map.

# Tartarus Pro backlight lighting

## Destination

A reviewed **`spec.md`** for a per-Profile **Lighting assignment**: every Profile carries
either a **Fixed effect** (one of the firmware's built-in device lighting effects — static,
breathing, wave, spectrum, starlight, ripple, reactive, or none) or a **Custom layout** (a
per-key colour map across the 20 grid keys plus the scroll wheel), plus a brightness level,
asserted whenever that Profile becomes active — the same shape as Status LED assignment.
Ready to hand to a separate implementation effort, targeting the 1.3 release.

**Ungated** — no kill-gate prototype. The extended-matrix hidraw transport is already proven
host-controllable on this exact device by ADR-0006 (Status LEDs use the same command family,
different LED id); this effort only needs the backlight-specific byte layout and one
hardware-verification pass, not a feasibility test that could kill the effort outright.

## Notes

**Domain:** single global `CONTEXT.md` (confirmed while charting — no `CONTEXT-MAP.md` at the
root). New terms land there only once the spec ticket resolves, the same lazy discipline
`tartarus-status-leds` held.

**Skills every session should consult:** `/grilling` + `/domain-modeling` for the two grilling
tickets (03, 04); `/research` already fired for ticket 01.

**Standing decisions from charting (2026-09-15) — not to be re-litigated per ticket:**

- **Destination = gated spec, but ungated effort** — no kill-gate (Q2). Confidence comes from
  ADR-0006 already proving the transport, plus OpenRazer's kernel driver source (vendored
  locally at `/usr/src/openrazer-driver-3.12.4/driver/`) confirming the Tartarus Pro's
  `BACKLIGHT_LED` rides the identical `razer_chroma_extended_matrix_effect_*` family used by
  dozens of other Chroma keyboards.
- **Lighting is Profile-only, never Layer-scoped** (Q7) — mirrors Status LEDs, and for a
  sharper reason than consistency: a Layer override would mean re-asserting the frame on every
  Mode-key press/release, a much hotter path than a Profile switch, conflicting with
  ADR-0006's "occasional one-shot write" design for the `led` task.
- **A new Profile defaults to off** (matrix dark) (Q8) — mirrors Status LEDs' migration-safe
  all-off default.
- **Fixed effect and Custom layout are mutually exclusive, one per Profile** (Q4) — matches
  firmware reality (only one matrix-effect command is "active" at a time) and stays symmetric
  with the one-state-per-Profile shape Status LEDs already uses.
- **Brightness is in scope**, as a per-Profile level (Q5) — one more byte on the same one-shot
  frame.
- **The firmware's own `reactive` effect is an in-scope Fixed effect** (Q3) — ticket 01
  confirmed it's genuinely firmware-autonomous (one static "arm reactive" frame, device reacts
  to physical keypresses on its own), same one-shot cost as static/breathing/wave. This is a
  *different* thing from the Acheron-driven, remap-aware reactive lighting ruled out below —
  keep the two "reactive"s terminologically distinct. **Correction from Q3's premise:** ripple
  turned out *not* to be firmware-autonomous the way reactive is — ticket 01 found OpenRazer
  fakes it with a ~25 Hz host-side thread streaming Custom-layout frames, which is a genuine
  streaming write path, not a one-shot select-effect command. Whether Acheron implements ripple
  at all (and if so, whether it belongs in this effort given ADR-0006's one-shot-write design)
  is now an open call for ticket 03, not a settled inclusion.
- **"Effect" is fine to reuse for Lighting's built-in patterns** (Q9), despite `CONTEXT.md`'s
  Action entry avoiding it as an Action-synonym (`_Avoid_: output, effect`) — confirmed there's
  only one global glossary (no `CONTEXT-MAP.md`), so this is a real same-word/different-concept
  situation, not a scoped exception. Kept unambiguous by always pairing it with "Lighting"
  ("Lighting effect", never a bare "Effect" as a standalone concept name) — the glossary entry
  for Action's avoid-list stays as-is; it governs what *Action* is called, not a blanket ban on
  the word.
- **The scroll wheel, not the Mode key, is a lit matrix cell** — corrected during charting (the
  user's own hardware knowledge overrides the initial `device_overview.py`-layout inference).
  The Mode key and thumbstick are shown in the Lighting tab for physical-layout fidelity but
  stay inert/unlit (Q6). **Superseded by ticket 02's hardware pass:** the wheel is column `19`
  of 21, not the 21st/last cell as this note originally assumed — see ticket 02's Decisions-so-far
  entry for the full column order. The Mode key/thumbstick finding held up, and turned out
  stronger than stated: they're solid black plastic, not RGB-capable at all.

**Precedent:** `tartarus-status-leds/map.md` and its `spec.md` are the template for this map's
shape and for this effort's eventual spec's section list.

## Decisions so far

<!-- one line per closed ticket: enough to judge relevance, then zoom the link -->

- [Research: Backlight wire protocol](./issues/01-research-backlight-wire-protocol.md) —
  implementation-ready ([write-up](./research/backlight-wire-protocol.md), all primary-source
  cited) for 11 of 13 effects plus both custom-path commands and brightness get/set: all ride
  `command_class 0x0F`, **`BACKLIGHT_LED = 0x05`** (confirmed distinct from the Status LEDs'
  `0x0B`), `transaction_id 0x1F`, VARSTORE. Two surprises: **ripple/ripple-random are not
  device-side commands** — the daemon fakes them with a ~25 Hz software thread streaming
  `matrix_custom_frame`/`matrix_effect_custom`, a scope/cost decision for ticket 03, not a byte
  table; and **Breath actually transmits `transaction_id 0x3F`**, not `0x1F` like every other
  effect, due to what reads as a driver copy/paste bug. Brightness is the one place LED id is
  **`ZERO_LED` (`0x00`)**, not `BACKLIGHT_LED`. Confirmed from source: the custom-frame
  write-then-arm two-step (`matrix_custom_frame` before `matrix_effect_custom`). **Genuinely
  undetermined from source and needs the real device:** which of the 21 matrix columns is the
  scroll wheel — no Tartarus-Pro-specific remap table exists anywhere in the driver or daemon.
- [Hardware verification](./issues/02-hardware-verification.md) —
  ([raw run log](./assets/02-RESULTS.md)) confirms Q6 physically (20 grid keys + wheel lit;
  Mode key/thumbstick are solid black plastic, not merely unlit) and settles the column-order
  gap ticket 01 left open: wire columns **`0..18` = grid keys `1..19`, `19` = the scroll wheel,
  `20` = grid key `20`** — the wheel is second-to-last, **not** the 21st/last cell as charted
  below in the original Q6 note. Also confirms no driver-mode change is needed for any backlight
  effect (wave/static/breathing — single/dual/random — all ran from `device_mode 00 00`), that
  Breath's `transaction_id` quirk is harmless (both `0x1F` and the driver's actual `0x3F` work —
  spec can just use `0x1F`),
  brightness is genuinely `0x00`–`0xFF` with `0x00` alone blacking out the matrix independent of
  effect state, and no adverse device behaviour across ~15 writes + one relock.
- [Daemon architecture for Lighting](./issues/03-daemon-architecture-for-lighting.md) —
  ripple confirmed out of scope (it's exactly the "host-streamed animation" the map already
  excludes). Lighting extends the existing `led` task rather than a sibling task, via a second
  `watch<Option<LightingState>>` channel the same task `select!`s alongside the untouched
  Status-LEDs channel, keeping writes to the shared control interface serialized. Config is a
  new sum type, `LightingAssignment { Off | FixedEffect | CustomLayout }` plus a sibling
  `brightness: u8`, with `FixedEffect`'s six variants fully typed now against ticket 01's wire
  facts (GUI exposure per field stays ticket 04's call); `Off` doubles as the firmware's own
  "none" effect frame, not a separate variant. Plain `#[serde(default)]`, no schema bump.
  Dispatch wiring (`Effect::AssertLighting`) and startup/reconnect assertion timing mirror
  Status LEDs exactly. **One deliberate asymmetry:** no shutdown-clear for Lighting — a new
  fact surfaced that backlight effect-select frames use `VARSTORE` (persisted device-side),
  unlike Status LEDs' `NOSTORE`, so leaving the last-asserted effect in place on daemon exit is
  correct, not an oversight. Domain terms and the ADR are deferred to ticket 05, same lazy
  discipline as Status LEDs.
- [GUI/daemon surface for Lighting](./issues/04-gui-daemon-surface-for-lighting.md) —
  corrects two gaps in ticket 03's type (`Reactive`/`Starlight` gain a `speed: u8` field;
  `Wave` gets a fresh 2-variant `WaveDirection` instead of reusing the unrelated 4-variant
  `input::Direction`). GUI: one flat 8-entry mode selector (Off + the 6 Fixed effects + Custom
  layout, no nesting), every control living in a single horizontal strip above the device area
  — settled via [a 3-variant prototype](../../../prototype/04-lighting-tab-layout/prototype.py)
  (branch `prototype/tartarus-backlight-04-lighting-tab-layout`, not on `dev`) after text
  grilling alone proved insufficient for this layout question. Custom layout's current-colour
  picker + bulk-fill button live in that same strip (not a sidebar); its paint grid only
  renders when Custom layout is selected, paints immediately per click (no Apply step);
  eyedropper/palette reuse explicitly deferred past this ticket. Brightness is one always-
  visible 0–255 slider, commit-on-release. D-Bus: one `SetLighting(a{sv}, y)` call mirroring
  `SetStatusLeds`'s whole-payload shape and `Action`'s tagged-dict encoding; no `GetState()`
  addition, no `rules.py` changes. Copy-from-Profile is client-side only, no new D-Bus method.
- [Write lighting spec](./issues/05-write-lighting-spec.md) — consolidated tickets 01–04 into
  [`spec.md`](./spec.md) (same section list as `tartarus-status-leds/spec.md`), plus
  [ADR-0012](../../docs/adr/0012-lighting-shares-the-led-task-no-varstore-shutdown-clear.md)
  and four new `CONTEXT.md` entries (**Lighting**, **Lighting assignment**, **Fixed effect**,
  **Custom layout**). `.scratch/README.md` flipped to "spec ready." Pure consolidation, no new
  decisions — every ticket on this map is now resolved; implementation is a fresh effort
  (`tartarus-backlight-impl`), not a resumption of this map.

## Not yet specified

<!-- in-scope fog; graduates to tickets as the frontier advances -->

(empty — ticket 04 graduated both remaining fog patches: the per-effect parameter surface and
the Custom-layout painting UX. Nothing left before ticket 05's spec.)

## Out of scope

<!-- ruled beyond this destination; never graduates -->

- **Acheron-driven, remap-aware reactive/per-keypress lighting** ("tier 3") — the dispatch task
  writing a lighting frame per keypress, e.g. reacting to the *remapped* key rather than the
  physical one. Deferred; a fresh effort later if wanted, not a resumption of this map (Q3).
- **Layer-scoped lighting overrides** — ruled out (Q7); Lighting is Profile-only, mirroring
  Status LEDs.
- **A bindable "set Lighting" Action** — Lighting is Profile-driven only, same as Status LEDs; a
  Binding cannot fire a lighting change directly.
- **Automatic or per-application Profile/Lighting switching** — excluded by the definition of
  Profile (never switched automatically).
- **Host-streamed per-frame Custom-layout animation** — this effort covers one static per-key
  colour layout only. ADR-0006 already flagged streamed animation as a distinct future revisit
  of the `led` task's fd lifetime.
- **Custom-layout painter eyedropper and named palette/swatch reuse** — ruled out of ticket 05's
  spec (ticket 04 §3): a bulk-fill button ships, but loading a painted key's colour as the new
  current-colour, or saving/reusing named colours across keys or Profiles, are real scope growth
  (a second interaction mode; a new persisted concept with no config-model home) better judged
  after the plain painter has seen use — a fresh ticket later if wanted, not a resumption of
  this map.
