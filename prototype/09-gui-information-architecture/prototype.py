#!/usr/bin/env python3
"""
PROTOTYPE — throwaway code, not production. Do not import from real Acheron code.

Answers: what should Acheron's GTK4 GUI information architecture look like?
Ticket: .scratch/tartarus-keybinder/issues/09-design-gui-information-architecture.md

Started as three structurally different variants (compared via a switchable
prototype-only picker); the resolved answer settled on one main view, with a
second variant demoted to an optional panel off of it, and the third dropped:

  - Device Overview (the main and only view): mirrors the physical Tartarus Pro
    (grid + thumbstick + wheel + Mode key, real relative positions per Device
    Picture.jpg and layout.md), click a control to edit it.
  - Action Table: closed by default, toggled open as a sidebar off Device
    Overview — one expandable row per Input, only bound Inputs shown by
    default. No longer a standalone page; reads whatever Profile/Layer is
    already selected in Device Overview rather than duplicating those pickers.
  - "Focused wizard" (dot overview + one-binding-at-a-time editor) — dropped,
    no longer in this file.

No real D-Bus connection — DaemonStub stands in for the Daemon decided in ticket
08 (same method names, same wire-encoding conventions: Input as a string, Action
as a type-tagged dict). Every "mutation" updates the stub and prints to the
terminal so you can see what changed. Nothing persists across runs.

Run:
    python3 prototype/09-gui-information-architecture/prototype.py
"""

import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gtk

GRID_ROWS, GRID_COLS = 4, 5


def grid_input(r, c):
    return f"grid_r{r}c{c}"


ALL_INPUTS = (
    ["mode_key"]
    + [grid_input(r, c) for r in range(1, GRID_ROWS + 1) for c in range(1, GRID_COLS + 1)]
    + ["thumbstick_up", "thumbstick_down", "thumbstick_left", "thumbstick_right"]
    + ["wheel_scroll_up", "wheel_scroll_down", "wheel_middle"]
)

LAYOUT_NUMBER = {
    grid_input(r, c): (r - 1) * GRID_COLS + c
    for r in range(1, GRID_ROWS + 1)
    for c in range(1, GRID_COLS + 1)
}

INPUT_LABELS = {
    "mode_key": "Mode",
    # Arrow glyphs match each Input's own default/passthrough evdev keycode
    # (Thumbstick Up passes through KEY_UP, etc.) — NOT the visual position it's
    # drawn at in the Device Overview diamond, which is rotated 90° clockwise
    # from these directions (see layout.md and the ticket 09 resolution).
    "thumbstick_up": "↑",
    "thumbstick_down": "↓",
    "thumbstick_left": "←",
    "thumbstick_right": "→",
    "wheel_scroll_up": "Wheel ▲",
    "wheel_scroll_down": "Wheel ▼",
    "wheel_middle": "Wheel •",
}


def input_label(inp):
    if inp in LAYOUT_NUMBER:
        return str(LAYOUT_NUMBER[inp])
    return INPUT_LABELS[inp]


TRIGGER_OPTIONS = [("fire_once", "Fire-once"), ("hold_to_repeat", "Hold-to-repeat"), ("toggle", "Toggle")]
TRIGGER_SHORT = {"fire_once": "1x", "hold_to_repeat": "hold", "toggle": "toggle"}
ACTION_TYPES = [("keypress", "Keypress"), ("macro", "Macro")]


class DaemonStub:
    """Stands in for the D-Bus methods decided in ticket 08 (GetConfig/GetState/
    SetBinding/ClearBinding/SwitchProfile) — same names, same wire conventions,
    but in-memory and synchronous since this prototype is about layout, not IPC."""

    def __init__(self):
        self.profiles = {
            "gaming": {
                "mode_key_role": "layer_switch",
                "layers": {
                    "base": {
                        grid_input(1, 1): {
                            "trigger": "fire_once",
                            "action": {"type": "keypress", "key": "KEY_F1", "modifiers": []},
                        },
                        grid_input(2, 3): {
                            "trigger": "toggle",
                            "action": {
                                "type": "macro",
                                "steps": [
                                    {"key_down": "KEY_A"},
                                    {"delay_ms": 50},
                                    {"key_up": "KEY_A"},
                                    {"delay_ms": 100},
                                ],
                            },
                        },
                        grid_input(3, 2): {
                            "trigger": "hold_to_repeat",
                            "action": {"type": "keypress", "key": "KEY_W", "modifiers": []},
                        },
                    },
                    "held": {
                        grid_input(1, 1): {
                            "trigger": "fire_once",
                            "action": {"type": "keypress", "key": "KEY_F5", "modifiers": []},
                        },
                    },
                },
            },
            "editing": {
                "mode_key_role": "bound",
                "layers": {
                    "base": {
                        grid_input(1, 1): {
                            "trigger": "fire_once",
                            "action": {"type": "keypress", "key": "KEY_T", "modifiers": ["ctrl", "shift"]},
                        },
                    },
                    "held": {},
                },
            },
        }
        self.active_profile = "gaming"
        self.active_layer = "base"
        self.active_toggles = []

    def binding_for(self, profile, layer, inp):
        return self.profiles[profile]["layers"][layer].get(inp)

    def set_binding(self, profile, layer, inp, binding):
        self.profiles[profile]["layers"][layer][inp] = binding
        print(f"[SetBinding] {profile}/{layer}/{input_label(inp)} ({inp}) -> {binding}")

    def clear_binding(self, profile, layer, inp):
        self.profiles[profile]["layers"][layer].pop(inp, None)
        print(f"[ClearBinding] {profile}/{layer}/{input_label(inp)} ({inp})")

    def switch_profile(self, name):
        self.active_profile = name
        print(f"[SwitchProfile] -> {name}   (ActiveProfileChanged signal fires)")


STATE = DaemonStub()


def action_summary(binding):
    if not binding:
        return "passthrough"
    action = binding["action"]
    if action["type"] == "keypress":
        mods = "+".join(m.capitalize() for m in action.get("modifiers", []))
        key = action["key"].replace("KEY_", "")
        chord = f"{mods}+{key}" if mods else key
        return f"{chord}  [{TRIGGER_SHORT[binding['trigger']]}]"
    steps = action.get("steps", [])
    return f"Macro ({len(steps)} steps)  [{TRIGGER_SHORT[binding['trigger']]}]"


def describe_step(step):
    if "key_down" in step:
        return f"KeyDown {step['key_down']}"
    if "key_up" in step:
        return f"KeyUp {step['key_up']}"
    if "delay_ms" in step:
        return f"Delay {step['delay_ms']}ms"
    return str(step)


def labeled_row(label, widget):
    row = Gtk.Box(spacing=8)
    lbl = Gtk.Label(label=label, xalign=0)
    lbl.set_size_request(90, -1)
    row.append(lbl)
    widget.set_hexpand(True)
    row.append(widget)
    return row


def build_binding_editor(state, profile, layer, inp, on_saved):
    """Shared between Device Overview and the Action Table sidebar — the
    trigger/action editing controls are the same domain object regardless of
    how you navigated to reach them; what differs is the navigation, not
    this form."""
    existing = state.binding_for(profile, layer, inp)
    starting = existing or {"trigger": "fire_once", "action": {"type": "keypress", "key": "KEY_A", "modifiers": []}}

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    box.set_margin_top(10)
    box.set_margin_bottom(10)
    box.set_margin_start(10)
    box.set_margin_end(10)
    heading = Gtk.Label(label=f"{profile} / {layer} / {input_label(inp)}", xalign=0)
    heading.add_css_class("heading")
    box.append(heading)

    trigger_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in TRIGGER_OPTIONS]))
    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index(starting["trigger"]))
    box.append(labeled_row("Trigger mode", trigger_dd))

    action_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in ACTION_TYPES]))
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index(starting["action"]["type"]))
    box.append(labeled_row("Action", action_dd))

    editor_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.append(editor_slot)

    draft = {
        "keypress": dict(starting["action"]) if starting["action"]["type"] == "keypress" else {"key": "KEY_A", "modifiers": []},
        "steps": list(starting["action"].get("steps", [])) if starting["action"]["type"] == "macro" else [],
    }

    def render_action_editor():
        child = editor_slot.get_first_child()
        while child is not None:
            nxt = child.get_next_sibling()
            editor_slot.remove(child)
            child = nxt

        kind = ACTION_TYPES[action_dd.get_selected()][0]
        if kind == "keypress":
            key_entry = Gtk.Entry(text=draft["keypress"].get("key", "KEY_A"))
            key_entry.connect("changed", lambda e: draft["keypress"].__setitem__("key", e.get_text()))
            editor_slot.append(labeled_row("Key", key_entry))

            mod_box = Gtk.Box(spacing=8)
            mods = set(draft["keypress"].get("modifiers", []))
            for m in ("ctrl", "shift", "alt", "super"):
                cb = Gtk.CheckButton(label=m)
                cb.set_active(m in mods)

                def on_mod(c, m=m):
                    cur = set(draft["keypress"].get("modifiers", []))
                    if c.get_active():
                        cur.add(m)
                    else:
                        cur.discard(m)
                    draft["keypress"]["modifiers"] = sorted(cur)

                cb.connect("toggled", on_mod)
                mod_box.append(cb)
            editor_slot.append(mod_box)
        else:
            steps_list = Gtk.ListBox()
            steps_list.add_css_class("boxed-list")

            def render_steps():
                row = steps_list.get_first_child()
                while row is not None:
                    nxt = row.get_next_sibling()
                    steps_list.remove(row)
                    row = nxt
                for i, step in enumerate(draft["steps"]):
                    row_box = Gtk.Box(spacing=6)
                    row_box.set_margin_top(2)
                    row_box.set_margin_bottom(2)
                    row_box.set_margin_start(4)
                    row_box.set_margin_end(4)
                    row_box.append(Gtk.Label(label=describe_step(step), hexpand=True, xalign=0))
                    rm = Gtk.Button(label="×")

                    def on_remove(b, i=i):
                        draft["steps"].pop(i)
                        render_steps()

                    rm.connect("clicked", on_remove)
                    row_box.append(rm)
                    steps_list.append(row_box)

            render_steps()
            editor_slot.append(steps_list)

            add_box = Gtk.Box(spacing=6)
            step_kind_dd = Gtk.DropDown(model=Gtk.StringList.new(["KeyDown", "KeyUp", "Delay (ms)"]))
            value_entry = Gtk.Entry(text="KEY_A", width_chars=10)
            add_box.append(step_kind_dd)
            add_box.append(value_entry)
            add_btn = Gtk.Button(label="+ Add step")

            def on_add(b):
                kind_i = step_kind_dd.get_selected()
                val = value_entry.get_text()
                if kind_i == 0:
                    step = {"key_down": val}
                elif kind_i == 1:
                    step = {"key_up": val}
                else:
                    step = {"delay_ms": int(val) if val.isdigit() else 0}
                draft["steps"].append(step)
                render_steps()

            add_btn.connect("clicked", on_add)
            add_box.append(add_btn)
            editor_slot.append(add_box)

    action_dd.connect("notify::selected", lambda *_: render_action_editor())
    render_action_editor()

    btn_row = Gtk.Box(spacing=8)
    save_btn = Gtk.Button(label="Save")
    save_btn.add_css_class("suggested-action")

    def on_save(b):
        kind = ACTION_TYPES[action_dd.get_selected()][0]
        if kind == "keypress":
            action = {"type": "keypress", "key": draft["keypress"].get("key", "KEY_A"), "modifiers": draft["keypress"].get("modifiers", [])}
        else:
            action = {"type": "macro", "steps": draft["steps"]}
        new_binding = {"trigger": TRIGGER_OPTIONS[trigger_dd.get_selected()][0], "action": action}
        state.set_binding(profile, layer, inp, new_binding)
        on_saved()

    save_btn.connect("clicked", on_save)
    btn_row.append(save_btn)

    clear_btn = Gtk.Button(label="Clear (passthrough)")

    def on_clear(b):
        state.clear_binding(profile, layer, inp)
        on_saved()

    clear_btn.connect("clicked", on_clear)
    btn_row.append(clear_btn)
    box.append(btn_row)
    return box


def build_layer_bar(state, on_change):
    box = Gtk.Box(spacing=6)
    for layer in ("base", "held"):
        b = Gtk.Button(label=layer.capitalize())
        if state.active_layer == layer:
            b.add_css_class("suggested-action")

        def on_click(btn, l=layer):
            state.active_layer = l
            on_change()

        b.connect("clicked", on_click)
        box.append(b)
    return box


def build_tray_mock(state, on_change):
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
    status = f"{state.active_profile} · {state.active_layer}"
    icon_row.append(Gtk.Label(label=status, xalign=0))
    box.append(icon_row)

    menu_btn = Gtk.MenuButton(label="Quick switch ▾")
    popover = Gtk.Popover()
    menu_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
    for name in state.profiles:
        b = Gtk.Button(label=name)

        def on_pick(btn, n=name):
            state.switch_profile(n)
            popover.popdown()
            on_change()

        b.connect("clicked", on_pick)
        menu_box.append(b)
    popover.set_child(menu_box)
    menu_btn.set_popover(popover)
    box.append(menu_btn)

    note = Gtk.Label(label="real tray uses AppIndicator3\n(not available in this sandbox)")
    note.add_css_class("dim")
    note.set_wrap(True)
    box.append(note)
    return box


def make_input_button(state, inp, on_change, w=76, h=52):
    binding = state.binding_for(state.active_profile, state.active_layer, inp)
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

    editor = build_binding_editor(state, state.active_profile, state.active_layer, inp, on_saved)
    popover.set_child(editor)
    btn.set_popover(popover)
    return btn


def build_main_view(state, on_change, ui_state):
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
    for name in state.profiles:
        b = Gtk.Button(label=name)
        if name == state.active_profile:
            b.add_css_class("suggested-action")

        def on_click(btn, n=name):
            state.switch_profile(n)
            on_change()

        b.connect("clicked", on_click)
        sidebar.append(b)
    new_btn = Gtk.Button(label="+ New Profile")
    new_btn.connect("clicked", lambda b: print("[stub] would open a Create Profile dialog"))
    sidebar.append(new_btn)
    root.append(sidebar)

    main = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    main.set_hexpand(True)

    top_row = Gtk.Box(spacing=12)
    top_row.append(build_layer_bar(state, on_change))
    spacer = Gtk.Box(hexpand=True)
    top_row.append(spacer)
    table_toggle = Gtk.ToggleButton(label="Action Table ◂" if ui_state["table_open"] else "Action Table ▸")
    table_toggle.set_active(ui_state["table_open"])
    top_row.append(table_toggle)
    main.append(top_row)

    device = Gtk.Box(spacing=28)

    # Grid: rows 1-3 are a full 5 columns; row 4 is only 4 wide (16-19). The
    # wheel occupies the same column-5 slot row 4's missing key would sit in
    # (next to 19), continuing straight down for two more rows (scroll up,
    # click, scroll down) — same Gtk.Grid, same button size, so it lines up
    # exactly like a real 5th column rather than a detached side panel.
    grid = Gtk.Grid(row_spacing=4, column_spacing=4)
    for r in range(1, GRID_ROWS + 1):
        cols = GRID_COLS if r < GRID_ROWS else GRID_COLS - 1
        for c in range(1, cols + 1):
            grid.attach(make_input_button(state, grid_input(r, c), on_change), c - 1, r - 1, 1, 1)
    wheel_col_index = GRID_COLS - 1
    grid.attach(make_input_button(state, "wheel_scroll_up", on_change), wheel_col_index, GRID_ROWS - 1, 1, 1)
    grid.attach(make_input_button(state, "wheel_middle", on_change), wheel_col_index, GRID_ROWS, 1, 1)
    grid.attach(make_input_button(state, "wheel_scroll_down", on_change), wheel_col_index, GRID_ROWS + 1, 1, 1)
    device.append(grid)

    # Thumbstick, further right — the Mode key sits directly above the diamond's
    # top lobe (Left, per the rotation below), and "20" below it, each its own
    # block with breathing room between; the diamond itself stays tight so it
    # still reads as one control.
    stick_col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=18, halign=Gtk.Align.CENTER, valign=Gtk.Align.START)
    mode_btn = make_input_button(state, "mode_key", on_change, 52, 40)
    mode_btn.add_css_class("mode-key")
    stick_col.append(mode_btn)

    # Diamond rotated 90° clockwise from a plain N/S/E/W layout: the lobe
    # nearest the user's viewing angle when the device sits beside them on
    # the desk (per layout.md) fires Left at top, Down at left, Up at right,
    # Right at bottom — not the naive Up-at-top mapping.
    diamond = Gtk.Grid(row_spacing=2, column_spacing=2)
    diamond.attach(make_input_button(state, "thumbstick_left", on_change, 52, 40), 1, 0, 1, 1)
    diamond.attach(make_input_button(state, "thumbstick_down", on_change, 52, 40), 0, 1, 1, 1)
    diamond.attach(make_input_button(state, "thumbstick_up", on_change, 52, 40), 2, 1, 1, 1)
    diamond.attach(make_input_button(state, "thumbstick_right", on_change, 52, 40), 1, 2, 1, 1)
    stick_col.append(diamond)

    stick_col.append(make_input_button(state, grid_input(4, 5), on_change, 52, 40))
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
    table_sidebar.append(build_action_table(state, on_change))
    table_revealer.set_child(table_sidebar)
    root.append(table_revealer)

    def on_toggle(btn):
        ui_state["table_open"] = btn.get_active()
        table_revealer.set_reveal_child(btn.get_active())
        btn.set_label("Action Table ◂" if btn.get_active() else "Action Table ▸")

    table_toggle.connect("toggled", on_toggle)

    root.append(build_tray_mock(state, on_change))
    return root


def build_table_row(state, inp, on_change):
    binding = state.binding_for(state.active_profile, state.active_layer, inp)
    expander = Gtk.Expander()
    header = Gtk.Box(spacing=12)
    header.append(Gtk.Label(label=input_label(inp), width_chars=8, xalign=0))
    header.append(Gtk.Label(label=action_summary(binding), hexpand=True, xalign=0))
    expander.set_label_widget(header)
    expander.set_child(build_binding_editor(state, state.active_profile, state.active_layer, inp, on_change))
    return expander


def build_action_table(state, on_change):
    """The Action Table sidebar body. No Profile/Layer pickers of its own —
    it reads whatever's already selected in Device Overview rather than
    duplicating those controls."""
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    show_all = Gtk.CheckButton(label="Show all inputs")
    box.append(show_all)

    scroller = Gtk.ScrolledWindow(vexpand=True)
    listbox = Gtk.ListBox()
    listbox.add_css_class("boxed-list")
    scroller.set_child(listbox)

    def render_rows():
        row = listbox.get_first_child()
        while row is not None:
            nxt = row.get_next_sibling()
            listbox.remove(row)
            row = nxt
        for inp in ALL_INPUTS:
            binding = state.binding_for(state.active_profile, state.active_layer, inp)
            if not binding and not show_all.get_active():
                continue
            listbox.append(build_table_row(state, inp, on_change))

    show_all.connect("toggled", lambda b: render_rows())
    render_rows()
    box.append(scroller)
    return box


CSS = """
.heading { font-weight: bold; }
.dim { opacity: 0.6; font-size: smaller; }
.sidebar { padding: 8px; background-color: alpha(currentColor, 0.06); border-radius: 6px; }
/* Gtk.MenuButton's CSS node is named "menubutton", not "button" — these must
   be bare class selectors, not element-qualified, or they silently never match. */
.bound { border: 2px solid #4caf50; }
.empty { opacity: 0.75; }
.mode-key, .mode-key > button { border-radius: 999px; }
.tray-mock { border: 1px dashed alpha(currentColor, 0.35); border-radius: 8px; padding: 8px; }
"""


class PrototypeApp(Gtk.Application):
    def __init__(self):
        super().__init__(application_id="com.acheron.prototype.gui_ia")

    def do_activate(self):
        provider = Gtk.CssProvider()
        provider.load_from_string(CSS)
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )

        win = Gtk.ApplicationWindow(application=self, title="Acheron — GUI IA Prototype (ticket 09)")
        win.set_default_size(920, 680)

        content_box = Gtk.Box()
        # GUI-only view state (not Daemon state) that must survive a rebuild —
        # otherwise reopening the sidebar and then editing a binding would snap
        # it shut again, since rebuild() reconstructs the whole widget tree
        # (including a fresh Gtk.Revealer, which defaults closed) from scratch.
        ui_state = {"table_open": False}

        def rebuild():
            child = content_box.get_first_child()
            while child is not None:
                nxt = child.get_next_sibling()
                content_box.remove(child)
                child = nxt
            content_box.append(build_main_view(STATE, rebuild, ui_state))
            print(
                f"\n=== state: active_profile={STATE.active_profile} "
                f"active_layer={STATE.active_layer} active_toggles={STATE.active_toggles} ==="
            )

        rebuild()
        win.set_child(content_box)
        win.present()


def main():
    app = PrototypeApp()
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
