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

from gi.repository import GLib, Gtk, Pango

from .binding_editor import action_summary, build_binding_editor, build_chord_binding_dialog
from .daemon_client import DaemonError
from .gtk_utils import build_name_prompt_popover, build_pinned_sidebar_box
from .inputs import GRID_COLS, GRID_ROWS, grid_input, input_label
from .library_view import build_library_content, build_library_sidebar

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
            # Ticket 71: read unconditionally by `build_binding_editor`
            # (every grid-button popover build) and `make_input_button`'s
            # own axis-stripe check, so this placeholder needs both present
            # too, not just the real seed Profile shape.
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
        other = set(_chord_members(key))
        if candidate <= other or other <= candidate:
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

    def on_saved():
        window.close()
        on_change()

    # No scrolling wrapper here — this window has no other container
    # imposing a height on it, so it always sizes to `editor`'s own natural
    # height, and `build_binding_editor` itself (ticket 70 follow-up)
    # already defers only its Actuation & release section (grid Inputs
    # only, needed less often) behind an internal scroll, so this window
    # is guaranteed tall enough on first open for everything above that —
    # heading, error, the Trigger/Action fields including the inline key/
    # mouse-button picker's full expanded shape — without scrolling, per
    # the user's own "always reachable" ask for those specifically.
    editor = build_binding_editor(client, config, profile, layer, inp, on_saved, capture_mode)
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

    def input_btn(inp: str, w=100, h=100, sensitive=True, insensitive_reason=None) -> Gtk.Button:
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
            100,
            100,
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
        diamond.attach(input_btn("thumbstick_left", 100, 100), 1, 0, 1, 1)
        diamond.attach(input_btn("thumbstick_down", 100, 100), 0, 1, 1, 1)
        diamond.attach(input_btn("thumbstick_up", 100, 100), 2, 1, 1, 1)
        diamond.attach(input_btn("thumbstick_right", 100, 100), 1, 2, 1, 1)
        stick_col.append(diamond)

        # Key 20 (the paddle below the diamond) is physically wider on the
        # hardware — 150×100, per ticket 87's settled sizing.
        stick_col.append(input_btn(grid_input(4, 5), 150, 100))
        device.append(stick_col)

        device_row.append(device)
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
