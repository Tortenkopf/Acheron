"""The real Steppers/Macros library screen (ticket 52), replacing
`device_overview.py`'s old placeholder for the "Library" destination — the
tab-switched panel pair ticket 31's prototype settled on (variant B: two
adjacent panels, never merged into one list, "same widget shape as Device
Overview's own Base/Held layer tabs"). Mounted from
`device_overview.build_main_view` whenever `ui_state["dest"] == "library"`.

Only the Macros panel is real here — Steppers stays an honest stub
(`build_steppers_stub`) pending ticket 55, which needs ticket 54's Daemon/
D-Bus surface this ticket doesn't build.

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
"""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk

from .binding_editor import describe_step, labeled_row
from .daemon_client import DaemonError
from .gtk_utils import build_name_prompt_popover, clear_children
from .key_picker import build_inline_key_picker


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


def build_steppers_stub() -> Gtk.Widget:
    """Ticket 55's job — the Stepper library needs ticket 54's Daemon/D-Bus
    surface first, which doesn't exist yet. An honest stub rather than a
    hidden tab, matching `device_overview.build_chords_placeholder`'s own
    "reserved slot, not built yet" convention."""
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    placeholder = Gtk.Label(
        label="Ticket 55 wires the real Stepper library screen into this tab.",
        xalign=0,
        wrap=True,
    )
    placeholder.add_css_class("dim")
    box.append(placeholder)
    return box


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


def build_macro_editor(
    client, config: dict, macro_id: str, on_change: Callable[[], None]
) -> Gtk.Widget:
    macro = config["macros"][macro_id]
    steps = macro["steps"]

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.append(Gtk.Label(label=macro["name"], xalign=0, css_classes=["heading"]))
    box.append(
        Gtk.Label(
            label="Changes save automatically.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )

    error_label = Gtk.Label(xalign=0, wrap=True, css_classes=["error"])
    error_label.set_visible(False)
    box.append(error_label)

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
    box.append(steps_list)

    add_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    step_kind_dd = Gtk.DropDown(model=Gtk.StringList.new(["KeyDown", "KeyUp", "Delay (ms)"]))
    add_box.append(labeled_row("New step", step_kind_dd))

    value_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    add_box.append(value_slot)

    new_step_value = {"key": "KEY_A", "ms_text": "0"}

    def render_value_slot():
        clear_children(value_slot)
        if step_kind_dd.get_selected() == 2:
            ms_entry = Gtk.Entry(text=new_step_value["ms_text"], width_chars=10)
            ms_entry.connect("changed", lambda e: new_step_value.__setitem__("ms_text", e.get_text()))
            value_slot.append(labeled_row("Value", ms_entry))
        else:
            def on_value_key_changed(code: str) -> None:
                new_step_value["key"] = code

            # No modifier warning here, same reasoning as the pre-ticket-51
            # editor this was ported from: a KeyDown-only step *is* that
            # warning's own recommended workaround, not a case it applies to.
            value_picker, _refresh = build_inline_key_picker(
                new_step_value["key"], on_value_key_changed, warn_predicate=lambda: False
            )
            value_slot.append(labeled_row("Value", value_picker))

    step_kind_dd.connect("notify::selected", lambda *_: render_value_slot())
    render_value_slot()

    add_btn = Gtk.Button(label="+ Add step")

    def on_add(_b):
        kind_i = step_kind_dd.get_selected()
        if kind_i == 0:
            step = {"type": "key_down", "key": new_step_value["key"]}
        elif kind_i == 1:
            step = {"type": "key_up", "key": new_step_value["key"]}
        else:
            val = new_step_value["ms_text"]
            step = {"type": "delay_ms", "ms": int(val) if val.isdigit() else 0}
        persist(list(steps) + [step])

    add_btn.connect("clicked", on_add)
    add_box.append(add_btn)
    box.append(add_box)

    return box


def build_macros_panel(
    client, config: dict, ui_state: dict, on_change: Callable[[], None]
) -> Gtk.Widget:
    macros = config.get("macros", {})
    macro_ids = _sorted_macro_ids(macros)

    selected_macro_id = ui_state.get("library_selected_macro")
    if selected_macro_id not in macros:
        selected_macro_id = macro_ids[0] if macro_ids else None
        ui_state["library_selected_macro"] = selected_macro_id

    root = Gtk.Box(spacing=16)

    list_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    list_box.add_css_class("sidebar")
    list_box.set_size_request(220, -1)
    heading = Gtk.Label(label="Macros", xalign=0)
    heading.add_css_class("heading")
    list_box.append(heading)

    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.set_visible(False)
    list_box.append(error_label)

    def show_error(exc: Exception) -> None:
        error_label.set_label(str(exc))
        error_label.set_visible(True)

    for macro_id in macro_ids:
        list_box.append(
            build_macro_row(client, config, macro_id, selected_macro_id, ui_state, on_change, show_error)
        )

    new_btn = Gtk.MenuButton(label="+ New")

    def on_create_submitted(name: str):
        macro_id = client.create_macro(name, [])
        ui_state["library_selected_macro"] = macro_id
        on_change()

    new_btn.set_popover(build_name_prompt_popover("Creating a Macro", "", "Create", on_create_submitted))
    list_box.append(new_btn)
    # Same round-2 fix as build_profile_sidebar: pin the list's width so it
    # doesn't compete for slack the editor pane below leaves unclaimed.
    list_box.set_hexpand(False)
    root.append(list_box)

    editor_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    editor_box.set_hexpand(True)
    if selected_macro_id is None:
        editor_box.append(
            Gtk.Label(
                label="No Macros yet — use “+ New” to create one.",
                xalign=0,
                wrap=True,
                css_classes=["dim"],
            )
        )
    else:
        editor_box.append(build_macro_editor(client, config, selected_macro_id, on_change))
    root.append(editor_box)

    return root


def build_library_view(
    client, config: dict, ui_state: dict, on_change: Callable[[], None]
) -> Gtk.Widget:
    # Defaults to the Macros tab, not the display order's "Steppers" first —
    # Steppers is an inert stub until ticket 55, so opening straight into it
    # would be a worse first look at the "Library" destination than the tab
    # that's actually real.
    selected_tab = ui_state.setdefault("library_tab", "macros")

    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)

    def on_tab_select(tab_key: str) -> None:
        ui_state["library_tab"] = tab_key
        on_change()

    root.append(build_library_tabs(selected_tab, on_tab_select))
    root.append(Gtk.Separator())

    if selected_tab == "steppers":
        root.append(build_steppers_stub())
    else:
        root.append(build_macros_panel(client, config, ui_state, on_change))

    return root
