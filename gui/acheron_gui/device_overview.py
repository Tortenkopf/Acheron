"""Device Overview — the GUI's one main view, mirroring the physical
Tartarus Pro layout exactly as built and settled in ticket 09's prototype
(`prototype/09-gui-information-architecture/prototype.py`): a 4x5 grid (row
4 four-wide), the wheel as a column-5 continuation, the thumbstick as a
diamond rotated 90° clockwise, a circular Mode key above it, and key 20 as
a separate paddle below it. Clicking any control opens the shared Binding
editor (`binding_editor.build_binding_editor`) in a popover.

Profile switching and the Held Layer tab are rendered (matching the
prototype's structure) but deliberately disabled here: `SetBinding`/
`ClearBinding` (ticket 15) only ever act on the Daemon's current active
Profile's Base Layer — there is no `SwitchProfile` or Layer parameter on
the wire yet (tickets 19/18) for these controls to actually drive, and
letting them *look* interactive without being backed by anything real
would silently mis-target edits.
"""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk

from .action_table import build_action_table
from .binding_editor import action_summary, build_binding_editor
from .inputs import GRID_COLS, GRID_ROWS, grid_input, input_label


def build_layer_bar() -> Gtk.Box:
    box = Gtk.Box(spacing=6)
    base_btn = Gtk.Button(label="Base")
    base_btn.add_css_class("suggested-action")
    base_btn.set_sensitive(False)
    base_btn.set_tooltip_text("Base is the only Layer the Daemon supports so far")
    box.append(base_btn)

    held_btn = Gtk.Button(label="Held")
    held_btn.set_sensitive(False)
    held_btn.set_tooltip_text("Held Layer support lands in a later ticket")
    box.append(held_btn)
    return box


def build_tray_mock(profile: str, layer: str, profiles: list[str]) -> Gtk.Box:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.add_css_class("tray-mock")
    box.set_size_request(190, -1)
    box.set_margin_top(12)
    box.set_margin_end(12)
    heading = Gtk.Label(label="Tray icon (simulated)", xalign=0)
    heading.add_css_class("heading")
    box.append(heading)

    icon_row = Gtk.Box(spacing=6)
    icon_row.append(Gtk.Label(label="\U0001f3ae"))
    icon_row.append(Gtk.Label(label=f"{profile} · {layer}", xalign=0))
    box.append(icon_row)

    menu_btn = Gtk.MenuButton(label="Quick switch ▾")
    popover = Gtk.Popover()
    menu_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
    for name in profiles:
        b = Gtk.Button(label=name)
        b.set_sensitive(False)
        b.set_tooltip_text("Profile switching lands in a later ticket")
        menu_box.append(b)
    popover.set_child(menu_box)
    menu_btn.set_popover(popover)
    box.append(menu_btn)

    note = Gtk.Label(label="real tray uses AppIndicator3")
    note.add_css_class("dim")
    note.set_wrap(True)
    box.append(note)
    return box


def make_input_button(client, config: dict, profile: str, inp: str, on_change: Callable[[], None], w=76, h=52) -> Gtk.MenuButton:
    binding = config["profiles"][profile]["base"].get(inp)
    inner = Gtk.Label(label=f"{input_label(inp)}\n{action_summary(binding)}", justify=Gtk.Justification.CENTER)
    inner.set_wrap(True)
    btn = Gtk.MenuButton()
    btn.set_child(inner)
    btn.set_size_request(w, h)
    btn.add_css_class("bound" if binding else "empty")
    popover = Gtk.Popover()

    def on_saved():
        popover.popdown()
        on_change()

    editor = build_binding_editor(client, config, profile, inp, on_saved)
    popover.set_child(editor)
    btn.set_popover(popover)
    return btn


def build_main_view(client, config: dict, profile: str, layer: str, on_change: Callable[[], None], ui_state: dict) -> Gtk.Widget:
    root = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=16)
    root.set_margin_top(12)
    root.set_margin_bottom(12)
    root.set_margin_start(12)
    root.set_margin_end(12)

    sidebar = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    sidebar.add_css_class("sidebar")
    sidebar.set_size_request(150, -1)
    heading = Gtk.Label(label="Profiles", xalign=0)
    heading.add_css_class("heading")
    sidebar.append(heading)
    for name in config["profiles"]:
        b = Gtk.Button(label=name)
        if name == profile:
            b.add_css_class("suggested-action")
        b.set_sensitive(False)
        b.set_tooltip_text("Profile switching lands in a later ticket")
        sidebar.append(b)
    new_btn = Gtk.Button(label="+ New Profile")
    new_btn.set_sensitive(False)
    new_btn.set_tooltip_text("Creating Profiles lands in a later ticket")
    sidebar.append(new_btn)
    root.append(sidebar)

    main = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    main.set_hexpand(True)

    top_row = Gtk.Box(spacing=12)
    top_row.append(build_layer_bar())
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
            grid.attach(make_input_button(client, config, profile, grid_input(r, c), on_change), c - 1, r - 1, 1, 1)
    wheel_col_index = GRID_COLS - 1
    grid.attach(make_input_button(client, config, profile, "wheel_scroll_up", on_change), wheel_col_index, GRID_ROWS - 1, 1, 1)
    grid.attach(make_input_button(client, config, profile, "wheel_middle", on_change), wheel_col_index, GRID_ROWS, 1, 1)
    grid.attach(make_input_button(client, config, profile, "wheel_scroll_down", on_change), wheel_col_index, GRID_ROWS + 1, 1, 1)
    device.append(grid)

    # Thumbstick, further right — the Mode key sits directly above the
    # diamond's top lobe (Left, per the rotation below), and "20" below it,
    # each its own block with breathing room between; the diamond itself
    # stays tight so it still reads as one control.
    stick_col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=18, halign=Gtk.Align.CENTER, valign=Gtk.Align.START)
    mode_btn = make_input_button(client, config, profile, "mode_key", on_change, 52, 40)
    mode_btn.add_css_class("mode-key")
    stick_col.append(mode_btn)

    # Diamond rotated 90° clockwise from a plain N/S/E/W layout: the lobe
    # nearest the user's viewing angle when the device sits beside them on
    # the desk (per layout.md) fires Left at top, Down at left, Up at
    # right, Right at bottom — not the naive Up-at-top mapping.
    diamond = Gtk.Grid(row_spacing=2, column_spacing=2)
    diamond.attach(make_input_button(client, config, profile, "thumbstick_left", on_change, 52, 40), 1, 0, 1, 1)
    diamond.attach(make_input_button(client, config, profile, "thumbstick_down", on_change, 52, 40), 0, 1, 1, 1)
    diamond.attach(make_input_button(client, config, profile, "thumbstick_up", on_change, 52, 40), 2, 1, 1, 1)
    diamond.attach(make_input_button(client, config, profile, "thumbstick_right", on_change, 52, 40), 1, 2, 1, 1)
    stick_col.append(diamond)

    stick_col.append(make_input_button(client, config, profile, grid_input(4, 5), on_change, 52, 40))
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
    table_sidebar.append(build_action_table(client, config, profile, on_change, ui_state))
    table_revealer.set_child(table_sidebar)
    root.append(table_revealer)

    def on_toggle(btn):
        ui_state["table_open"] = btn.get_active()
        table_revealer.set_reveal_child(btn.get_active())
        btn.set_label("Action Table ◂" if btn.get_active() else "Action Table ▸")

    table_toggle.connect("toggled", on_toggle)

    root.append(build_tray_mock(profile, layer, list(config["profiles"])))
    return root
