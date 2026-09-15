# 05 — Custom-layout painter

**What to build:** A user paints an arbitrary colour onto each of the 20 grid
keys and the scroll wheel individually to build a Custom layout, with a
"Fill all keys" shortcut to set a base colour quickly. The paint grid renders
in the Lighting tab's device area only when Custom layout is the selected
mode — matching the real device's physical layout, with the Mode key and
thumbstick shown for fidelity but never paintable or lit. Every click persists
immediately; there is no working-copy/Apply step.

Source of truth: [`spec.md`](../../tartarus-backlight/spec.md) §"GUI"
(Custom-layout painter), §"The wire frames" (column addressing).

**Blocked by:** 04

**Status:** ready-for-agent

- [ ] The Grid destination's key **geometry** (the `Gtk.Grid` row/col loop plus
      wheel/Mode-key/thumbstick placement) is factored into a shared helper both
      the real Grid button grid and this new paint-button grid call. Button
      **behaviour** stays separate — `make_input_button` is tightly coupled to
      Binding/Chord editing; a paint button just applies the current colour on
      click.
- [ ] Column addressing matches the wire spec exactly: columns `0..18` = grid
      keys `1..19` in order, column `19` = the scroll wheel, column `20` = grid
      key `20`. The Mode key and thumbstick occupy no column and render as
      inert/unpaintable — they are solid black plastic, not RGB-capable.
- [ ] A persistent **current-colour** `Gtk.ColorDialogButton` and a **"Fill all
      keys"** bulk-fill button live in the same horizontal control strip
      ticket 04 built (alongside the mode selector, per-effect params, and
      brightness slider).
- [ ] The paint grid **only renders when Custom layout is selected** — not
      shown, not even dimmed, for any other mode.
- [ ] Clicking a paintable key/wheel cell paints it with the current colour
      **immediately**: reads the active Profile's stored 21-colour array, sets
      the clicked index to the current colour, and calls
      `set_lighting({"type": "custom_layout", "colours": [...]}, brightness)`
      with the full array (current brightness unchanged) — no working-copy/Apply
      step, `Config` stays the sole source of truth.
- [ ] "Fill all keys" sets all 21 entries to the current colour and calls
      `set_lighting(...)` once with the full array.
- [ ] Explicitly **out of scope, do not build**: an eyedropper (loading a
      painted key's colour as the new current-colour) and named palette/swatch
      reuse.
- [ ] When the device is disconnected the grid still renders the stored config
      state, matching every other Device Overview control.
- [ ] `rules.py` gets nothing.
- [ ] Tests via `DaemonStub`: the paint grid only renders in Custom-layout mode;
      clicking a key/wheel cell calls `set_lighting` with only that index
      changed in the full 21-colour array; "Fill all keys" calls `set_lighting`
      with all 21 entries set to the current colour; the Mode key/thumbstick
      cells are not clickable/paintable; the grid still renders the stored
      state when the stub reports the device disconnected; the shared geometry
      helper produces the same column order the Grid destination uses.
