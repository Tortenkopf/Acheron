"""PROTOTYPE — throwaway, answers ticket 38 (Design the controller-button
picker GUI): .scratch/tartarus-input-expansion/issues/38-prototype-controller-button-picker-ux.md

Builds on ticket 14's settled model: `Action::ControllerButton { button: KeyCode }`,
freely assignable from any physical Input to any button in the standard Linux
Gamepad Spec set — no hardcoded physical->button correspondence. The catalog
here (57 entries: 4 face + 4 shoulders/triggers + 2 stick clicks + 4 d-pad +
3 select/start/mode + 40 Trigger-Happy extras) hand-lists ticket 14's answer;
the real build reads the true evdev::KeyCode allowlist.

Plan: two structurally different variants, switchable via the same floating
bottom pill as tickets 19/30/31/32 (Prev/Next buttons + Left/Right arrow
keys). Both share one catalog (GAMEPAD_CATEGORIES). Round 3's live
reaction settled the ticket's "sane defaults without hardcoding" and
"discoverability of the freedom" questions the same way: **no** suggested-
default chip and **no** explicit freedom note — an early build of both
pickers carried a per-physical-Input "Suggested: ..." chip plus explanatory
text, cut after live reaction judged them unnecessary noise (the diagram
and the full always-visible catalog already make "pick anything" obvious
without being told). Both variants now just open on a plain starting value
with zero built-in opinion about what a given Input "should" be. Both
mount inside a host mock whose Action dropdown offers "Keypress" alongside
"Controller Button"
(unmocked keypress body — ticket 32/42's concern, not this ticket's) to
settle the "shared container vs. independent picker" question: they stay
independent pickers under one Action-kind dropdown, matching how
`Action::Keypress` and `Action::ControllerButton` are already separate
enum variants, not a shared field — no merged catalog, no shared widget
beyond the same *shape* of component (label + summary/trigger + panel),
which both variants already demonstrate by structurally mirroring ticket
32's two components with a different catalog swapped in.

Variants:
    A - Gamepad Diagram: a labeled visual controller face (Gtk.Fixed,
        approximate real gamepad geometry) for the 17 named buttons -
        d-pad diamond, ABXY diamond, shoulders/triggers, stick clicks,
        Select/Start/Mode - plus a collapsed "Extra buttons (Trigger-Happy
        1-40) +" section below it that expands into an 8-wide grid, kept
        separate from the diagram per the ticket's own steer ("the ~40-slot
        range needs a layout that doesn't overwhelm - likely category/list
        even if the named buttons get a visual diagram"). Bets that a
        recognizable controller shape answers "where's the button I want"
        faster than reading label text, for the 17 buttons a physical
        controller actually shows you.
    B - Category List: no diagram at all - a "Choose button..." MenuButton
        opens a popover with a category dropdown (mirrors ticket 32
        variant B's shape exactly, catalog swapped for GAMEPAD_CATEGORIES)
        driving a scrollable list, search on top. Bets that most users
        don't have Xbox-diagram button names memorized well enough for a
        picture to be faster than reading "Y / North", and that one
        consistent picker shape (already used for keys/mouse in ticket 32)
        beats teaching a second, diagram-specific interaction.

Wipe me: nothing here persists past process exit; none of it should be
promoted as-is (see each variant's own fold-in note).

Run:
    python3 gui/prototype_38_controller_button_picker_ux.py
"""

from __future__ import annotations

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gtk

from .gtk_utils import clear_children

CSS = """
.heading { font-weight: bold; }
.dim { opacity: 0.6; font-size: smaller; }
.editor-pane { padding: 10px; background-color: alpha(currentColor, 0.04); border-radius: 6px; margin-bottom: 10px; }
.picker-panel { padding: 8px; background-color: alpha(currentColor, 0.05); border-radius: 6px; }
.section-label { font-size: smaller; opacity: 0.65; font-weight: bold; }
.pad-body { background-color: alpha(currentColor, 0.06); border-radius: 18px; }
.padbtn { min-width: 0; min-height: 0; font-size: 11px; padding: 2px 4px; }
.padbtn-face { background-color: alpha(#4a90e2, 0.22); }
.padbtn-shoulder { background-color: alpha(#e67e22, 0.22); }
.padbtn-stick { background-color: alpha(#8e44ad, 0.22); }
.padbtn-dpad { background-color: alpha(#27ae60, 0.22); }
.padbtn-center { background-color: alpha(currentColor, 0.10); }
.switcher-pill { background-color: alpha(currentColor, 0.08); border: 1px solid alpha(currentColor, 0.25); border-radius: 999px; padding: 6px 14px; }
.variant-label { font-weight: bold; }
.category-row { padding: 4px 6px; }
"""

# ---------------------------------------------------------------------
# Shared catalog — ticket 14's settled device-advertising scope: the
# named BTN_GAMEPAD range, BTN_TRIGGER_HAPPY1-40, and the 4 dpad buttons
# (axes deferred, so dpad is 4 discrete buttons for now).
# ---------------------------------------------------------------------

FACE = [
    ("BTN_SOUTH", "A / South"),
    ("BTN_EAST", "B / East"),
    ("BTN_NORTH", "Y / North"),
    ("BTN_WEST", "X / West"),
]
SHOULDERS = [
    ("BTN_TL", "LB (Left bumper)"),
    ("BTN_TR", "RB (Right bumper)"),
    ("BTN_TL2", "LT (Left trigger)"),
    ("BTN_TR2", "RT (Right trigger)"),
]
STICKS = [
    ("BTN_THUMBL", "L3 (Left stick click)"),
    ("BTN_THUMBR", "R3 (Right stick click)"),
]
DPAD = [
    ("BTN_DPAD_UP", "D-pad Up"),
    ("BTN_DPAD_DOWN", "D-pad Down"),
    ("BTN_DPAD_LEFT", "D-pad Left"),
    ("BTN_DPAD_RIGHT", "D-pad Right"),
]
CENTER = [
    ("BTN_SELECT", "Select / Back"),
    ("BTN_START", "Start"),
    ("BTN_MODE", "Guide / Mode"),
]
EXTRA = [(f"BTN_TRIGGER_HAPPY{i}", f"Extra {i}") for i in range(1, 41)]

GAMEPAD_CATEGORIES: dict[str, list[tuple[str, str]]] = {
    "Face buttons": FACE,
    "Shoulders & triggers": SHOULDERS,
    "Sticks": STICKS,
    "D-pad": DPAD,
    "Select / Start / Mode": CENTER,
    "Extra (Trigger-Happy 1-40)": EXTRA,
}

_ALL_ENTRIES: list[tuple[str, str, str]] = [
    (code, label, category)
    for category, entries in GAMEPAD_CATEGORIES.items()
    for code, label in entries
]
LABEL_BY_CODE = {code: label for code, label, _cat in _ALL_ENTRIES}


def button_css_class(code: str) -> str | None:
    if code in {c for c, _ in FACE}:
        return "padbtn-face"
    if code in {c for c, _ in SHOULDERS}:
        return "padbtn-shoulder"
    if code in {c for c, _ in STICKS}:
        return "padbtn-stick"
    if code in {c for c, _ in DPAD}:
        return "padbtn-dpad"
    return None


# ---------------------------------------------------------------------
# Mock physical Inputs (a representative subset, not all 28).
# ---------------------------------------------------------------------

MOCK_INPUTS = [
    "Grid 1", "Grid 2", "Grid 7", "Grid 20",
    "Mode key", "Thumbstick Up", "Thumbstick Down", "Thumbstick Left", "Thumbstick Right",
    "Wheel CW", "Wheel CCW",
]


# ---------------------------------------------------------------------
# Variant A — Gamepad Diagram: a visual controller face for the 17 named
# buttons, plus a collapsed Extra-buttons grid kept separate so the
# 40-slot range never crowds the diagram.
# ---------------------------------------------------------------------

# (code, label-on-pad, x, y, width) — hand-placed to approximate a real
# gamepad's face layout. Canvas is ~440x210. The trigger/bumper row sits
# well clear of the diamonds below it (18px vertical gap on each side)
# after round 1's live reaction found LT/LB and RT/RB actually overlapping
# the D-pad and Y button — the diamonds moved down and the shoulder row
# moved further up/outward to open real clearance, not just a touching
# boundary. R3 also moved from an arbitrary x=260 to sit centered under
# South (A), mirroring L3's already-correct centering under D-pad Down.
_PAD_LAYOUT = [
    # D-pad diamond, left side
    ("BTN_DPAD_UP", "↑", 55, 35, 30),
    ("BTN_DPAD_LEFT", "←", 25, 65, 30),
    ("BTN_DPAD_RIGHT", "→", 85, 65, 30),
    ("BTN_DPAD_DOWN", "↓", 55, 95, 30),
    # Stick clicks — each centered under its diamond's bottom button
    ("BTN_THUMBL", "L3", 55, 135, 34),
    ("BTN_THUMBR", "R3", 338, 135, 34),
    # Center cluster
    ("BTN_SELECT", "Select", 150, 80, 50),
    ("BTN_START", "Start", 210, 80, 46),
    ("BTN_MODE", "Guide", 175, 40, 50),
    # ABXY diamond, right side
    ("BTN_NORTH", "Y", 340, 35, 30),
    ("BTN_WEST", "X", 310, 65, 30),
    ("BTN_EAST", "B", 370, 65, 30),
    ("BTN_SOUTH", "A", 340, 95, 30),
    # Shoulders / triggers, top strip — clear of the diamonds now
    ("BTN_TL2", "LT", 5, -35, 40),
    ("BTN_TL", "LB", 5, -9, 40),
    ("BTN_TR2", "RT", 375, -35, 40),
    ("BTN_TR", "RB", 375, -9, 40),
]

# Added to every raw y above before placing on the Fixed, so the trigger
# row's -35 still lands at a non-negative screen coordinate (5px).
_OFFSET_Y = 40


def build_pad_diagram(on_pick, current: str) -> Gtk.Widget:
    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    outer.set_margin_top(6)

    fixed = Gtk.Fixed()
    body = Gtk.Box(css_classes=["pad-body"])
    body.set_size_request(440, 210)
    fixed.put(body, 0, 0)

    for code, cap, x, y, w in _PAD_LAYOUT:
        btn = Gtk.Button(label=cap, css_classes=["padbtn"])
        cls = button_css_class(code)
        if cls:
            btn.add_css_class(cls)
        if code == current:
            btn.add_css_class("suggested-action")
        btn.set_size_request(w, 26)
        btn.set_tooltip_text(LABEL_BY_CODE[code])
        btn.connect("clicked", lambda b, code=code: on_pick(code))
        fixed.put(btn, x, y + _OFFSET_Y)

    outer.append(fixed)
    return outer


def build_extra_grid(on_pick, current: str) -> Gtk.Widget:
    state = {"shown": False}
    section = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    toggle_btn = Gtk.Button(label="Extra buttons (Trigger-Happy 1-40) ▸", halign=Gtk.Align.START)
    slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    section.append(toggle_btn)
    section.append(slot)

    def render():
        clear_children(slot)
        if not state["shown"]:
            return
        grid = Gtk.Grid(row_spacing=3, column_spacing=3)
        for i, (code, _label) in enumerate(EXTRA):
            btn = Gtk.Button(label=str(i + 1), css_classes=["padbtn"])
            btn.set_size_request(26, 24)
            btn.set_tooltip_text(LABEL_BY_CODE[code])
            if code == current:
                btn.add_css_class("suggested-action")
            btn.connect("clicked", lambda b, code=code: on_pick(code))
            grid.attach(btn, i % 8, i // 8, 1, 1)
        slot.append(grid)

    def on_toggle(b):
        state["shown"] = not state["shown"]
        toggle_btn.set_label(("Extra buttons (Trigger-Happy 1-40) ▾" if state["shown"] else "Extra buttons (Trigger-Happy 1-40) ▸"))
        render()

    toggle_btn.connect("clicked", on_toggle)
    render()
    return section


def build_diagram_button_picker(field_label: str, current_code: str, on_change) -> Gtk.Widget:
    """Variant A's reusable picker: a collapsed summary that expands into
    the gamepad diagram + Extra grid, mirroring ticket 32 variant A's
    collapse/expand shape with a gamepad catalog swapped in."""
    state = {"code": current_code, "expanded": False}
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

    summary_row = Gtk.Box(spacing=8)
    summary_row.append(Gtk.Label(label=field_label, xalign=0))
    toggle_btn = Gtk.Button(hexpand=True)
    summary_row.append(toggle_btn)
    root.append(summary_row)

    panel_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, css_classes=["picker-panel"])
    root.append(panel_slot)

    def render_summary():
        chevron = "▾" if state["expanded"] else "▸"
        toggle_btn.set_label(f"{LABEL_BY_CODE.get(state['code'], state['code'])}  {chevron} Change")

    def render_panel():
        clear_children(panel_slot)
        panel_slot.set_visible(state["expanded"])
        if state["expanded"]:
            panel_slot.append(build_pad_diagram(on_pick, state["code"]))
            panel_slot.append(build_extra_grid(on_pick, state["code"]))

    def on_pick(code: str):
        state["code"] = code
        state["expanded"] = False
        render_summary()
        render_panel()
        on_change(code)

    def on_toggle(b):
        state["expanded"] = not state["expanded"]
        render_summary()
        render_panel()

    toggle_btn.connect("clicked", on_toggle)
    render_summary()
    render_panel()
    return root


# ---------------------------------------------------------------------
# Variant B — Category List: no diagram, a search+category popover.
# Structurally identical to ticket 32 variant B, GAMEPAD_CATEGORIES
# swapped in for KEY_CATEGORIES — demonstrates the underlying "search +
# category popover" component generalizes across catalogs.
# ---------------------------------------------------------------------

_CATEGORY_NAMES = ["All"] + list(GAMEPAD_CATEGORIES.keys())


def build_popover_button_picker(field_label: str, current_code: str, on_change) -> Gtk.Widget:
    state = {"code": current_code}
    row = Gtk.Box(spacing=8)
    row.append(Gtk.Label(label=field_label, xalign=0))

    menu_btn = Gtk.MenuButton(hexpand=True)
    row.append(menu_btn)

    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    root.append(row)

    def render_button():
        menu_btn.set_label(f"Choose button: {LABEL_BY_CODE.get(state['code'], state['code'])} ▾")

    popover = Gtk.Popover()
    popover.set_size_request(300, 360)
    pbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    for setter in (pbox.set_margin_top, pbox.set_margin_bottom, pbox.set_margin_start, pbox.set_margin_end):
        setter(8)

    search = Gtk.SearchEntry()
    pbox.append(search)

    cat_dd = Gtk.DropDown(model=Gtk.StringList.new(_CATEGORY_NAMES))
    pbox.append(cat_dd)

    scroller = Gtk.ScrolledWindow(vexpand=True)
    scroller.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
    results = Gtk.ListBox()
    results.add_css_class("boxed-list")
    scroller.set_child(results)
    pbox.append(scroller)
    popover.set_child(pbox)
    menu_btn.set_popover(popover)

    def matching_entries():
        query = search.get_text().strip().lower()
        if query:
            return [(c, l) for c, l, _cat in _ALL_ENTRIES if query in l.lower() or query in c.lower()]
        cat_i = cat_dd.get_selected()
        cat_name = _CATEGORY_NAMES[cat_i]
        if cat_name == "All":
            return [(c, l) for c, l, _cat in _ALL_ENTRIES]
        return GAMEPAD_CATEGORIES[cat_name]

    def render_results():
        clear_children(results)
        for code, label in matching_entries():
            row_box = Gtk.Box(spacing=6, css_classes=["category-row"])
            cls = button_css_class(code)
            dot = Gtk.Label(label="●", css_classes=[cls] if cls else [])
            row_box.append(dot)
            row_box.append(Gtk.Label(label=label, hexpand=True, xalign=0))
            btn = Gtk.Button(label="Use")

            def on_use(b, code=code):
                on_use_code(code)
                popover.popdown()

            btn.connect("clicked", on_use)
            row_box.append(btn)
            list_row = Gtk.ListBoxRow(child=row_box)
            results.append(list_row)

    def on_use_code(code: str):
        state["code"] = code
        render_button()
        on_change(code)

    search.connect("search-changed", lambda e: render_results())
    cat_dd.connect("notify::selected", lambda *_: render_results())

    render_button()
    render_results()
    return root


# ---------------------------------------------------------------------
# Host mock — a "Binding editor" pane whose Action dropdown offers
# Keypress alongside Controller Button, settling the "shared container
# vs. independent picker" question: independent pickers under one
# Action-kind selector, since Action::Keypress/Action::ControllerButton
# are already separate enum variants, not one field to share.
# ---------------------------------------------------------------------


def build_host(build_picker) -> Gtk.Widget:
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    root.set_margin_top(12)
    root.set_margin_bottom(12)
    root.set_margin_start(12)
    root.set_margin_end(12)

    pane = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["editor-pane"])
    pane.append(Gtk.Label(label="Binding editor (mock)", xalign=0, css_classes=["heading"]))

    input_row = Gtk.Box(spacing=8)
    input_row.append(Gtk.Label(label="Input"))
    input_dd = Gtk.DropDown(model=Gtk.StringList.new(MOCK_INPUTS))
    input_row.append(input_dd)
    pane.append(input_row)

    trig_row = Gtk.Box(spacing=8)
    trig_row.append(Gtk.Label(label="Trigger mode"))
    trig_row.append(Gtk.DropDown(model=Gtk.StringList.new(["Fire-once", "Hold-to-repeat", "Toggle"])))
    pane.append(trig_row)

    action_row = Gtk.Box(spacing=8)
    action_row.append(Gtk.Label(label="Action"))
    action_dd = Gtk.DropDown(model=Gtk.StringList.new(["Keypress", "Controller Button"]))
    action_dd.set_selected(1)
    action_row.append(action_dd)
    pane.append(action_row)

    editor_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    pane.append(editor_slot)

    def render_editor():
        clear_children(editor_slot)
        if action_dd.get_selected() == 0:
            editor_slot.append(Gtk.Label(label="(Keypress body — see ticket 32/42, not mocked here)", xalign=0, css_classes=["dim"]))
            return
        editor_slot.append(build_picker("Button", "BTN_SOUTH", lambda code: None))

    action_dd.connect("notify::selected", lambda *_: render_editor())
    render_editor()

    root.append(pane)

    root.append(
        Gtk.Label(
            label=(
                "A Chord's or Macro step's target reuses this exact picker unchanged — "
                "ticket 14 already settled Action::ControllerButton as an ordinary Action "
                "and a KeyDown/KeyUp step target, not separately mocked here."
            ),
            xalign=0,
            wrap=True,
            max_width_chars=48,
            css_classes=["dim"],
        )
    )
    return root


# --- Switcher chrome ---

VARIANTS = [
    ("A", "Gamepad Diagram · visual controller face + collapsed Extra grid"),
    ("B", "Category List · search + category popover, no diagram"),
]


def build_window(app: Gtk.Application) -> Gtk.ApplicationWindow:
    provider = Gtk.CssProvider()
    provider.load_from_data(CSS.encode())
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
    )

    win = Gtk.ApplicationWindow(application=app, title="Ticket 38 prototype — controller-button picker UX")
    win.set_default_size(-1, 620)

    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
    win.set_child(outer)

    scroller = Gtk.ScrolledWindow(vexpand=True, hexpand=True)
    scroller.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
    scroller.set_propagate_natural_width(True)
    outer.append(scroller)

    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    scroller.set_child(content)

    index = {"i": 0}

    def render():
        clear_children(content)
        key, label = VARIANTS[index["i"]]
        if key == "A":
            content.append(build_host(build_diagram_button_picker))
        else:
            content.append(build_host(build_popover_button_picker))
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
    app = Gtk.Application(application_id="com.acheron.prototype.ticket38")
    app.connect("activate", lambda a: build_window(a).present())
    app.run(None)


if __name__ == "__main__":
    main()
