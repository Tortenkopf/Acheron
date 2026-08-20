"""Device Overview — the GUI's one main view, mirroring the physical
Tartarus Pro layout exactly as built and settled in ticket 09's prototype
(`prototype/09-gui-information-architecture/prototype.py`): a 4x5 grid (row
4 four-wide), the wheel as a column-5 continuation, the thumbstick as a
diamond rotated 90° clockwise, a circular Mode key above it, and key 20 as
a separate paddle below it. Clicking any control opens the shared Binding
editor (`binding_editor.build_binding_editor`) in a popover.

The Profile sidebar (ticket 19) is real: switching a Profile calls
`SwitchProfile`, "+ New Profile" calls `CreateProfile`, and each row's "✎"/
"×" call `RenameProfile`/`DeleteProfile` — "×" is disabled on the active
Profile, mirroring the Daemon's own "can't delete the Profile out from under
itself" rule rather than only surfacing it as a post-hoc error. The tray
mock's "Quick switch" popover lists the same real Profiles and calls
`SwitchProfile` too.

The Base/Held tab row (ticket 18) is real: clicking a tab sets
`ui_state["selected_layer"]`, the Layer whose Bindings Device Overview and
the Action Table sidebar currently show/edit — independent of which Layer
is *live* on the physical device. `app.py` also calls
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

from .action_table import build_action_table
from .binding_editor import action_summary, build_binding_editor
from .daemon_client import DaemonError
from .inputs import GRID_COLS, GRID_ROWS, grid_input, input_label

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


def build_name_prompt_popover(
    client_error_context: str, initial_text: str, submit_label: str, on_submit: Callable[[str], None]
) -> Gtk.Popover:
    """A small Entry-plus-submit-button popover shared by "+ New Profile"
    and each row's rename ("✎") control — the closest existing pattern is
    `build_binding_editor`'s own Save/error handling, reused here at a much
    smaller scale rather than inventing a separate dialog mechanism."""
    popover = Gtk.Popover()
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.set_margin_top(8)
    box.set_margin_bottom(8)
    box.set_margin_start(8)
    box.set_margin_end(8)

    entry = Gtk.Entry(text=initial_text)
    entry.set_width_chars(16)
    box.append(entry)

    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.set_visible(False)
    box.append(error_label)

    def on_submit_clicked(_widget):
        name = entry.get_text().strip()
        if not name:
            error_label.set_label("Name can't be empty")
            error_label.set_visible(True)
            return
        try:
            on_submit(name)
        except DaemonError as exc:
            error_label.set_label(f"{client_error_context}: {exc}")
            error_label.set_visible(True)
            return
        popover.popdown()

    submit_btn = Gtk.Button(label=submit_label)
    submit_btn.add_css_class("suggested-action")
    submit_btn.connect("clicked", on_submit_clicked)
    entry.connect("activate", on_submit_clicked)
    box.append(submit_btn)

    popover.set_child(box)
    return popover


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
    return sidebar


def build_tray_mock(
    client,
    profile: str,
    layer: str,
    profiles: list[str],
    on_change: Callable[[], None],
    status: str | None = None,
) -> Gtk.Box:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.add_css_class("tray-mock")
    box.set_size_request(190, -1)
    box.set_margin_top(12)
    box.set_margin_end(12)
    heading = Gtk.Label(label="Tray icon (simulated)", xalign=0)
    heading.add_css_class("heading")
    box.append(heading)

    # `status` is `None` for callers with nothing to say about it (ticket
    # 09's original prototype reuse, this module's own tests building
    # `build_main_view` directly) — ticket 12/20's status line only appears
    # when a real 3-way status is actually known.
    if status is not None:
        label, colour, glyph = STATUS_STATES[status]
        status_lbl = Gtk.Label()
        status_lbl.set_markup(f"{glyph}  <span foreground='{colour}'>{label}</span>")
        box.append(status_lbl)

    icon_row = Gtk.Box(spacing=6)
    icon_row.append(Gtk.Label(label="\U0001f3ae"))
    icon_row.append(Gtk.Label(label=f"{profile} · {layer}", xalign=0))
    box.append(icon_row)

    menu_btn = Gtk.MenuButton(label="Quick switch ▾")
    popover = Gtk.Popover()
    menu_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.set_visible(False)
    for name in profiles:
        b = Gtk.Button(label=name)
        if name == profile:
            b.add_css_class("suggested-action")

        def on_pick(_b, name=name):
            if name != profile:
                try:
                    client.switch_profile(name)
                except DaemonError as exc:
                    # Same reasoning as the sidebar's own show_error: leave
                    # the popover open with the failure visible rather than
                    # popping down as if the switch had happened.
                    error_label.set_label(str(exc))
                    error_label.set_visible(True)
                    return
            popover.popdown()
            on_change()

        b.connect("clicked", on_pick)
        menu_box.append(b)
    menu_box.append(error_label)
    popover.set_child(menu_box)
    menu_btn.set_popover(popover)
    box.append(menu_btn)

    note = Gtk.Label(label="real tray uses AppIndicator3")
    note.add_css_class("dim")
    note.set_wrap(True)
    box.append(note)
    return box


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
) -> Gtk.MenuButton:
    binding = config["profiles"][profile][layer].get(inp)
    inner = Gtk.Label(label=f"{input_label(inp)}\n{action_summary(binding, inp)}", justify=Gtk.Justification.CENTER)
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
    btn = Gtk.MenuButton()
    btn.set_child(inner)
    btn.set_size_request(w, h)
    # A MenuButton's default halign is FILL: inside a plain Gtk.Box (the Mode
    # key and key-20's paddle, both appended straight to `stick_col` rather
    # than gridded), that stretches it to the box's full cross-width — the
    # diamond's ~160px, not this button's own 52px — live-verified via a
    # real screenshot as the actual cause of the oversized Mode-key oval,
    # not missing wrapping. A Gtk.Grid cell (every other caller) already
    # sizes to its own column, so this is a no-op there.
    btn.set_halign(Gtk.Align.CENTER)
    btn.add_css_class("bound" if binding else "empty")
    btn.set_sensitive(sensitive)
    if not sensitive and insensitive_reason:
        btn.set_tooltip_text(insensitive_reason)
    popover = Gtk.Popover()

    def on_saved():
        popover.popdown()
        on_change()

    editor = build_binding_editor(client, config, profile, layer, inp, on_saved, capture_mode)
    popover.set_child(editor)
    btn.set_popover(popover)
    return btn


def build_main_view(
    client,
    config: dict,
    profile: str,
    layer: str,
    on_change: Callable[[], None],
    ui_state: dict,
    status: str | None = None,
    capture_mode: str = "digital",
) -> Gtk.Widget:
    selected_layer = ui_state.setdefault("selected_layer", "base")
    mode_key_role = config["profiles"][profile]["mode_key_role"]
    mode_key_bindable = mode_key_role == "bound"

    def input_btn(inp: str, w=76, h=99, sensitive=True, insensitive_reason=None) -> Gtk.MenuButton:
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

    main = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    main.set_hexpand(True)

    top_row = Gtk.Box(spacing=12)
    top_row.append(build_layer_bar(client, selected_layer, mode_key_role, on_change, ui_state))
    spacer = Gtk.Box(hexpand=True)
    top_row.append(spacer)
    table_toggle = Gtk.ToggleButton(label="Action Table ◂" if ui_state["table_open"] else "Action Table ▸")
    table_toggle.set_active(ui_state["table_open"])
    top_row.append(table_toggle)
    main.append(top_row)

    device = Gtk.Box(spacing=28)

    # Grid: rows 1-3 are a full 5 columns; row 4 is only 4 wide (16-19). The
    # wheel occupies the same column-5 slot row 4's missing key would sit
    # in (next to 19), continuing straight down for two more rows (scroll
    # up, click, scroll down) — same Gtk.Grid, same button size, so it
    # lines up exactly like a real 5th column rather than a detached panel.
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
    # diamond's top lobe (Left, per the rotation below), and "20" below it,
    # each its own block with breathing room between; the diamond itself
    # stays tight so it still reads as one control.
    stick_col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=18, halign=Gtk.Align.CENTER, valign=Gtk.Align.START)
    # The Mode key's own Binding only matters — and is only editable — while
    # `mode_key_role` is `Bound`; under the default `LayerSwitch` it's
    # intercepted before any Binding lookup ever runs (ticket 18).
    mode_btn = input_btn(
        "mode_key",
        52,
        40,
        sensitive=mode_key_bindable,
        insensitive_reason="Layer-shift Mode key: switch it to Bound above to give it its own Binding",
    )
    mode_btn.add_css_class("mode-key")
    stick_col.append(mode_btn)

    # Diamond rotated 90° clockwise from a plain N/S/E/W layout: the lobe
    # nearest the user's viewing angle when the device sits beside them on
    # the desk (per layout.md) fires Left at top, Down at left, Up at
    # right, Right at bottom — not the naive Up-at-top mapping.
    diamond = Gtk.Grid(row_spacing=2, column_spacing=2)
    diamond.attach(input_btn("thumbstick_left", 52, 40), 1, 0, 1, 1)
    diamond.attach(input_btn("thumbstick_down", 52, 40), 0, 1, 1, 1)
    diamond.attach(input_btn("thumbstick_up", 52, 40), 2, 1, 1, 1)
    diamond.attach(input_btn("thumbstick_right", 52, 40), 1, 2, 1, 1)
    stick_col.append(diamond)

    stick_col.append(input_btn(grid_input(4, 5), 52, 40))
    device.append(stick_col)

    main.append(device)
    root.append(main)

    table_revealer = Gtk.Revealer(transition_type=Gtk.RevealerTransitionType.SLIDE_LEFT)
    table_revealer.set_reveal_child(ui_state["table_open"])
    table_sidebar = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    table_sidebar.add_css_class("sidebar")
    table_sidebar.set_size_request(280, -1)
    table_heading = Gtk.Label(label="Action Table", xalign=0)
    table_heading.add_css_class("heading")
    table_sidebar.append(table_heading)
    table_sidebar.append(build_action_table(client, config, profile, selected_layer, on_change, ui_state))
    table_revealer.set_child(table_sidebar)
    root.append(table_revealer)

    def on_toggle(btn):
        ui_state["table_open"] = btn.get_active()
        table_revealer.set_reveal_child(btn.get_active())
        btn.set_label("Action Table ◂" if btn.get_active() else "Action Table ▸")

    table_toggle.connect("toggled", on_toggle)

    root.append(build_tray_mock(client, profile, layer, list(config["profiles"]), on_change, status))
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
    grid, Action Table, tray mock — all one widget tree, per ticket 09) with
    ticket 12/20's status chip above it and, whenever `status` isn't
    `"running_connected"`, a dimmed `Gtk.Overlay` disabling the whole thing —
    matching prototype/12-daemon-device-status-indicators/prototype.py's
    variant C, per this ticket's "build from the prototype directly, not
    from the prose" instruction rather than redesigning it. That includes
    disabling the tray mock's own Quick-switch control along with
    everything else: `root.set_sensitive(False)` covers the whole subtree
    the prototype already validated this against live.

    Unlike the prototype (which spliced its tray status line into a
    `build_main_view` it didn't own, via `get_last_child()`/
    `get_first_child()` — the only option available there), this ticket owns
    the real `build_main_view`/`build_tray_mock` outright, so `status` is
    passed straight through as a parameter instead: a future reordering of
    `build_main_view`'s own `root.append(...)` calls can't silently break
    the tray status line's placement the way reaching into the built tree
    from outside could.
    """
    root = build_main_view(client, config, profile, layer, on_change, ui_state, status, capture_mode)

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
