"""Device Overview — the GUI's one main view, mirroring the physical
Tartarus Pro layout exactly as built and settled in ticket 09's prototype
(`prototype/09-gui-information-architecture/prototype.py`): a 4x5 grid (row
4 four-wide), the wheel as a column-5 continuation, the thumbstick as a
diamond rotated 90° clockwise, a circular Mode key above it, and key 20 as
a separate paddle below it. Clicking any control opens the shared Binding
editor (`binding_editor.build_binding_editor`) in a popover.

Ticket 48 replaced the old permanent grid+Action-Table-sidebar layout with
a Grid/Library destination switcher (`build_destination_switch`): the
Profile sidebar stays exactly as it was (ticket 47's round-2 fix pins it
at a stable width via `set_hexpand(False)`), the Action Table is cut
outright (superseded by ticket 42's inline key/mouse-button picker), and
selecting "Library" fully replaces the content area with the real Library
screen (`library_view.build_library_view`, ticket 52 — a Steppers/Macros
tab-switched panel pair; the Macros panel is real, the Steppers panel
stays a stub pending ticket 55). The Grid destination keeps the real
`build_layer_bar` above the grid, plus an always-visible reserved slot
beside it (`build_chords_placeholder`) for ticket 40's real Chords list.

The Profile sidebar (ticket 19) is real: switching a Profile calls
`SwitchProfile`, "+ New Profile" calls `CreateProfile`, and each row's "✎"/
"×" call `RenameProfile`/`DeleteProfile` — "×" is disabled on the active
Profile, mirroring the Daemon's own "can't delete the Profile out from under
itself" rule rather than only surfacing it as a post-hoc error. The real
system tray icon (ticket 36, `tray.TrayIcon`) has its own equivalent Switch
Profile submenu, listing the same real Profiles and calling `SwitchProfile`
too — it lives outside this module entirely, not as a widget built here.

The Base/Held tab row (ticket 18) is real: clicking a tab sets
`ui_state["selected_layer"]`, the Layer whose Bindings Device Overview
currently shows/edits — independent of which Layer is *live* on the
physical device. `app.py` also calls
`client.subscribe_layer_changed` and updates the same `ui_state` key on
every push, so the tab auto-follows a real Mode-key hold/release too — "wired
to real `ActiveLayerChanged` state and to editing each Layer's Bindings
independently," per the ticket. A `mode_key_role` toggle sits alongside the
tabs; the Mode key's own device button only opens its Binding editor once
`Bound` is selected there — while `LayerSwitch` (the default) it's always
intercepted before any Binding lookup, so editing it would be pointless.
"""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk, Pango

from .binding_editor import action_summary, build_binding_editor
from .daemon_client import DaemonError
from .gtk_utils import build_name_prompt_popover
from .inputs import GRID_COLS, GRID_ROWS, grid_input, input_label
from .library_view import build_library_view

# Ticket 12/20 — Daemon/device status surface. Mirrors
# prototype/12-daemon-device-status-indicators/prototype.py's STATUS_STATES
# exactly: (label, colour, tray glyph) per reachable 3-way state — this
# ticket wires it to the real Daemon instead of that prototype's StatusStub.
# `device_connected` is meaningless while the Daemon isn't running, so this
# is one 3-way state, not two independent booleans.
STATUS_STATES = {
    "running_connected": ("Connected", "#4caf50", "\U0001f3ae"),
    "running_disconnected": ("Daemon running — device disconnected", "#ff9800", "\U0001f50c"),
    "not_running": ("Daemon not running", "#f44336", "\U0001f480"),
}

_OVERLAY_MESSAGES = {
    "not_running": "Daemon not running — start it to edit Bindings",
    "running_disconnected": "Device disconnected — plug in the Tartarus Pro to edit Bindings",
}

# Rendered instead of a real GetConfig() while the Daemon has never
# successfully answered one yet (e.g. the GUI launched before the Daemon
# finished starting) — same shape as issue 11's seed Config. Purely inert
# placeholder data: build_status_wrapped_view never shows this config
# set_sensitive(True), since status is never "running_connected" while it's
# what's being rendered.
PLACEHOLDER_CONFIG = {
    "schema_version": 1,
    "active_profile": "Default",
    "profiles": {"Default": {"base": {}, "held": {}, "mode_key_role": "layer_switch"}},
}


def compute_status(daemon_running: bool, device_connected: bool) -> str:
    """The one 3-way status ticket 12 settled on: `device_connected` is
    meaningless while the Daemon isn't running, so this collapses the two
    booleans down to the three reachable states rather than treating them
    as independent."""
    if not daemon_running:
        return "not_running"
    if not device_connected:
        return "running_disconnected"
    return "running_connected"


def build_layer_bar(
    client, selected_layer: str, mode_key_role: str, on_change: Callable[[], None], ui_state: dict
) -> Gtk.Box:
    box = Gtk.Box(spacing=6)
    for layer_key, label in (("base", "Base"), ("held", "Held")):
        btn = Gtk.Button(label=label)
        if layer_key == selected_layer:
            btn.add_css_class("suggested-action")

        def on_clicked(_b, layer_key=layer_key):
            ui_state["selected_layer"] = layer_key
            on_change()

        btn.connect("clicked", on_clicked)
        box.append(btn)

    role_btn = Gtk.ToggleButton(
        label="Mode key: Bound" if mode_key_role == "bound" else "Mode key: Layer-shift"
    )
    role_btn.set_active(mode_key_role == "bound")
    role_btn.set_tooltip_text(
        "Bound: the Mode key fires its own Binding like any other Input.\n"
        "Layer-shift: holding it activates the Held Layer instead."
    )

    def on_role_toggled(b):
        try:
            client.set_mode_key_role("bound" if b.get_active() else "layer_switch")
        except DaemonError:
            # Mirror build_binding_editor's Save/Clear: a failed mutation
            # must not leave the widget showing a state the Daemon never
            # actually applied. `Gtk.ToggleButton` has already flipped
            # `get_active()` by the time "toggled" fires, so revert it —
            # blocking this handler first, or `set_active` here would
            # re-emit "toggled" and recurse.
            role_btn.handler_block(role_handler_id)
            b.set_active(not b.get_active())
            role_btn.handler_unblock(role_handler_id)
            return
        on_change()

    role_handler_id = role_btn.connect("toggled", on_role_toggled)
    box.append(role_btn)
    return box


def build_profile_sidebar(client, config: dict, profile: str, on_change: Callable[[], None]) -> Gtk.Box:
    sidebar = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    sidebar.add_css_class("sidebar")
    sidebar.set_size_request(150, -1)
    heading = Gtk.Label(label="Profiles", xalign=0)
    heading.add_css_class("heading")
    sidebar.append(heading)

    # A plain sidebar Button has no popover to host build_name_prompt_popover's
    # own inline error_label, so this shared one covers Switch/Delete the same
    # way — matching build_binding_editor's show_error convention rather than
    # swallowing a failed mutation silently (e.g. a stale row surviving a
    # concurrent client's delete: the click still visibly does something).
    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.add_css_class("sidebar-error")
    error_label.set_visible(False)
    sidebar.append(error_label)

    def show_error(exc: Exception) -> None:
        error_label.set_label(str(exc))
        error_label.set_visible(True)

    for name in config["profiles"]:
        row = Gtk.Box(spacing=4)

        switch_btn = Gtk.Button(label=name, hexpand=True)
        if name == profile:
            switch_btn.add_css_class("suggested-action")

        def on_switch_clicked(_b, name=name):
            if name == profile:
                return
            try:
                client.switch_profile(name)
            except DaemonError as exc:
                show_error(exc)
                return
            on_change()

        switch_btn.connect("clicked", on_switch_clicked)
        row.append(switch_btn)

        rename_btn = Gtk.MenuButton(label="✎")
        rename_btn.set_tooltip_text(f"Rename {name!r}")

        def on_rename_submitted(new_name: str, name=name):
            client.rename_profile(name, new_name)
            on_change()

        rename_btn.set_popover(
            build_name_prompt_popover(f"Renaming {name!r}", name, "Rename", on_rename_submitted)
        )
        row.append(rename_btn)

        delete_btn = Gtk.Button(label="×")
        is_active = name == profile
        delete_btn.set_sensitive(not is_active)
        delete_btn.set_tooltip_text(
            "Can't delete the active Profile — switch away from it first"
            if is_active
            else f"Delete {name!r}"
        )

        def on_delete_clicked(_b, name=name):
            try:
                client.delete_profile(name)
            except DaemonError as exc:
                show_error(exc)
                return
            on_change()

        delete_btn.connect("clicked", on_delete_clicked)
        row.append(delete_btn)

        sidebar.append(row)

    new_btn = Gtk.MenuButton(label="+ New Profile")

    def on_create_submitted(name: str):
        client.create_profile(name)
        on_change()

    new_btn.set_popover(build_name_prompt_popover("Creating a Profile", "", "Create", on_create_submitted))
    sidebar.append(new_btn)

    # Ticket 47's round-2 fix, caught live: a plain Gtk.Box with no explicit
    # hexpand still computes one via GTK4's expand-propagation — it infers
    # from a row's `switch_btn` (`hexpand=True`, three container levels
    # down) that the sidebar itself wants to expand, so it competes for
    # whatever horizontal slack the *other* destination's content leaves
    # unclaimed in `build_main_view` (Library claims less than the grid,
    # so the sidebar visibly widened only there). Pin it explicitly so it
    # stays exactly its own `set_size_request(150, -1)` width regardless of
    # which destination is showing — measured at a stable 197px in both.
    sidebar.set_hexpand(False)
    return sidebar


def make_input_button(
    client,
    config: dict,
    profile: str,
    layer: str,
    inp: str,
    on_change: Callable[[], None],
    w=76,
    h=99,
    sensitive: bool = True,
    insensitive_reason: str | None = None,
    capture_mode: str = "digital",
) -> Gtk.Button:
    binding = config["profiles"][profile][layer].get(inp)
    inner = Gtk.Label(
        label=f"{input_label(inp)}\n{action_summary(binding, inp, config.get('macros', {}))}",
        justify=Gtk.Justification.CENTER,
    )
    inner.set_wrap(True)
    # Plain `wrap=True` alone only breaks at whitespace (`Pango.WrapMode.WORD`,
    # the Gtk.Label default). Every summary string does have at least one
    # legal break (the space before "[trigger]"/"(default)"), so ordinary
    # content ("passthrough (Q)", "Ctrl+A  [1x]") already wraps cleanly on
    # that alone — WORD_CHAR is only the fallback for the one run that has
    # none: a multi-modifier chord like "Ctrl+Shift+Alt+Super+F12" is one
    # unbroken token, live-verified to force the button (and its Grid cell)
    # to ~300px wide under plain WORD wrap instead of wrapping at all. Not
    # paired with `max-width-chars`: that caps the label's *natural* width
    # request globally, which sounded right for the chord case but live
    # verification (a real screenshot) showed it also forces ordinary
    # "passthrough" into an ugly mid-word "passthr/ough" split, since 7-8
    # chars isn't enough room for an 11-letter word. Leaving natural width
    # uncapped means ordinary content wraps at its own word boundaries as
    # before, and only the rare no-space chord run falls back to a character
    # split inside a button that's already `w`-wide — the same "floor, not
    # ceiling" tradeoff already accepted for `h` below, not a new one.
    inner.set_wrap_mode(Pango.WrapMode.WORD_CHAR)
    btn = Gtk.Button()
    btn.set_child(inner)
    btn.set_size_request(w, h)
    # A MenuButton's default halign is FILL: inside a plain Gtk.Box (the Mode
    # key and key-20's paddle, both appended straight to `stick_col` rather
    # than gridded), that stretches it to the box's full cross-width — the
    # diamond's ~160px, not this button's own 52px — live-verified via a
    # real screenshot as the actual cause of the oversized Mode-key oval,
    # not missing wrapping. A Gtk.Grid cell (every other caller) already
    # sizes to its own column, so this is a no-op there. (Still relevant
    # now that `btn` is a plain Gtk.Button, not a Gtk.MenuButton — both
    # default to halign FILL.)
    btn.set_halign(Gtk.Align.CENTER)
    btn.add_css_class("bound" if binding else "empty")
    btn.set_sensitive(sensitive)
    if not sensitive and insensitive_reason:
        btn.set_tooltip_text(insensitive_reason)

    # Ticket 44 (live-verified on real hardware): a real top-level Gtk.Window
    # instead of a Gtk.Popover anchored to `btn`. The Binding editor's
    # content — now including ticket 44's always-inline key/mouse-button
    # picker — is tall enough that GTK4/Wayland's Popover positioning has no
    # valid place to put it for nearly every Device Overview grid button,
    # even with the main window maximized: a Popover is constrained to its
    # own toplevel's local bounds, not the full screen, and this content
    # routinely needs more room than a grid button's surrounding space
    # provides within that toplevel. A real Window is placed by the window
    # manager across the whole screen instead, sidestepping the constraint
    # entirely. Built once (like the old popover) and shown/hidden via
    # present()/close() rather than recreated per click — which needs
    # `set_hide_on_close`: unlike Gtk.Popover.popdown(), Gtk.Window.close()
    # *destroys* the window by default. Live-verified as a real bug: without
    # this, the second open of the same key re-presented an already-
    # destroyed window ("A window is shown after it has been destroyed" per
    # GTK's own warning), corrupting its content and eventually hanging the
    # whole app after a few open/close cycles.
    window = Gtk.Window(modal=True, title=f"{profile} / {layer} / {input_label(inp)}")
    window.set_hide_on_close(True)

    def on_saved():
        window.close()
        on_change()

    editor = build_binding_editor(client, config, profile, layer, inp, on_saved, capture_mode)
    window.set_child(editor)

    def on_click(_b):
        window.set_transient_for(btn.get_root())
        window.present()

    btn.connect("clicked", on_click)
    # Exposed for tests, which need to reach the editor's content without
    # actually presenting a real top-level window in a headless run (unlike
    # the old Gtk.Popover, there's no `btn.get_popover()`-style GTK API for
    # "the window this button opens").
    btn.binding_editor_window = window
    return btn


def build_destination_switch(selected_dest: str, on_select: Callable[[str], None]) -> Gtk.Box:
    """Ticket 47's round-2 (variant D) winner: a plain-text, icon-free
    "Grid"/"Library" switcher sitting where the old Action-Table toggle
    used to, fully replacing `build_main_view`'s content area on
    selection. `on_select` is expected to write the pick into `ui_state`
    and call `on_change()` — this widget carries no state of its own,
    matching `build_layer_bar`'s own selected-tab pattern."""
    row = Gtk.Box(spacing=6)
    for dest_key, label in (("grid", "Grid"), ("library", "Library")):
        btn = Gtk.Button(label=label)
        if dest_key == selected_dest:
            btn.add_css_class("suggested-action")

        def on_clicked(_b, dest_key=dest_key):
            on_select(dest_key)

        btn.connect("clicked", on_clicked)
        row.append(btn)
    return row


def build_chords_placeholder() -> Gtk.Widget:
    """Reserved layout space beside the grid for the Chords list — always
    visible while the Grid destination is selected (no toggle, per ticket
    47's round-2: nothing should rescale on open/close). Ticket 40 wires
    the real Chord-recording content in here; this ticket only needs the
    slot itself to be real and testable."""
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    box.add_css_class("sidebar")
    box.set_size_request(220, -1)
    heading = Gtk.Label(label="Chords", xalign=0)
    heading.add_css_class("heading")
    box.append(heading)
    placeholder = Gtk.Label(
        label="Ticket 40 wires the real Chord-recording UX into this slot.",
        xalign=0,
        wrap=True,
    )
    placeholder.add_css_class("dim")
    box.append(placeholder)
    return box


def build_main_view(
    client,
    config: dict,
    profile: str,
    layer: str,
    on_change: Callable[[], None],
    ui_state: dict,
    capture_mode: str = "digital",
) -> Gtk.Widget:
    selected_layer = ui_state.setdefault("selected_layer", "base")
    dest = ui_state.setdefault("dest", "grid")
    mode_key_role = config["profiles"][profile]["mode_key_role"]
    mode_key_bindable = mode_key_role == "bound"

    def input_btn(inp: str, w=76, h=99, sensitive=True, insensitive_reason=None) -> Gtk.Button:
        return make_input_button(
            client,
            config,
            profile,
            selected_layer,
            inp,
            on_change,
            w,
            h,
            sensitive,
            insensitive_reason,
            capture_mode,
        )

    root = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=16)
    root.set_margin_top(12)
    root.set_margin_bottom(12)
    root.set_margin_start(12)
    root.set_margin_end(12)

    root.append(build_profile_sidebar(client, config, profile, on_change))

    right = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    right.set_hexpand(True)

    def on_dest_select(dest_key: str) -> None:
        ui_state["dest"] = dest_key
        on_change()

    right.append(build_destination_switch(dest, on_dest_select))
    # Round 2's own fix: a light separator between the switcher and
    # whatever it's switching, so the destination-level chrome reads as
    # visually distinct from the per-destination content below it.
    right.append(Gtk.Separator())

    if dest == "library":
        right.append(build_library_view(client, config, ui_state, on_change))
    else:
        main = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
        main.set_hexpand(True)

        main.append(build_layer_bar(client, selected_layer, mode_key_role, on_change, ui_state))

        device_row = Gtk.Box(spacing=16)
        # Round 2's own fix for round 1's grid crowding the Base/Held/Mode-key
        # row directly above it — a top margin here, distinct from `main`'s
        # own inter-row spacing, gives the grid itself room to breathe.
        device_row.set_margin_top(8)

        device = Gtk.Box(spacing=28)

        # Grid: rows 1-3 are a full 5 columns; row 4 is only 4 wide (16-19).
        # The wheel occupies the same column-5 slot row 4's missing key would
        # sit in (next to 19), continuing straight down for two more rows
        # (scroll up, click, scroll down) — same Gtk.Grid, same button size,
        # so it lines up exactly like a real 5th column rather than a
        # detached panel.
        grid = Gtk.Grid(row_spacing=4, column_spacing=4)
        for r in range(1, GRID_ROWS + 1):
            cols = GRID_COLS if r < GRID_ROWS else GRID_COLS - 1
            for c in range(1, cols + 1):
                grid.attach(input_btn(grid_input(r, c)), c - 1, r - 1, 1, 1)
        wheel_col_index = GRID_COLS - 1
        grid.attach(input_btn("wheel_scroll_up"), wheel_col_index, GRID_ROWS - 1, 1, 1)
        grid.attach(input_btn("wheel_middle"), wheel_col_index, GRID_ROWS, 1, 1)
        grid.attach(input_btn("wheel_scroll_down"), wheel_col_index, GRID_ROWS + 1, 1, 1)
        device.append(grid)

        # Thumbstick, further right — the Mode key sits directly above the
        # diamond's top lobe (Left, per the rotation below), and "20" below
        # it, each its own block with breathing room between; the diamond
        # itself stays tight so it still reads as one control.
        stick_col = Gtk.Box(
            orientation=Gtk.Orientation.VERTICAL, spacing=18, halign=Gtk.Align.CENTER, valign=Gtk.Align.START
        )
        # The Mode key's own Binding only matters — and is only editable —
        # while `mode_key_role` is `Bound`; under the default `LayerSwitch`
        # it's intercepted before any Binding lookup ever runs (ticket 18).
        mode_btn = input_btn(
            "mode_key",
            52,
            40,
            sensitive=mode_key_bindable,
            insensitive_reason="Layer-shift Mode key: switch it to Bound above to give it its own Binding",
        )
        mode_btn.add_css_class("mode-key")
        stick_col.append(mode_btn)

        # Diamond rotated 90° clockwise from a plain N/S/E/W layout: the
        # lobe nearest the user's viewing angle when the device sits beside
        # them on the desk (per layout.md) fires Left at top, Down at left,
        # Up at right, Right at bottom — not the naive Up-at-top mapping.
        diamond = Gtk.Grid(row_spacing=2, column_spacing=2)
        diamond.attach(input_btn("thumbstick_left", 52, 40), 1, 0, 1, 1)
        diamond.attach(input_btn("thumbstick_down", 52, 40), 0, 1, 1, 1)
        diamond.attach(input_btn("thumbstick_up", 52, 40), 2, 1, 1, 1)
        diamond.attach(input_btn("thumbstick_right", 52, 40), 1, 2, 1, 1)
        stick_col.append(diamond)

        stick_col.append(input_btn(grid_input(4, 5), 52, 40))
        device.append(stick_col)

        device_row.append(device)
        # Always visible while Grid is selected, no toggle (ticket 47's
        # round 2: nothing should rescale on open/close) — reserved layout
        # space for ticket 40's real Chord-recording UX.
        device_row.append(build_chords_placeholder())
        main.append(device_row)
        right.append(main)

    root.append(right)
    return root


def build_status_badge(status: str) -> Gtk.Box:
    label, colour, _glyph = STATUS_STATES[status]
    box = Gtk.Box(spacing=6)
    box.add_css_class("status-badge")
    lbl = Gtk.Label()
    lbl.set_markup(f"<span foreground='{colour}'>●</span> {label}")
    box.append(lbl)
    box.set_margin_top(6)
    box.set_margin_bottom(2)
    box.set_margin_start(12)
    return box


def build_status_wrapped_view(
    client,
    config: dict,
    profile: str,
    layer: str,
    status: str,
    on_change: Callable[[], None],
    ui_state: dict,
    capture_mode: str = "digital",
) -> Gtk.Widget:
    """Wraps `build_main_view`'s whole Device Overview (profile sidebar,
    Grid/Library destination — per tickets 09/48) with ticket 12/20's status
    chip above it and, whenever `status` isn't `"running_connected"`, a
    dimmed `Gtk.Overlay` disabling the whole thing — matching
    prototype/12-daemon-device-status-indicators/prototype.py's variant C,
    per this ticket's "build from the prototype directly, not from the
    prose" instruction rather than redesigning it. `root.set_sensitive(False)`
    covers the whole subtree the prototype already validated this against
    live.

    Ticket 36 removed the old in-window `build_tray_mock` placeholder
    outright — the real tray icon (`tray.TrayIcon`) is a standalone D-Bus
    service outside this widget tree entirely, kept in sync by `app.py`'s
    own `rebuild()` calling `TrayIcon.update(config, profile, status)`
    alongside this function, not by anything reaching into what's built
    here.
    """
    root = build_main_view(client, config, profile, layer, on_change, ui_state, capture_mode)

    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    outer.append(build_status_badge(status))

    healthy = status == "running_connected"
    root.set_sensitive(healthy)
    if healthy:
        outer.append(root)
        return outer

    overlay = Gtk.Overlay()
    overlay.set_child(root)
    dim = Gtk.Box(halign=Gtk.Align.FILL, valign=Gtk.Align.FILL)
    dim.add_css_class("dim-overlay")
    msg = Gtk.Label(label=_OVERLAY_MESSAGES[status])
    msg.add_css_class("dim-overlay-label")
    # halign/valign alone only position a widget *within space it has
    # claimed* — without hexpand/vexpand it claims just its own natural
    # size, so the box packs it at the start instead of centering it (the
    # same pitfall prototype/12's own docstring already caught).
    msg.set_hexpand(True)
    msg.set_vexpand(True)
    msg.set_halign(Gtk.Align.CENTER)
    msg.set_valign(Gtk.Align.CENTER)
    dim.append(msg)
    overlay.add_overlay(dim)
    outer.append(overlay)
    return outer
