# 04 — Status LEDs lozenge group in the Device Overview

**What to build:** A user configures a Profile's Status LED assignment from the GUI
by clicking three colour lozenges near the device picture — as direct as editing a
Binding. The lozenges show the **active** Profile's stored state (lit or dark),
edit it on click, and keep showing it even while the device is disconnected. The
group means "which Profile", never "which Layer" — it looks identical on Base and
Held and does not move or disappear when the layer bar flips.

Source of truth: [`spec.md`](../../tartarus-status-leds/spec.md) §"GUI", per the
user's mockup
[`screenshots/Status LED location Mockup.png`](../../tartarus-status-leds/screenshots/Status%20LED%20location%20Mockup.png).

**Blocked by:** 03

**Status:** ready-for-agent

- [ ] A new "Status LEDs" group in `build_main_view`'s `device_row`, **between the
      thumbstick column and the Chords section** — a heading over three
      **vertically-stacked** colour lozenges: orange (top), green (middle), blue
      (bottom), roughly mirroring the physical LEDs' left-side placement. Uniform,
      aligned widgets (the mockup is a rough sketch).
- [ ] Each lozenge is a click-to-toggle widget. **Lit** = full-saturation colour
      fill + a visible border/glow; **unlit** = heavily desaturated + flat. The
      contrast must be *strong* — it is the primary state signal, not a faint
      brightness shift. Drive it with a `.status-led` CSS class plus a per-colour
      class.
- [ ] **No visible per-lozenge text.** Each lozenge gets a tooltip
      (`"Orange status LED — on"` / `"… — off"`) and an accessible name/description
      (`set_tooltip_text` + `Gtk.Accessible` properties) so a colour-blind user is
      not relying on hue + brightness alone. Group heading stays `Status LEDs`.
- [ ] State always comes from `config["profiles"][profile]["status_leds"]` for the
      **active** Profile — never a live hardware read. On a newly created Profile
      all three show dark (`status_leds` defaults all-`false`; "never set" is
      byte-identical to "explicitly all-off" — no special-casing).
- [ ] When the device is disconnected the lozenges still show the stored config
      state, matching every other Device Overview control.
- [ ] Visibility: **Grid destination only** (alongside Chords); shown
      **identically on both Base and Held** — Profile-scoped, not Layer-scoped,
      renders from `status_leds` regardless of `selected_layer`. Not rendered in
      the Library destination.
- [ ] Edit flow: a lozenge's toggle handler reads **all three** current lozenge
      states and calls `client.set_status_leds(orange, green, blue)`; the group
      then rebuilds from config on the shared `on_change`, like everything else on
      the panel.
- [ ] `rules.py` gets nothing.
- [ ] Tests via `DaemonStub`: the three lozenges render lit/unlit from the stub's
      active-Profile `status_leds`; clicking one calls `set_status_leds` with the
      full triple; the group rebuilds from config on `on_change`; a newly created
      Profile shows all three dark; the group renders identically on Base and Held
      and only on the Grid destination; it still renders the stored state when the
      stub reports the device disconnected.
