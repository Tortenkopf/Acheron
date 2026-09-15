# 04 — Lighting tab: mode selector, Fixed-effect params, brightness

**What to build:** A user configures the active Profile's Lighting assignment from
a new **Lighting** tab — a third arm of the Device Overview destination switch,
alongside Grid and Library. A flat mode selector lists Off plus the six Fixed
effects plus a Custom-layout entry; picking Off or any Fixed effect commits
immediately and drives the real backlight. Per-effect parameter controls
(colour, speed, direction, Breath/Starlight style) appear inline for whichever
mode is selected. A brightness slider is always visible and commits on
drag-end, independent of mode. Selecting "Custom layout" switches the mode but
paints nothing yet — the paint grid itself is the next ticket.

Source of truth: [`spec.md`](../../tartarus-backlight/spec.md) §"GUI" (mode
selector, per-effect params, brightness, tab layout), per the
[prototype](../../../prototype/04-lighting-tab-layout/prototype.py) on branch
`prototype/tartarus-backlight-04-lighting-tab-layout`.

**Blocked by:** 03

**Status:** ready-for-agent

- [ ] A new "Lighting" arm of `device_overview.py`'s `build_destination_switch`,
      keeping the Profile sidebar exactly as Grid does (not Library's
      Steppers/Macros sidebar swap).
- [ ] One horizontal control strip sits **above** the device area: mode selector
      + per-effect params + brightness, all in one row (not a vertical stack —
      the spec's prototype found a vertical stack made the strip change height
      per mode). Window gets a **1100px minimum width**, not auto-fit.
- [ ] Mode selector: one flat 8-entry list — Off, Static, Spectrum, Reactive,
      Wave, Breath, Starlight, Custom layout — reading from and rendering the
      active Profile's stored `lighting` type. Selecting Off or any Fixed-effect
      entry commits immediately via `client.set_lighting(...)` (current
      brightness unchanged), same as every other immediate-write control on this
      panel — no Save/Apply button on the tab. Selecting "Custom layout" updates
      the selector's own state and commits `{"type": "custom_layout", "colours":
      [...]}\ ` using whatever colours are already stored (or 21 black entries
      for a Profile that has none yet); the device area for this ticket shows a
      placeholder when Custom layout is selected (the real paint grid lands in
      the next ticket).
- [ ] Per-effect parameter controls, all `Gtk.ColorDialogButton` for colour
      fields:
  - Static: colour.
  - Spectrum: no controls.
  - Reactive: colour + speed 1–4 (`Gtk.SpinButton`).
  - Wave: Left/Right toggle pair (`WaveDirection`).
  - Breath: style Random/Single/Dual (`Gtk.ToggleButton` group) + 0/1/2 colour
    pickers as the style requires.
  - Starlight: same style group + speed 1–3 (`Gtk.SpinButton`) + 0/1/2 colour
    pickers.
  - Any param change calls `set_lighting(...)` immediately with the full
    updated `FixedEffect` payload (current brightness unchanged).
- [ ] Brightness: one `Gtk.Scale`, 0–255 raw byte, no percentage mapping.
      **Always visible** regardless of the selected mode. **Commits on
      drag-end only** (`on_drag_end`, matching the actuation-point depth
      marker's own commit pattern) via `set_lighting(...)` with the current
      assignment unchanged and the new brightness.
- [ ] Copy-from-Profile affordance: a control to pick another Profile and copy
      its whole Lighting assignment (`lighting` + `brightness`) onto the active
      one — reads the other Profile's stored values out of the already-loaded
      config dict (from `GetConfig`) and calls `set_lighting(...)` with them;
      no new D-Bus method.
- [ ] When the device is disconnected the tab still shows the stored config
      state, matching every other Device Overview control.
- [ ] `rules.py` gets nothing.
- [ ] Tests via `DaemonStub`: the mode selector renders from the stub's
      active-Profile `lighting`; selecting a mode calls `set_lighting` with the
      full assignment; each per-effect param control calls `set_lighting` with
      the updated payload; brightness commits only on drag-end; a newly created
      Profile shows Off at brightness 0; Copy-from-Profile reads another
      Profile's stored values and calls `set_lighting` with them; the tab still
      renders the stored state when the stub reports the device disconnected.
