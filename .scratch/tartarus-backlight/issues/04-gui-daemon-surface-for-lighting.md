Type: grilling
Blocked by: 03
Status: resolved (Charon, 2026-09-15)

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

**The settled GUI/D-Bus surface for Lighting.** Grilled against ticket 01's byte tables and the
real `gui/acheron_gui/` code (HITL, Charon, 2026-09-15), plus a throwaway prototype for the tab
layout — see [prototype/04-lighting-tab-layout/prototype.py on branch
`prototype/tartarus-backlight-04-lighting-tab-layout`](../../../prototype/04-lighting-tab-layout/prototype.py)
(not on `dev`; the layout decision below is the primary source now, the prototype code is kept
only as a reference). Decisions only — no build. Feeds
[ticket 05](./05-write-lighting-spec.md), now unblocked.

### 0. Two corrections to ticket 03's config type

Ticket 01's wire research gives `Reactive` a real speed byte (`0x01–0x04`) and `Starlight` a
real speed byte (`0x01–0x03`), but ticket 03's `FixedEffect` enum typed neither — an omission,
not a deliberate "fixed default." Both variants gain `speed: u8`:

```rust
Reactive { colour: Colour, speed: u8 },   // 1-4
Starlight { style: BreathStyle, speed: u8 },   // 1-3
```

`Wave { direction: Direction }` also gets corrected: `daemon/src/input.rs`'s existing
`Direction` (`Up`/`Down`/`Left`/`Right`, for axis input) is the wrong type to reuse — Wave only
ever sends 1 of 2 values (`WAVE_DIRS = (1, 2)`; driver headers confirm `RIGHT = 0x01`, `LEFT =
0x02`), and reusing a 4-variant type would make `Up`/`Down` representable-but-invalid. A fresh
2-variant `WaveDirection { Left, Right }` replaces it:

```rust
Wave { direction: WaveDirection },
```

### 1. Mode selector — one flat list, not nested

`LightingAssignment` is `Off | FixedEffect{effect} | CustomLayout{colours}`, but the GUI
presents it as **one flat 8-entry selector**: Off, Static, Spectrum, Reactive, Wave, Breath,
Starlight, Custom layout. The `FixedEffect` nesting is a type-level concern only — picking
"Off" and picking "Static" are equally one click. Selecting any entry commits immediately via
`set_lighting(...)` (§6), same as every other immediate-write control on this panel — no
Save/Apply button anywhere on the tab.

### 2. Per-effect parameter controls

- **Static:** colour (`Gtk.ColorDialogButton`).
- **Spectrum:** no controls — cycles autonomously.
- **Reactive:** colour + speed 1–4 (`Gtk.SpinButton`).
- **Wave:** Left/Right (`Gtk.ToggleButton` pair, `WaveDirection` per §0).
- **Breath:** style Random/Single/Dual (`Gtk.ToggleButton` group) + 0/1/2 colour pickers as the
  style requires.
- **Starlight:** same style group + speed 1–3 (`Gtk.SpinButton`) + 0/1/2 colour pickers.

`Gtk.ColorDialogButton` (GTK4's native colour picker, confirmed available) for every colour
field — genuinely arbitrary RGB, unlike Status LEDs' 3 fixed hues, so no bespoke swatch widget.

### 3. Custom-layout painter

- **Component reuse:** the Grid destination's key **geometry** (the `Gtk.Grid` row/col loop +
  wheel/Mode-key/thumbstick placement, currently inlined in `build_main_view`) gets factored
  into a shared helper both the real `make_input_button` grid and a new paint-button grid call
  — but button **behaviour** stays separate. `make_input_button` is tightly coupled to
  Binding/Chord editing (config lookups, a binding-editor popover); a paint button just applies
  the current colour on click. No shared button widget, only shared placement logic.
- **Paint interaction:** a persistent **current-colour** `Gtk.ColorDialogButton` plus a
  **"Fill all keys"** bulk-fill button, both living in the same horizontal control strip as
  every other mode's params (§5) — clicking a key on the grid paints it with the current colour
  **immediately** (`set_lighting(...)` fires per click, full 21-colour array each time, no
  working-copy/Apply step — matches Status LEDs' click→persist→drive pattern exactly; `Config`
  stays the sole source of truth per ticket 03 §4).
- **Scope:** bulk-fill only. **Eyedropper** (click a painted key to load its colour as the new
  current-colour) and **named palette/swatch reuse** are explicitly deferred — real scope
  growth (a second interaction mode; a new persisted concept with no config-model home) better
  decided after the basic painter has been used. Not fog on this map (the destination doesn't
  change) — just out of *this ticket's* build, flagged for a future ticket if wanted.
- The physical grid **only renders when Custom layout is selected** (§5) — not shown, not even
  dimmed, for any other mode.

### 4. Brightness

One `Gtk.Scale`, **0–255 raw byte** — no percentage mapping, unlike OpenRazer's own daemon.
Acheron's `brightness: u8` (ticket 03) is already the raw wire value, and no other Acheron field
does a cosmetic 0–100 remap, so brightness doesn't get a one-off exception. **Always visible**
regardless of the selected mode (harmless no-op while Off — the matrix stays dark regardless),
avoiding mode-dependent show/hide logic for a field that structurally always exists on
`Profile`. **Commits on drag-end only**, not per `value-changed` tick — matches the actuation-
point depth marker's own `on_drag_end` commit pattern (`binding_editor.py:268-271,338-340`);
firing a full `HIDIOCSFEATURE` write (a 21-colour payload in Custom-layout mode) on every pixel
of slider movement would be wasteful for no benefit.

### 5. Tab layout — settled via prototype, not text

Grilling text alone wasn't enough to pin this down — building
[the 3-variant prototype](../../../prototype/04-lighting-tab-layout/prototype.py) and looking at
a real running window is what actually settled it (this project's established pattern for GUI
layout questions; see `prototype/04-dual-stage-binding-editor-layout`,
`prototype/09-gui-information-architecture`). Two rounds against the running prototype:

1. **Placement (settled first, no prototype needed):** third arm of `build_destination_switch`
   ("Lighting"), keeps the Profile sidebar as Grid does.
2. **Round 1 — variant A wins:** one horizontal control strip (mode selector + per-effect
   params + brightness) sits **above** the device area; the device area shows the paint grid
   only for Custom layout (§3), a placeholder otherwise. Variant A beat "persistent 220px
   sidebar" (B) and "two-pane with canvas toolbar" (C).
3. **Round 2 — one correction to variant A:** the Custom-layout current-colour picker
   + Fill-all-keys button also live **in the top strip** (as part of §2/§3's per-effect params
   area), not in a sidebar reusing the Chords 220px slot as first assumed mid-grilling — that
   slot is **unused** for Lighting entirely.
4. **Round 3 — one more correction:** the per-effect params panel is **horizontal**, not
   vertical. Vertically stacked fields (Reactive/Breath/Starlight/Custom all have 2+ fields)
   made the whole top strip change height per mode — visually unstable. Horizontal keeps every
   mode's panel one row tall, so the strip's height is constant regardless of selected mode; the
   window gets a **1100px minimum width** (not auto-fit) so it doesn't need to grow when
   switching to the busiest mode (Starlight: style toggle + speed + 2 colour pickers).

### 6. D-Bus surface — one whole-assignment call, tagged-dict encoding

**`SetLighting(assignment: a{sv}, brightness: y)`**, mirroring `SetStatusLeds`'s whole-payload
shape (one call, no partial-update bookkeeping — `Config` stays authoritative, ticket 03 §4) and
`action_to_dict`'s existing tagged-dict convention for sum types (`dbus/wire.rs:262`, a `"type"`
key plus per-variant fields) — the same pattern already used for `Action`, now extended to
`LightingAssignment`/`FixedEffect`. `Colour` rides as an `(yyy)` byte-triple nested inside that
dict, the first RGB value ever encoded on this project's D-Bus surface (Status LEDs are fixed
hues, no colour value existed before).

- **No new signal** — `GetConfig` already carries everything; no live device-state divergence
  to report (VARSTORE persists effect selection device-side, §5 of ticket 03; same "no
  `GetState()` addition" conclusion the Status LEDs' own ticket 04 reached, §3 there).
- **`rules.py`: nothing to mirror.** No cross-field validation beyond what each widget's own
  range already enforces (speed/brightness bounds, RGB from the colour dialog) — same
  conclusion as Status LEDs' ticket 04 §6.

### 7. Copy-from-Profile — client-side, no new D-Bus method

The GUI already holds every Profile's `lighting`/`brightness` via `GetConfig`. "Copy from
Profile X" reads Profile X's stored values out of the already-loaded config dict and calls
`set_lighting(...)` with them against the active Profile — exactly the write `SetLighting`
already does. A dedicated `CopyLighting(source_profile)` method would just be `SetLighting`
wrapped in a lookup, protecting no invariant a client-side copy doesn't.

### 8. Domain — deferred to ticket 05, same lazy discipline as ticket 03

No `CONTEXT.md` edits from this ticket. `WaveDirection` and the corrected `FixedEffect` fields
(§0) are wire-level/type facts, not new glossary terms — nothing here needs a term beyond what
ticket 03 already deferred.

