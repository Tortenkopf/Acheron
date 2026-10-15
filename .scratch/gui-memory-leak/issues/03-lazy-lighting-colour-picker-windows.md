# 03: Build Lighting colour-picker windows on click and destroy them on close

Spec: [`../spec.md`](../spec.md). Implementation Decisions, "Same treatment for the Lighting
colour picker".

**What to build:** The Lighting tab's colour-swatch buttons (per-effect colours, Breath's style
colours, the Custom-layout paint colour) build their "Pick a colour" window when clicked and
destroy it when it closes, so it is no longer pre-built and kept hidden. Cancel discards the
pick, and Select commits it and updates the swatch, exactly as today.

**Blocked by:** 01.

**Status:** done (2026-10-15)

- [x] Building the Lighting tab creates no top-level windows.
- [x] Clicking a swatch opens the picker, pre-set to the swatch's current colour.
- [x] Select commits the colour (same `on_commit` payload) and refreshes the swatch. Cancel
      commits nothing. Both destroy the window, and the accessor returns `None` afterwards.
- [x] Soak test: N rebuilds with `ui_state` set to the Lighting tab leave the top-level window
      count at 0.

## Comments

- 2026-10-15: Done. `_build_colour_picker_window` builds the picker per click; Cancel, Select and
  the window's own close destroy it, and a rebuild with a picker open destroys it via the shared
  `_destroy_editor_window_on_leaving_root`. The get-or-build-then-present step is shared with the
  Binding editor as `_present_editor_window`. One small visible delta: closing with the WM close
  button used to hide the picker and keep an uncommitted pick for the next open; now the next open
  starts from the swatch's committed colour (nothing was committed either way). Soak coverage is
  top-level window count only; widget-liveness/wrapper-count soak is ticket 05's.
