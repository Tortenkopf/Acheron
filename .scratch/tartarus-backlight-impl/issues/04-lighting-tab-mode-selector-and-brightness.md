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

**Status:** done

- [x] A new "Lighting" arm of `device_overview.py`'s `build_destination_switch`,
      keeping the Profile sidebar exactly as Grid does (not Library's
      Steppers/Macros sidebar swap).
- [x] One horizontal control strip sits **above** the device area: mode selector
      + per-effect params + brightness, all in one row (not a vertical stack —
      the spec's prototype found a vertical stack made the strip change height
      per mode). Window gets a **1100px minimum width**, not auto-fit.
- [x] Mode selector: one flat 8-entry list — Off, Static, Spectrum, Reactive,
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
- [x] Per-effect parameter controls, all `Gtk.ColorDialogButton` for colour
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
- [x] Brightness: one `Gtk.Scale`, 0–255 raw byte, no percentage mapping.
      **Always visible** regardless of the selected mode. **Commits on
      drag-end only** (`on_drag_end`, matching the actuation-point depth
      marker's own commit pattern) via `set_lighting(...)` with the current
      assignment unchanged and the new brightness.
- [x] Copy-from-Profile affordance: a control to pick another Profile and copy
      its whole Lighting assignment (`lighting` + `brightness`) onto the active
      one — reads the other Profile's stored values out of the already-loaded
      config dict (from `GetConfig`) and calls `set_lighting(...)` with them;
      no new D-Bus method.
- [x] When the device is disconnected the tab still shows the stored config
      state, matching every other Device Overview control.
- [x] `rules.py` gets nothing.
- [x] Tests via `DaemonStub`: the mode selector renders from the stub's
      active-Profile `lighting`; selecting a mode calls `set_lighting` with the
      full assignment; each per-effect param control calls `set_lighting` with
      the updated payload; brightness commits only on drag-end; a newly created
      Profile shows Off at brightness 0; Copy-from-Profile reads another
      Profile's stored values and calls `set_lighting` with them; the tab still
      renders the stored state when the stub reports the device disconnected.

## Comments

Implemented in `gui/acheron_gui/device_overview.py`: `build_destination_switch`
gains the "Lighting" arm; `build_main_view` routes `dest == "lighting"` to a new
`build_lighting_content`, falling into the same `else` branch as Grid for the
Profile sidebar. New builders: `build_lighting_mode_selector`,
`build_lighting_params`, `build_lighting_brightness`,
`build_lighting_copy_from_profile`, `build_lighting_device_area`, plus small
helpers (`_lighting_mode`, `_default_fixed_effect`, `_default_style`,
`_custom_layout_colours`, `_lighting_colour_button`, `_toggle_button_row`,
`_lighting_speed_spin`, `_style_colour_rows`, `_commit_lighting`).
`gui/acheron_gui/app.py` gets `win.set_size_request(1100, -1)` alongside the
existing `set_default_size`.

One deliberate deviation from the ticket's literal widget call-out: the mode
selector, the Wave Left/Right pair, and the Breath/Starlight style group are
all plain `Gtk.Button` rows (one shared `_toggle_button_row` helper) with a
`"suggested-action"` CSS class marking the active entry and a reclick-is-a-
no-op guard, not `Gtk.ToggleButton`. This matches every other exclusive
selector already in this file (`build_destination_switch`, `build_layer_bar`,
`build_profile_sidebar`) rather than introducing a second selector idiom for
just these three spots. `Gtk.ColorDialogButton`/`Gtk.SpinButton` are used
exactly as specced for colour/speed fields.

Forked code review (high effort, ~87k tokens) caught two real issues, both
fixed before this ticket's commit:

1. The brightness slider's drag-end commit used a `Gtk.GestureClick`
   attached directly to the `Gtk.Scale`; its own internal `Gtk.Range` drag
   gesture claims the pointer sequence first, so the added gesture's
   "released" never fires (a documented GTK4 issue,
   `JuliaGtk/Gtk4.jl#77`). Fixed at the time by moving the `GestureClick` to
   the Scale's parent row with `PropagationPhase.CAPTURE` — a workaround
   confirmed against that same upstream report. **For the record, not this
   ticket's own work:** a later live-daemon check (commit `1cdaa74`, after
   this ticket landed) found that capture-phase workaround still didn't
   reliably fire against a real mouse drag either, and replaced it with a
   `GLib.timeout_add` debounce off `value-changed` instead — that debounce
   is what ships today.
2. `_lighting_style_toggle_row`/`_lighting_wave_direction_row` and the mode
   selector's own inline loop were three copies of the same segmented-button
   pattern; collapsed into the one `_toggle_button_row` helper referenced
   above.

Tests: added 18 cases to `gui/tests/test_device_overview.py` covering every
item in this ticket's own test list. `gui/.venv/bin/pytest gui/tests`: 593
passed, up from 575 at the start of this ticket. No daemon changes in this
ticket (GUI-only, per "What to build"), so no `cargo test` run.
