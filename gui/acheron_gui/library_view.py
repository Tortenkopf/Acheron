# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""The real Steppers/Macros library screen (ticket 52 for Macros, ticket 55
for Steppers), replacing `device_overview.py`'s old placeholder for the
"Library" destination — the tab-switched panel pair ticket 31's prototype
settled on (variant B: two adjacent panels, never merged into one list,
"same widget shape as Device Overview's own Base/Held layer tabs"). Mounted
from `device_overview.build_main_view` whenever `ui_state["dest"] ==
"library"`.

Ticket 69/70 reorganized the Library destination into three columns,
reusing the Profile-sidebar's own slot rather than adding a fourth (and
superseding ticket 48's original "Profile sidebar stays exactly as it is,
in both destinations" for the Library case specifically):

- **Column 1** — `build_library_sidebar`, mounted by `device_overview.
  build_main_view` in the Profile-sidebar's own slot whenever `dest ==
  "library"`: `build_library_tabs` directly above the selected panel's
  browse list and "+ New" button — no "Profiles" chrome, no per-panel
  "Macros"/"Steppers" heading (the tab row already says which). Pinned at
  the same fixed 220px as `device_overview.build_profile_sidebar`
  (`gtk_utils.build_pinned_sidebar_box`) so nothing visibly resizes when
  flipping Grid↔Library.
- **Column 2** — the selected Macro/Stepper's own name heading plus its
  steps/items `ScrolledWindow` (unchanged 240px-cap treatment), now with
  the full vertical space column 1's old list used to occupy.
- **Column 3** — everything else that used to share one `editor_box` with
  column 2: the "Changes save automatically" hint, the error/toast label,
  the add-new-step/add-new-item controls, and (Stepper only) the
  `build_stepper_assignment_row`.

`build_library_content` builds columns 2+3 together (mounted by
`build_main_view` in the same "right" slot the Grid destination's own
device grid occupies); `build_library_sidebar` builds column 1 separately.
Profile switching is unreachable while Library is showing — accepted
directly (ticket 69's Answer): Macros/Steppers are Profile-agnostic
entities, and no ticket has surfaced a need to switch Profile mid-edit.

The Macro step editor is relocated near-verbatim from `binding_editor.py`'s
pre-ticket-51 inline step editor (git history, commit cb20cc9~1), now
operating against `MacroDef.steps` via the library (`client.set_macro_steps`,
ticket 52's own addition to the Daemon surface — `CreateMacro` alone only
covers the steps a Macro is born with) instead of a Binding's own inline
field, with round 2's ↑/↓ reorder buttons added alongside the original "×"
remove. Every mutation here — add/remove/reorder/rename/delete/create —
calls the Daemon and then a full `on_change()` rebuild, with no local
Save button, mirroring the Profile sidebar's own autosave convention
(ticket 31's Answer) — which is why the editor pane says so upfront.

The Steppers panel mirrors the Macros panel's browse-list-plus-editor shape
closely, with two settled differences (ticket 31 round 2's Answer) plus one
deliberate parity choice: delete is gated exactly like Macro's — disabled
with a "Used by N Binding(s) — can't delete" tooltip while
`stepper_used_by_count` is nonzero (`dispatch.rs`'s `DeleteStepper` handler,
landed by ticket 54, refuses exactly like `DeleteMacro` does; this ticket's
own text originally read that as "no gate," since ticket 03 never specified
an "in use" *concept*, but the user directed the GUI treatment to match
Macro's regardless — consistent delete UX across both library panels
outweighs that textual distinction). The two real differences: (1) the item
editor has no step-kind selector, since `StepperItem` has exactly one wire
variant (`Key`, covering both keyboard keys and mouse buttons through
`key_picker`'s one unified picker) unlike Macro's three (KeyDown/KeyUp/
Delay); (2) an assignment row (Forward/Backward Input dropdowns) sits below
the item list — a Stepper Binding has no other GUI surface that lets a
*list* pick its own Input pair (only the reverse: `binding_editor.py`'s
Stepper Action branch lets one Input pick a list).
Reassigning the *same* list's forward/backward off its old pair is the
Daemon's own job (`SetBinding`'s `take_stepper_direction_elsewhere`, ticket
54) — this module only needs to detect what the Daemon doesn't announce
back: silently overwriting a *different* list's Binding, or this same
list's *other* direction, at the newly-picked Input — both surfaced as a
one-shot toast via `ui_state["stepper_toast"]`.
"""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk, Pango

from .binding_editor import describe_step, labeled_row
from .controller_picker import LABEL_BY_CODE as CONTROLLER_LABEL_BY_CODE
from .controller_picker import build_inline_controller_picker
from .daemon_client import DaemonError
from .gtk_utils import build_name_prompt_popover, build_pinned_sidebar_box, clear_children
from .inputs import ALL_INPUTS, input_label
from .key_picker import LABEL_BY_CODE, build_inline_key_picker

# Ticket 92: the keyboard↔controller picker switcher's session-only mode,
# shared across both library editors (so working in controller mode "stays
# put" when moving between Steppers and Macros) via this single `ui_state`
# key. Resets to "keyboard" on GUI restart — not persisted to the daemon,
# matching `ui_state["dest"]`.
_PICKER_MODE_KEY = "library_picker_mode"
_DEFAULT_CONTROLLER_CODE = "BTN_SOUTH"

# Shown in the Macro step editor when a KeyDown/KeyUp step targets a
# controller button (ticket 92's Answer) — the polled-input dwell caveat
# the user must manage by hand in an authored sequence.
_CONTROLLER_MACRO_HINT = (
    "Controller buttons are polled by most games once per frame — add a Delay step of at "
    "least 35 ms between a button's Down and Up (and before pressing it again) or the "
    "press may not register."
)

_UNASSIGNED_LABEL = "— Unassigned —"

# Ticket 91: the Macro and Stepper editors are built to identical
# measurements so nothing visibly shifts when the user flips between the two
# library tabs. `build_macro_editor_columns` / `build_stepper_editor_columns`
# are structured in lockstep:
#
#   column 2 : name heading + vexpanding list scroller          (`_build_editor_col2`)
#   column 3 : error label + "+ Add …" button                  (pinned, shared)
#              + editor-specific middle (Stepper: the Forward/Backward
#                assignment row; Macro: `_header_middle_reserve()`, an inert
#                copy of that same widget stack so it occupies the identical
#                height on any theme) + a separator
#              + `_vscrollable` body: "Changes save automatically." hint,
#                then exactly one `labeled_row` (Macro: the step-kind
#                dropdown; Stepper: `labeled_row("Modifiers", …)`), then the
#                "Key"/"Delay (ms)" picker row — so the picker lands at the
#                same y on both tabs.
#
# No hardcoded pixel constants: every "reserve the same space" is done by
# building the same widget stack rather than a magic height (a shared
# `Gtk.SizeGroup` can't span the two, since only one editor is realized at a
# time).
_EDITOR_COL_SPACING = 6


def build_library_tabs(selected_tab: str, on_select: Callable[[str], None]) -> Gtk.Box:
    """Same widget shape as `device_overview.build_layer_bar`'s own Base/
    Held tabs — a plain button row toggling `suggested-action`, carrying no
    state of its own (the caller owns `ui_state`, same pattern as every
    other tab/destination switch in this GUI)."""
    row = Gtk.Box(spacing=6)
    for tab_key, label in (("steppers", "Steppers"), ("macros", "Macros")):
        btn = Gtk.Button(label=label)
        if tab_key == selected_tab:
            btn.add_css_class("suggested-action")

        def on_clicked(_b, tab_key=tab_key):
            on_select(tab_key)

        btn.connect("clicked", on_clicked)
        row.append(btn)
    return row


def macro_used_by_count(config: dict, macro_id: str) -> int:
    """How many Bindings, across every Profile's Base/Held Layer, reference
    `macro_id` — computed client-side from `GetConfig()`'s own data (no new
    wire field needed, unlike the ticket text's own phrasing might suggest):
    mirrors the real Daemon's `dispatch.rs::macro_references` scan exactly,
    just counted rather than boolean so the delete tooltip can name N."""
    return sum(
        1
        for profile in config["profiles"].values()
        for layer_key in ("base", "held")
        for binding in profile[layer_key].values()
        if binding.get("type") == "macro" and binding.get("macro_id") == macro_id
    )


def _sorted_macro_ids(macros: dict) -> list[str]:
    return sorted(macros, key=lambda mid: macros[mid]["name"].lower())


def build_macro_row(
    client,
    config: dict,
    macro_id: str,
    selected_macro_id: str | None,
    ui_state: dict,
    on_change: Callable[[], None],
    show_error: Callable[[Exception], None],
) -> Gtk.Box:
    name = config["macros"][macro_id]["name"]
    row = Gtk.Box(spacing=4)

    select_btn = Gtk.Button(label=name, hexpand=True)
    if macro_id == selected_macro_id:
        select_btn.add_css_class("suggested-action")

    def on_select_clicked(_b, macro_id=macro_id):
        ui_state["library_selected_macro"] = macro_id
        on_change()

    select_btn.connect("clicked", on_select_clicked)
    row.append(select_btn)

    rename_btn = Gtk.MenuButton(label="✎")
    rename_btn.set_tooltip_text(f"Rename {name!r}")

    def on_rename_submitted(new_name: str, macro_id=macro_id):
        client.rename_macro(macro_id, new_name)
        on_change()

    rename_btn.set_popover(
        build_name_prompt_popover(f"Renaming {name!r}", name, "Rename", on_rename_submitted)
    )
    row.append(rename_btn)

    used_by = macro_used_by_count(config, macro_id)
    delete_btn = Gtk.Button(label="×")
    delete_btn.set_sensitive(used_by == 0)
    delete_btn.set_tooltip_text(
        f"Used by {used_by} Binding(s) — can't delete" if used_by else f"Delete {name!r}"
    )

    def on_delete_clicked(_b, macro_id=macro_id):
        try:
            client.delete_macro(macro_id)
        except DaemonError as exc:
            show_error(exc)
            return
        if ui_state.get("library_selected_macro") == macro_id:
            ui_state["library_selected_macro"] = None
        on_change()

    delete_btn.connect("clicked", on_delete_clicked)
    row.append(delete_btn)

    return row


def _selected_macro_id(config: dict, ui_state: dict) -> str | None:
    macros = config.get("macros", {})
    selected_macro_id = ui_state.get("library_selected_macro")
    if selected_macro_id not in macros:
        selected_macro_id = _sorted_macro_ids(macros)[0] if macros else None
        ui_state["library_selected_macro"] = selected_macro_id
    return selected_macro_id


def build_macros_browse_list(client, config: dict, ui_state: dict, on_change: Callable[[], None]) -> Gtk.Widget:
    """Column 1's Macro-tab content (ticket 70): the browse rows and "+ New"
    button alone — no heading (the tab row above already reads "Macros")
    and no width/`sidebar`-css treatment of its own, since the caller
    (`build_library_sidebar`) already wraps column 1 in
    `gtk_utils.build_pinned_sidebar_box`."""
    macros = config.get("macros", {})
    macro_ids = _sorted_macro_ids(macros)
    selected_macro_id = _selected_macro_id(config, ui_state)

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)

    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.set_visible(False)
    box.append(error_label)

    def show_error(exc: Exception) -> None:
        error_label.set_label(str(exc))
        error_label.set_visible(True)

    rows_list = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    for macro_id in macro_ids:
        rows_list.append(
            build_macro_row(client, config, macro_id, selected_macro_id, ui_state, on_change, show_error)
        )
    rows_scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER)
    # Ticket 70 follow-up, live-verified: column 1 now has the grid/chords
    # section's full height to work with (no editor_box crowding it below,
    # unlike ticket 61's original two-column layout), so a short list left
    # a slab of dead space beneath it under ticket 61's own "cap at 240,
    # size to content" treatment. `vexpand` alone (no `propagate_natural_
    # height`/`max_content_height`) lets the ScrolledWindow claim whatever
    # height column 1 actually has and still scroll past it — it does not
    # reintroduce ticket 61's "window grows past the screen" bug, since
    # that came from an unbounded plain Gtk.Box driving the window's own
    # natural-size request, not from a ScrolledWindow filling space it's
    # already been given.
    rows_scroller.set_vexpand(True)
    rows_scroller.set_child(rows_list)
    box.append(rows_scroller)

    new_btn = Gtk.MenuButton(label="+ New")

    def on_create_submitted(name: str):
        macro_id = client.create_macro(name, [])
        ui_state["library_selected_macro"] = macro_id
        on_change()

    new_btn.set_popover(build_name_prompt_popover("Creating a Macro", "", "Create", on_create_submitted))
    box.append(new_btn)

    return box


def _build_editor_col2(name: str, list_scroller: Gtk.Widget) -> Gtk.Box:
    """Column 2, built identically for both editors (ticket 91): the selected
    entry's name heading above its vexpanding steps/items list scroller. The
    heading ellipsizes rather than growing the column to fit a long
    Macro/Stepper name — otherwise the column-2/column-3 split would depend
    on the name and shift when flipping tabs."""
    col2 = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=_EDITOR_COL_SPACING)
    heading = Gtk.Label(label=name, xalign=0, css_classes=["heading"])
    heading.set_ellipsize(Pango.EllipsizeMode.END)
    col2.append(heading)
    col2.append(list_scroller)
    return col2


def _vexpanding_list_scroller(rows: Gtk.Widget) -> Gtk.ScrolledWindow:
    """The steps/items list container — same treatment for both editors
    (ticket 70 follow-up, kept identical by ticket 91): fills column 2's full
    height and scrolls past it, never an `hscrollbar`."""
    scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER)
    scroller.set_vexpand(True)
    scroller.set_child(rows)
    return scroller


def _dropdown_row_height() -> int:
    """The rendered height of a `labeled_row` wrapping a `Gtk.DropDown` on
    the current theme, measured from a throwaway probe rather than
    hardcoded (ticket 91). Both editors' single "row before the picker" is
    sized to this — the Macro editor's step-kind selector already is one —
    and the Macro editor reserves two of them (plus one inter-row gap) to
    match the Stepper editor's Forward/Backward assignment row."""
    probe = labeled_row("", Gtk.DropDown(model=Gtk.StringList.new([""])))
    _minimum, natural, _bl, _nbl = probe.measure(Gtk.Orientation.VERTICAL, -1)
    return natural


def _header_middle_reserve() -> Gtk.Widget:
    """The Macro editor's stand-in for the Stepper editor's Forward/Backward
    assignment row (ticket 91 #1): a blank box sized to that row's rendered
    height (`build_stepper_assignment_row` is a `_EDITOR_COL_SPACING` VBox
    of two dropdown `labeled_row`s), so the separator, the middle
    `labeled_row`, and the whole scrollable body land at the same y as
    their Stepper counterparts. Sized from `_dropdown_row_height()`, not a
    hardcoded pixel reserve."""
    spacer = Gtk.Box()
    spacer.set_size_request(-1, 2 * _dropdown_row_height() + _EDITOR_COL_SPACING)
    return spacer


def build_macro_editor_columns(
    client, config: dict, macro_id: str, ui_state: dict, on_change: Callable[[], None]
) -> tuple[Gtk.Widget, Gtk.Widget]:
    """Columns 2+3 for the selected Macro (ticket 70): column 2 is the name
    heading plus the steps list; column 3 is the error label and
    "+ Add step" pinned above a `_vscrollable` body holding the save-hint
    and the kind selector/key-value picker (ticket 70 follow-up) — both
    stay visible regardless of where the body is scrolled to, rather than
    merely sitting above the picker within the same scrolled content.
    Split from one `editor_box` so `build_library_content` can place them
    either side of column 1's old space, per the map's settled
    three-column shape."""
    macro = config["macros"][macro_id]
    steps = macro["steps"]

    # col3 is an unscrolled outer container: the error label and
    # "+ Add step" (appended below) stay pinned at the top, with the hint
    # and the kind selector/key-value picker inside a `_vscrollable` body
    # beneath them (ticket 70 follow-up, live-verified: the picker's own
    # expandable content can grow tall enough to scroll everything below
    # it out of view — the error label, the button, and (Stepper's own)
    # toast message are exactly what the user asked to stay visible
    # regardless of scroll position). Structured in lockstep with
    # `build_stepper_editor_columns` (ticket 91) — see `_header_middle_reserve`.
    col3 = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=_EDITOR_COL_SPACING)

    error_label = Gtk.Label(xalign=0, wrap=True, css_classes=["error"])
    error_label.set_visible(False)
    col3.append(error_label)

    body = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=_EDITOR_COL_SPACING)
    body.append(
        Gtk.Label(
            label="Changes save automatically.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )

    def persist(new_steps: list[dict]) -> None:
        try:
            client.set_macro_steps(macro_id, new_steps)
        except DaemonError as exc:
            error_label.set_label(str(exc))
            error_label.set_visible(True)
            return
        on_change()

    steps_list = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    for i, step in enumerate(steps):
        row_box = Gtk.Box(spacing=6)
        row_box.set_margin_top(2)
        row_box.set_margin_bottom(2)
        row_box.set_margin_start(4)
        row_box.set_margin_end(4)
        row_box.append(Gtk.Label(label=describe_step(step), hexpand=True, xalign=0))

        up_btn = Gtk.Button(label="↑")
        up_btn.set_sensitive(i > 0)

        def on_up(_b, i=i):
            new_steps = list(steps)
            new_steps[i - 1], new_steps[i] = new_steps[i], new_steps[i - 1]
            persist(new_steps)

        up_btn.connect("clicked", on_up)
        row_box.append(up_btn)

        down_btn = Gtk.Button(label="↓")
        down_btn.set_sensitive(i < len(steps) - 1)

        def on_down(_b, i=i):
            new_steps = list(steps)
            new_steps[i + 1], new_steps[i] = new_steps[i], new_steps[i + 1]
            persist(new_steps)

        down_btn.connect("clicked", on_down)
        row_box.append(down_btn)

        rm_btn = Gtk.Button(label="×")

        def on_remove(_b, i=i):
            new_steps = list(steps)
            new_steps.pop(i)
            persist(new_steps)

        rm_btn.connect("clicked", on_remove)
        row_box.append(rm_btn)

        steps_list.append(row_box)

    # Ticket 70 follow-up (kept in lockstep by ticket 91's `_build_editor_col2`
    # / `_vexpanding_list_scroller`): column 2 fills the full column height
    # column 1's old list used to occupy, so a short list leaves no dead space.
    col2 = _build_editor_col2(macro["name"], _vexpanding_list_scroller(steps_list))

    # "+ Add step" is appended straight into col3 (not body), pinning it
    # above the scrolled area entirely rather than merely above add_box
    # within it. `on_add` closes over `step_kind_dd`/`new_step_value`, both
    # assigned below but only read here at click time (Python's
    # late-binding closures), so construction order doesn't need to match
    # visual order.
    # Ticket 92 §3: each picker mode keeps its own independent draft —
    # `controller_key` alongside the existing keyboard `key` — so flipping
    # the switcher back and forth never clobbers either. Only the active
    # mode's value is what "+ Add step" commits.
    new_step_value = {"key": "KEY_A", "ms_text": "0", "controller_key": _DEFAULT_CONTROLLER_CODE}

    add_btn = Gtk.Button(label="+ Add step")

    def on_add(_b):
        kind_i = step_kind_dd.get_selected()
        if kind_i == 2:
            val = new_step_value["ms_text"]
            step = {"type": "delay_ms", "ms": int(val) if val.isdigit() else 0}
        else:
            kind = "key_down" if kind_i == 0 else "key_up"
            in_controller = ui_state.get(_PICKER_MODE_KEY, "keyboard") == "controller"
            code = new_step_value["controller_key"] if in_controller else new_step_value["key"]
            step = {"type": kind, "key": code}
        persist(list(steps) + [step])

    add_btn.connect("clicked", on_add)
    col3.append(add_btn)

    # Ticket 91 #1: the Stepper editor's column 3 carries a Forward/Backward
    # assignment row + separator here; the Macro editor reserves the same
    # vertical space (blank) + its own separator, so the scrollable body
    # below starts at the same y on both tabs.
    col3.append(_header_middle_reserve())
    col3.append(Gtk.Separator())

    add_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=_EDITOR_COL_SPACING)

    # Ticket 92 §3: the keyboard↔controller switcher, its own row directly
    # below the hint and above the step-kind dropdown — on *both* editors
    # (ticket 91 lockstep). It's greyed when the step-kind is Delay (no
    # value picker to switch between) and is orthogonal to the step-kind
    # dropdown: "controller button" is *not* a fourth step-kind, it just
    # changes which picker fills the value slot for a KeyDown/KeyUp step.
    switch_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    add_box.append(switch_slot)

    step_kind_dd = Gtk.DropDown(model=Gtk.StringList.new(["KeyDown", "KeyUp", "Delay (ms)"]))
    add_box.append(labeled_row("New step", step_kind_dd))

    value_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    add_box.append(value_slot)

    current_mode, rerender_switch = _mount_picker_mode_switch(
        switch_slot,
        ui_state,
        on_mode_changed=lambda _mode: render_value_slot(),
        sensitive=lambda: step_kind_dd.get_selected() != 2,
    )

    def render_value_slot():
        clear_children(value_slot)
        if step_kind_dd.get_selected() == 2:
            ms_entry = Gtk.Entry(text=new_step_value["ms_text"], width_chars=10)
            ms_entry.connect("changed", lambda e: new_step_value.__setitem__("ms_text", e.get_text()))
            # Ticket 91 #2: this field genuinely isn't a key (the step-kind
            # dropdown selects KeyDown / KeyUp / Delay).
            value_slot.append(labeled_row("Delay (ms)", ms_entry))
        elif current_mode() == "controller":
            def on_controller_changed(code: str) -> None:
                new_step_value["controller_key"] = code

            picker = build_inline_controller_picker(
                new_step_value["controller_key"], on_controller_changed
            )
            value_slot.append(labeled_row("Button", picker))
            value_slot.append(
                Gtk.Label(label=_CONTROLLER_MACRO_HINT, xalign=0, wrap=True, css_classes=["dim"])
            )
        else:
            def on_value_key_changed(code: str) -> None:
                new_step_value["key"] = code

            # No modifier warning here, same reasoning as the pre-ticket-51
            # editor this was ported from: a KeyDown-only step *is* that
            # warning's own recommended workaround, not a case it applies to.
            value_picker, _refresh = build_inline_key_picker(
                new_step_value["key"], on_value_key_changed, warn_predicate=lambda: False
            )
            # Ticket 91 #2: "Key", matching the grid-view key-picker's own
            # label and the Stepper item editor's.
            value_slot.append(labeled_row("Key", value_picker))

    def on_kind_changed(*_):
        rerender_switch()  # re-evaluate the greyed-when-Delay state
        render_value_slot()

    step_kind_dd.connect("notify::selected", on_kind_changed)
    rerender_switch()
    render_value_slot()

    body.append(add_box)
    col3.append(_vscrollable(body))

    return col2, col3


def _sorted_stepper_ids(steppers: dict) -> list[str]:
    return sorted(steppers, key=lambda sid: steppers[sid]["name"].lower())


def describe_stepper_item(item: dict) -> str:
    if item.get("type") == "controller_button":
        # Ticket 92: reuse the gamepad picker's catalog label, e.g.
        # "Btn: A / South". No modifier combination on this variant.
        raw = item["button"]
        return f"Btn: {CONTROLLER_LABEL_BY_CODE.get(raw, raw)}"
    raw_key = item["key"]
    key = LABEL_BY_CODE.get(raw_key, raw_key)
    mods = "+".join(m.capitalize() for m in item.get("modifiers", []))
    return f"{mods}+{key}" if mods else key


def build_library_picker_switch(selected_mode: str, on_select: Callable[[str], None]) -> Gtk.Box:
    """The keyboard↔controller picker switcher for the library editors
    (ticket 92's Answer §3) — a plain-text two-button segmented control, the
    same shape as `device_overview.build_destination_switch` (the Grid/
    Library switcher the user named as the reference), each button floored
    at `_dropdown_row_height()` so it reads a little shorter than the
    Grid/Library switcher's own buttons (the user's explicit request).
    Carries no state of its own: `on_select` writes the pick into
    `ui_state[_PICKER_MODE_KEY]` and re-renders, matching every other
    tab/destination switch in this GUI."""
    row = Gtk.Box(spacing=6)
    for mode_key, label in (("keyboard", "Keyboard / mouse"), ("controller", "Controller")):
        btn = Gtk.Button(label=label)
        btn.set_size_request(-1, _dropdown_row_height())
        if mode_key == selected_mode:
            btn.add_css_class("suggested-action")

        def on_clicked(_b, mode_key=mode_key):
            on_select(mode_key)

        btn.connect("clicked", on_clicked)
        row.append(btn)
    return row


def _mount_picker_mode_switch(
    switch_slot: Gtk.Box,
    ui_state: dict,
    *,
    on_mode_changed: Callable[[str], None],
    sensitive: Callable[[], bool] = lambda: True,
) -> tuple[Callable[[], str], Callable[[], None]]:
    """Ticket 92 §3: the shared keyboard↔controller switcher orchestration
    for both library editors — kept in one place so the two editors can't
    drift on the `ui_state[_PICKER_MODE_KEY]` contract. Renders the switcher
    row into `switch_slot` and returns `(current_mode, rerender)`:

    - `current_mode()` reads the shared session mode (default `"keyboard"`).
    - `rerender()` rebuilds the switcher row — call it when `sensitive()`'s
      inputs change (the Macro editor greys the switch on a Delay step).
    - Clicking a switch button writes the mode and calls `on_mode_changed`,
      the editor-specific reaction (re-render the value slot; the Stepper
      editor also toggles its Modifiers row).
    """

    def current_mode() -> str:
        return ui_state.get(_PICKER_MODE_KEY, "keyboard")

    def set_mode(mode: str) -> None:
        ui_state[_PICKER_MODE_KEY] = mode
        rerender()
        on_mode_changed(mode)

    def rerender() -> None:
        clear_children(switch_slot)
        switch = build_library_picker_switch(current_mode(), set_mode)
        switch.set_sensitive(sensitive())
        switch_slot.append(labeled_row("Picker", switch))

    return current_mode, rerender


def _stepper_pair_inputs(bindings: dict, stepper_id: str) -> dict[str, str | None]:
    """Which Input (if any), within `bindings` (one Profile/Layer's own
    flat Input->Binding map), currently carries this Stepper's forward/
    backward direction — at most one of each, mirroring the Daemon's own
    "at most one pair may reference a given list" invariant (ticket 03)."""
    result: dict[str, str | None] = {"forward": None, "backward": None}
    for inp, binding in bindings.items():
        if binding.get("type") == "step" and binding.get("stepper_id") == stepper_id:
            direction = binding.get("direction")
            if direction in result:
                result[direction] = inp
    return result


def stepper_used_by_count(config: dict, stepper_id: str) -> int:
    """How many Bindings, across every Profile's Base/Held Layer, reference
    `stepper_id` (either direction counts) — mirrors `macro_used_by_count`
    exactly, just for `dispatch.rs::stepper_references`'s Stepper
    counterpart, so the delete tooltip can name N the same way Macro's
    does."""
    return sum(
        1
        for profile in config["profiles"].values()
        for layer_key in ("base", "held")
        for binding in profile[layer_key].values()
        if binding.get("type") == "step" and binding.get("stepper_id") == stepper_id
    )


def build_stepper_row(
    client,
    config: dict,
    stepper_id: str,
    selected_stepper_id: str | None,
    ui_state: dict,
    on_change: Callable[[], None],
    show_error: Callable[[Exception], None],
) -> Gtk.Box:
    name = config["steppers"][stepper_id]["name"]
    row = Gtk.Box(spacing=4)

    select_btn = Gtk.Button(label=name, hexpand=True)
    if stepper_id == selected_stepper_id:
        select_btn.add_css_class("suggested-action")

    def on_select_clicked(_b, stepper_id=stepper_id):
        ui_state["library_selected_stepper"] = stepper_id
        on_change()

    select_btn.connect("clicked", on_select_clicked)
    row.append(select_btn)

    rename_btn = Gtk.MenuButton(label="✎")
    rename_btn.set_tooltip_text(f"Rename {name!r}")

    def on_rename_submitted(new_name: str, stepper_id=stepper_id):
        client.rename_stepper(stepper_id, new_name)
        on_change()

    rename_btn.set_popover(
        build_name_prompt_popover(f"Renaming {name!r}", name, "Rename", on_rename_submitted)
    )
    row.append(rename_btn)

    used_by = stepper_used_by_count(config, stepper_id)
    delete_btn = Gtk.Button(label="×")
    delete_btn.set_sensitive(used_by == 0)
    delete_btn.set_tooltip_text(
        f"Used by {used_by} Binding(s) — can't delete" if used_by else f"Delete {name!r}"
    )

    def on_delete_clicked(_b, stepper_id=stepper_id):
        try:
            client.delete_stepper(stepper_id)
        except DaemonError as exc:
            show_error(exc)
            return
        if ui_state.get("library_selected_stepper") == stepper_id:
            ui_state["library_selected_stepper"] = None
        on_change()

    delete_btn.connect("clicked", on_delete_clicked)
    row.append(delete_btn)

    return row


def build_stepper_assignment_row(
    client,
    config: dict,
    profile: str,
    layer: str,
    stepper_id: str,
    ui_state: dict,
    on_change: Callable[[], None],
    show_error: Callable[[Exception], None],
) -> Gtk.Widget:
    """The Forward/Backward Input dropdowns beneath a Stepper's item list
    (ticket 31 round 2's Answer). Scoped to the currently selected Profile/
    Layer (the same pair Device Overview's own per-key editor targets,
    threaded in from `device_overview.build_main_view`) since `SetBinding`/
    `ClearBinding` always operate on the Daemon's *active* Profile — matching
    every other Binding mutation in this GUI, not a Stepper-specific choice.

    Reassigning the *same* list off its own old pair (a different Input)
    needs no client-side logic: `SetBinding`'s `Action::Step` handling
    already does that server-side (`take_stepper_direction_elsewhere`,
    ticket 54). Two things it does *not* announce, and this function
    detects itself (by reading the target Input's existing Binding before
    calling `SetBinding`), leaving a one-shot toast in
    `ui_state["stepper_toast"]` for `build_stepper_editor_columns`'s next
    render to show: a plain overwrite stealing a *different* list's Binding
    at the newly-picked Input, and — since `take_stepper_direction_
    elsewhere` only guards the *same* direction living on two Inputs — this
    *same* list's other direction already sitting on the newly-picked Input
    (e.g. reassigning Forward onto the Input that's currently this list's
    own Backward), which an ordinary Binding-slot overwrite would otherwise
    drop with no signal at all.
    """
    bindings = config["profiles"][profile][layer]
    pair = _stepper_pair_inputs(bindings, stepper_id)
    direction_labels = {"forward": "Forward", "backward": "Backward"}

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    for direction, direction_label in (("forward", "Forward"), ("backward", "Backward")):
        current_input = pair[direction]
        option_labels = [_UNASSIGNED_LABEL] + [input_label(inp) for inp in ALL_INPUTS]
        dd = Gtk.DropDown(model=Gtk.StringList.new(option_labels))
        dd.set_selected(0 if current_input is None else ALL_INPUTS.index(current_input) + 1)

        def on_changed(dd, _pspec, direction=direction, current_input=current_input):
            idx = dd.get_selected()
            new_input = None if idx == 0 else ALL_INPUTS[idx - 1]
            if new_input == current_input:
                return
            if new_input is None:
                try:
                    client.clear_binding(current_input, layer)
                except DaemonError as exc:
                    show_error(exc)
                    return
                on_change()
                return
            existing = bindings.get(new_input)
            toast = None
            if existing and existing.get("type") == "step":
                other_id = existing.get("stepper_id")
                other_direction = existing.get("direction")
                if other_id != stepper_id:
                    other_name = config.get("steppers", {}).get(other_id, {}).get("name", other_id)
                    toast = f"Moved off {other_name!r} (it no longer has an assigned pair)"
                elif other_direction != direction:
                    # This Input already carries *this same* list's other
                    # direction — plain SetBinding overwrite loses it with
                    # no server-side signal (`take_stepper_direction_
                    # elsewhere` only guards the *same* direction living on
                    # two Inputs, not this case), so it needs its own toast
                    # too, not just the cross-list "stolen" one above.
                    toast = f"Also cleared this list's own {direction_labels[other_direction]} assignment (it was on the same Input)"
            try:
                client.set_binding(
                    new_input,
                    layer,
                    {"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": direction},
                )
            except DaemonError as exc:
                show_error(exc)
                return
            if toast is not None:
                ui_state["stepper_toast"] = toast
            on_change()

        dd.connect("notify::selected", on_changed)
        box.append(labeled_row(direction_label, dd))
    return box


def _selected_stepper_id(config: dict, ui_state: dict) -> str | None:
    steppers = config.get("steppers", {})
    selected_stepper_id = ui_state.get("library_selected_stepper")
    if selected_stepper_id not in steppers:
        selected_stepper_id = _sorted_stepper_ids(steppers)[0] if steppers else None
        ui_state["library_selected_stepper"] = selected_stepper_id
    return selected_stepper_id


def build_steppers_browse_list(client, config: dict, ui_state: dict, on_change: Callable[[], None]) -> Gtk.Widget:
    """Column 1's Stepper-tab content — the Macro browse list's counterpart,
    see `build_macros_browse_list`."""
    steppers = config.get("steppers", {})
    stepper_ids = _sorted_stepper_ids(steppers)
    selected_stepper_id = _selected_stepper_id(config, ui_state)

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)

    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.set_visible(False)
    box.append(error_label)

    def show_error(exc: Exception) -> None:
        error_label.set_label(str(exc))
        error_label.set_visible(True)

    rows_list = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    for stepper_id in stepper_ids:
        rows_list.append(
            build_stepper_row(client, config, stepper_id, selected_stepper_id, ui_state, on_change, show_error)
        )
    rows_scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER)
    # See build_macros_browse_list's matching comment — same fix, same
    # reasoning, for the Steppers browse list.
    rows_scroller.set_vexpand(True)
    rows_scroller.set_child(rows_list)
    box.append(rows_scroller)

    new_btn = Gtk.MenuButton(label="+ New")

    def on_create_submitted(name: str):
        stepper_id = client.create_stepper(name, [])
        ui_state["library_selected_stepper"] = stepper_id
        on_change()

    new_btn.set_popover(build_name_prompt_popover("Creating a Stepper", "", "Create", on_create_submitted))
    box.append(new_btn)

    return box


def build_stepper_editor_columns(
    client,
    config: dict,
    profile: str,
    layer: str,
    stepper_id: str,
    ui_state: dict,
    on_change: Callable[[], None],
) -> tuple[Gtk.Widget, Gtk.Widget]:
    """Columns 2+3 for the selected Stepper (ticket 70) — the Macro editor
    columns' counterpart, see `build_macro_editor_columns`. Column 3's
    pinned header — the toast (if any), the error label, "+ Add item", and
    the Forward/Backward `build_stepper_assignment_row` — sits above a
    `_vscrollable` body holding the hint label and the "New item" key
    picker (ticket 70 follow-up): the header stays visible regardless of
    where the body is scrolled to, rather than merely sitting above the
    picker within the same scrolled content."""
    stepper = config["steppers"][stepper_id]
    items = stepper["items"]

    # col3 is an unscrolled outer container: the toast/error-label/
    # "+ Add item"/assignment-row header (appended below) stays pinned at
    # the top, with the hint and the "New item" key picker inside a
    # `_vscrollable` body beneath it. The buttons, the error label, and the
    # toast are exactly what the user asked to stay visible regardless of
    # scroll position (ticket 70 follow-up, live-verified). Structured in
    # lockstep with `build_macro_editor_columns` (ticket 91).
    col3 = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=_EDITOR_COL_SPACING)

    toast = ui_state.pop("stepper_toast", None)
    if toast is not None:
        col3.append(Gtk.Label(label=toast, xalign=0, wrap=True, css_classes=["toast"]))

    error_label = Gtk.Label(xalign=0, wrap=True, css_classes=["error"])
    error_label.set_visible(False)
    col3.append(error_label)

    body = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=_EDITOR_COL_SPACING)
    body.append(
        Gtk.Label(
            label="Changes save automatically.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )

    def show_error(exc: Exception) -> None:
        error_label.set_label(str(exc))
        error_label.set_visible(True)

    def persist(new_items: list[dict]) -> None:
        try:
            client.set_stepper_items(stepper_id, new_items)
        except DaemonError as exc:
            show_error(exc)
            return
        on_change()

    items_list = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    for i, item in enumerate(items):
        row_box = Gtk.Box(spacing=6)
        row_box.set_margin_top(2)
        row_box.set_margin_bottom(2)
        row_box.set_margin_start(4)
        row_box.set_margin_end(4)
        row_box.append(Gtk.Label(label=describe_stepper_item(item), hexpand=True, xalign=0))

        up_btn = Gtk.Button(label="↑")
        up_btn.set_sensitive(i > 0)

        def on_up(_b, i=i):
            new_items = list(items)
            new_items[i - 1], new_items[i] = new_items[i], new_items[i - 1]
            persist(new_items)

        up_btn.connect("clicked", on_up)
        row_box.append(up_btn)

        down_btn = Gtk.Button(label="↓")
        down_btn.set_sensitive(i < len(items) - 1)

        def on_down(_b, i=i):
            new_items = list(items)
            new_items[i + 1], new_items[i] = new_items[i], new_items[i + 1]
            persist(new_items)

        down_btn.connect("clicked", on_down)
        row_box.append(down_btn)

        rm_btn = Gtk.Button(label="×")

        def on_remove(_b, i=i):
            new_items = list(items)
            new_items.pop(i)
            persist(new_items)

        rm_btn.connect("clicked", on_remove)
        row_box.append(rm_btn)

        items_list.append(row_box)

    # Same `_build_editor_col2` / `_vexpanding_list_scroller` as the Macro
    # editor (ticket 70 follow-up, kept in lockstep by ticket 91).
    col2 = _build_editor_col2(stepper["name"], _vexpanding_list_scroller(items_list))

    # "+ Add item" and the Forward/Backward assignment row are appended
    # straight into col3 (not body), pinning them above the scrolled area
    # entirely rather than merely above add_box within it. `on_add` closes
    # over `new_item_value`, assigned below but only read here at click
    # time (Python's late-binding closures), so construction order doesn't
    # need to match visual order.
    # Ticket 92 §3: each picker mode keeps its own independent draft —
    # `controller_key` alongside the keyboard `key`/`modifiers` — so
    # flipping the switcher never clobbers either. Only the active mode's
    # value is what "+ Add item" commits.
    new_item_value = {"key": "KEY_A", "modifiers": [], "controller_key": _DEFAULT_CONTROLLER_CODE}

    add_btn = Gtk.Button(label="+ Add item")

    def on_add(_b):
        if ui_state.get(_PICKER_MODE_KEY, "keyboard") == "controller":
            item = {"type": "controller_button", "button": new_item_value["controller_key"]}
        else:
            item = {
                "type": "key",
                "key": new_item_value["key"],
                "modifiers": sorted(new_item_value["modifiers"]),
            }
        persist(list(items) + [item])

    add_btn.connect("clicked", on_add)
    col3.append(add_btn)

    # Ticket 91 #1: the Forward/Backward assignment row + separator here is
    # what the Macro editor reserves the same blank space for (via
    # `_header_middle_reserve`), so both editors' scrollable bodies start at
    # the same y.
    col3.append(
        build_stepper_assignment_row(client, config, profile, layer, stepper_id, ui_state, on_change, show_error)
    )
    col3.append(Gtk.Separator())

    add_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=_EDITOR_COL_SPACING)

    # Ticket 92 §3: the keyboard↔controller switcher, its own row directly
    # below the hint and above the Modifiers row — on *both* editors so the
    # cross-tab lockstep (ticket 91) holds. Always functional here (a
    # Stepper item is either a keyboard/mouse key or a controller button).
    switch_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    add_box.append(switch_slot)

    def on_mode_changed(mode: str) -> None:
        mod_row.set_visible(mode != "controller")
        render_value_slot()

    current_mode, rerender_switch = _mount_picker_mode_switch(
        switch_slot, ui_state, on_mode_changed=on_mode_changed
    )

    # The same Ctrl/Shift/Alt/Super checkbox block `binding_editor.py`
    # renders for Keypress (ticket 62's Answer) — a Stepper item's modifier
    # combination compiles through the same canned mods-down/key/mods-up
    # sequence as Keypress (ticket 63).
    #
    # Ticket 91 #3: rendered *above* the "Key" picker row rather than below
    # it. The picker owns a tall on-screen keyboard grid that otherwise
    # pushed these checkboxes past the default window height (the user had
    # to scroll "a tiny bit" to see them); above the grid they're always
    # visible on first glance. Wrapped in a `labeled_row` (below) so it's
    # structurally the same one-row control as the Macro editor's step-kind
    # `labeled_row`, keeping the "Key" picker row at the same y on both tabs
    # without a hardcoded row height.
    #
    # Ticket 92 §3: hidden (not greyed) in controller mode — a gamepad
    # button has no modifier concept, so a greyed row would be pure clutter.
    mod_box = Gtk.Box(spacing=8)
    mod_box.set_valign(Gtk.Align.CENTER)
    # Floor this row at a real dropdown row's height so the "Key" picker row
    # lands at the same y as it does on the Macro tab, whose step-kind
    # `labeled_row` is exactly that shape (ticket 91). Measured, not
    # hardcoded — see `_dropdown_row_height`.
    mod_box.set_size_request(-1, _dropdown_row_height())
    for m in ("ctrl", "shift", "alt", "super"):
        cb = Gtk.CheckButton(label=m)
        cb.set_active(m in new_item_value["modifiers"])

        def on_mod(c, m=m):
            cur = set(new_item_value["modifiers"])
            if c.get_active():
                cur.add(m)
            else:
                cur.discard(m)
            new_item_value["modifiers"] = sorted(cur)

        cb.connect("toggled", on_mod)
        mod_box.append(cb)
    mod_row = labeled_row("Modifiers", mod_box)
    add_box.append(mod_row)

    value_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    add_box.append(value_slot)

    def render_value_slot() -> None:
        clear_children(value_slot)
        if current_mode() == "controller":
            def on_controller_changed(code: str) -> None:
                new_item_value["controller_key"] = code

            picker = build_inline_controller_picker(
                new_item_value["controller_key"], on_controller_changed
            )
            value_slot.append(labeled_row("Button", picker))
        else:
            def on_value_key_changed(code: str) -> None:
                new_item_value["key"] = code

            # The modifier warning is suppressed for a different reason than
            # the Macro editor's own KeyDown-only step: there the workaround
            # (Toggle + a KeyDown-only Macro step) *is* a KeyDown-only step;
            # here it's simply unreachable — a Stepper item always compiles
            # to a bare KeyDown/KeyUp pair (ticket 03/54) and Toggle is
            # disallowed for a Stepper Binding.
            value_picker, _refresh = build_inline_key_picker(
                new_item_value["key"], on_value_key_changed, warn_predicate=lambda: False
            )
            # Ticket 91 #2: "Key", matching the grid-view key-picker's own
            # label and the Macro step editor's KeyDown/KeyUp value row.
            value_slot.append(labeled_row("Key", value_picker))

    rerender_switch()
    mod_row.set_visible(current_mode() != "controller")
    render_value_slot()

    body.append(add_box)
    col3.append(_vscrollable(body))

    return col2, col3


def build_library_sidebar(client, config: dict, ui_state: dict, on_change: Callable[[], None]) -> Gtk.Widget:
    """Column 1 for the Library destination (ticket 70) — mounted by
    `device_overview.build_main_view` in the Profile-sidebar's own slot.
    The tab row plus the selected panel's browse list, pinned to the same
    fixed 220px width `device_overview.build_profile_sidebar` uses so
    nothing visibly resizes when flipping Grid↔Library."""
    selected_tab = ui_state.setdefault("library_tab", "macros")

    sidebar = build_pinned_sidebar_box()

    def on_tab_select(tab_key: str) -> None:
        ui_state["library_tab"] = tab_key
        on_change()

    sidebar.append(build_library_tabs(selected_tab, on_tab_select))
    sidebar.append(Gtk.Separator())

    if selected_tab == "steppers":
        sidebar.append(build_steppers_browse_list(client, config, ui_state, on_change))
    else:
        sidebar.append(build_macros_browse_list(client, config, ui_state, on_change))

    return sidebar


def _vscrollable(widget: Gtk.Widget) -> Gtk.Widget:
    """Wraps `widget` in a vertically-scrolling, horizontally-fixed
    `Gtk.ScrolledWindow` — column 3's fallback once its content grows
    taller than the window, ticket 70 follow-up: the inline key/item
    picker's own expandable modifier-group toggles (`key_picker`'s "hi
    keycaps"/numpad reveals) can make it far taller than column 3's own
    natural content. Used on column 3's *body* sub-box specifically (below
    "+ Add step"/"+ Add item" and, for Stepper, the toast/assignment row —
    a second ticket 70 follow-up), so those stay pinned above the scrolled
    area rather than scrolling away with everything else."""
    scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER)
    scroller.set_vexpand(True)
    scroller.set_hexpand(True)
    scroller.set_child(widget)
    return scroller


def build_library_content(
    client, config: dict, profile: str, layer: str, ui_state: dict, on_change: Callable[[], None]
) -> Gtk.Widget:
    """Columns 2+3 for the Library destination (ticket 70) — mounted by
    `device_overview.build_main_view` in the "right" slot below the
    Grid/Library switcher, the same slot the Grid destination's own device
    grid occupies. Reads `ui_state["library_tab"]`, already established by
    `build_library_sidebar`'s own render this cycle."""
    selected_tab = ui_state.setdefault("library_tab", "macros")

    root = Gtk.Box(spacing=16)
    root.set_hexpand(True)

    if selected_tab == "steppers":
        selected_stepper_id = _selected_stepper_id(config, ui_state)
        if selected_stepper_id is None:
            root.append(_empty_library_label("No Steppers yet — use “+ New” to create one."))
        else:
            _mount_editor_columns(
                root,
                *build_stepper_editor_columns(
                    client, config, profile, layer, selected_stepper_id, ui_state, on_change
                ),
            )
    else:
        selected_macro_id = _selected_macro_id(config, ui_state)
        if selected_macro_id is None:
            root.append(_empty_library_label("No Macros yet — use “+ New” to create one."))
        else:
            _mount_editor_columns(
                root,
                *build_macro_editor_columns(
                    client, config, selected_macro_id, ui_state, on_change
                ),
            )

    return root


def _empty_library_label(text: str) -> Gtk.Widget:
    return Gtk.Label(label=text, xalign=0, wrap=True, css_classes=["dim"])


def _mount_editor_columns(root: Gtk.Box, col2: Gtk.Widget, col3: Gtk.Widget) -> None:
    """Places the two editor columns identically for both tabs (ticket 91).
    Column 3 holds its natural width — which is the same on both tabs, since
    both editors' column 3 is built in lockstep and its width is driven by
    the shared inline key picker — so it never shifts when flipping tabs.
    Column 2 (the name + list, whose own natural width *does* vary with the
    step/item text) expands into whatever is left, absorbing that variation
    instead of passing it on as a visible jump."""
    col2.set_hexpand(True)
    col3.set_hexpand(False)
    root.append(col2)
    root.append(col3)
