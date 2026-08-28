"""PROTOTYPE — throwaway, answers ticket 87 (Prototype fixed-size Device
Overview buttons):
.scratch/tartarus-input-expansion/issues/87-prototype-fixed-size-device-overview-buttons.md

Plan: three structurally different answers to "what happens when a fixed-size
button's label doesn't fit", switchable via a floating bottom pill (same GTK4
adaptation of the skill's `?variant=` convention as tickets 06/19/30/31/32/
38/47 — Prev/Next buttons plus Left/Right arrow keys). Reuses the real
`input_label`/`action_summary`/`INPUT_DEFAULT_LABEL` content-generation
functions and the real `.bound`/`.empty`/`.mode-key` CSS classes, driven by
the real `DaemonStub` seeded with realistic Bindings — including ticket 06's
own worst-case strings — so every variant is judged against real content,
not lorem-ipsum. `make_input_button` itself isn't reused: today's "floor,
not cap" sizing (`set_size_request` alone) is exactly the thing being
replaced, so each variant provides its own `build_button` with a genuine
width+height cap.

Sizing is the settled part (confirm live, don't re-litigate unless something
looks actually wrong):
    Grid keys / wheel buttons / thumbstick diamond lobes  -> 100x100
    Key 20 (the wider hardware paddle)                    -> 150x100
    Mode key                                               -> 52x40, UNCHANGED
        (not mentioned when the user enumerated what grows — flagged with a
        dashed outline + a small "unchanged?" tag in every variant so it's
        impossible to miss during live review, per this ticket's own
        "flag rather than silently assume" instruction)
Positions relative to each other are unchanged from the real
`build_main_view` assembly (grid + wheel column, diamond + Mode key + key-20
in a side column) — only the box sizes grew, which is why the window itself
needs to start bigger to fit them (the user's own expectation going in).

The open question this prototype exists to answer: once both dimensions are
a genuine cap (not a floor), what happens to label text that doesn't fit?
Ticket 06 tried a width cap (`max-width-chars`) once already and rejected it
live — paired only with wrapping, it mid-word-split ordinary short labels
("passthrough" -> "passthr"/"ough"). All three variants below fix that by
also giving the label a genuine line limit + ellipsis fallback
(`Gtk.Label.set_lines()` + `set_ellipsize()`), which GTK applies *after*
ordinary word/char wrapping rather than instead of it — the missing half of
ticket 06's own attempt.

Wipe me: nothing here persists past process exit (DaemonStub is in-memory);
`build_button` in each variant is a prototype sketch, not something to
import into `device_overview.py` as-is — the winning variant's approach gets
rewritten properly into a real `make_input_button` change by this ticket's
own follow-up build ticket.

Run:
    python3 gui/prototype_87_fixed_size_device_overview_buttons.py

Variants:
    A — Tight ellipsis: every button is capped at a snug character width (a
        little narrower than the box's true natural width), so ordinary
        content sometimes ellipsizes too, not just the pathological cases.
        Every truncated label carries a tooltip with the untruncated text.
        Tests "uniform and predictable, lean on hover for detail."
    B — Auto-shrink font: no aggressive width cap; instead the label's own
        font size steps down (three sizes) as its text gets longer, so
        longer content stays fully visible just smaller, rather than being
        cut off. A line-limit + ellipsize safety net still guards the
        smallest size against the genuinely pathological case. Tests
        "never hide text, but let it get small."
    C — Wrap-then-ellipsize (hybrid): width cap set loose enough that
        ordinary short content (including today's "passthrough"-style
        defaults) wraps and fits *without* ellipsizing at all — only the
        genuinely long worst-case strings (a 4-modifier chord, a long
        keycode plus a Trigger-mode tag) hit the ellipsis fallback. Tests
        "don't degrade the common case chasing the rare one."
"""

from __future__ import annotations

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gio, GLib, Gtk, Pango

from .binding_editor import action_summary
from .daemon_stub import DaemonStub
from .gtk_utils import clear_children
from .inputs import GRID_COLS, GRID_ROWS, grid_input, input_label

# Real CSS classes (device_overview.py/app.py) plus this prototype's own
# fixed-box/flag/switcher chrome.
CSS = """
.bound { border: 2px solid #4caf50; }
.empty { opacity: 0.75; }
.mode-key { border-radius: 999px; }
.fixed-btn { padding: 2px; }
.dim { opacity: 0.65; font-size: smaller; }
.switcher-pill { padding: 6px 10px; background-color: alpha(currentColor, 0.08); border-radius: 999px; }
.variant-label { font-weight: bold; }
"""

GRID_SIZE = (100, 100)
KEY20_SIZE = (150, 100)
# Settled on live reaction to round 1: the Mode key is sized like every
# other button now (previously flagged, not assumed) — its `.mode-key` CSS
# (`border-radius: 999px`) only renders a true circle when width == height,
# so making it square-footprint at GRID_SIZE is what actually gives it a
# genuine circle instead of the oval ticket 06 flagged but didn't fix.
MODE_SIZE = GRID_SIZE


def seed_daemon() -> DaemonStub:
    client = DaemonStub()
    # Ticket 06's own worst cases, reused verbatim so this prototype is
    # judged against the same pathological strings that broke the earlier
    # attempt, not new ones invented for this ticket.
    client.set_binding(
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "keypress", "key": "KEY_F12", "modifiers": ["ctrl", "shift", "alt", "super"]},
    )
    client.set_binding(
        "grid_r1c2",
        "base",
        {
            "trigger": "hold_to_repeat",
            "type": "keypress",
            "key": "KEY_KBDILLUMTOGGLE",
            "modifiers": ["ctrl", "shift", "alt", "super"],
        },
    )
    # Ordinary short content — the case a width cap must not degrade.
    client.set_binding("grid_r1c3", "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_A", "modifiers": ["ctrl"]})
    client.set_binding("grid_r2c1", "base", {"trigger": "toggle", "type": "keypress", "key": "KEY_CAPSLOCK", "modifiers": []})
    client.set_binding(
        "grid_r4c5",  # key 20, the wider paddle
        "base",
        {"trigger": "toggle", "type": "keypress", "key": "KEY_F5", "modifiers": ["ctrl", "shift"]},
    )
    # Everything else stays unbound, exercising the real INPUT_DEFAULT_LABEL
    # passthrough-style content ("Q", "Tab", "Scroll", "Middle Click", ...).
    return client


def label_text(config: dict, profile: str, layer: str, inp: str) -> str:
    binding = config["profiles"][profile][layer].get(inp)
    axis_target = config["profiles"][profile][f"axis_{layer}"].get(inp)
    return (
        f"{input_label(inp)}\n"
        f"{action_summary(binding, inp, config.get('macros', {}), config.get('steppers', {}), axis_target)}"
    )


def label_markup(config: dict, profile: str, layer: str, inp: str) -> str:
    """Same two lines as `label_text`, but the Input's own label (its grid
    number / "Mode" / arrow glyph) is bold — settled on live reaction to
    round 1: distinguishes *which Input this is* from *what it does*, the
    same two pieces of information the plain-text version conflated."""
    binding = config["profiles"][profile][layer].get(inp)
    axis_target = config["profiles"][profile][f"axis_{layer}"].get(inp)
    top = GLib.markup_escape_text(input_label(inp))
    bottom = GLib.markup_escape_text(
        action_summary(binding, inp, config.get("macros", {}), config.get("steppers", {}), axis_target)
    )
    return f"<b>{top}</b>\n{bottom}"


def styled_button(config: dict, profile: str, layer: str, inp: str, w: int, h: int, inner: Gtk.Label) -> Gtk.Button:
    binding = config["profiles"][profile][layer].get(inp)
    btn = Gtk.Button(css_classes=["fixed-btn"])
    btn.set_child(inner)
    btn.set_size_request(w, h)
    btn.set_halign(Gtk.Align.CENTER)
    btn.set_hexpand(False)
    btn.set_vexpand(False)
    btn.add_css_class("bound" if binding else "empty")
    if inp == "mode_key":
        # Square footprint (MODE_SIZE == GRID_SIZE) + this CSS class is what
        # actually renders a circle — see MODE_SIZE's own comment.
        btn.add_css_class("mode-key")
    return btn


# --- Variant A: tight ellipsis + tooltip -----------------------------------


def build_button_a(config: dict, profile: str, layer: str, inp: str, w: int, h: int) -> Gtk.Button:
    inner = Gtk.Label(justify=Gtk.Justification.CENTER)
    inner.set_markup(label_markup(config, profile, layer, inp))
    inner.set_wrap(True)
    inner.set_wrap_mode(Pango.WrapMode.WORD_CHAR)
    # Snug on purpose — narrower than the box's true natural width, so
    # ordinary content sometimes wraps/truncates too, not just extreme cases.
    inner.set_max_width_chars(8 if w <= 100 else 14)
    inner.set_lines(3)
    inner.set_ellipsize(Pango.EllipsizeMode.END)
    inner.set_tooltip_text(label_text(config, profile, layer, inp).replace("\n", "  "))
    return styled_button(config, profile, layer, inp, w, h, inner)


# --- Variant B: auto-shrink font --------------------------------------------


def build_button_b(config: dict, profile: str, layer: str, inp: str, w: int, h: int) -> Gtk.Button:
    text = label_text(config, profile, layer, inp)
    # Three steps down from GTK's ~11pt default, keyed off raw length —
    # crude but enough to test "shrink instead of hide" as a concept; a real
    # implementation would measure actual layout width instead of len().
    n = len(text)
    pt = 11 if n <= 14 else 9 if n <= 22 else 7
    inner = Gtk.Label(justify=Gtk.Justification.CENTER)
    inner.set_markup(f'<span size="{pt * 1024}">{label_markup(config, profile, layer, inp)}</span>')
    inner.set_wrap(True)
    inner.set_wrap_mode(Pango.WrapMode.WORD_CHAR)
    # Safety net even at the smallest step: a hand-edited config.toml could
    # still produce something absurd (ticket 06's own reasoning for never
    # trusting "seems to be the maximum").
    inner.set_max_width_chars(16 if w <= 100 else 22)
    inner.set_lines(4)
    inner.set_ellipsize(Pango.EllipsizeMode.END)
    if n > 22:
        inner.set_tooltip_text(text.replace("\n", "  "))
    return styled_button(config, profile, layer, inp, w, h, inner)


# --- Variant C: wrap-then-ellipsize (hybrid) --------------------------------


def build_button_c(config: dict, profile: str, layer: str, inp: str, w: int, h: int) -> Gtk.Button:
    inner = Gtk.Label(justify=Gtk.Justification.CENTER)
    inner.set_markup(label_markup(config, profile, layer, inp))
    inner.set_wrap(True)
    inner.set_wrap_mode(Pango.WrapMode.WORD_CHAR)
    # Looser than Variant A — close to what the box can actually hold at the
    # default font, so ordinary content (including "passthrough"-style
    # defaults) wraps and fits without ever hitting the ellipsis.
    inner.set_max_width_chars(13 if w <= 100 else 20)
    inner.set_lines(4 if w <= 100 else 3)
    inner.set_ellipsize(Pango.EllipsizeMode.END)
    inner.set_tooltip_text(label_text(config, profile, layer, inp).replace("\n", "  "))
    return styled_button(config, profile, layer, inp, w, h, inner)


BUILDERS = {"A": build_button_a, "B": build_button_b, "C": build_button_c}


def build_device(builder, config: dict, profile: str, layer: str) -> Gtk.Widget:
    gw, gh = GRID_SIZE
    device = Gtk.Box(spacing=28)

    grid = Gtk.Grid(row_spacing=4, column_spacing=4)
    for r in range(1, GRID_ROWS + 1):
        cols = GRID_COLS if r < GRID_ROWS else GRID_COLS - 1
        for c in range(1, cols + 1):
            grid.attach(builder(config, profile, layer, grid_input(r, c), gw, gh), c - 1, r - 1, 1, 1)
    wheel_col = GRID_COLS - 1
    grid.attach(builder(config, profile, layer, "wheel_scroll_up", gw, gh), wheel_col, GRID_ROWS - 1, 1, 1)
    grid.attach(builder(config, profile, layer, "wheel_middle", gw, gh), wheel_col, GRID_ROWS, 1, 1)
    grid.attach(builder(config, profile, layer, "wheel_scroll_down", gw, gh), wheel_col, GRID_ROWS + 1, 1, 1)
    device.append(grid)

    stick_col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=18, halign=Gtk.Align.CENTER, valign=Gtk.Align.START)
    mw, mh = MODE_SIZE
    stick_col.append(builder(config, profile, layer, "mode_key", mw, mh))

    diamond = Gtk.Grid(row_spacing=2, column_spacing=2)
    diamond.attach(builder(config, profile, layer, "thumbstick_left", gw, gh), 1, 0, 1, 1)
    diamond.attach(builder(config, profile, layer, "thumbstick_down", gw, gh), 0, 1, 1, 1)
    diamond.attach(builder(config, profile, layer, "thumbstick_up", gw, gh), 2, 1, 1, 1)
    diamond.attach(builder(config, profile, layer, "thumbstick_right", gw, gh), 1, 2, 1, 1)
    stick_col.append(diamond)

    kw, kh = KEY20_SIZE
    stick_col.append(builder(config, profile, layer, grid_input(4, 5), kw, kh))
    device.append(stick_col)
    return device


VARIANTS = [
    ("A", "Tight ellipsis + tooltip — uniform, lean on hover for detail"),
    ("B", "Auto-shrink font — never hide text, let it get small"),
    ("C", "Wrap-then-ellipsize (hybrid) — only the pathological case truncates"),
]


def build_window(app: Gtk.Application) -> Gtk.ApplicationWindow:
    provider = Gtk.CssProvider()
    provider.load_from_data(CSS.encode())
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
    )

    win = Gtk.ApplicationWindow(application=app, title="Ticket 87 prototype — fixed-size Device Overview buttons")
    win.set_default_size(760, 640)

    client = seed_daemon()
    config = client.get_config()
    profile = config["active_profile"]
    layer = "base"

    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
    win.set_child(outer)

    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    for setter in (content.set_margin_top, content.set_margin_bottom, content.set_margin_start, content.set_margin_end):
        setter(12)
    outer.append(content)

    hint = Gtk.Label(
        label=(
            "Grid/wheel/diamond/Mode key fixed at 100×100 (square footprint gives the "
            "Mode key a true circle), key 20 at 150×100. Round 2: variant A, bold Input labels."
        ),
        css_classes=["dim"],
        wrap=True,
    )
    content.append(hint)

    index = {"i": 0}

    def render():
        clear_children(content)
        content.append(hint)
        key, _label = VARIANTS[index["i"]]
        content.append(build_device(BUILDERS[key], config, profile, layer))
        variant_label.set_label(f"{key} — {VARIANTS[index['i']][1]}")

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
    # NON_UNIQUE: this prototype gets relaunched repeatedly during live
    # iteration — GTK's default unique-app-id behavior would otherwise just
    # re-present a stale already-running window (old code) instead of
    # starting a fresh process, which is confusing mid-review.
    app = Gtk.Application(application_id="com.acheron.prototype.ticket87", flags=Gio.ApplicationFlags.NON_UNIQUE)
    app.connect("activate", lambda a: build_window(a).present())
    app.run(None)


if __name__ == "__main__":
    main()
