"""PROTOTYPE — throwaway, answers ticket 32 (Design the key/mouse-button
picker GUI): .scratch/tartarus-input-expansion/issues/32-prototype-key-mouse-button-picker-ux.md

Plan: two structurally different variants of the picker that replaces
`binding_editor.py`'s free-text `Gtk.Entry` key field, switchable via a
floating bottom pill (same GTK4 adaptation of the skill's `?variant=`
convention as tickets 19/30/31 — Prev/Next buttons plus Left/Right arrow
keys). Both variants share one catalog (`KEY_CATEGORIES`/`MOUSE_BUTTONS`)
covering ticket 02's settled "no exclusions" scope — this prototype
hand-lists a representative full set rather than importing the real
`evdev::KeyCode` enum (the GUI side has no binding to it); the real build
reads the true list. Each variant is mounted twice in its host mock — once
as a Binding's main "Key" field, once as a Macro step's KeyDown value — to
demonstrate the *same* picker component answering the "reused for Macro
step editing" question structurally, not just in prose. Mouse buttons are
labeled "Mouse Left/Right/Middle/Back/Forward" (not bare "Left"/"Right")
to avoid colliding with the navigation cluster's arrow-key labels once
everything sits in one searchable/browsable catalog — a prototype-level
naming call, not part of ticket 02's own settled scope.

Variants:
    A — Inline Keyboard Panel: the Key field starts collapsed (a button
        showing the current key's label + "Change ▾"); clicking it expands
        a full graphical keyboard grid in place — function row, number row,
        QWERTY/home/bottom rows sized like a real keyboard, a nav cluster,
        lock/misc strips, a multimedia strip, and a mouse-button strip —
        directly inline in the Binding editor's existing vertical flow.
        Picking a key collapses the panel back down. Bets that seeing the
        real keyboard shape (not a flat list) makes "where's the key I
        want" faster to answer, and that inline (not a popover) is fine
        since the panel isn't shown until asked for.
    B — Popover Search + Category: the Key field is a single "Choose key…"
        MenuButton; clicking it opens a Gtk.Popover with a search entry on
        top and a category dropdown (All / Letters & digits / Modifiers /
        Function keys / Navigation / Lock keys / Misc / Multimedia / Mouse
        buttons) driving a scrollable list below. Typing filters across
        every category regardless of the dropdown. Bets that a flat
        searchable list is faster once you know the key's name, and stays
        small regardless of how large F1-F24 or the multimedia set gets —
        the popover never has to be wider than the editor itself.

Both variants surface the same warning when the selected key is a
modifier (`KEY_*CTRL/SHIFT/ALT/META`): a bare modifier as a Fire-once/
Hold-to-repeat main key is a near-instant pulse, not a sustained hold —
Toggle with a single KeyDown-only Macro step is the correct pattern for a
real held modifier (per ticket 02's own settled finding).

Wipe me: nothing here persists past process exit; none of it should be
promoted as-is (see each variant's own fold-in note).

Run:
    python3 gui/prototype_32_key_mouse_button_picker_ux.py
"""

from __future__ import annotations

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk

from .gtk_utils import clear_children

CSS = """
.heading { font-weight: bold; }
.dim { opacity: 0.6; font-size: smaller; }
.warning { background-color: alpha(#e6991a, 0.18); border-radius: 6px; padding: 6px 8px; font-size: smaller; }
.editor-pane { padding: 10px; background-color: alpha(currentColor, 0.04); border-radius: 6px; margin-bottom: 10px; }
.picker-panel { padding: 8px; background-color: alpha(currentColor, 0.05); border-radius: 6px; }
.section-label { font-size: smaller; opacity: 0.65; font-weight: bold; }
.keycap { min-height: 25px; padding: 2px 4px; font-size: 12px; }
.keycap-mod { background-color: alpha(#4a90e2, 0.22); }
.keycap-mouse { background-color: alpha(#8e44ad, 0.22); }
.keycap-mm { background-color: alpha(#27ae60, 0.22); }
.switcher-pill { background-color: alpha(currentColor, 0.08); border: 1px solid alpha(currentColor, 0.25); border-radius: 999px; padding: 6px 14px; }
.variant-label { font-weight: bold; }
.category-row { padding: 4px 6px; }
"""


# ---------------------------------------------------------------------
# Shared catalog — ticket 02's "no exclusions" scope: letters/digits,
# modifiers, F1-F24, navigation cluster, lock keys, misc, multimedia,
# plus the five mouse buttons. One source feeds both variants.
# ---------------------------------------------------------------------

MODIFIER_CODES = {
    "KEY_LEFTCTRL", "KEY_RIGHTCTRL",
    "KEY_LEFTSHIFT", "KEY_RIGHTSHIFT",
    "KEY_LEFTALT", "KEY_RIGHTALT",
    "KEY_LEFTMETA", "KEY_RIGHTMETA",
}

MOUSE_BUTTONS = [
    ("BTN_LEFT", "Mouse Left"),
    ("BTN_RIGHT", "Mouse Right"),
    ("BTN_MIDDLE", "Mouse Middle"),
    ("BTN_SIDE", "Mouse Back"),
    ("BTN_EXTRA", "Mouse Forward"),
]

_LETTERS = [(f"KEY_{c.upper()}", c.upper()) for c in "abcdefghijklmnopqrstuvwxyz"]
_DIGITS = [(f"KEY_{d}", d) for d in "1234567890"]
_MODIFIERS = [
    ("KEY_LEFTCTRL", "Left Ctrl"), ("KEY_RIGHTCTRL", "Right Ctrl"),
    ("KEY_LEFTSHIFT", "Left Shift"), ("KEY_RIGHTSHIFT", "Right Shift"),
    ("KEY_LEFTALT", "Left Alt"), ("KEY_RIGHTALT", "Right Alt"),
    ("KEY_LEFTMETA", "Left Super"), ("KEY_RIGHTMETA", "Right Super"),
]
_FUNCTION = [(f"KEY_F{i}", f"F{i}") for i in range(1, 25)]
_NAVIGATION = [
    ("KEY_UP", "Up"), ("KEY_DOWN", "Down"), ("KEY_LEFT", "Left"), ("KEY_RIGHT", "Right"),
    ("KEY_HOME", "Home"), ("KEY_END", "End"), ("KEY_PAGEUP", "Page Up"), ("KEY_PAGEDOWN", "Page Down"),
    ("KEY_INSERT", "Insert"), ("KEY_DELETE", "Delete"),
]
_LOCK = [("KEY_CAPSLOCK", "Caps Lock"), ("KEY_SCROLLLOCK", "Scroll Lock"), ("KEY_NUMLOCK", "Num Lock")]
_MISC = [
    ("KEY_TAB", "Tab"), ("KEY_ESC", "Esc"), ("KEY_ENTER", "Enter"), ("KEY_SPACE", "Space"),
    ("KEY_BACKSPACE", "Backspace"), ("KEY_SYSRQ", "Print Screen"), ("KEY_PAUSE", "Pause / Break"),
    ("KEY_MENU", "Menu"), ("KEY_GRAVE", "` ~"), ("KEY_MINUS", "- _"), ("KEY_EQUAL", "= +"),
    ("KEY_LEFTBRACE", "[ {"), ("KEY_RIGHTBRACE", "] }"), ("KEY_BACKSLASH", "\\ |"),
    ("KEY_SEMICOLON", "; :"), ("KEY_APOSTROPHE", "' \""), ("KEY_COMMA", ", <"),
    ("KEY_DOT", ". >"), ("KEY_SLASH", "/ ?"),
]
_MULTIMEDIA = [
    ("KEY_MUTE", "Mute"), ("KEY_VOLUMEDOWN", "Vol -"), ("KEY_VOLUMEUP", "Vol +"),
    ("KEY_PLAYPAUSE", "Play / Pause"), ("KEY_PREVIOUSSONG", "Prev Track"), ("KEY_NEXTSONG", "Next Track"),
    ("KEY_MICMUTE", "Mic Mute"),
]

KEY_CATEGORIES: dict[str, list[tuple[str, str]]] = {
    "Letters & digits": _LETTERS + _DIGITS,
    "Modifiers": _MODIFIERS,
    "Function keys": _FUNCTION,
    "Navigation": _NAVIGATION,
    "Lock keys": _LOCK,
    "Misc": _MISC,
    "Multimedia": _MULTIMEDIA,
    "Mouse buttons": MOUSE_BUTTONS,
}

_ALL_ENTRIES: list[tuple[str, str, str]] = [
    (code, label, category)
    for category, entries in KEY_CATEGORIES.items()
    for code, label in entries
]
LABEL_BY_CODE = {code: label for code, label, _cat in _ALL_ENTRIES}


def key_css_class(code: str) -> str | None:
    if code in MODIFIER_CODES:
        return "keycap-mod"
    if code.startswith("BTN_"):
        return "keycap-mouse"
    if code in {c for c, _ in _MULTIMEDIA}:
        return "keycap-mm"
    return None


def build_modifier_warning() -> Gtk.Widget:
    # max_width_chars caps the label's *natural* size request — a wrapping
    # Gtk.Label otherwise asks for its full unwrapped line width, which
    # silently became the window's real width floor once the ScrolledWindow
    # started propagating natural width (round 4's fix) since this text is
    # far wider than the keyboard grid itself.
    return Gtk.Label(
        label=(
            "⚠ A bare modifier as a Fire-once/Hold-to-repeat main key fires a near-instant "
            "pulse, not a sustained hold. Use Toggle with a single KeyDown-only Macro step "
            "to hold a modifier down."
        ),
        xalign=0,
        wrap=True,
        max_width_chars=48,
        css_classes=["warning"],
    )


# ---------------------------------------------------------------------
# Variant A — Inline Keyboard Panel: expands a real keyboard shape
# in place, directly in the editor's own vertical flow.
# ---------------------------------------------------------------------

# Rows: (code, label, width-in-units). code == "" marks a blank spacer,
# keeping the visual grid honest without inventing a fake key for it.
_UNIT_PX = 28.5  # round 3: 10% (30 -> 27) read as too small — settled on 5% (30 -> 28.5)

_FN_ROW = [("KEY_ESC", "Esc", 1.4)] + [(f"KEY_F{i}", f"F{i}", 1.0) for i in range(1, 13)]
_FN_ROW_HI = [(f"KEY_F{i}", f"F{i}", 1.0) for i in range(13, 25)]
_NUM_ROW = (
    [("KEY_GRAVE", "`", 1.0)]
    + [(f"KEY_{d}", d, 1.0) for d in "1234567890"]
    + [("KEY_MINUS", "-", 1.0), ("KEY_EQUAL", "=", 1.0), ("KEY_BACKSPACE", "⌫", 2.0)]
)
_QWERTY_ROW = (
    [("KEY_TAB", "Tab", 1.5)]
    + [(f"KEY_{c}", c.upper(), 1.0) for c in "qwertyuiop"]
    + [("KEY_LEFTBRACE", "[", 1.0), ("KEY_RIGHTBRACE", "]", 1.0), ("KEY_BACKSLASH", "\\", 1.5)]
)
_HOME_ROW = (
    [("KEY_CAPSLOCK", "Caps", 1.8)]
    + [(f"KEY_{c}", c.upper(), 1.0) for c in "asdfghjkl"]
    + [("KEY_SEMICOLON", ";", 1.0), ("KEY_APOSTROPHE", "'", 1.0), ("KEY_ENTER", "Enter", 2.2)]
)
_BOTTOM_ROW = (
    [("KEY_LEFTSHIFT", "Shift", 2.3)]
    + [(f"KEY_{c}", c.upper(), 1.0) for c in "zxcvbnm"]
    + [("KEY_COMMA", ",", 1.0), ("KEY_DOT", ".", 1.0), ("KEY_SLASH", "/", 1.0), ("KEY_RIGHTSHIFT", "Shift", 2.3)]
)
_SPACE_ROW = [
    ("KEY_LEFTCTRL", "Ctrl", 1.3), ("KEY_LEFTMETA", "Super", 1.3), ("KEY_LEFTALT", "Alt", 1.3),
    ("KEY_SPACE", "Space", 6.0),
    ("KEY_RIGHTALT", "Alt", 1.3), ("KEY_RIGHTMETA", "Super", 1.3), ("KEY_MENU", "Menu", 1.3), ("KEY_RIGHTCTRL", "Ctrl", 1.3),
]
_NAV_BLOCK = [
    [("KEY_INSERT", "Ins", 1.0), ("KEY_HOME", "Home", 1.0), ("KEY_PAGEUP", "PgUp", 1.0)],
    [("KEY_DELETE", "Del", 1.0), ("KEY_END", "End", 1.0), ("KEY_PAGEDOWN", "PgDn", 1.0)],
]
_ARROW_BLOCK = [
    [("", "", 1.0), ("KEY_UP", "↑", 1.0), ("", "", 1.0)],
    [("KEY_LEFT", "←", 1.0), ("KEY_DOWN", "↓", 1.0), ("KEY_RIGHT", "→", 1.0)],
]
_LOCK_STRIP = [("KEY_SCROLLLOCK", "Scroll Lock", 2.2), ("KEY_NUMLOCK", "Num Lock", 2.2)]
_MISC_STRIP = [("KEY_SYSRQ", "Print Screen", 2.6), ("KEY_PAUSE", "Pause / Break", 2.6)]
_MM_STRIP = [
    ("KEY_MUTE", "Mute", 1.6), ("KEY_VOLUMEDOWN", "Vol -", 1.6), ("KEY_VOLUMEUP", "Vol +", 1.6),
    ("KEY_PLAYPAUSE", "Play/Pause", 2.0), ("KEY_PREVIOUSSONG", "Prev", 1.6), ("KEY_NEXTSONG", "Next", 1.6),
    ("KEY_MICMUTE", "Mic Mute", 2.0),
]
# Round 2: physical mouse layout (Left/Middle/Right) with the two thumb
# buttons (Back/Forward) visually separated by a gap, not catalog order.
_MOUSE_STRIP = [
    ("BTN_LEFT", "Left", 1.8),
    ("BTN_MIDDLE", "Middle", 1.8),
    ("BTN_RIGHT", "Right", 1.8),
    ("", "", 0.6),
    ("BTN_SIDE", "Back", 1.8),
    ("BTN_EXTRA", "Forward", 1.8),
]


def _keycap_row(entries, on_pick, current: str) -> Gtk.Box:
    row = Gtk.Box(spacing=3)
    for code, label, width in entries:
        if not code:
            spacer = Gtk.Box()
            spacer.set_size_request(int(_UNIT_PX * width), _UNIT_PX)
            row.append(spacer)
            continue
        btn = Gtk.Button(label=label, css_classes=["keycap"])
        cls = key_css_class(code)
        if cls:
            btn.add_css_class(cls)
        if code == current:
            btn.add_css_class("suggested-action")
        btn.set_size_request(int(_UNIT_PX * width), _UNIT_PX)
        btn.connect("clicked", lambda b, code=code: on_pick(code))
        row.append(btn)
    return row


def _keyboard_grid(on_pick, current: str) -> Gtk.Widget:
    grid = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    grid.append(_keycap_row(_FN_ROW, on_pick, current))

    fn_hi_state = {"shown": False}
    fn_hi_row_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    show_hi_btn = Gtk.Button(label="Show F13-F24 ▸", halign=Gtk.Align.START)

    def toggle_hi(b):
        fn_hi_state["shown"] = not fn_hi_state["shown"]
        clear_children(fn_hi_row_slot)
        if fn_hi_state["shown"]:
            fn_hi_row_slot.append(_keycap_row(_FN_ROW_HI, on_pick, current))
            show_hi_btn.set_label("Hide F13-F24 ▾")
        else:
            show_hi_btn.set_label("Show F13-F24 ▸")

    show_hi_btn.connect("clicked", toggle_hi)
    grid.append(show_hi_btn)
    grid.append(fn_hi_row_slot)

    grid.append(Gtk.Box(css_classes=[], height_request=6))
    grid.append(_keycap_row(_NUM_ROW, on_pick, current))
    grid.append(_keycap_row(_QWERTY_ROW, on_pick, current))
    grid.append(_keycap_row(_HOME_ROW, on_pick, current))
    grid.append(_keycap_row(_BOTTOM_ROW, on_pick, current))
    grid.append(_keycap_row(_SPACE_ROW, on_pick, current))

    clusters = Gtk.Box(spacing=16)
    nav_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=3)
    nav_box.append(Gtk.Label(label="Navigation", xalign=0, css_classes=["section-label"]))
    for r in _NAV_BLOCK:
        nav_box.append(_keycap_row(r, on_pick, current))
    clusters.append(nav_box)

    arrow_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=3)
    arrow_box.append(Gtk.Label(label=" ", xalign=0, css_classes=["section-label"]))
    for r in _ARROW_BLOCK:
        arrow_box.append(_keycap_row(r, on_pick, current))
    clusters.append(arrow_box)

    lock_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=3)
    lock_box.append(Gtk.Label(label="Lock keys", xalign=0, css_classes=["section-label"]))
    lock_box.append(_keycap_row(_LOCK_STRIP, on_pick, current))
    lock_box.append(Gtk.Label(label="Misc", xalign=0, css_classes=["section-label"]))
    lock_box.append(_keycap_row(_MISC_STRIP, on_pick, current))
    clusters.append(lock_box)
    grid.append(clusters)

    grid.append(Gtk.Label(label="Multimedia", xalign=0, css_classes=["section-label"]))
    grid.append(_keycap_row(_MM_STRIP, on_pick, current))

    grid.append(Gtk.Label(label="Mouse buttons", xalign=0, css_classes=["section-label"]))
    grid.append(_keycap_row(_MOUSE_STRIP, on_pick, current))

    return grid


def build_inline_key_picker(field_label: str, current_code: str, on_change) -> Gtk.Widget:
    """Variant A's reusable picker: a collapsed summary button that expands
    into the full keyboard grid in place. Mounted twice in the host (Key
    field + Macro KeyDown value) to prove it's one component, not two."""
    state = {"code": current_code, "expanded": False}
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

    summary_row = Gtk.Box(spacing=8)
    summary_row.append(Gtk.Label(label=field_label, xalign=0))
    toggle_btn = Gtk.Button(hexpand=True)
    summary_row.append(toggle_btn)
    root.append(summary_row)

    warn_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    root.append(warn_slot)

    panel_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, css_classes=["picker-panel"])
    root.append(panel_slot)

    def render_warning():
        clear_children(warn_slot)
        if state["code"] in MODIFIER_CODES:
            warn_slot.append(build_modifier_warning())

    def render_summary():
        chevron = "▾" if state["expanded"] else "▸"
        toggle_btn.set_label(f"{LABEL_BY_CODE.get(state['code'], state['code'])}  {chevron} Change")

    def render_panel():
        clear_children(panel_slot)
        panel_slot.set_visible(state["expanded"])
        if state["expanded"]:
            panel_slot.append(_keyboard_grid(on_pick, state["code"]))

    def on_pick(code: str):
        state["code"] = code
        state["expanded"] = False
        render_summary()
        render_warning()
        render_panel()
        on_change(code)

    def on_toggle(b):
        state["expanded"] = not state["expanded"]
        render_summary()
        render_panel()

    toggle_btn.connect("clicked", on_toggle)
    render_summary()
    render_warning()
    render_panel()
    return root


# ---------------------------------------------------------------------
# Variant B — Popover Search + Category: a flat, filterable list that
# never grows wider than the editor itself.
# ---------------------------------------------------------------------

_CATEGORY_NAMES = ["All"] + list(KEY_CATEGORIES.keys())


def build_popover_key_picker(field_label: str, current_code: str, on_change) -> Gtk.Widget:
    """Variant B's reusable picker: a MenuButton opening a search+category
    popover. Mounted twice in the host (Key field + Macro KeyDown value)."""
    state = {"code": current_code}
    row = Gtk.Box(spacing=8)
    row.append(Gtk.Label(label=field_label, xalign=0))

    menu_btn = Gtk.MenuButton(hexpand=True)
    row.append(menu_btn)

    warn_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)

    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    root.append(row)
    root.append(warn_slot)

    def render_button():
        menu_btn.set_label(f"Choose key: {LABEL_BY_CODE.get(state['code'], state['code'])} ▾")

    def render_warning():
        clear_children(warn_slot)
        if state["code"] in MODIFIER_CODES:
            warn_slot.append(build_modifier_warning())

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
        return KEY_CATEGORIES[cat_name]

    def render_results():
        clear_children(results)
        for code, label in matching_entries():
            row_box = Gtk.Box(spacing=6, css_classes=["category-row"])
            cls = key_css_class(code)
            dot = Gtk.Label(label="●", css_classes=[cls] if cls else [])
            row_box.append(dot)
            row_box.append(Gtk.Label(label=label, hexpand=True, xalign=0))
            btn = Gtk.Button(label="Use")

            def on_use(b, code=code):
                state["code"] = code
                render_button()
                render_warning()
                popover.popdown()
                on_change(code)

            btn.connect("clicked", on_use)
            row_box.append(btn)
            list_row = Gtk.ListBoxRow(child=row_box)
            results.append(list_row)

    search.connect("search-changed", lambda e: render_results())
    cat_dd.connect("notify::selected", lambda *_: render_results())

    render_button()
    render_warning()
    render_results()
    return root


# ---------------------------------------------------------------------
# Host mocks — each variant is mounted in a "Binding editor" section
# (Key field + Modifiers checkboxes, matching binding_editor.py's real
# render_action_editor shape) and a "Macro step editor" section (a
# KeyDown step's value), to demonstrate the same picker answering both.
# ---------------------------------------------------------------------


def _modifiers_row(active: set[str]) -> Gtk.Widget:
    box = Gtk.Box(spacing=8)
    for m in ("ctrl", "shift", "alt", "super"):
        cb = Gtk.CheckButton(label=m)
        cb.set_active(m in active)
        box.append(cb)
    return box


def build_host(build_picker) -> Gtk.Widget:
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    root.set_margin_top(12)
    root.set_margin_bottom(12)
    root.set_margin_start(12)
    root.set_margin_end(12)

    binding_pane = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["editor-pane"])
    binding_pane.append(Gtk.Label(label="Binding editor (mock) — Default / base / Grid 7", xalign=0, css_classes=["heading"]))
    trig_row = Gtk.Box(spacing=8)
    trig_row.append(Gtk.Label(label="Trigger mode"))
    trig_row.append(Gtk.DropDown(model=Gtk.StringList.new(["Fire-once", "Hold-to-repeat", "Toggle"])))
    binding_pane.append(trig_row)
    binding_pane.append(build_picker("Key", "KEY_A", lambda code: None))
    binding_pane.append(_modifiers_row({"ctrl"}))
    root.append(binding_pane)

    macro_pane = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["editor-pane"])
    macro_pane.append(Gtk.Label(label="Macro step editor (mock) — reusing the same picker", xalign=0, css_classes=["heading"]))
    macro_pane.append(Gtk.Label(label="Step 1: KeyDown", xalign=0, css_classes=["dim"]))
    macro_pane.append(build_picker("Value", "KEY_LEFTCTRL", lambda code: None))
    root.append(macro_pane)

    root.append(
        Gtk.Label(
            label=(
                "Stepper library items (ticket 31) reuse this same picker unchanged — not "
                "separately mocked here, since it's structurally identical to the Macro "
                "KeyDown/KeyUp value above (a single fire-once key or mouse-button)."
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
    ("A", "Inline Keyboard Panel · expands a real keyboard shape in place"),
    ("B", "Popover Search + Category · flat filterable list, stays small"),
]


def build_window(app: Gtk.Application) -> Gtk.ApplicationWindow:
    provider = Gtk.CssProvider()
    provider.load_from_data(CSS.encode())
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
    )

    win = Gtk.ApplicationWindow(application=app, title="Ticket 32 prototype — key/mouse-button picker UX")
    # Round 2: no forced width — never scroll horizontally, so the window's
    # natural size is exactly what the keyboard grid needs (variant A, shown
    # first) rather than an arbitrary wide default. Height still scrolls.
    win.set_default_size(-1, 660)

    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
    win.set_child(outer)

    scroller = Gtk.ScrolledWindow(vexpand=True, hexpand=True)
    scroller.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
    # ScrolledWindow doesn't propagate its child's natural width by default
    # (GTK4 gotcha) — without this the window falls back to a wider-than-
    # needed size regardless of set_default_size(-1, ...). Height stays
    # un-propagated so it still scrolls instead of growing to full content.
    scroller.set_propagate_natural_width(True)
    outer.append(scroller)

    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    scroller.set_child(content)

    index = {"i": 0}

    def render():
        clear_children(content)
        key, label = VARIANTS[index["i"]]
        if key == "A":
            content.append(build_host(build_inline_key_picker))
        else:
            content.append(build_host(build_popover_key_picker))
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
    app = Gtk.Application(application_id="com.acheron.prototype.ticket32")
    app.connect("activate", lambda a: build_window(a).present())
    app.run(None)


if __name__ == "__main__":
    main()
