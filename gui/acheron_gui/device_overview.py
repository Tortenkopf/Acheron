# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Device Overview — the GUI's one main view, mirroring the physical
Tartarus Pro layout exactly as built and settled in ticket 09's prototype
(`prototype/09-gui-information-architecture/prototype.py`): a 4x5 grid (row
4 four-wide), the wheel as a column-5 continuation, the thumbstick as a
diamond rotated 90° clockwise, a circular Mode key above it, and key 20 as
a separate paddle below it. Clicking any control opens the shared Binding
editor (`binding_editor.build_binding_editor`) in a popover.

Ticket 48 replaced the old permanent grid+Action-Table-sidebar layout with
a Grid/Library destination switcher (`build_destination_switch`); the
Action Table is cut outright (superseded by ticket 42's inline key/
mouse-button picker). The Grid destination keeps the real `build_layer_bar`
above the grid, plus an always-visible slot beside it (`build_chords_section`,
ticket 40) holding the real Chord-recording flow.

Column 1 — `build_profile_sidebar`'s slot in `build_main_view` — is
destination-dependent as of ticket 69/70, superseding ticket 48's original
"Profile sidebar stays exactly as it is, in both destinations": Grid shows
the Profile sidebar exactly as before; Library shows
`library_view.build_library_sidebar` instead — the Steppers/Macros tab row
plus the selected panel's browse list, full swap, no "Profiles" chrome.
Both share `gtk_utils.build_pinned_sidebar_box`'s fixed 220px width so
nothing visibly resizes when flipping destinations — Profile switching is
simply unreachable while Library is showing (Macros/Steppers are
Profile-agnostic, ticket 69's Answer). The rest of the Library destination
— the selected item's name/steps-or-items column plus its editor controls —
is `library_view.build_library_content` (ticket 52 for Macros, ticket 55
for Steppers, reorganized into three columns by ticket 70).

Ticket 40's Chord recording (settled in the prototype's variant A, round 3
— `.scratch/tartarus-input-expansion/issues/30-prototype-chord-recording-ux.md`):
a "Select Chord members" toggle in the Chords section is what changes what a
device click does — off (the default) it opens the ordinary per-Input
Binding editor exactly as always; on, clicking any Input (grid, thumbstick,
Mode key, or wheel — a Chord's members are open-ended, not Grid-only) toggles
it into the in-progress selection instead, per `ui_state["chord"]`. A
"Binding →" button enables once ≥2 Inputs are selected and no subset/
superset conflict exists with an existing Chord on the current Layer
(ticket 01's amended Answer — an Input may belong to any number of Chords;
only a subset/superset relationship between two Chords' member sets is
rejected), opening `binding_editor.build_chord_binding_dialog` for just the
Trigger/Action step. The Chords list below shows every Chord on the current
Layer; each row's "Edit" re-enters selection mode pre-loaded with its
members, a click on the row previews its members on the grid (a distinct
highlight), and "×" calls `ClearChordBinding` directly.

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

from gi.repository import Gdk, GLib, Gtk, Pango

from .binding_editor import action_summary, build_binding_editor, build_chord_binding_dialog
from .daemon_client import DaemonError
from .gtk_utils import build_name_prompt_popover, build_pinned_sidebar_box
from .inputs import GRID_COLS, GRID_ROWS, LAYOUT_NUMBER, grid_input, input_label
from .library_view import build_library_content, build_library_sidebar
from .rules import chord_members_conflict

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
    "profiles": {
        "Default": {
            "base": {},
            "held": {},
            "mode_key_role": "layer_switch",
            "chords_base": {},
            "chords_held": {},
            # Every Profile key a `build_main_view` render reads
            # *unconditionally* must be present here, not just the ones the
            # dimmed grid strictly needs — `build_binding_editor` is built
            # eagerly for every grid button (`make_input_button`), and it
            # reads `default_actuation` / `actuation_overrides` (ticket 26)
            # and `status_leds` (tartarus-status-leds ticket 04); the
            # axis-stripe check reads `axis_base` / `axis_held` (ticket 71).
            # A missing key here is a launch-time KeyError on the
            # before-the-Daemon-answers path. Mirror `DaemonStub._SEED_PROFILE`.
            "default_actuation": {"actuation": 128, "release": 112},
            "actuation_overrides": {},
            "status_leds": {"orange": False, "green": False, "blue": False},
            # `tartarus-backlight` ticket 01: mirrors `DaemonStub._SEED_
            # PROFILE`'s `lighting`/`brightness` keys — no unconditional
            # reader exists yet, but the mirror obligation above applies to
            # every `_SEED_PROFILE` key, not just the ones read today.
            "lighting": {"type": "off"},
            "brightness": 0,
            "axis_base": {},
            "axis_held": {},
        }
    },
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
    sidebar = build_pinned_sidebar_box()
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


# --- Chord recording (ticket 01/40) — a Chord's `ChordKey` wire form (see
# `daemon/src/config.rs::ChordKey`'s Display) is a "+"-joined, sorted string
# of member Input strings; these helpers all operate on that same string
# form, matching what `GetConfig()`'s `chords_base`/`chords_held` dicts
# actually key by. ---


def _chord_members(key: str) -> list[str]:
    return key.split("+")


def _chord_members_text(members: list[str]) -> str:
    return " + ".join(input_label(m) for m in members)


def _chords_containing(chords: dict, inp: str) -> list[str]:
    """Every Chord key on `chords` (one Layer's worth) that has `inp` among
    its members — an Input may belong to any number of Chords (ticket 01's
    amended Answer), so this can return more than one."""
    return [key for key in chords if inp in _chord_members(key)]


def _chord_conflict(chords: dict, members: list[str], exclude_key: str | None = None) -> str | None:
    """The only conflict that survives ticket 01's correction: a subset/
    superset relationship between `members` and an existing Chord's own set
    — a plain intersection (the thumbstick-diagonal shape) is not a
    conflict. `exclude_key` is the Chord currently being edited, if any —
    editing it back to the exact same members is not a conflict with
    itself."""
    candidate = set(members)
    for key in chords:
        if key == exclude_key:
            continue
        if chord_members_conflict(candidate, set(_chord_members(key))):
            return key
    return None


def _chord_button_style(
    chords: dict, config: dict, chord_ui: dict, inp: str
) -> tuple[list[str], str | None]:
    """Per-device-button CSS classes/tooltip for the current Chord-UI state
    — recomputed on every rebuild for every device button (grid, thumbstick,
    Mode key, wheel alike, since a Chord's members are open-ended), mirroring
    the prototype's `sync_surface`/`styler` pattern."""
    classes = []
    if chord_ui["selecting"] and inp in chord_ui["recorded"]:
        classes.append("chord-selected")
    preview = chord_ui.get("preview")
    if preview is not None and inp in _chord_members(preview):
        classes.append("chord-preview")
    owners = [
        key for key in _chords_containing(chords, inp) if key != chord_ui.get("edit_key")
    ]
    tooltip = None
    if owners:
        lines = [
            f"{_chord_members_text(_chord_members(key))} → "
            f"{action_summary(chords[key], '', config.get('macros', {}), config.get('steppers', {}))}"
            for key in owners
        ]
        heading = "Part of Chord:" if len(lines) == 1 else "Part of Chords:"
        tooltip = heading + "\n" + "\n".join(lines)
    return classes, tooltip


def make_input_button(
    client,
    config: dict,
    profile: str,
    layer: str,
    inp: str,
    on_change: Callable[[], None],
    w=100,
    h=100,
    sensitive: bool = True,
    insensitive_reason: str | None = None,
    capture_mode: str = "digital",
    chord_classes: list[str] | None = None,
    chord_tooltip: str | None = None,
    on_click_override: Callable[[str], None] | None = None,
) -> Gtk.Button:
    binding = config["profiles"][profile][layer].get(inp)
    axis_target = config["profiles"][profile][f"axis_{layer}"].get(inp)
    label_line = input_label(inp)
    summary_line = action_summary(
        binding, inp, config.get("macros", {}), config.get("steppers", {}), axis_target
    )
    inner = Gtk.Label(justify=Gtk.Justification.CENTER)
    # Ticket 87/88: the Input's own label — its grid number, "Mode", or an
    # arrow glyph, i.e. *which* Input this is — renders bold; the binding
    # summary below it — *what it does* — stays regular weight.
    # `GLib.markup_escape_text` on both lines: the summary line carries
    # user-influenced content (Chord member text, Macro/Stepper display
    # names) that could contain markup-special characters.
    inner.set_markup(
        f"<b>{GLib.markup_escape_text(label_line)}</b>\n"
        f"{GLib.markup_escape_text(summary_line)}"
    )
    inner.set_wrap(True)
    # WORD_CHAR still runs first — ordinary content ("Ctrl+A  [1x]") wraps at
    # its own whitespace, and only a no-space run (a multi-modifier chord
    # like "Ctrl+Shift+Alt+Super+F12") falls back to a character split.
    inner.set_wrap_mode(Pango.WrapMode.WORD_CHAR)
    # Ticket 87's settled variant A: both dimensions are a genuine cap now,
    # not ticket 06's "floor, not ceiling". `max_width_chars` + `set_lines` +
    # `set_ellipsize` apply *after* the wrap above and bound the label's
    # requested size, so the button never has to grow past `w`/`h` — the
    # missing half of ticket 06's own rejected `max-width-chars` attempt,
    # which paired a width cap with wrapping alone and mid-word-split
    # "passthrough" for lack of a line limit + ellipsis fallback. The char
    # cap is deliberately snug (tuned live in ticket 88 against the real
    # font/theme), so ordinary content sometimes ellipsizes too — every
    # button always carries a full-text tooltip regardless (below), cheaper
    # than tracking per-button truncation state.
    # Two buckets keyed off the button's own width: 8 chars for the 100px
    # buttons (the grid, wheel, thumbstick lobes, Mode key), 14 for key 20's
    # 150px paddle — the only wider footprint. Not derived from `w` or font
    # metrics: these are the snug values tuned by eye in ticket 88 against the
    # real Yaru theme font, where the label still fills the button without
    # forcing it to grow. A wider button added later needs its own bucket.
    chars = 8 if w <= 100 else 14
    # `width_chars == max_width_chars` pins the label's requested width to a
    # fixed value instead of letting it float between a tiny wrap-minimum and
    # the max — without this, `wrap=True` + `ellipsize` intermittently report
    # a natural width below the minimum during transient allocation, which
    # GTK clamps but also warns about ("natural size must be >= min size").
    # A fixed request is also exactly the predictability this ticket is for;
    # the button's own `w` still bounds it.
    inner.set_width_chars(chars)
    inner.set_max_width_chars(chars)
    inner.set_lines(3)
    inner.set_ellipsize(Pango.EllipsizeMode.END)
    btn = Gtk.Button()
    btn.set_child(inner)
    btn.set_size_request(w, h)
    # A MenuButton's default halign is FILL: inside a plain Gtk.Box (the Mode
    # key and key-20's paddle, both appended straight to `stick_col` rather
    # than gridded), that stretches it to the box's full cross-width — the
    # diamond's own width, not this button's own 100px — live-verified via a
    # real screenshot as the actual cause of the oversized Mode-key oval,
    # not missing wrapping. A Gtk.Grid cell (every other caller) already
    # sizes to its own column, so this is a no-op there. (Still relevant
    # now that `btn` is a plain Gtk.Button, not a Gtk.MenuButton — both
    # default to halign FILL.)
    btn.set_halign(Gtk.Align.CENTER)
    if axis_target is not None:
        # Ticket 60's Answer: always-visible, regardless of Chord-selection
        # mode — unlike `chord_classes` below, which only ever applies while
        # `chord_ui["selecting"]` gates them. Neither "bound" (a Binding
        # exists) nor "empty" (nothing is configured) applies here — an
        # Axis-assigned key has `binding is None` but is very much
        # configured, so falling into "empty" (code-review finding) would
        # dim it to 0.75 opacity, visually contradicting the whole point of
        # the always-visible stripe.
        btn.add_css_class("axis-stripe")
    else:
        btn.add_css_class("bound" if binding else "empty")
    for cls in chord_classes or []:
        btn.add_css_class(cls)
    btn.set_sensitive(sensitive)
    # Ticket 88: every button carries a tooltip with its full untruncated
    # two-line text (label + summary, newline flattened to two spaces), set
    # unconditionally — not gated on whether the label actually ellipsized.
    full_text = f"{label_line}  {summary_line}"
    if not sensitive and insensitive_reason:
        # A disabled key (only the Mode key, ticket 11) has nothing
        # actionable to describe — its reason is the whole tooltip.
        btn.set_tooltip_text(insensitive_reason)
    elif chord_tooltip and binding is not None:
        # Ticket 96: a grid key can be *both* a Chord member *and* carry its
        # own individual Binding. The old override showed only `chord_tooltip`
        # (the Chord's members + action), so once the face ellipsized to ~8
        # chars this key's own binding summary was readable nowhere. Stack
        # both — full face text first, Chord membership below.
        btn.set_tooltip_text(f"{full_text}\n\n{chord_tooltip}")
    elif chord_tooltip:
        # Chord-only member (no individual Binding): the membership tooltip
        # says everything, same as before ticket 96.
        btn.set_tooltip_text(chord_tooltip)
    else:
        btn.set_tooltip_text(full_text)

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

    # tartarus-dual-stage-keys ticket 10: the full app rebuild moved off the
    # per-Save path onto the window's close-request handler, fired once per
    # editing session and only when a Save or Apply actually committed
    # something (`committed`). The grid-key panel's **Apply** commits and
    # rebuilds itself in place *without* closing; `on_commit` arms this flag
    # so the eventual dismissal — Save's own `window.close()`, the WM close
    # button, or Escape — still drives exactly one `on_change()`. A pure
    # open/close (nothing committed) leaves the cached hide-on-close window
    # untouched and skips the rebuild, keeping "reopen is instant".
    committed = {"yes": False}

    def mark_committed():
        committed["yes"] = True

    def rebuild_if_committed():
        if committed["yes"]:
            committed["yes"] = False
            on_change()

    def on_saved():
        committed["yes"] = True
        window.close()
        # `Gtk.Window.close()` only emits `close-request` for a realized
        # window, so drive the deferred rebuild directly here too; when the
        # signal *does* also fire (a mapped window), `rebuild_if_committed`
        # is a no-op the second time (the flag is already cleared).
        rebuild_if_committed()

    def on_close_request(_w):
        rebuild_if_committed()
        return False  # let hide-on-close proceed

    window.connect("close-request", on_close_request)

    # No scrolling wrapper here — this window has no other container
    # imposing a height on it, so it always sizes to `editor`'s own natural
    # height, and `build_binding_editor` itself (ticket 70 follow-up)
    # already defers only its Actuation & release section (grid Inputs
    # only, needed less often) behind an internal scroll, so this window
    # is guaranteed tall enough on first open for everything above that —
    # heading, error, the Trigger/Action fields including the inline key/
    # mouse-button picker's full expanded shape — without scrolling, per
    # the user's own "always reachable" ask for those specifically.
    editor = build_binding_editor(
        client, config, profile, layer, inp, on_saved, capture_mode, on_commit=mark_committed
    )
    window.set_child(editor)

    def on_click(_b):
        if on_click_override is not None:
            on_click_override(inp)
            return
        window.set_transient_for(btn.get_root())
        window.present()

    btn.connect("clicked", on_click)
    # Exposed for tests, which need to reach the editor's content without
    # actually presenting a real top-level window in a headless run (unlike
    # the old Gtk.Popover, there's no `btn.get_popover()`-style GTK API for
    # "the window this button opens").
    btn.binding_editor_window = window
    return btn


def build_device_geometry(cell_factory: Callable[[str, int, int], Gtk.Widget]) -> Gtk.Widget:
    """The physical layout ticket 09's prototype settled (see this module's
    own docstring): a 4x5 grid (row 4 four-wide), the scroll wheel
    continuing row 4's missing 5th slot for three more rows (scroll up,
    click, scroll down), and a thumbstick-diamond/Mode-key/key-20 column
    beside it. Factored out of `build_main_view` by `tartarus-backlight`
    ticket 05 so the Lighting tab's Custom-layout paint grid can render at
    the exact same physical positions as the real Grid destination's
    Binding buttons — button **behaviour** stays entirely with the caller's
    `cell_factory(inp, w, h)`; this function only ever decides *where* each
    Input's widget goes (and, for the Mode key, its round shape — a layout
    fact, not a behavioural one, so it belongs here rather than in either
    caller)."""
    device = Gtk.Box(spacing=28)

    grid = Gtk.Grid(row_spacing=4, column_spacing=4)
    for r in range(1, GRID_ROWS + 1):
        cols = GRID_COLS if r < GRID_ROWS else GRID_COLS - 1
        for c in range(1, cols + 1):
            grid.attach(cell_factory(grid_input(r, c), 100, 100), c - 1, r - 1, 1, 1)
    wheel_col_index = GRID_COLS - 1
    grid.attach(cell_factory("wheel_scroll_up", 100, 100), wheel_col_index, GRID_ROWS - 1, 1, 1)
    grid.attach(cell_factory("wheel_middle", 100, 100), wheel_col_index, GRID_ROWS, 1, 1)
    grid.attach(cell_factory("wheel_scroll_down", 100, 100), wheel_col_index, GRID_ROWS + 1, 1, 1)
    device.append(grid)

    stick_col = Gtk.Box(
        orientation=Gtk.Orientation.VERTICAL, spacing=18, halign=Gtk.Align.CENTER, valign=Gtk.Align.START
    )
    mode_widget = cell_factory("mode_key", 100, 100)
    mode_widget.add_css_class("mode-key")
    stick_col.append(mode_widget)

    # Diamond rotated 90° clockwise from a plain N/S/E/W layout — see
    # `build_main_view`'s own historical comment for why (layout.md).
    diamond = Gtk.Grid(row_spacing=2, column_spacing=2)
    diamond.attach(cell_factory("thumbstick_left", 100, 100), 1, 0, 1, 1)
    diamond.attach(cell_factory("thumbstick_down", 100, 100), 0, 1, 1, 1)
    diamond.attach(cell_factory("thumbstick_up", 100, 100), 2, 1, 1, 1)
    diamond.attach(cell_factory("thumbstick_right", 100, 100), 1, 2, 1, 1)
    stick_col.append(diamond)

    # Key 20 (the paddle below the diamond) is physically wider on the
    # hardware — 150×100, per ticket 87's settled sizing.
    stick_col.append(cell_factory(grid_input(4, 5), 150, 100))
    device.append(stick_col)

    return device


def build_destination_switch(selected_dest: str, on_select: Callable[[str], None]) -> Gtk.Box:
    """Ticket 47's round-2 (variant D) winner: a plain-text, icon-free
    "Grid"/"Library" switcher sitting where the old Action-Table toggle
    used to, fully replacing `build_main_view`'s content area on
    selection. `on_select` is expected to write the pick into `ui_state`
    and call `on_change()` — this widget carries no state of its own,
    matching `build_layer_bar`'s own selected-tab pattern.

    `tartarus-backlight` ticket 04 adds "Lighting" as a third arm,
    alongside Grid and Library — `build_main_view` keeps the Profile
    sidebar for it exactly as Grid does (not Library's sidebar swap)."""
    row = Gtk.Box(spacing=6)
    for dest_key, label in (("grid", "Grid"), ("library", "Library"), ("lighting", "Lighting")):
        btn = Gtk.Button(label=label)
        if dest_key == selected_dest:
            btn.add_css_class("suggested-action")

        def on_clicked(_b, dest_key=dest_key):
            on_select(dest_key)

        btn.connect("clicked", on_clicked)
        row.append(btn)
    return row


def build_chords_section(
    client,
    config: dict,
    profile: str,
    layer: str,
    chord_ui: dict,
    on_change: Callable[[], None],
) -> Gtk.Widget:
    """The real Chord-recording flow (ticket 01/40, prototype variant A
    round 3) — always visible beside the grid while the Grid destination is
    selected (no toggle, per ticket 47's round-2: nothing should rescale on
    open/close).

    `chord_ui` (`ui_state["chord"]`, so it survives a rebuild) is
    `{"selecting": bool, "recorded": list[str], "edit_key": str | None,
    "preview": str | None}`. `selecting` is what actually changes device
    click behaviour — see `make_input_button`'s `on_click_override` and this
    module's docstring — everything else here is just this state's own
    rendering.
    """
    chords = config["profiles"][profile][f"chords_{layer}"]

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    box.add_css_class("sidebar")
    box.set_size_request(220, -1)
    box.append(Gtk.Label(label="Chords", xalign=0, css_classes=["heading"]))

    selecting_btn = Gtk.ToggleButton(label="Select Chord members")
    selecting_btn.set_active(chord_ui["selecting"])
    selecting_btn.set_tooltip_text(
        "On: clicking any Input on the device above toggles it into the Chord "
        "selection below, instead of opening its own Binding editor."
    )

    def on_selecting_toggled(b):
        chord_ui["selecting"] = b.get_active()
        if not chord_ui["selecting"]:
            chord_ui["recorded"] = []
            chord_ui["edit_key"] = None
            chord_ui["axis_error"] = None
        on_change()

    selecting_btn.connect("toggled", on_selecting_toggled)
    box.append(selecting_btn)

    if chord_ui["selecting"]:
        conflict_key = (
            _chord_conflict(chords, chord_ui["recorded"], exclude_key=chord_ui["edit_key"])
            if len(chord_ui["recorded"]) >= 2
            else None
        )

        axis_error = chord_ui.get("axis_error")
        if axis_error is not None:
            box.append(Gtk.Label(label=axis_error, xalign=0, wrap=True, css_classes=["error"]))

        status = Gtk.Label(xalign=0, wrap=True, css_classes=["dim"])
        if conflict_key is not None:
            status.set_label(
                f"{_chord_members_text(chord_ui['recorded'])} conflicts with the existing Chord "
                f"{_chord_members_text(_chord_members(conflict_key))}: one member set fully "
                "contains the other. Adjust the selection, or edit that Chord instead."
            )
        elif chord_ui["recorded"]:
            verb = "Editing" if chord_ui["edit_key"] is not None else "Selected"
            status.set_label(f"{verb}: {_chord_members_text(chord_ui['recorded'])}")
        else:
            status.set_label(
                "Click two or more Inputs on the device above to start a Chord — an Input "
                "may belong to more than one."
            )
        box.append(status)

        action_row = Gtk.Box(spacing=8)
        binding_btn = Gtk.Button(label="Binding →", css_classes=["suggested-action"])
        binding_btn.set_sensitive(len(chord_ui["recorded"]) >= 2 and conflict_key is None)

        def on_binding(b):
            members = list(chord_ui["recorded"])
            existing = chords.get(chord_ui["edit_key"]) if chord_ui["edit_key"] is not None else None

            def on_saved():
                chord_ui["recorded"] = []
                chord_ui["edit_key"] = None
                chord_ui["axis_error"] = None
                on_change()

            dialog = build_chord_binding_dialog(
                client,
                config,
                profile,
                layer,
                members,
                existing,
                on_saved,
                binding_btn.get_root(),
                chord_ui["edit_key"],
            )
            # Exposed for tests, which need to reach a fresh dialog's
            # content without a real windowing system — mirrors
            # `make_input_button`'s own `btn.binding_editor_window`, except
            # this one is rebuilt per click rather than built once, since
            # `members`/`existing` differ every time.
            binding_btn.last_chord_dialog = dialog
            dialog.present()

        binding_btn.connect("clicked", on_binding)
        action_row.append(binding_btn)

        clear_btn = Gtk.Button(label="Clear selection")
        clear_btn.set_sensitive(bool(chord_ui["recorded"]))

        def on_clear(b):
            chord_ui["recorded"] = []
            chord_ui["edit_key"] = None
            chord_ui["axis_error"] = None
            on_change()

        clear_btn.connect("clicked", on_clear)
        action_row.append(clear_btn)
        box.append(action_row)

        if conflict_key is not None:
            conflict_btn = Gtk.Button(label="Edit conflicting Chord")

            def on_edit_conflict(b, key=conflict_key):
                chord_ui["edit_key"] = key
                chord_ui["recorded"] = _chord_members(key)
                chord_ui["axis_error"] = None
                on_change()

            conflict_btn.connect("clicked", on_edit_conflict)
            box.append(conflict_btn)

        box.append(
            Gtk.Label(
                label="Tip: click two adjacent thumbstick directions together to define a diagonal.",
                xalign=0,
                wrap=True,
                css_classes=["dim"],
            )
        )

    box.append(Gtk.Separator())
    if not chords:
        box.append(Gtk.Label(label="No Chords defined on this Layer yet.", xalign=0, wrap=True, css_classes=["dim"]))
    for key in sorted(chords):
        binding = chords[key]
        row = Gtk.Box(spacing=6)
        summary = action_summary(binding, "", config.get("macros", {}), config.get("steppers", {}))
        preview_btn = Gtk.Button(
            label=f"{_chord_members_text(_chord_members(key))} → {summary}", hexpand=True
        )
        preview_btn.set_tooltip_text("Click to preview this Chord's members on the grid above.")
        if chord_ui.get("preview") == key:
            preview_btn.add_css_class("suggested-action")

        def on_preview(b, key=key):
            chord_ui["preview"] = None if chord_ui.get("preview") == key else key
            on_change()

        preview_btn.connect("clicked", on_preview)
        row.append(preview_btn)

        edit_btn = Gtk.Button(label="Edit")

        def on_edit(b, key=key):
            chord_ui["selecting"] = True
            chord_ui["edit_key"] = key
            chord_ui["recorded"] = _chord_members(key)
            chord_ui["preview"] = None
            chord_ui["axis_error"] = None
            on_change()

        edit_btn.connect("clicked", on_edit)
        row.append(edit_btn)

        remove_btn = Gtk.Button(label="×")
        remove_btn.set_tooltip_text(f"Delete the {_chord_members_text(_chord_members(key))} Chord")

        def on_remove(b, key=key):
            try:
                client.clear_chord_binding(_chord_members(key), layer)
            except DaemonError:
                # Racing another client's own delete — nothing left to
                # remove; the rebuild below simply won't show it any more.
                pass
            if chord_ui.get("edit_key") == key:
                chord_ui["edit_key"] = None
                chord_ui["recorded"] = []
            if chord_ui.get("preview") == key:
                chord_ui["preview"] = None
            on_change()

        remove_btn.connect("clicked", on_remove)
        row.append(remove_btn)

        box.append(row)

    return box


# tartarus-status-leds ticket 04 — the three fixed-colour Status LEDs, top
# to bottom, roughly mirroring their physical placement on the device's left
# side. `(config key, display word)`; the display word feeds the tooltip and
# the accessible name.
_STATUS_LED_CHANNELS = (("orange", "Orange"), ("green", "Green"), ("blue", "Blue"))


def build_status_leds_section(
    client, config: dict, profile: str, on_change: Callable[[], None]
) -> Gtk.Widget:
    """The Status LEDs lozenge group (tartarus-status-leds ticket 04, spec
    §"GUI") — three vertically-stacked colour lozenges (orange top, green
    middle, blue bottom) showing and editing the **active** Profile's Status
    LED assignment, as direct as editing a Binding.

    Profile-scoped, never Layer-scoped: it renders identically on Base and
    Held from `config["profiles"][profile]["status_leds"]` regardless of
    `selected_layer`, and shows the stored state even while the device is
    disconnected — the state is always config, never a live hardware read.
    On a newly created Profile all three show dark (`status_leds` defaults
    all-off; "never set" is byte-identical to "explicitly all-off"). Grid
    destination only — built in `build_main_view`'s `device_row` alongside
    `build_chords_section`, not in the Library branch.

    Each lozenge's click reads all three current states, flips its own, and
    calls `client.set_status_leds(orange, green, blue)`; the group then
    rebuilds from config on the shared `on_change`, like everything else on
    the panel. The `Effect::AssertStatusLeds` that `SetStatusLeds` emits
    drives the hardware immediately.
    """
    stored = config["profiles"][profile]["status_leds"]
    current = {name: bool(stored.get(name, False)) for name, _ in _STATUS_LED_CHANNELS}

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, valign=Gtk.Align.START)
    box.append(Gtk.Label(label="Status LEDs", xalign=0, css_classes=["heading"]))

    # A failed `SetStatusLeds` (Daemon vanished mid-click) must visibly do
    # something rather than silently no-op — same convention as
    # `build_profile_sidebar`'s shared error label.
    error_label = Gtk.Label(xalign=0, wrap=True, css_classes=["error"])
    error_label.set_visible(False)
    box.append(error_label)

    for name, title in _STATUS_LED_CHANNELS:
        on = current[name]
        state_word = "on" if on else "off"

        lozenge = Gtk.Button(halign=Gtk.Align.CENTER)
        lozenge.add_css_class("status-led")
        lozenge.add_css_class(f"status-led-{name}")
        if on:
            lozenge.add_css_class("lit")
        # No visible per-lozenge text — hue + brightness alone fails a
        # colour-blind user, so the colour and the on/off state both go in
        # the tooltip and the accessible name/description.
        lozenge.set_tooltip_text(f"{title} status LED — {state_word}")
        lozenge.update_property(
            [Gtk.AccessibleProperty.LABEL, Gtk.AccessibleProperty.DESCRIPTION],
            [f"{title} status LED", state_word],
        )

        def on_clicked(_b, name=name):
            triple = dict(current)
            triple[name] = not triple[name]
            try:
                client.set_status_leds(triple["orange"], triple["green"], triple["blue"])
            except DaemonError as exc:
                error_label.set_label(str(exc))
                error_label.set_visible(True)
                return
            on_change()

        lozenge.connect("clicked", on_clicked)
        box.append(lozenge)

    return box


# --- Lighting (tartarus-backlight ticket 04, spec §"GUI") — a flat 8-entry
# mode selector (Off, the six Fixed effects, Custom layout) plus per-effect
# parameter controls and an always-visible brightness slider, all in one
# horizontal control strip above the device area (settled by the ticket's
# own prototype, `prototype/04-lighting-tab-layout` — variant A, with the
# strip laid out horizontally so its height stays constant across every
# mode). Off/Fixed-effect selection and every per-effect param change commit
# immediately via `client.set_lighting(...)`, mirroring every other
# immediate-write control on this panel — no Save/Apply button on the tab.
# The Custom-layout paint grid (ticket 05, `build_lighting_device_area`) is
# the only device-area content that isn't a placeholder — it shares its
# physical layout with the Grid destination's own button grid via
# `build_device_geometry`. ---

_LIGHTING_MODES = (
    ("off", "Off"),
    ("static", "Static"),
    ("spectrum", "Spectrum"),
    ("reactive", "Reactive"),
    ("wave", "Wave"),
    ("breath", "Breath"),
    ("starlight", "Starlight"),
    ("custom_layout", "Custom layout"),
)

# Seeded default for a colour a freshly-selected effect/style needs but has
# none stored yet (e.g. switching Off -> Static, or a Breath style from
# Random -> Single) — an arbitrary but harmless starting point; the colour
# picker immediately lets the user change it.
_DEFAULT_COLOUR = {"r": 255, "g": 255, "b": 255}
# `LightingAssignment::CustomLayout`'s own migration-safe default (spec
# "Config schema") — a Profile selecting Custom layout for the first time
# starts fully dark, not an arbitrary colour.
_BLACK_COLOUR = {"r": 0, "g": 0, "b": 0}
_CUSTOM_LAYOUT_KEY_COUNT = 21
# How long the brightness slider waits after the last `value-changed` before
# committing (`build_lighting_brightness`) — long enough that a drag's own
# stream of ticks never lands a commit mid-drag, short enough to feel
# immediate once the user stops moving it.
_BRIGHTNESS_DEBOUNCE_MS = 300


def _lighting_mode(lighting: dict) -> str:
    """The flat 8-entry mode key a stored `LightingAssignment` dict maps to
    — the mode selector's own reverse of `_default_fixed_effect`/the
    Custom-layout branch below, collapsing the config type's `Off |
    FixedEffect{effect} | CustomLayout` shape to one flat list (spec's
    settled "not nested" mode selector)."""
    kind = lighting["type"]
    if kind == "fixed_effect":
        return lighting["effect"]["type"]
    return kind


def _default_fixed_effect(effect_type: str) -> dict:
    """A fresh `FixedEffect` payload for `effect_type`, seeded with sane
    defaults — used when the mode selector switches *into* a Fixed effect
    that wasn't already active, so there's no stored colour/speed/style to
    carry over. Breath/Starlight default to the Random style, the only
    variant needing no colour picker at all."""
    if effect_type == "static":
        return {"type": "static", "colour": dict(_DEFAULT_COLOUR)}
    if effect_type == "spectrum":
        return {"type": "spectrum"}
    if effect_type == "reactive":
        return {"type": "reactive", "colour": dict(_DEFAULT_COLOUR), "speed": 1}
    if effect_type == "wave":
        return {"type": "wave", "direction": "right"}
    if effect_type == "breath":
        return {"type": "breath", "style": {"style": "random"}}
    if effect_type == "starlight":
        return {"type": "starlight", "style": {"style": "random"}, "speed": 1}
    raise ValueError(f"{effect_type!r} is not a valid FixedEffect type")


def _default_style(style_kind: str) -> dict:
    if style_kind == "random":
        return {"style": "random"}
    if style_kind == "single":
        return {"style": "single", "colour": dict(_DEFAULT_COLOUR)}
    if style_kind == "dual":
        return {"style": "dual", "first": dict(_DEFAULT_COLOUR), "second": dict(_DEFAULT_COLOUR)}
    raise ValueError(f"{style_kind!r} is not a valid BreathStyle")


def _custom_layout_colours(lighting: dict) -> list[dict]:
    """The colours a freshly-selected Custom layout commits with — the
    Profile's already-stored colours if it has any, else 21 black entries
    (ticket 04's settled default for a Profile that has none yet)."""
    if lighting["type"] == "custom_layout":
        return [dict(c) for c in lighting["colours"]]
    return [dict(_BLACK_COLOUR) for _ in range(_CUSTOM_LAYOUT_KEY_COUNT)]


def _custom_layout_column(inp: str) -> int | None:
    """Ticket 05's settled column addressing (spec "The wire frames", hardware-
    verified): columns `0..18` = grid keys `1..19` in order, column `19` =
    the scroll wheel — its three physical detents (`wheel_scroll_up`/
    `_middle`/`_scroll_down`) all address this one column, since the real
    device has exactly one LED under the whole wheel assembly, not one per
    detent — and column `20` = grid key `20`. `None` for the Mode key and
    every thumbstick direction: solid black plastic on the real unit, not
    RGB-capable at all, so they have no column to paint."""
    if inp in ("wheel_scroll_up", "wheel_middle", "wheel_scroll_down"):
        return 19
    number = LAYOUT_NUMBER.get(inp)
    if number is None:
        return None
    # `LAYOUT_NUMBER` numbers every grid Input 1..20 (key 20 included); the
    # wire's column order slots 1..19 in at 0..18 but jumps key 20 to
    # column 20, past the wheel's column 19 — not a plain `number - 1`.
    return 20 if number == 20 else number - 1


_LIGHTING_SWATCH_PROVIDER_STATE: dict[str, Gtk.CssProvider] = {}


def _swatch_css_id(column: int) -> str:
    """A paint cell's colour-swatch CSS id is keyed by its own wire column
    (0-20, `_custom_layout_column`'s range) — a small, fixed set reused
    across every rebuild, not per-colour or per-widget-instance."""
    return f"lighting-swatch-col-{column}"


def _install_lighting_swatch_provider() -> Gtk.CssProvider:
    """The one `Gtk.CssProvider` every Custom-layout paint cell's background
    colour is expressed through — Gtk4 has no per-widget inline-style API,
    and the app's one global stylesheet (`app.CSS`) is static text with no
    notion of an arbitrary runtime RGB value. Registered on the display at
    most once (lazily, the same `add_provider_for_display` call `app.py`
    already makes once at startup for its own static CSS) and reused —
    `build_lighting_device_area` below fully rewrites its content on every
    render instead of adding a fresh provider per cell per rebuild, which
    would otherwise accumulate unboundedly on the display for the lifetime
    of this long-running tray app."""
    provider = _LIGHTING_SWATCH_PROVIDER_STATE.get("provider")
    if provider is None:
        provider = Gtk.CssProvider()
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )
        _LIGHTING_SWATCH_PROVIDER_STATE["provider"] = provider
    return provider


def _rgba_to_colour(rgba: Gdk.RGBA) -> dict:
    return {"r": round(rgba.red * 255), "g": round(rgba.green * 255), "b": round(rgba.blue * 255)}


def _colour_to_rgba(colour: dict) -> Gdk.RGBA:
    rgba = Gdk.RGBA()
    rgba.red = colour["r"] / 255
    rgba.green = colour["g"] / 255
    rgba.blue = colour["b"] / 255
    rgba.alpha = 1.0
    return rgba


def _lighting_labeled(label_text: str, widget: Gtk.Widget) -> Gtk.Box:
    """A compact label+control pair for the horizontal params strip — unlike
    `binding_editor.labeled_row`, no fixed label width or `hexpand`: several
    of these sit side by side in one row here, not stacked in a form."""
    row = Gtk.Box(spacing=6)
    row.append(Gtk.Label(label=label_text, xalign=0))
    row.append(widget)
    return row


def _lighting_colour_button(colour: dict, on_commit: Callable[[dict], None]) -> Gtk.ColorDialogButton:
    dialog = Gtk.ColorDialog(title="Pick a colour")
    btn = Gtk.ColorDialogButton(dialog=dialog)
    # Connected only after the initial value is set, so seeding the button
    # from stored config never itself fires a spurious commit.
    btn.set_rgba(_colour_to_rgba(colour))

    def on_notify(b, _pspec):
        on_commit(_rgba_to_colour(b.get_rgba()))

    btn.connect("notify::rgba", on_notify)
    return btn


_STYLE_OPTIONS = (("random", "Random"), ("single", "Single"), ("dual", "Dual"))
_WAVE_DIRECTION_OPTIONS = (("left", "◀ Left"), ("right", "Right ▶"))


def _toggle_button_row(
    options: tuple[tuple[str, str], ...], current: str, on_select: Callable[[str], None]
) -> Gtk.Widget:
    """A row of plain buttons acting as an exclusive selector — the active
    option carries `"suggested-action"`, and reclicking it is a no-op
    (mirroring `build_profile_sidebar`'s own "already active" guard).
    Shared by the mode selector, the Wave direction pair, and the Breath/
    Starlight style group — all the same shape, just a different option
    list and commit."""
    row = Gtk.Box(spacing=4)
    for key, label in options:
        btn = Gtk.Button(label=label)
        if key == current:
            btn.add_css_class("suggested-action")

        def on_clicked(_b, key=key):
            if key == current:
                return
            on_select(key)

        btn.connect("clicked", on_clicked)
        row.append(btn)
    return row


def _lighting_speed_spin(value: int, lo: int, hi: int, on_commit: Callable[[int], None]) -> Gtk.SpinButton:
    adj = Gtk.Adjustment(value=value, lower=lo, upper=hi, step_increment=1)
    spin = Gtk.SpinButton(adjustment=adj)

    def on_changed(s):
        on_commit(s.get_value_as_int())

    spin.connect("value-changed", on_changed)
    return spin


def _style_colour_rows(style: dict, on_commit: Callable[[str, dict], None]) -> list[Gtk.Widget]:
    """0/1/2 colour-picker rows as `style["style"]` requires — `slot` is
    the field `on_commit` writes the picked colour back onto (`"colour"`
    for Single, `"first"`/`"second"` for Dual), matching `BreathStyle`'s
    own field names exactly so the caller can splice the result straight
    back into the stored style dict."""
    if style["style"] == "single":
        return [
            _lighting_labeled(
                "Colour", _lighting_colour_button(style["colour"], lambda c: on_commit("colour", c))
            )
        ]
    if style["style"] == "dual":
        return [
            _lighting_labeled(
                "Colour 1", _lighting_colour_button(style["first"], lambda c: on_commit("first", c))
            ),
            _lighting_labeled(
                "Colour 2", _lighting_colour_button(style["second"], lambda c: on_commit("second", c))
            ),
        ]
    return []


def _commit_lighting(
    client, config: dict, profile: str, assignment: dict, on_change: Callable[[], None]
) -> None:
    """The one write path every mode-select/per-effect-param control uses —
    always the *current* brightness (untouched), matching the brightness
    slider's own mirror-image "assignment unchanged" commit below."""
    brightness = config["profiles"][profile]["brightness"]
    client.set_lighting(assignment, brightness)
    on_change()


def build_lighting_mode_selector(
    client, config: dict, profile: str, on_change: Callable[[], None]
) -> Gtk.Widget:
    lighting = config["profiles"][profile]["lighting"]
    current_mode = _lighting_mode(lighting)

    def on_select(mode_key: str) -> None:
        if mode_key == "off":
            assignment = {"type": "off"}
        elif mode_key == "custom_layout":
            assignment = {"type": "custom_layout", "colours": _custom_layout_colours(lighting)}
        else:
            assignment = {"type": "fixed_effect", "effect": _default_fixed_effect(mode_key)}
        _commit_lighting(client, config, profile, assignment, on_change)

    box = _toggle_button_row(_LIGHTING_MODES, current_mode, on_select)
    box.add_css_class("lighting-mode-selector")
    return box


def build_lighting_custom_layout_controls(
    client, config: dict, profile: str, paint_colour: dict, on_change: Callable[[], None]
) -> Gtk.Widget:
    """Custom layout's own per-mode params (ticket 05, spec §"GUI" Custom-
    layout painter): a persistent current-colour picker plus the "Fill all
    keys" bulk-fill button, in the same params slot every other mode's
    controls occupy. `paint_colour` is `ui_state["lighting_paint_colour"]`
    (mutated in place by the picker) — threaded down from `build_lighting_
    content` so the picked colour survives a rebuild and the paint grid
    below reads the same live value on every click, the way `chord_ui`
    already survives rebuilds for Chord recording."""
    box = Gtk.Box(spacing=12)

    def on_colour_picked(c: dict) -> None:
        paint_colour.update(c)

    box.append(_lighting_labeled("Colour", _lighting_colour_button(paint_colour, on_colour_picked)))

    fill_btn = Gtk.Button(label="Fill all keys")

    def on_fill(_b):
        filled = [dict(paint_colour) for _ in range(_CUSTOM_LAYOUT_KEY_COUNT)]
        _commit_lighting(
            client, config, profile, {"type": "custom_layout", "colours": filled}, on_change
        )

    fill_btn.connect("clicked", on_fill)
    box.append(fill_btn)
    return box


def build_lighting_params(
    client, config: dict, profile: str, paint_colour: dict, on_change: Callable[[], None]
) -> Gtk.Widget:
    lighting = config["profiles"][profile]["lighting"]
    box = Gtk.Box(spacing=12)
    box.add_css_class("lighting-params-panel")

    if lighting["type"] == "custom_layout":
        box.append(build_lighting_custom_layout_controls(client, config, profile, paint_colour, on_change))
        return box

    if lighting["type"] != "fixed_effect":
        box.append(Gtk.Label(label="No parameters for this mode.", css_classes=["dim"], xalign=0))
        return box

    effect = lighting["effect"]
    kind = effect["type"]

    def commit_effect(updated_effect: dict) -> None:
        _commit_lighting(
            client, config, profile, {"type": "fixed_effect", "effect": updated_effect}, on_change
        )

    if kind == "static":
        box.append(
            _lighting_labeled(
                "Colour",
                _lighting_colour_button(effect["colour"], lambda c: commit_effect({**effect, "colour": c})),
            )
        )
    elif kind == "spectrum":
        box.append(Gtk.Label(label="Cycles autonomously — no parameters.", css_classes=["dim"], xalign=0))
    elif kind == "reactive":
        box.append(
            _lighting_labeled(
                "Colour",
                _lighting_colour_button(effect["colour"], lambda c: commit_effect({**effect, "colour": c})),
            )
        )
        box.append(
            _lighting_labeled(
                "Speed",
                _lighting_speed_spin(effect["speed"], 1, 4, lambda v: commit_effect({**effect, "speed": v})),
            )
        )
    elif kind == "wave":
        box.append(
            _lighting_labeled(
                "Direction",
                _toggle_button_row(
                    _WAVE_DIRECTION_OPTIONS,
                    effect["direction"],
                    lambda d: commit_effect({**effect, "direction": d}),
                ),
            )
        )
    elif kind in ("breath", "starlight"):
        style = effect["style"]
        box.append(
            _lighting_labeled(
                "Style",
                _toggle_button_row(
                    _STYLE_OPTIONS,
                    style["style"],
                    lambda s: commit_effect({**effect, "style": _default_style(s)}),
                ),
            )
        )
        if kind == "starlight":
            box.append(
                _lighting_labeled(
                    "Speed",
                    _lighting_speed_spin(
                        effect["speed"], 1, 3, lambda v: commit_effect({**effect, "speed": v})
                    ),
                )
            )

        def on_style_colour(slot: str, colour: dict) -> None:
            commit_effect({**effect, "style": {**style, slot: colour}})

        for row in _style_colour_rows(style, on_style_colour):
            box.append(row)

    return box


def build_lighting_brightness(
    client, config: dict, profile: str, on_change: Callable[[], None]
) -> Gtk.Widget:
    """One always-visible `Gtk.Scale`, 0-255 raw byte, independent of the
    selected mode — commits shortly after the value stops changing, not
    per-tick, so a drag doesn't flood `set_lighting` with one call per pixel.
    An earlier version tried to commit on drag-*end* specifically, via a
    `Gtk.GestureClick`'s "released" captured on `row` (the Scale's own
    parent) ahead of `Gtk.Range`'s own internal drag gesture claiming the
    pointer sequence (a documented GTK4 gotcha, e.g.
    github.com/JuliaGtk/Gtk4.jl/issues/77). That capture-phase workaround
    turned out not to fire reliably against a real `Gtk.Scale` drag in
    practice (confirmed live: `value-changed` tracks the drag correctly, but
    "released" never lands, so brightness silently never committed) — a gap
    the test suite couldn't catch since it only ever drove the exposed
    `scale.on_drag_end` seam directly, never the real gesture. A short
    `GLib.timeout_add` debounce off `value-changed` instead has no gesture
    ownership to race: it recommits ~this many ms after the last change,
    covering a mouse drag-release and a keyboard nudge alike.
    `scale.on_drag_end` stays exposed as the same test seam (a real pointer
    drag-release can't be synthesized in a headless test)."""
    brightness = config["profiles"][profile]["brightness"]

    row = Gtk.Box(spacing=8)
    row.append(Gtk.Label(label="Brightness", xalign=0))
    adj = Gtk.Adjustment(value=brightness, lower=0, upper=255, step_increment=1)
    scale = Gtk.Scale(orientation=Gtk.Orientation.HORIZONTAL, adjustment=adj, hexpand=False)
    scale.set_draw_value(True)
    scale.set_digits(0)
    scale.set_size_request(160, -1)

    draft = {"value": brightness, "timeout_id": None}

    def commit(*_args):
        lighting = config["profiles"][profile]["lighting"]
        client.set_lighting(lighting, draft["value"])
        on_change()

    def on_value_changed(s):
        draft["value"] = int(s.get_value())
        if draft["timeout_id"] is not None:
            GLib.source_remove(draft["timeout_id"])

        def fire():
            draft["timeout_id"] = None
            commit()
            return GLib.SOURCE_REMOVE

        draft["timeout_id"] = GLib.timeout_add(_BRIGHTNESS_DEBOUNCE_MS, fire)

    scale.connect("value-changed", on_value_changed)
    scale.on_drag_end = commit

    row.append(scale)
    return row


def build_lighting_copy_from_profile(
    client, config: dict, profile: str, on_change: Callable[[], None]
) -> Gtk.Widget:
    """No new D-Bus method (spec "D-Bus surface"): reads the source
    Profile's already-loaded `lighting`/`brightness` straight out of
    `config` (from `GetConfig`) and calls `set_lighting(...)` with them
    against the active Profile — a one-shot copy, not a live link."""
    other_profiles = sorted(name for name in config["profiles"] if name != profile)

    row = Gtk.Box(spacing=6)
    row.append(Gtk.Label(label="Copy from Profile", xalign=0))
    if not other_profiles:
        row.append(Gtk.Label(label="(no other Profiles)", css_classes=["dim"], xalign=0))
        return row

    dropdown = Gtk.DropDown(model=Gtk.StringList.new(other_profiles))
    row.append(dropdown)

    copy_btn = Gtk.Button(label="Copy")

    def on_copy(_b):
        source = other_profiles[dropdown.get_selected()]
        source_profile = config["profiles"][source]
        client.set_lighting(source_profile["lighting"], source_profile["brightness"])
        on_change()

    copy_btn.connect("clicked", on_copy)
    row.append(copy_btn)
    return row


def _lighting_paint_cell(
    client,
    config: dict,
    profile: str,
    colours: list[dict],
    paint_colour: dict,
    swatch_css: list[str],
    on_change: Callable[[], None],
    inp: str,
    w: int,
    h: int,
) -> Gtk.Widget:
    """One `build_device_geometry` cell for the Custom-layout paint grid.
    The Mode key and every thumbstick direction (`_custom_layout_column`
    returns `None` for them) render as an inert, unpaintable placeholder —
    shown for physical fidelity only, matching the real device's solid
    black plastic; `build_device_geometry` itself already adds the Mode
    key's round `mode-key` shape class, so nothing more happens here for it.
    Every other cell paints immediately on click: the full 21-colour array,
    `colours[column]` set to the current `paint_colour`, committed via
    `set_lighting` at the unchanged brightness — no working-copy/Apply
    step, per the ticket. Its own current colour is appended to
    `swatch_css` rather than applied via a fresh `Gtk.CssProvider` per
    cell — see `_install_lighting_swatch_provider`."""
    column = _custom_layout_column(inp)
    if column is None:
        inert = Gtk.Box()
        inert.set_size_request(w, h)
        inert.set_halign(Gtk.Align.CENTER)
        inert.add_css_class("lighting-inert-cell")
        inert.set_tooltip_text(f"{input_label(inp)} — solid black plastic, not RGB-capable")
        return inert

    css_id = _swatch_css_id(column)
    colour = colours[column]
    swatch_css.append(
        f"#{css_id} {{ background-color: rgb({colour['r']},{colour['g']},{colour['b']}); }}"
    )

    btn = Gtk.Button()
    btn.set_size_request(w, h)
    btn.set_halign(Gtk.Align.CENTER)
    btn.add_css_class("lighting-paint-cell")
    btn.set_name(css_id)
    btn.set_tooltip_text(input_label(inp))

    def on_clicked(_b):
        updated = [dict(c) for c in colours]
        updated[column] = dict(paint_colour)
        _commit_lighting(
            client, config, profile, {"type": "custom_layout", "colours": updated}, on_change
        )

    btn.connect("clicked", on_clicked)
    return btn


def build_lighting_device_area(
    client,
    config: dict,
    profile: str,
    lighting_mode: str,
    paint_colour: dict,
    on_change: Callable[[], None],
) -> Gtk.Widget:
    """The device area below the control strip: the real Custom-layout paint
    grid (ticket 05) while Custom layout is selected — sharing `build_
    device_geometry`'s exact physical positions with the Grid destination's
    own button grid — or a plain placeholder for every other mode (ticket
    04's original behaviour, unchanged). Not shown, not even dimmed, for
    any mode but Custom layout."""
    if lighting_mode != "custom_layout":
        placeholder = Gtk.Label(label="", css_classes=["dim"])
        placeholder.add_css_class("lighting-device-placeholder")
        placeholder.set_size_request(280, 160)
        return placeholder

    lighting = config["profiles"][profile]["lighting"]
    colours = _custom_layout_colours(lighting)
    swatch_css: list[str] = []

    def cell_factory(inp: str, w: int, h: int) -> Gtk.Widget:
        return _lighting_paint_cell(
            client, config, profile, colours, paint_colour, swatch_css, on_change, inp, w, h
        )

    device = build_device_geometry(cell_factory)
    # One rewrite of the one shared provider's content per render — bounded
    # to the 21 columns `_swatch_css_id` ever names, never growing with
    # rebuild count the way a fresh per-cell provider would.
    _install_lighting_swatch_provider().load_from_string("\n".join(swatch_css))
    device.add_css_class("lighting-paint-grid")
    return device


def build_lighting_content(
    client, config: dict, profile: str, ui_state: dict, on_change: Callable[[], None]
) -> Gtk.Widget:
    """The Lighting destination's whole content area (ticket 04, spec
    §"GUI"): a mode selector + brightness strip so that row's height stays
    constant across every mode (the ticket's own prototype rejected a
    vertical stack for exactly that reason), then a second row pairing
    Copy-from-Profile with the per-effect params, above the device area
    (a placeholder, except the ticket 05 Custom-layout paint grid)."""
    lighting = config["profiles"][profile]["lighting"]
    mode = _lighting_mode(lighting)
    # Ticket 05: the paint grid's current-colour, surviving a rebuild the
    # same way `chord_ui` does — a plain local default would reset to white
    # every time a paint click's own `on_change` rebuilds this whole tree.
    paint_colour = ui_state.setdefault("lighting_paint_colour", dict(_DEFAULT_COLOUR))

    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)

    strip = Gtk.Box(spacing=16)
    strip.append(build_lighting_mode_selector(client, config, profile, on_change))
    strip.append(Gtk.Separator(orientation=Gtk.Orientation.VERTICAL))
    strip.append(build_lighting_brightness(client, config, profile, on_change))
    root.append(strip)

    copy_row = build_lighting_copy_from_profile(client, config, profile, on_change)
    copy_row.append(Gtk.Separator(orientation=Gtk.Orientation.VERTICAL))
    copy_row.append(build_lighting_params(client, config, profile, paint_colour, on_change))
    root.append(copy_row)
    root.append(Gtk.Separator())
    root.append(build_lighting_device_area(client, config, profile, mode, paint_colour, on_change))

    return root


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
    # Ticket 40: `selecting`/`recorded`/`edit_key`/`preview` survive a
    # rebuild the same way `dest`/`selected_layer` do — mutated in place by
    # `build_chords_section` and `input_btn`'s own click-override below.
    chord_ui = ui_state.setdefault(
        "chord",
        {"selecting": False, "recorded": [], "edit_key": None, "preview": None, "axis_error": None},
    )
    chords_on_layer = config["profiles"][profile][f"chords_{selected_layer}"]

    def chord_click_override(inp: str) -> None:
        # Ticket 71: an Axis-assigned Input can't produce the discrete
        # Down/Up transitions a Chord's own membership depends on (ticket
        # 59 §2) — clicking one while selecting surfaces an inline error
        # instead of toggling it into the selection or disabling the
        # button outright (ticket 60's Answer; the button itself stays
        # fully clickable at all times, per `make_input_button`'s own
        # always-visible `.axis-stripe` treatment).
        if inp in config["profiles"][profile][f"axis_{selected_layer}"]:
            chord_ui["axis_error"] = f"{input_label(inp)} is Axis-assigned — can't join a Chord"
            on_change()
            return
        chord_ui["axis_error"] = None
        if inp in chord_ui["recorded"]:
            chord_ui["recorded"].remove(inp)
        else:
            chord_ui["recorded"].append(inp)
        on_change()

    def input_btn(inp: str, w=100, h=100) -> Gtk.Button:
        # Only the Mode key's sensitivity depends on anything beyond `inp`
        # itself (`mode_key_role`, above) — computed here rather than taken
        # as call-site args now that every caller goes through
        # `build_device_geometry`'s fixed `cell_factory(inp, w, h)` shape.
        sensitive = mode_key_bindable if inp == "mode_key" else True
        insensitive_reason = (
            "Layer-shift Mode key: switch it to Bound above to give it its own Binding"
            if inp == "mode_key"
            else None
        )
        chord_classes, chord_tooltip = _chord_button_style(chords_on_layer, config, chord_ui, inp)
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
            chord_classes,
            chord_tooltip,
            chord_click_override if chord_ui["selecting"] else None,
        )

    root = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=16)
    root.set_margin_top(12)
    root.set_margin_bottom(12)
    root.set_margin_start(12)
    root.set_margin_end(12)

    if dest == "library":
        root.append(build_library_sidebar(client, config, ui_state, on_change))
    else:
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
        right.append(build_library_content(client, config, profile, selected_layer, ui_state, on_change))
    elif dest == "lighting":
        right.append(build_lighting_content(client, config, profile, ui_state, on_change))
    else:
        main = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=16)
        main.set_hexpand(True)

        main.append(build_layer_bar(client, selected_layer, mode_key_role, on_change, ui_state))

        device_row = Gtk.Box(spacing=16)
        # Round 2's own fix for round 1's grid crowding the Base/Held/Mode-key
        # row directly above it — a top margin here, distinct from `main`'s
        # own inter-row spacing, gives the grid itself room to breathe.
        device_row.set_margin_top(8)

        # Ticket 05 factored the actual row/col loop + wheel/Mode-key/
        # thumbstick placement into `build_device_geometry`, shared with the
        # Lighting tab's Custom-layout paint grid — `input_btn` above is
        # this destination's own `cell_factory`, unchanged behaviourally.
        device = build_device_geometry(input_btn)
        device_row.append(device)
        # tartarus-status-leds ticket 04: the Status LEDs lozenge group,
        # between the thumbstick column and the Chords section. Profile-scoped
        # — passes no `selected_layer`, renders identically on Base and Held.
        device_row.append(build_status_leds_section(client, config, profile, on_change))
        # Always visible while Grid is selected, no toggle (ticket 47's
        # round 2: nothing should rescale on open/close) — the real
        # Chord-recording flow (ticket 40).
        device_row.append(build_chords_section(client, config, profile, selected_layer, chord_ui, on_change))
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
