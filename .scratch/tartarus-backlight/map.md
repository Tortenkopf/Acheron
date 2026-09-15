Label: wayfinder:map

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
- **The scroll wheel, not the Mode key, is the 21st lit matrix cell** — corrected during
  charting (the user's own hardware knowledge overrides the initial `device_overview.py`-layout
  inference). The Mode key and thumbstick are shown in the Lighting tab for physical-layout
  fidelity but stay inert/unlit (Q6).

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

## Not yet specified

<!-- in-scope fog; graduates to tickets as the frontier advances -->

- **Exact per-effect parameter surface** — which of colour(s)/speed/direction each Fixed
  effect exposes in the GUI. Graduates via ticket 04, once ticket 01's byte tables exist.
- **`config.toml` schema shape and serde defaulting** for the `LightingAssignment` type.
  Graduates via ticket 03.
- **Matrix column-addressing order** — whether the Custom-layout frame's 21 cells run
  grid-keys-then-wheel or some interleaved order. Graduates via tickets 01/02.
- **Custom-layout painting UX details** — bulk-fill, palette reuse, eyedropper, and whether the
  Grid destination's physical-layout button component can be reused as-is or needs a paint-mode
  variant. Graduates via ticket 04.

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
