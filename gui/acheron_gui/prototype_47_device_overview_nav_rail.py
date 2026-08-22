"""PROTOTYPE — throwaway, answers ticket 47 (Prototype the Device Overview
nav-rail restructuring):
.scratch/tartarus-input-expansion/issues/47-prototype-device-overview-nav-rail.md

Plan: three structurally different nav-rail restructurings of Device
Overview's window chrome, switchable via a floating bottom pill (same GTK4
adaptation of the skill's `?variant=` convention as tickets 19/30/31/32/38 —
Prev/Next buttons plus Left/Right arrow keys). Unlike those prototypes, this
one mounts *real* production widgets wherever the real thing already exists
— `device_overview.build_profile_sidebar`/`make_input_button`,
`action_table.build_action_table` — driven by the real `DaemonStub`
(`daemon_stub.DaemonStub`, the same fake backend the GUI's own test suite
uses), seeded with a few real Bindings so Action Table rows and grid buttons
have actual content to judge width against, not lorem-ipsum placeholders.
Ticket 31's winning Library screen (`build_variant_b`, copied onto this
branch from `prototype/31-stepper-macro-library-ux`) is reused verbatim —
it's already a validated answer to "what does the Library look like", this
ticket only asks where it's reached from. The Chords list is the one piece
mocked fresh (`build_chords_list_panel`): ticket 30 already settled its own
internal shape ({members} -> {action summary} rows, Edit + click-preview),
so this only needs enough fidelity to test the actual open question —
whether the rail entry hosting it can still reach and highlight the real
grid.

Wipe me: nothing here persists past process exit (DaemonStub is in-memory);
none of it should be promoted as-is. Each variant's own fold-in note says
what to actually carry into `device_overview.py`.

Run:
    python3 gui/prototype_47_device_overview_nav_rail.py

Variants:
    A — Exclusive rail: a narrow icon-only rail always visible on the far
        left; selecting any of the four destinations (Grid / Action Table /
        Library / Chords) fully replaces the content area, testing the
        user's own "full swap" instinct literally everywhere, including
        Chords — which brings its own copy of the real grid along inside
        its destination (grid + Chords list side by side) so it never loses
        reachability even though it's an exclusive view. Profile switching
        lives in a slim horizontal strip pinned above the rail+content,
        independent of which destination is selected.
    B — Coexisting rail: an icon+label rail with only three destinations —
        Grid, Library, Chords — Action Table is *not* promoted to a rail
        entry at all; it stays a toggle-beside-the-grid pane (like today,
        just wider), and Chords is demoted the same way: a second
        toggle-beside-the-grid pane rather than a fourth destination, so the
        grid is *never* out of view except when Library — the one
        genuinely standalone, non-grid-shaped surface — is open. The
        Profile list is folded into the rail itself, docked above the four
        destination buttons in the same column.
    C — Collapsible rail: the rail starts icon-only and expands to
        icon+label on a pin toggle (testing "collapsible" as its own
        structural question, not just a style choice). Profile switching is
        a full-width horizontal tab strip across the very top of the
        window, on an axis orthogonal to the vertical rail (visible under
        every destination uniformly). All four destinations are exclusive,
        full-replace, matching Variant A's swap answer — but Chords solves
        reachability differently: a real grid at a shrunk (but still real,
        still-clickable) scale sits pinned above the Chords list inside
        that one destination, rather than Variant A's full-size two-pane
        layout.

Round 2 (A/B/C all rejected outright — "none of these feel right", but
sharp enough about *why* to specify the fix directly rather than needing
another 3-way spread):

    D — A/B/C's rail concept is dropped entirely. The Profile sidebar stays
        exactly as it is in the live GUI today (`build_profile_sidebar`,
        permanent, un-folded). Action Table is cut outright — the user's
        own real-world verdict, from actually using the software, is that
        they never needed it once the real key/mouse-button picker (ticket
        42) landed inline in the per-key editor; with that picker present
        Action Table's expanded rows just read as crowded, not useful.
        "Grid" / "Library" — plain text, no glyphs — become a two-button
        switcher sitting where B's Action-Table/Chords toggle row used to
        be (above the grid, inline in the content area, not a separate rail
        column). The Chords toggle moves to the exact live-GUI slot the old
        Action Table toggle occupied — top-right of `top_row`, beside the
        real `build_layer_bar` — since Chord recording is grid-native
        (ticket 30) and only ever needs to be reachable while the Grid
        destination is showing; it and its revealed list disappear
        entirely when Library is selected, rather than persisting
        somewhere it has nothing to point at.
"""

from __future__ import annotations

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk

from .action_table import build_action_table
from .daemon_stub import DaemonStub
from .device_overview import build_layer_bar, build_profile_sidebar, make_input_button
from .gtk_utils import clear_children
from .inputs import GRID_COLS, GRID_ROWS, grid_input
from .prototype_31_stepper_macro_library_ux import build_variant_b as build_library_panel
from .prototype_31_stepper_macro_library_ux import make_seed_state as make_library_seed_state

# Real CSS (app.py) plus this prototype's own rail/highlight/switcher chrome.
CSS = """
.heading { font-weight: bold; }
.dim { opacity: 0.6; font-size: smaller; }
.sidebar { padding: 8px; background-color: alpha(currentColor, 0.06); border-radius: 6px; }
.bound { border: 2px solid #4caf50; }
.empty { opacity: 0.75; }
.mode-key { border-radius: 999px; }
.error { color: #e53935; font-size: smaller; }
.editor-pane { padding: 10px; background-color: alpha(currentColor, 0.04); border-radius: 6px; }
.badge { border-radius: 999px; padding: 0px 6px; font-size: smaller; font-weight: bold; }
.badge-stepper { background-color: #4a90e2; color: white; }
.badge-macro { background-color: #8e44ad; color: white; }
.toast { background-color: alpha(#e6991a, 0.18); border-radius: 6px; padding: 4px 8px; font-size: smaller; }

/* Ticket 47's own chrome */
.nav-rail { padding: 6px; background-color: alpha(currentColor, 0.05); border-radius: 6px; }
.nav-rail-btn { padding: 8px; }
.nav-rail-btn-active { background-color: alpha(#4a90e2, 0.25); font-weight: bold; }
.profile-strip { padding: 6px; background-color: alpha(currentColor, 0.05); border-radius: 6px; }
.profile-strip-btn-active { font-weight: bold; }
.device-btn-chord-highlight { border: 2px solid #e6991a; }
.switcher-pill { background-color: alpha(currentColor, 0.08); border: 1px solid alpha(currentColor, 0.25); border-radius: 999px; padding: 6px 14px; }
.variant-label { font-weight: bold; }
.action-table-pane { padding: 8px; background-color: alpha(currentColor, 0.03); border-radius: 6px; }
"""


# --- Shared real content, seeded once so every variant judges the same
# material (this is the whole point of using real widgets: a multi-modifier
# Keypress chord and a 4-step Macro are exactly the cases that made the real
# Action Table/grid buttons feel cramped in the first place, per the map's
# own ticket 06/46 history — lorem-ipsum content would hide that). ---


def seed_daemon() -> DaemonStub:
    client = DaemonStub()
    client.set_binding(
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "keypress", "key": "KEY_F12", "modifiers": ["ctrl", "shift", "alt"]},
    )
    client.set_binding(
        "grid_r1c2",
        "base",
        {
            "trigger": "hold_to_repeat",
            "type": "macro",
            "steps": [
                {"type": "key_down", "key": "KEY_W"},
                {"type": "delay_ms", "ms": 40},
                {"type": "key_up", "key": "KEY_W"},
                {"type": "key_down", "key": "KEY_LEFTSHIFT"},
            ],
        },
    )
    client.set_binding("grid_r1c3", "base", {"trigger": "fire_once", "type": "keypress", "key": "BTN_LEFT"})
    client.set_binding("grid_r2c1", "base", {"trigger": "toggle", "type": "keypress", "key": "KEY_CAPSLOCK"})
    return client


def seed_chords() -> list[dict]:
    """Mock only — ticket 30 already settled the Chords list's own row shape
    (`{members} -> {action summary}` + Edit + click-preview); this ticket
    only needs enough of it to test rail placement and grid reachability."""
    return [
        {"members": ["thumbstick_up", "thumbstick_right"], "summary": "Dodge-roll NE  [1x]"},
        {"members": ["grid_r4c1", "grid_r4c2"], "summary": "→ Combat profile"},
        {"members": ["grid_r1c4", "grid_r1c5", "grid_r2c5"], "summary": "Macro (3 steps)  [1x]"},
    ]


# --- Shared real primitive: the grid, mirroring build_main_view's own
# assembly (device_overview.py) minus the Profile sidebar/Action Table
# revealer/tray mock this ticket is restructuring — reusing the *real*
# make_input_button so every grid button still opens the real per-key
# Binding editor Window, not a simplified stand-in. ---


def build_device_grid(
    client, config: dict, profile: str, layer: str, on_change, w: int = 76, h: int = 99
) -> tuple[Gtk.Widget, dict[str, Gtk.Button]]:
    buttons: dict[str, Gtk.Button] = {}

    def input_btn(inp: str, bw=w, bh=h) -> Gtk.Button:
        btn = make_input_button(client, config, profile, layer, inp, on_change, bw, bh)
        buttons[inp] = btn
        return btn

    device = Gtk.Box(spacing=max(10, w // 3))
    grid = Gtk.Grid(row_spacing=4, column_spacing=4)
    for r in range(1, GRID_ROWS + 1):
        cols = GRID_COLS if r < GRID_ROWS else GRID_COLS - 1
        for c in range(1, cols + 1):
            grid.attach(input_btn(grid_input(r, c)), c - 1, r - 1, 1, 1)
    wheel_col = GRID_COLS - 1
    grid.attach(input_btn("wheel_scroll_up"), wheel_col, GRID_ROWS - 1, 1, 1)
    grid.attach(input_btn("wheel_middle"), wheel_col, GRID_ROWS, 1, 1)
    grid.attach(input_btn("wheel_scroll_down"), wheel_col, GRID_ROWS + 1, 1, 1)
    device.append(grid)

    stick_col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=max(8, w // 5), halign=Gtk.Align.CENTER, valign=Gtk.Align.START)
    mode_btn = input_btn("mode_key", max(30, w * 2 // 3), max(24, h * 2 // 5))
    mode_btn.add_css_class("mode-key")
    stick_col.append(mode_btn)
    diamond = Gtk.Grid(row_spacing=2, column_spacing=2)
    dw, dh = max(30, w * 2 // 3), max(24, h * 2 // 5)
    diamond.attach(input_btn("thumbstick_left", dw, dh), 1, 0, 1, 1)
    diamond.attach(input_btn("thumbstick_down", dw, dh), 0, 1, 1, 1)
    diamond.attach(input_btn("thumbstick_up", dw, dh), 2, 1, 1, 1)
    diamond.attach(input_btn("thumbstick_right", dw, dh), 1, 2, 1, 1)
    stick_col.append(diamond)
    stick_col.append(input_btn(grid_input(4, 5), dw, dh))
    device.append(stick_col)

    return device, buttons


# --- Shared mock: the Chords list. Row click highlights the real grid
# buttons it's given (proving reachability); Edit is a stub — actually
# opening the Chord's own Binding editor is ticket 30's already-settled
# design, not this ticket's question. ---


def build_chords_list_panel(chords: list[dict], grid_buttons: dict[str, Gtk.Button], on_toast) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    box.append(Gtk.Label(label="Chords", xalign=0, css_classes=["heading"]))
    box.append(
        Gtk.Label(
            label="Click a row to preview its members on the grid; Edit opens the Chord's Binding dialog.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )
    listbox = Gtk.ListBox(css_classes=["boxed-list"])
    highlighted: list[str] = []

    def clear_highlight():
        for inp in highlighted:
            btn = grid_buttons.get(inp)
            if btn is not None:
                btn.remove_css_class("device-btn-chord-highlight")
        highlighted.clear()

    def on_row_selected(lb, row):
        clear_highlight()
        if row is None:
            return
        chord = chords[row.get_index()]
        for inp in chord["members"]:
            btn = grid_buttons.get(inp)
            if btn is not None:
                btn.add_css_class("device-btn-chord-highlight")
                highlighted.append(inp)

    listbox.connect("row-selected", on_row_selected)
    for chord in chords:
        row = Gtk.Box(spacing=6)
        members = " + ".join(chord["members"])
        row.append(Gtk.Label(label=f"{members} → {chord['summary']}", hexpand=True, xalign=0, wrap=True))
        edit_btn = Gtk.Button(label="Edit")
        edit_btn.connect("clicked", lambda b: on_toast("Edit opens the Chord's Binding dialog (ticket 30, not this ticket)."))
        row.append(edit_btn)
        listbox.append(row)
    box.append(listbox)
    return box


def build_toast(outer: Gtk.Box) -> tuple[Gtk.Label, callable]:
    toast_label = Gtk.Label(css_classes=["toast"], xalign=0, wrap=True)
    toast_label.set_visible(False)
    outer.append(toast_label)

    def show(msg: str) -> None:
        toast_label.set_label(msg)
        toast_label.set_visible(True)
        GLib.timeout_add(3000, lambda: (toast_label.set_visible(False), False)[1])

    return toast_label, show


# --- Variant A: exclusive rail, icon-only, profile strip on top, Chords
# brings its own grid copy along inside its own destination. ---


def build_variant_a(client, ui_state: dict) -> Gtk.Widget:
    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    _, show_toast = build_toast(outer)

    config = client.get_config()
    profile = config["active_profile"]
    layer = "base"

    def on_change():
        rerender()

    profile_strip = Gtk.Box(spacing=4, css_classes=["profile-strip"])
    for name in config["profiles"]:
        btn = Gtk.Button(label=name)
        if name == profile:
            btn.add_css_class("profile-strip-btn-active")
        profile_strip.append(btn)
    profile_strip.append(Gtk.Button(label="+ New"))
    outer.append(profile_strip)

    body = Gtk.Box(spacing=0)
    outer.append(body)

    rail = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4, css_classes=["nav-rail"])
    rail.set_size_request(64, -1)
    body.append(rail)

    content = Gtk.Box(hexpand=True, vexpand=True)
    content.set_margin_start(12)
    body.append(content)

    destinations = [("grid", "\U0001f3ae"), ("table", "\U0001f4cb"), ("library", "\U0001f4da"), ("chords", "⛓")]
    state = {"dest": "grid"}

    def render_content():
        clear_children(content)
        fresh_config = client.get_config()
        if state["dest"] == "grid":
            widget, _buttons = build_device_grid(client, fresh_config, profile, layer, on_change)
            content.append(widget)
        elif state["dest"] == "table":
            ui_state.setdefault("expanded_rows", set())
            table = build_action_table(client, fresh_config, profile, layer, on_change, ui_state)
            table.set_hexpand(True)
            content.append(table)
        elif state["dest"] == "library":
            content.append(build_library_panel(ui_state.setdefault("library_state", make_library_seed_state()), show_toast))
        else:
            pane = Gtk.Box(spacing=16)
            grid_widget, grid_buttons = build_device_grid(client, fresh_config, profile, layer, on_change)
            pane.append(grid_widget)
            pane.append(build_chords_list_panel(seed_chords(), grid_buttons, show_toast))
            content.append(pane)

    def render_rail():
        clear_children(rail)
        for key, glyph in destinations:
            btn = Gtk.Button(label=glyph, css_classes=["nav-rail-btn"])
            if key == state["dest"]:
                btn.add_css_class("nav-rail-btn-active")
            btn.set_tooltip_text(key.capitalize())

            def on_click(b, key=key):
                state["dest"] = key
                render_rail()
                render_content()

            btn.connect("clicked", on_click)
            rail.append(btn)

    def rerender():
        render_rail()
        render_content()

    rerender()
    return outer


# --- Variant B: coexisting rail, icon+label, only Grid/Library/Chords are
# rail destinations — Action Table and the Chords list are both
# toggle-beside-the-grid panes, never fully hiding it. Profile list folded
# into the rail column itself. ---


def build_variant_b(client, ui_state: dict) -> Gtk.Widget:
    outer = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=0)
    toast_holder = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    _, show_toast = build_toast(toast_holder)

    config = client.get_config()
    profile = config["active_profile"]
    layer = "base"

    def on_change():
        rerender()

    rail = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["nav-rail"])
    rail.set_size_request(170, -1)
    rail.append(build_profile_sidebar(client, config, profile, on_change))
    rail.append(Gtk.Separator())

    destinations = [("grid", "\U0001f3ae Grid"), ("library", "\U0001f4da Library")]
    state = {"dest": "grid", "table_open": ui_state.get("table_open", False), "chords_open": ui_state.get("chords_open", False)}

    outer_vbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6, hexpand=True, vexpand=True)
    outer_vbox.append(toast_holder)
    content = Gtk.Box(spacing=12, hexpand=True, vexpand=True)
    content.set_margin_start(12)
    outer_vbox.append(content)

    def render_content():
        clear_children(content)
        fresh_config = client.get_config()
        if state["dest"] == "library":
            content.append(build_library_panel(ui_state.setdefault("library_state", make_library_seed_state()), show_toast))
            return
        grid_widget, grid_buttons = build_device_grid(client, fresh_config, profile, layer, on_change)
        main_col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        toggle_row = Gtk.Box(spacing=6)
        table_toggle = Gtk.ToggleButton(label="Action Table")
        table_toggle.set_active(state["table_open"])
        chords_toggle = Gtk.ToggleButton(label="Chords")
        chords_toggle.set_active(state["chords_open"])
        toggle_row.append(table_toggle)
        toggle_row.append(chords_toggle)
        main_col.append(toggle_row)
        main_col.append(grid_widget)
        content.append(main_col)

        if state["table_open"]:
            table = build_action_table(client, fresh_config, profile, layer, on_change, ui_state)
            table_pane = Gtk.Box(css_classes=["action-table-pane"])
            table_pane.set_size_request(420, -1)
            table_pane.append(table)
            content.append(table_pane)
        if state["chords_open"]:
            chords_pane = Gtk.Box(css_classes=["action-table-pane"])
            chords_pane.set_size_request(320, -1)
            chords_pane.append(build_chords_list_panel(seed_chords(), grid_buttons, show_toast))
            content.append(chords_pane)

        def on_table_toggled(b):
            state["table_open"] = b.get_active()
            render_content()

        def on_chords_toggled(b):
            state["chords_open"] = b.get_active()
            render_content()

        table_toggle.connect("toggled", on_table_toggled)
        chords_toggle.connect("toggled", on_chords_toggled)

    def render_rail():
        for child in list(rail)[2:]:
            rail.remove(child)
        for key, label in destinations:
            btn = Gtk.Button(label=label, css_classes=["nav-rail-btn"])
            if key == state["dest"]:
                btn.add_css_class("nav-rail-btn-active")

            def on_click(b, key=key):
                state["dest"] = key
                render_rail()
                render_content()

            btn.connect("clicked", on_click)
            rail.append(btn)

    def rerender():
        render_rail()
        render_content()

    outer.append(rail)
    outer.append(outer_vbox)
    rerender()
    return outer


# --- Variant C: collapsible rail (icon-only, pins to icon+label), a
# full-width horizontal Profile tab strip on an axis orthogonal to the
# rail, all four destinations exclusive/full-replace like Variant A —
# Chords solves reachability with a shrunk-but-real grid pinned above its
# list, rather than Variant A's full-size two-pane layout. ---


def build_variant_c(client, ui_state: dict) -> Gtk.Widget:
    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    _, show_toast = build_toast(outer)

    config = client.get_config()
    profile = config["active_profile"]
    layer = "base"

    def on_change():
        rerender()

    tab_strip = Gtk.Box(spacing=2, css_classes=["profile-strip"])
    for name in config["profiles"]:
        btn = Gtk.Button(label=name, hexpand=True)
        if name == profile:
            btn.add_css_class("profile-strip-btn-active")
        tab_strip.append(btn)
    outer.append(tab_strip)

    body = Gtk.Box(spacing=0)
    outer.append(body)

    pin_state = {"expanded": False}
    rail = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4, css_classes=["nav-rail"])
    rail.set_size_request(56, -1)
    body.append(rail)

    content = Gtk.Box(hexpand=True, vexpand=True)
    content.set_margin_start(12)
    body.append(content)

    destinations = [("grid", "\U0001f3ae", "Grid"), ("table", "\U0001f4cb", "Action Table"), ("library", "\U0001f4da", "Library"), ("chords", "⛓", "Chords")]
    state = {"dest": "grid"}

    def render_content():
        clear_children(content)
        fresh_config = client.get_config()
        if state["dest"] == "grid":
            widget, _buttons = build_device_grid(client, fresh_config, profile, layer, on_change)
            content.append(widget)
        elif state["dest"] == "table":
            ui_state.setdefault("expanded_rows", set())
            table = build_action_table(client, fresh_config, profile, layer, on_change, ui_state)
            table.set_hexpand(True)
            content.append(table)
        elif state["dest"] == "library":
            content.append(build_library_panel(ui_state.setdefault("library_state", make_library_seed_state()), show_toast))
        else:
            col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
            grid_widget, grid_buttons = build_device_grid(client, fresh_config, profile, layer, on_change, w=40, h=52)
            col.append(grid_widget)
            col.append(Gtk.Separator())
            col.append(build_chords_list_panel(seed_chords(), grid_buttons, show_toast))
            content.append(col)

    def render_rail():
        clear_children(rail)
        pin_btn = Gtk.Button(label="»" if not pin_state["expanded"] else "«")
        pin_btn.set_tooltip_text("Pin rail expanded" if not pin_state["expanded"] else "Collapse rail")

        def on_pin(b):
            pin_state["expanded"] = not pin_state["expanded"]
            render_rail()

        pin_btn.connect("clicked", on_pin)
        rail.append(pin_btn)
        rail.set_size_request(170 if pin_state["expanded"] else 56, -1)
        for key, glyph, label in destinations:
            text = f"{glyph}  {label}" if pin_state["expanded"] else glyph
            btn = Gtk.Button(label=text, css_classes=["nav-rail-btn"])
            btn.set_tooltip_text(label)
            if key == state["dest"]:
                btn.add_css_class("nav-rail-btn-active")

            def on_click(b, key=key):
                state["dest"] = key
                render_rail()
                render_content()

            btn.connect("clicked", on_click)
            rail.append(btn)

    def rerender():
        render_rail()
        render_content()

    rerender()
    return outer


# --- Variant D (round 2): A/B/C's rail concept dropped. Real, un-folded
# Profile sidebar (exactly as in the live GUI); Action Table cut outright;
# a plain-text "Grid"/"Library" switcher sits where B's Action-Table/Chords
# toggle row used to be; the Chords toggle moves to the live GUI's own
# Action-Table-toggle slot in `top_row`, beside the real layer bar, and
# only exists at all while Grid is the active destination. ---


def build_variant_d(client, ui_state: dict) -> Gtk.Widget:
    outer = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=16)
    outer.set_margin_top(12)
    outer.set_margin_bottom(12)
    outer.set_margin_start(12)
    outer.set_margin_end(12)

    config = client.get_config()
    profile = config["active_profile"]

    def on_change():
        rerender()

    sidebar = build_profile_sidebar(client, config, profile, on_change)
    # Round 2 fix: a plain Gtk.Box with no explicit hexpand still computes
    # one via GTK4's expand-propagation (it sees `switch_btn`'s own
    # `hexpand=True`, several layers down each profile row, and infers the
    # sidebar itself wants to expand) — invisible while the grid's own big
    # natural width already ate all the slack next to it, but the Library
    # panel next to it is narrower, leaving slack that GTK then handed to
    # the sidebar instead, visibly widening it. Pin it explicitly so it's
    # always exactly its own `set_size_request(150, -1)` width, identical
    # in both destinations, matching the live GUI's own un-widened sidebar.
    sidebar.set_hexpand(False)
    outer.append(sidebar)

    right = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10, hexpand=True, vexpand=True)
    toast_label, show_toast = build_toast(right)
    outer.append(right)

    # The Grid/Library switcher — plain text, no glyphs, sitting where
    # Variant B's Action-Table/Chords toggle row used to be, above whatever
    # the selected destination renders.
    switch_row = Gtk.Box(spacing=6)
    grid_switch_btn = Gtk.Button(label="Grid")
    library_switch_btn = Gtk.Button(label="Library")
    switch_row.append(grid_switch_btn)
    switch_row.append(library_switch_btn)
    right.append(switch_row)

    # Round 2: a light separator between the Grid/Library switcher and
    # whatever it's switching (the Base/Held/Mode-key row, for Grid) —
    # `Gtk.Separator`'s stock look is already a light grey hairline.
    right.append(Gtk.Separator())

    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16, hexpand=True, vexpand=True)
    right.append(content)

    state = {"dest": "grid"}
    ui_state.setdefault("selected_layer", "base")

    def render_switch():
        grid_switch_btn.remove_css_class("suggested-action")
        library_switch_btn.remove_css_class("suggested-action")
        (grid_switch_btn if state["dest"] == "grid" else library_switch_btn).add_css_class("suggested-action")

    def render_content():
        clear_children(content)
        fresh_config = client.get_config()
        if state["dest"] == "library":
            content.append(build_library_panel(ui_state.setdefault("library_state", make_library_seed_state()), show_toast))
            return

        mode_key_role = fresh_config["profiles"][profile]["mode_key_role"]
        content.append(build_layer_bar(client, ui_state["selected_layer"], mode_key_role, on_change, ui_state))

        # Round 2: the Chords toggle is gone — there's room to just show
        # the Chords list all the time, so opening/closing it never has to
        # rescale the rest of the window. Round 2 also fixed the grid
        # crowding the Base/Held/Mode-key row above it — a top margin here
        # (not just `content`'s own inter-row spacing) gives the grid
        # itself, specifically, room to breathe under that row.
        device = Gtk.Box(spacing=16)
        device.set_margin_top(8)
        grid_widget, grid_buttons = build_device_grid(client, fresh_config, profile, ui_state["selected_layer"], on_change)
        device.append(grid_widget)
        device.append(build_chords_list_panel(seed_chords(), grid_buttons, show_toast))
        content.append(device)

    def on_grid_clicked(b):
        state["dest"] = "grid"
        render_switch()
        render_content()

    def on_library_clicked(b):
        state["dest"] = "library"
        render_switch()
        render_content()

    grid_switch_btn.connect("clicked", on_grid_clicked)
    library_switch_btn.connect("clicked", on_library_clicked)

    def rerender():
        render_switch()
        render_content()

    rerender()
    return outer


# --- Switcher chrome (same convention as tickets 19/30/31/32/38) ---

VARIANTS = [
    ("A", "Exclusive rail · icon-only, top profile strip, Chords brings its own grid"),
    ("B", "Coexisting rail · icon+label, Grid/Table/Chords never lose sight of the grid"),
    ("C", "Collapsible rail · horizontal profile tabs, shrunk-grid Chords destination"),
    ("D", "Round 2 · no rail — live Profile sidebar, Action Table cut, text Grid/Library switch, Chords in Action Table's old slot"),
]


def build_window(app: Gtk.Application) -> Gtk.ApplicationWindow:
    provider = Gtk.CssProvider()
    provider.load_from_data(CSS.encode())
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
    )

    win = Gtk.ApplicationWindow(application=app, title="Ticket 47 prototype — Device Overview nav-rail restructuring")
    win.set_default_size(1150, 780)

    clients = {"A": seed_daemon(), "B": seed_daemon(), "C": seed_daemon(), "D": seed_daemon()}
    ui_states: dict[str, dict] = {"A": {}, "B": {}, "C": {}, "D": {}}

    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
    win.set_child(outer)

    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    for setter in (content.set_margin_top, content.set_margin_bottom, content.set_margin_start, content.set_margin_end):
        setter(4)
    outer.append(content)

    index = {"i": len(VARIANTS) - 1}  # start on D — A/B/C are already-rejected reference points

    def render():
        clear_children(content)
        key, label = VARIANTS[index["i"]]
        builder = {"A": build_variant_a, "B": build_variant_b, "C": build_variant_c, "D": build_variant_d}[key]
        content.append(builder(clients[key], ui_states[key]))
        variant_label.set_label(f"{key} — {label}")

    switcher = Gtk.Box(spacing=8, halign=Gtk.Align.CENTER, css_classes=["switcher-pill"])
    switcher.set_margin_bottom(10)
    prev_btn = Gtk.Button(label="←")
    next_btn = Gtk.Button(label="→")
    variant_label = Gtk.Label(css_classes=["variant-label"])

    def cycle(delta):
        index["i"] = (index["i"] + delta) % len(VARIANTS)
        render()

    prev_btn.connect("clicked", lambda b: cycle(-1))
    next_btn.connect("clicked", lambda b: cycle(1))
    switcher.append(prev_btn)
    switcher.append(variant_label)
    switcher.append(next_btn)
    outer.append(switcher)

    key_controller = Gtk.EventControllerKey()

    def on_key(controller, keyval, keycode, state_flags):
        focus = win.get_focus()
        if isinstance(focus, (Gtk.Editable, Gtk.Scale, Gtk.SpinButton)):
            return False
        if keyval == Gdk.KEY_Left:
            cycle(-1)
            return True
        if keyval == Gdk.KEY_Right:
            cycle(1)
            return True
        return False

    key_controller.connect("key-pressed", on_key)
    win.add_controller(key_controller)

    render()
    return win


def main() -> None:
    app = Gtk.Application(application_id="com.acheron.prototype.ticket47")
    app.connect("activate", lambda a: build_window(a).present())
    app.run(None)


if __name__ == "__main__":
    main()
