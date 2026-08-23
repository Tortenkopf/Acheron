"""The real key/mouse-button picker (ticket 42), replacing `binding_editor.py`'s
free-text `Gtk.Entry` key field. Ports ticket 32's winning variant A — Inline
Keyboard Panel — from `prototype/32-key-mouse-button-picker-ux` almost
unchanged: a collapsed "`<key label>` ▸ Change" summary button that expands
into a full graphical keyboard grid in place.

The GUI has no binding to evdev's `KeyCode` enum (confirmed while scoping this
ticket: no `python-evdev` import anywhere in `gui/`, and no D-Bus method
enumerates valid keys — the Daemon's own `all_injectable_key_codes()` just
advertises the raw `0..=KEY_MAX` range, no curated list). So, like the
prototype, this is a hand-maintained catalog — but every code below is
cross-checked against the real `evdev` crate's `scancodes.rs` (the same
strings `evdev::KeyCode`'s `FromStr`/`Display` round-trip), not invented.

One component serves both a Binding's Keypress `key` field and a Macro step's
KeyDown/KeyUp value (`build_inline_key_picker`) — the caller supplies the
field's own `labeled_row` wrapper, since this component only owns the
toggle/warning/keyboard-panel, not the row label.
"""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk

from .gtk_utils import clear_children

# ---------------------------------------------------------------------
# Catalog — ticket 02's settled "no exclusions" scope: letters/digits,
# modifiers, F1-F24, navigation cluster, lock keys, misc, multimedia, plus
# the five mouse buttons. Nice display names here (used for the collapsed
# summary label); the grid below uses its own compact per-key labels sized
# to fit a real keyboard shape.
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
_MODIFIERS_NICE = [
    ("KEY_LEFTCTRL", "Left Ctrl"), ("KEY_RIGHTCTRL", "Right Ctrl"),
    ("KEY_LEFTSHIFT", "Left Shift"), ("KEY_RIGHTSHIFT", "Right Shift"),
    ("KEY_LEFTALT", "Left Alt"), ("KEY_RIGHTALT", "Right Alt"),
    ("KEY_LEFTMETA", "Left Super"), ("KEY_RIGHTMETA", "Right Super"),
]
_FUNCTION_NICE = [(f"KEY_F{i}", f"F{i}") for i in range(1, 25)]
_NAVIGATION_NICE = [
    ("KEY_UP", "Up"), ("KEY_DOWN", "Down"), ("KEY_LEFT", "Left"), ("KEY_RIGHT", "Right"),
    ("KEY_HOME", "Home"), ("KEY_END", "End"), ("KEY_PAGEUP", "Page Up"), ("KEY_PAGEDOWN", "Page Down"),
    ("KEY_INSERT", "Insert"), ("KEY_DELETE", "Delete"),
]
_LOCK_NICE = [("KEY_CAPSLOCK", "Caps Lock"), ("KEY_SCROLLLOCK", "Scroll Lock"), ("KEY_NUMLOCK", "Num Lock")]
_MISC_NICE = [
    ("KEY_TAB", "Tab"), ("KEY_ESC", "Esc"), ("KEY_ENTER", "Enter"), ("KEY_SPACE", "Space"),
    ("KEY_BACKSPACE", "Backspace"), ("KEY_SYSRQ", "Print Screen"), ("KEY_PAUSE", "Pause / Break"),
    ("KEY_MENU", "Menu"), ("KEY_GRAVE", "` ~"), ("KEY_MINUS", "- _"), ("KEY_EQUAL", "= +"),
    ("KEY_LEFTBRACE", "[ {"), ("KEY_RIGHTBRACE", "] }"), ("KEY_BACKSLASH", "\\ |"),
    ("KEY_SEMICOLON", "; :"), ("KEY_APOSTROPHE", "' \""), ("KEY_COMMA", ", <"),
    ("KEY_DOT", ". >"), ("KEY_SLASH", "/ ?"),
]
_MULTIMEDIA_NICE = [
    ("KEY_MUTE", "Mute"), ("KEY_VOLUMEDOWN", "Vol -"), ("KEY_VOLUMEUP", "Vol +"),
    ("KEY_PLAYPAUSE", "Play / Pause"), ("KEY_PREVIOUSSONG", "Prev Track"), ("KEY_NEXTSONG", "Next Track"),
    ("KEY_MICMUTE", "Mic Mute"),
]
_MULTIMEDIA_CODES = {code for code, _ in _MULTIMEDIA_NICE}

# Ticket 65: the core 17 numpad keys — distinct evdev KeyCodes from their
# main-block twins (KEY_KP1 vs KEY_1), so labels disambiguate the same way
# _MODIFIERS_NICE already disambiguates Left/Right Ctrl. Obscure JIS/Mac/
# calculator-style extras (KEY_KPEQUAL, KEY_KPCOMMA, …) excluded per ticket
# 64's Answer — no physical numpad exposes them.
_NUMPAD_NICE = [(f"KEY_KP{d}", f"Num {d}") for d in "0123456789"] + [
    ("KEY_KPDOT", "Num ."),
    ("KEY_KPENTER", "Num Enter"),
    ("KEY_KPPLUS", "Num +"),
    ("KEY_KPMINUS", "Num -"),
    ("KEY_KPASTERISK", "Num *"),
    ("KEY_KPSLASH", "Num /"),
]

_ALL_ENTRIES: list[tuple[str, str]] = (
    _LETTERS + _DIGITS + _MODIFIERS_NICE + _FUNCTION_NICE + _NAVIGATION_NICE
    + _LOCK_NICE + _MISC_NICE + _MULTIMEDIA_NICE + _NUMPAD_NICE + MOUSE_BUTTONS
)
LABEL_BY_CODE = {code: label for code, label in _ALL_ENTRIES}


def key_css_class(code: str) -> str | None:
    if code in MODIFIER_CODES:
        return "keycap-mod"
    if code.startswith("BTN_"):
        return "keycap-mouse"
    if code in _MULTIMEDIA_CODES:
        return "keycap-mm"
    return None


def build_modifier_warning() -> Gtk.Widget:
    # max_width_chars caps the label's *natural* size request — a wrapping
    # Gtk.Label otherwise asks for its full unwrapped line width, which
    # ticket 32's round 4 found silently inflates the host's real width
    # once anything upstream propagates natural width.
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
# The keyboard grid — real keyboard shape, sized in units of _UNIT_PX.
# Rows: (code, label, width-in-units). code == "" marks a blank spacer.
# ---------------------------------------------------------------------

_UNIT_PX = 28.5  # ticket 32 round 3's settled size (30px -> 5% shrink)


def _letter_row_entries(letters: str, width: float = 1.0) -> list[tuple[str, str, float]]:
    """Shared shape for a keyboard row's letter span: code must be the
    uppercase evdev name (`KEY_A`, not `KEY_a`) — ticket 44 found one of these
    inlined with a lowercase `code` (label uppercased, code left lowercase),
    which the real Daemon correctly rejected. Centralized so that bug class
    can't recur per-row.
    """
    return [(f"KEY_{c.upper()}", c.upper(), width) for c in letters]


_FN_ROW = [("KEY_ESC", "Esc", 1.4)] + [(f"KEY_F{i}", f"F{i}", 1.0) for i in range(1, 13)]
_FN_ROW_HI = [(f"KEY_F{i}", f"F{i}", 1.0) for i in range(13, 25)]
_NUM_ROW = (
    [("KEY_GRAVE", "`", 1.0)]
    + [(f"KEY_{d}", d, 1.0) for d in "1234567890"]
    + [("KEY_MINUS", "-", 1.0), ("KEY_EQUAL", "=", 1.0), ("KEY_BACKSPACE", "⌫", 2.0)]
)
_QWERTY_ROW = (
    [("KEY_TAB", "Tab", 1.5)]
    + _letter_row_entries("qwertyuiop")
    + [("KEY_LEFTBRACE", "[", 1.0), ("KEY_RIGHTBRACE", "]", 1.0), ("KEY_BACKSLASH", "\\", 1.5)]
)
_HOME_ROW = (
    [("KEY_CAPSLOCK", "Caps", 1.8)]
    + _letter_row_entries("asdfghjkl")
    + [("KEY_SEMICOLON", ";", 1.0), ("KEY_APOSTROPHE", "'", 1.0), ("KEY_ENTER", "Enter", 2.2)]
)
_BOTTOM_ROW = (
    [("KEY_LEFTSHIFT", "Shift", 2.3)]
    + _letter_row_entries("zxcvbnm")
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
# Ticket 65: physical numpad shape. The top row (/, *, -) is a plain
# _keycap_row strip; the 4x4 block below needs real row/col spans (+ spans
# two rows, Enter spans two rows, 0 spans two columns) that _keycap_row's
# single-row Gtk.Box can't express, so it's laid out on a Gtk.Grid instead
# — see _numpad_block(). Cells: (code, label, col, row, col_span, row_span).
_NUMPAD_TOP_ROW = [("KEY_KPSLASH", "Num /", 1.0), ("KEY_KPASTERISK", "Num *", 1.0), ("KEY_KPMINUS", "Num -", 1.0)]
_NUMPAD_GRID_CELLS = [
    ("KEY_KP7", "Num 7", 0, 0, 1, 1), ("KEY_KP8", "Num 8", 1, 0, 1, 1), ("KEY_KP9", "Num 9", 2, 0, 1, 1),
    ("KEY_KPPLUS", "Num +", 3, 0, 1, 2),
    ("KEY_KP4", "Num 4", 0, 1, 1, 1), ("KEY_KP5", "Num 5", 1, 1, 1, 1), ("KEY_KP6", "Num 6", 2, 1, 1, 1),
    ("KEY_KP1", "Num 1", 0, 2, 1, 1), ("KEY_KP2", "Num 2", 1, 2, 1, 1), ("KEY_KP3", "Num 3", 2, 2, 1, 1),
    ("KEY_KPENTER", "Num Enter", 3, 2, 1, 2),
    ("KEY_KP0", "Num 0", 0, 3, 2, 1), ("KEY_KPDOT", "Num .", 2, 3, 1, 1),
]

# ticket 32 round 2: physical mouse layout (Left/Middle/Right) with the two
# thumb buttons (Back/Forward) visually separated by a gap, not catalog order.
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
            spacer.set_size_request(int(_UNIT_PX * width), int(_UNIT_PX))
            row.append(spacer)
            continue
        btn = Gtk.Button(label=label, css_classes=["keycap"])
        cls = key_css_class(code)
        if cls:
            btn.add_css_class(cls)
        if code == current:
            btn.add_css_class("suggested-action")
        btn.set_size_request(int(_UNIT_PX * width), int(_UNIT_PX))
        btn.connect("clicked", lambda b, code=code: on_pick(code))
        row.append(btn)
    return row


def _numpad_block(on_pick, current: str) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=3)
    box.append(_keycap_row(_NUMPAD_TOP_ROW, on_pick, current))

    grid = Gtk.Grid(row_spacing=3, column_spacing=3)
    for code, label, col, row, col_span, row_span in _NUMPAD_GRID_CELLS:
        btn = Gtk.Button(label=label, css_classes=["keycap"])
        if code == current:
            btn.add_css_class("suggested-action")
        width = _UNIT_PX * col_span + 3 * (col_span - 1)
        height = _UNIT_PX * row_span + 3 * (row_span - 1)
        btn.set_size_request(int(width), int(height))
        btn.connect("clicked", lambda b, code=code: on_pick(code))
        grid.attach(btn, col, row, col_span, row_span)
    box.append(grid)
    return box


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

    grid.append(Gtk.Box(height_request=6))
    grid.append(_keycap_row(_NUM_ROW, on_pick, current))
    grid.append(_keycap_row(_QWERTY_ROW, on_pick, current))
    grid.append(_keycap_row(_HOME_ROW, on_pick, current))
    grid.append(_keycap_row(_BOTTOM_ROW, on_pick, current))
    grid.append(_keycap_row(_SPACE_ROW, on_pick, current))

    numpad_state = {"shown": False}
    numpad_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    show_numpad_btn = Gtk.Button(label="Show Numpad ▸", halign=Gtk.Align.START)

    def toggle_numpad(b):
        numpad_state["shown"] = not numpad_state["shown"]
        clear_children(numpad_slot)
        if numpad_state["shown"]:
            numpad_slot.append(_numpad_block(on_pick, current))
            show_numpad_btn.set_label("Hide Numpad ▾")
        else:
            show_numpad_btn.set_label("Show Numpad ▸")

    show_numpad_btn.connect("clicked", toggle_numpad)
    grid.append(show_numpad_btn)
    grid.append(numpad_slot)

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


def build_inline_key_picker(
    current_code: str,
    on_change: Callable[[str], None],
    warn_predicate: Callable[[], bool] = lambda: True,
) -> tuple[Gtk.Widget, Callable[[], None]]:
    """Returns `(widget, refresh_warning)`. `widget` is the current-selection
    label plus the full keyboard grid, always shown inline (no collapse/
    expand toggle); wrap it in the caller's own `labeled_row(label, widget)`
    for the field label, since this component is mounted twice (Binding
    "Key", Macro step "Value") under two different labels.

    Ticket 44 (live-verified on real hardware): ticket 42's original shape
    was a collapsed "<label> ▸ Change" summary button that expanded the grid
    in place inside the *outer* per-key Binding-editor Popover. That grow-
    in-place resize silently failed for nearly every Device Overview grid
    position on GTK4/Wayland (the compositor's xdg_popup positioner is
    computed once at first show and can't be resatisfied for a much bigger
    size afterward — confirmed independent of Gtk.Popover autohide, which
    made no difference). A follow-up attempt nesting a second Gtk.Popover
    off a Gtk.MenuButton also live-verified broken: it rendered the grid's
    full content correctly, but positioned it at the window's origin instead
    of anchored to the toggle, a nested-popover-positioning bug on the same
    Wayland stack. Always showing the grid inline sidesteps both failure
    modes: the outer popover's size is fixed from its own very first render,
    never resized afterward.

    `warn_predicate` gates ticket 02's bare-modifier warning on ticket 42's
    added "Trigger mode isn't Toggle" condition — external state this
    component has no visibility into on its own. The caller must invoke the
    returned `refresh_warning` whenever that external state changes (the
    Trigger-mode dropdown), since picking a key is the only event this
    component reacts to by itself.
    """
    state = {"code": current_code}
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

    summary_label = Gtk.Label(xalign=0, css_classes=["key-picker-summary"])
    root.append(summary_label)

    warn_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    root.append(warn_slot)

    grid_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, css_classes=["picker-panel"])
    root.append(grid_slot)

    def render_warning():
        clear_children(warn_slot)
        if state["code"] in MODIFIER_CODES and warn_predicate():
            warn_slot.append(build_modifier_warning())

    def render_summary():
        summary_label.set_label(f"Selected: {LABEL_BY_CODE.get(state['code'], state['code'])}")

    def render_grid():
        clear_children(grid_slot)
        grid_slot.append(_keyboard_grid(on_pick, state["code"]))

    def on_pick(code: str):
        state["code"] = code
        render_summary()
        render_warning()
        render_grid()
        on_change(code)

    render_summary()
    render_warning()
    render_grid()
    return root, render_warning
