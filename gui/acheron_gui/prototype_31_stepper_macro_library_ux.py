"""PROTOTYPE — throwaway, answers ticket 31 (Design the look and feel of one
shared library-picker GUI covering both Stepper and Macro):
.scratch/tartarus-input-expansion/issues/31-prototype-stepper-library-ux.md

Plan: three structurally different variants of the Stepper/Macro library
picker, switchable via a floating bottom pill (same GTK4 adaptation of the
skill's `?variant=` convention as tickets 19/30 — Prev/Next buttons plus
Left/Right arrow keys). Both library entities are mocked in memory
(`make_seed_state`) since neither is built in the real Daemon yet (ticket
15/03 only *designed* the shapes — the Not-yet-specified "Macro library
build ticket" and Stepper's own build ticket both wait on this prototype).
Item/step key-entry is a plain `Gtk.Entry` standing in for whichever picker
ticket 32 lands on (its own look-and-feel prototype hasn't run) — not a
redesign of that control, per this ticket's own scope note.

Variants:
    A — Unified Library: one list mixing Steppers and Macros (small type
        badge per row), mirroring `device_overview.build_profile_sidebar`'s
        exact chrome (name / rename "✎" / delete "×" / "+ New ▾"). One
        right-hand editor pane whose shape swaps per the selected row's
        type. Bets that "one library" is the right mental model even though
        the two constructs' deletion/reassignment semantics differ — the
        editor pane is what has to surface that honestly, not the list.
    B — Two Adjacent Panels: a tab switcher ("Steppers" / "Macros", same
        widget as Device Overview's own Base/Held layer tabs) flips between
        two structurally separate full-width panels, each with its own
        list chrome and its own editor below it. Bets that keeping the two
        constructs visually un-mixed is worth never showing both at once.
    C — Inline-first: no dedicated browse-first library page. A mock
        Binding editor (Action: Keypress/Macro/Stepper) gains an "existing
        item ▾ + Create new…" combo right where a Binding is edited; the
        item's own editor opens in a floating popover from an "Edit…"
        button beside the combo. A small "Manage Library ▾" MenuButton
        (beside the mock Profile sidebar) covers pure housekeeping —
        rename/delete/see-what's-referenced — with no assignment control
        of its own. Bets that *assigning* is the common case (done inline,
        at the Binding being edited) and *browsing the whole library* is
        rare.

Wipe me: nothing here persists past process exit; none of it should be
promoted as-is (see each variant's own fold-in note).

Run:
    python3 gui/prototype_31_stepper_macro_library_ux.py
"""

from __future__ import annotations

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk

from .binding_editor import describe_step
from .gtk_utils import clear_children
from .inputs import ALL_INPUTS, input_label

CSS = """
.heading { font-weight: bold; }
.dim { opacity: 0.6; font-size: smaller; }
.error { color: #e53935; font-size: smaller; }
.sidebar { padding: 8px; background-color: alpha(currentColor, 0.06); border-radius: 6px; }
.editor-pane { padding: 10px; background-color: alpha(currentColor, 0.04); border-radius: 6px; }
.badge { border-radius: 999px; padding: 0px 6px; font-size: smaller; font-weight: bold; }
.badge-stepper { background-color: #4a90e2; color: white; }
.badge-macro { background-color: #8e44ad; color: white; }
.toast { background-color: alpha(#e6991a, 0.18); border-radius: 6px; padding: 4px 8px; font-size: smaller; }
.switcher-pill { background-color: alpha(currentColor, 0.08); border: 1px solid alpha(currentColor, 0.25); border-radius: 999px; padding: 6px 14px; }
.variant-label { font-weight: bold; }
.tab-active { font-weight: bold; }
.used-by { font-size: smaller; opacity: 0.75; }
"""


# --- Shared store + mock model (identical across variants: what a Stepper
# list / Macro *is* is settled by tickets 03/15, not a design question) ---


def make_seed_state() -> dict:
    return {
        "steppers": [
            {
                "id": "weapon-cycle",
                "name": "Weapon cycle",
                "items": [
                    {"type": "key", "key": "KEY_1"},
                    {"type": "key", "key": "KEY_2"},
                    {"type": "mouse", "key": "BTN_SIDE"},
                ],
                "forward": None,
                "backward": None,
            },
            {
                "id": "build-menu",
                "name": "Build menu",
                "items": [{"type": "key", "key": "KEY_B"}, {"type": "key", "key": "KEY_N"}],
                "forward": "grid_r1c1",
                "backward": "grid_r1c2",
            },
        ],
        "macros": [
            {
                "id": "combo-ult",
                "name": "Ult combo",
                "steps": [
                    {"type": "key_down", "key": "KEY_Q"},
                    {"type": "delay_ms", "ms": 30},
                    {"type": "key_up", "key": "KEY_Q"},
                ],
                "used_by": ["Default / base / Grid 4"],
            },
            {
                "id": "gg-text",
                "name": "GG spam",
                "steps": [
                    {"type": "key_down", "key": "KEY_ENTER"},
                    {"type": "key_up", "key": "KEY_ENTER"},
                ],
                "used_by": [],
            },
        ],
        "toast": None,
    }


def assign_stepper(state: dict, stepper_id: str, forward: str | None, backward: str | None) -> str | None:
    """Mutates `stepper_id`'s forward/backward pair. Per ticket 03, only one
    list may reference a given Input pair at once — stealing an already-
    claimed pair silently frees it from whichever list held it, and this
    returns a toast message describing that theft (`None` if nothing was
    stolen). Mirrors ticket 03's own "reassigning silently moves it, no
    reject-at-save step" — the *moved-from* list is the one that loses the
    pair, not the one being edited here."""
    stolen_from = None
    if forward or backward:
        for other in state["steppers"]:
            if other["id"] == stepper_id:
                continue
            if forward and other["forward"] == forward:
                other["forward"] = None
                stolen_from = other
            if backward and other["backward"] == backward:
                other["backward"] = None
                stolen_from = other
    target = next(s for s in state["steppers"] if s["id"] == stepper_id)
    target["forward"], target["backward"] = forward, backward
    if stolen_from:
        return f"Moved off {stolen_from['name']!r} (it no longer has an assigned pair)"
    return None


def describe_assignment(stepper: dict) -> str:
    if not stepper["forward"] and not stepper["backward"]:
        return "Unassigned"
    fwd = input_label(stepper["forward"]) if stepper["forward"] else "—"
    bwd = input_label(stepper["backward"]) if stepper["backward"] else "—"
    return f"Forward: {fwd}   Backward: {bwd}"


def describe_item(item: dict) -> str:
    return item["key"] if item["type"] == "key" else f"[Mouse] {item['key']}"


# --- Shared low-level primitive 1: item key-entry (stands in for whatever
# ticket 02/32's picker lands on — a plain Entry, not a redesign) ---


def build_item_entry_row(on_add) -> Gtk.Box:
    row = Gtk.Box(spacing=6)
    kind_dd = Gtk.DropDown(model=Gtk.StringList.new(["Key", "Mouse button"]))
    entry = Gtk.Entry(text="KEY_A", width_chars=10)
    row.append(kind_dd)
    row.append(entry)
    add_btn = Gtk.Button(label="+ Add item")

    def on_click(b):
        kind = "key" if kind_dd.get_selected() == 0 else "mouse"
        on_add({"type": kind, "key": entry.get_text()})

    add_btn.connect("clicked", on_click)
    row.append(add_btn)
    return row


# --- Shared low-level primitive 2: Stepper item list (add/reorder/remove) ---


def build_stepper_item_list(items: list[dict], on_change) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    list_box = Gtk.ListBox()
    list_box.add_css_class("boxed-list")
    box.append(list_box)

    def render():
        clear_children(list_box)
        for i, item in enumerate(items):
            row = Gtk.Box(spacing=6)
            row.set_margin_top(2)
            row.set_margin_bottom(2)
            row.set_margin_start(4)
            row.set_margin_end(4)
            row.append(Gtk.Label(label=f"{i + 1}. {describe_item(item)}", hexpand=True, xalign=0))

            up_btn = Gtk.Button(label="↑")
            up_btn.set_sensitive(i > 0)

            def on_up(b, i=i):
                items[i - 1], items[i] = items[i], items[i - 1]
                render()
                on_change()

            up_btn.connect("clicked", on_up)
            row.append(up_btn)

            down_btn = Gtk.Button(label="↓")
            down_btn.set_sensitive(i < len(items) - 1)

            def on_down(b, i=i):
                items[i + 1], items[i] = items[i], items[i + 1]
                render()
                on_change()

            down_btn.connect("clicked", on_down)
            row.append(down_btn)

            rm_btn = Gtk.Button(label="×")

            def on_remove(b, i=i):
                items.pop(i)
                render()
                on_change()

            rm_btn.connect("clicked", on_remove)
            row.append(rm_btn)

            list_box.append(row)

    render()

    def on_add(item: dict):
        items.append(item)
        render()
        on_change()

    box.append(build_item_entry_row(on_add))
    return box


# --- Shared low-level primitive 3: Macro step list (relocated near-verbatim
# from binding_editor.py's inline editor — same widget, new home) ---


def build_macro_step_list(steps: list[dict], on_change) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    steps_list = Gtk.ListBox()
    steps_list.add_css_class("boxed-list")
    box.append(steps_list)

    def render_steps():
        clear_children(steps_list)
        for i, step in enumerate(steps):
            row_box = Gtk.Box(spacing=6)
            row_box.set_margin_top(2)
            row_box.set_margin_bottom(2)
            row_box.set_margin_start(4)
            row_box.set_margin_end(4)
            row_box.append(Gtk.Label(label=describe_step(step), hexpand=True, xalign=0))

            up_btn = Gtk.Button(label="↑")
            up_btn.set_sensitive(i > 0)

            def on_up(b, i=i):
                steps[i - 1], steps[i] = steps[i], steps[i - 1]
                render_steps()
                on_change()

            up_btn.connect("clicked", on_up)
            row_box.append(up_btn)

            down_btn = Gtk.Button(label="↓")
            down_btn.set_sensitive(i < len(steps) - 1)

            def on_down(b, i=i):
                steps[i + 1], steps[i] = steps[i], steps[i + 1]
                render_steps()
                on_change()

            down_btn.connect("clicked", on_down)
            row_box.append(down_btn)

            rm = Gtk.Button(label="×")

            def on_remove(b, i=i):
                steps.pop(i)
                render_steps()
                on_change()

            rm.connect("clicked", on_remove)
            row_box.append(rm)
            steps_list.append(row_box)

    render_steps()

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
            step = {"type": "key_down", "key": val}
        elif kind_i == 1:
            step = {"type": "key_up", "key": val}
        else:
            step = {"type": "delay_ms", "ms": int(val) if val.isdigit() else 0}
        steps.append(step)
        render_steps()
        on_change()

    add_btn.connect("clicked", on_add)
    add_box.append(add_btn)
    box.append(add_box)
    return box


# --- Shared low-level primitive 4: Input-pair picker for Stepper assignment ---


def build_input_dropdown(selected: str | None, on_change) -> Gtk.DropDown:
    labels = ["(none)"] + [input_label(i) for i in ALL_INPUTS]
    dd = Gtk.DropDown(model=Gtk.StringList.new(labels))
    dd.set_selected(0 if selected is None else ALL_INPUTS.index(selected) + 1)

    def on_notify(*_a):
        idx = dd.get_selected()
        on_change(None if idx == 0 else ALL_INPUTS[idx - 1])

    dd.connect("notify::selected", on_notify)
    return dd


def build_name_prompt_popover(initial_text: str, submit_label: str, on_submit) -> Gtk.Popover:
    """Same small Entry-plus-submit-button shape as
    `device_overview.build_name_prompt_popover` — reused here rather than
    reinvented, since Stepper/Macro naming is the identical UX question
    Profile naming already answered."""
    popover = Gtk.Popover()
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    for setter in (box.set_margin_top, box.set_margin_bottom, box.set_margin_start, box.set_margin_end):
        setter(8)
    entry = Gtk.Entry(text=initial_text)
    entry.set_width_chars(16)
    box.append(entry)
    error_label = Gtk.Label(xalign=0, wrap=True, css_classes=["error"])
    error_label.set_visible(False)
    box.append(error_label)

    def on_click(b):
        name = entry.get_text().strip()
        if not name:
            error_label.set_label("Name can't be empty")
            error_label.set_visible(True)
            return
        on_submit(name)
        popover.popdown()

    submit_btn = Gtk.Button(label=submit_label)
    submit_btn.add_css_class("suggested-action")
    submit_btn.connect("clicked", on_click)
    box.append(submit_btn)
    popover.set_child(box)
    return popover


# =====================================================================
# Variant A — Unified Library: one list, type badges, one editor pane
# =====================================================================


def build_variant_a(state: dict, on_toast) -> Gtk.Widget:
    root = Gtk.Box(spacing=16)
    root.set_margin_top(12)
    root.set_margin_bottom(12)
    root.set_margin_start(12)
    root.set_margin_end(12)

    sidebar = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4, css_classes=["sidebar"])
    sidebar.set_size_request(220, -1)
    sidebar.append(Gtk.Label(label="Library", xalign=0, css_classes=["heading"]))
    root.append(sidebar)

    editor_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, hexpand=True)
    root.append(editor_slot)

    selected = {"kind": None, "id": None}

    def all_rows():
        for s in state["steppers"]:
            yield "stepper", s
        for m in state["macros"]:
            yield "macro", m

    def find(kind, id_):
        pool = state["steppers"] if kind == "stepper" else state["macros"]
        return next(e for e in pool if e["id"] == id_)

    def render_sidebar():
        for child in list(sidebar)[1:]:
            sidebar.remove(child)
        for kind, entry in all_rows():
            row = Gtk.Box(spacing=4)
            badge = Gtk.Label(label="Step" if kind == "stepper" else "Macro")
            badge.add_css_class("badge")
            badge.add_css_class("badge-stepper" if kind == "stepper" else "badge-macro")
            row.append(badge)
            open_btn = Gtk.Button(label=entry["name"], hexpand=True)
            if selected["kind"] == kind and selected["id"] == entry["id"]:
                open_btn.add_css_class("suggested-action")

            def on_open(b, kind=kind, id_=entry["id"]):
                selected["kind"], selected["id"] = kind, id_
                render_sidebar()
                render_editor()

            open_btn.connect("clicked", on_open)
            row.append(open_btn)

            rename_btn = Gtk.MenuButton(label="✎")

            def on_renamed(new_name, kind=kind, id_=entry["id"]):
                find(kind, id_)["name"] = new_name
                render_sidebar()
                render_editor()

            rename_btn.set_popover(build_name_prompt_popover(entry["name"], "Rename", on_renamed))
            row.append(rename_btn)

            delete_btn = Gtk.Button(label="×")
            blocking = entry.get("used_by") if kind == "macro" else None
            if blocking:
                delete_btn.set_sensitive(False)
                delete_btn.set_tooltip_text(f"Used by {len(blocking)} Binding(s) — can't delete")
            else:
                delete_btn.set_tooltip_text(f"Delete {entry['name']!r}")

            def on_delete(b, kind=kind, id_=entry["id"]):
                pool = state["steppers"] if kind == "stepper" else state["macros"]
                pool[:] = [e for e in pool if e["id"] != id_]
                if selected["kind"] == kind and selected["id"] == id_:
                    selected["kind"], selected["id"] = None, None
                render_sidebar()
                render_editor()

            delete_btn.connect("clicked", on_delete)
            row.append(delete_btn)
            sidebar.append(row)

        new_btn = Gtk.MenuButton(label="+ New ▾")
        menu = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        for kind, label in (("stepper", "New Stepper list"), ("macro", "New Macro")):
            b = Gtk.Button(label=label)

            def on_new(_b, kind=kind):
                new_id = f"{kind}-{len(state['steppers']) + len(state['macros']) + 1}"
                if kind == "stepper":
                    state["steppers"].append(
                        {"id": new_id, "name": "New Stepper", "items": [], "forward": None, "backward": None}
                    )
                else:
                    state["macros"].append({"id": new_id, "name": "New Macro", "steps": [], "used_by": []})
                selected["kind"], selected["id"] = kind, new_id
                new_btn.popdown()
                render_sidebar()
                render_editor()

            b.connect("clicked", on_new)
            menu.append(b)
        popover = Gtk.Popover()
        popover.set_child(menu)
        new_btn.set_popover(popover)
        sidebar.append(new_btn)

    def render_editor():
        clear_children(editor_slot)
        if selected["kind"] is None:
            editor_slot.append(Gtk.Label(label="Select an entry, or create a new one.", css_classes=["dim"]))
            return
        entry = find(selected["kind"], selected["id"])
        pane = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["editor-pane"])
        pane.append(Gtk.Label(label=entry["name"], xalign=0, css_classes=["heading"]))

        if selected["kind"] == "stepper":
            pane.append(Gtk.Label(label="Items (order matters — steps forward/back through this list):", xalign=0))
            pane.append(build_stepper_item_list(entry["items"], lambda: None))

            pane.append(Gtk.Label(label="Assignment", xalign=0, css_classes=["heading"]))
            assign_label = Gtk.Label(label=describe_assignment(entry), xalign=0, css_classes=["dim"])
            pane.append(assign_label)

            assign_row = Gtk.Box(spacing=8)
            assign_row.append(Gtk.Label(label="Forward"))
            fwd_dd = build_input_dropdown(
                entry["forward"], lambda inp: on_assign(inp, entry["backward"])
            )
            assign_row.append(fwd_dd)
            assign_row.append(Gtk.Label(label="Backward"))
            bwd_dd = build_input_dropdown(
                entry["backward"], lambda inp: on_assign(entry["forward"], inp)
            )
            assign_row.append(bwd_dd)
            pane.append(assign_row)

            def on_assign(fwd, bwd):
                toast = assign_stepper(state, entry["id"], fwd, bwd)
                assign_label.set_label(describe_assignment(entry))
                if toast:
                    on_toast(toast)
                render_sidebar()

        else:
            pane.append(build_macro_step_list(entry["steps"], lambda: None))
            used = entry.get("used_by", [])
            if used:
                pane.append(
                    Gtk.Label(
                        label="Used by: " + ", ".join(used), xalign=0, wrap=True, css_classes=["used-by"]
                    )
                )
            else:
                pane.append(Gtk.Label(label="Not currently used by any Binding.", xalign=0, css_classes=["dim"]))

        editor_slot.append(pane)

    render_sidebar()
    render_editor()
    return root


# =====================================================================
# Variant B — Two Adjacent Panels: a tab switch, never mixed
# =====================================================================


def build_variant_b(state: dict, on_toast) -> Gtk.Widget:
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    root.set_margin_top(12)
    root.set_margin_bottom(12)
    root.set_margin_start(12)
    root.set_margin_end(12)

    tabs = Gtk.Box(spacing=6)
    root.append(tabs)
    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, hexpand=True, vexpand=True)
    root.append(content)

    tab_state = {"which": "stepper"}

    def render_tabs():
        clear_children(tabs)
        for key, label in (("stepper", "Steppers"), ("macro", "Macros")):
            btn = Gtk.Button(label=label)
            if tab_state["which"] == key:
                btn.add_css_class("suggested-action")

            def on_click(b, key=key):
                tab_state["which"] = key
                render_tabs()
                render_content()

            btn.connect("clicked", on_click)
            tabs.append(btn)

    def render_content():
        clear_children(content)
        if tab_state["which"] == "stepper":
            content.append(build_stepper_panel(state, on_toast))
        else:
            content.append(build_macro_panel(state))

    render_tabs()
    render_content()
    return root


def build_stepper_panel(state: dict, on_toast) -> Gtk.Widget:
    panel = Gtk.Box(spacing=16)
    sidebar = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4, css_classes=["sidebar"])
    sidebar.set_size_request(200, -1)
    sidebar.append(Gtk.Label(label="Stepper lists", xalign=0, css_classes=["heading"]))
    panel.append(sidebar)
    editor_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, hexpand=True)
    panel.append(editor_slot)

    selected = {"id": state["steppers"][0]["id"] if state["steppers"] else None}

    def find(id_):
        return next(s for s in state["steppers"] if s["id"] == id_)

    def render_sidebar():
        for child in list(sidebar)[1:]:
            sidebar.remove(child)
        for s in state["steppers"]:
            row = Gtk.Box(spacing=4)
            btn = Gtk.Button(label=s["name"], hexpand=True)
            if selected["id"] == s["id"]:
                btn.add_css_class("suggested-action")

            def on_open(b, id_=s["id"]):
                selected["id"] = id_
                render_sidebar()
                render_editor()

            btn.connect("clicked", on_open)
            row.append(btn)

            rename_btn = Gtk.MenuButton(label="✎")

            def on_renamed(new_name, id_=s["id"]):
                find(id_)["name"] = new_name
                render_sidebar()
                render_editor()

            rename_btn.set_popover(build_name_prompt_popover(s["name"], "Rename", on_renamed))
            row.append(rename_btn)

            delete_btn = Gtk.Button(label="×", tooltip_text=f"Delete {s['name']!r}")

            def on_delete(b, id_=s["id"]):
                state["steppers"][:] = [e for e in state["steppers"] if e["id"] != id_]
                if selected["id"] == id_:
                    selected["id"] = state["steppers"][0]["id"] if state["steppers"] else None
                render_sidebar()
                render_editor()

            delete_btn.connect("clicked", on_delete)
            row.append(delete_btn)
            sidebar.append(row)

        new_btn = Gtk.Button(label="+ New Stepper list")

        def on_new(b):
            new_id = f"stepper-{len(state['steppers']) + 1}"
            state["steppers"].append(
                {"id": new_id, "name": "New Stepper", "items": [], "forward": None, "backward": None}
            )
            selected["id"] = new_id
            render_sidebar()
            render_editor()

        new_btn.connect("clicked", on_new)
        sidebar.append(new_btn)

    def render_editor():
        clear_children(editor_slot)
        if selected["id"] is None:
            editor_slot.append(Gtk.Label(label="No Stepper lists yet.", css_classes=["dim"]))
            return
        entry = find(selected["id"])
        pane = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["editor-pane"])
        pane.append(Gtk.Label(label=entry["name"], xalign=0, css_classes=["heading"]))
        pane.append(
            Gtk.Label(
                label="Changes save automatically as you edit — there's no Save button here.",
                xalign=0,
                css_classes=["dim"],
            )
        )
        pane.append(build_stepper_item_list(entry["items"], lambda: None))
        assign_label = Gtk.Label(label=describe_assignment(entry), xalign=0, css_classes=["dim"])
        pane.append(assign_label)
        assign_row = Gtk.Box(spacing=8)
        assign_row.append(Gtk.Label(label="Forward"))
        assign_row.append(build_input_dropdown(entry["forward"], lambda inp: on_assign(inp, entry["backward"])))
        assign_row.append(Gtk.Label(label="Backward"))
        assign_row.append(build_input_dropdown(entry["backward"], lambda inp: on_assign(entry["forward"], inp)))
        pane.append(assign_row)

        def on_assign(fwd, bwd):
            toast = assign_stepper(state, entry["id"], fwd, bwd)
            assign_label.set_label(describe_assignment(entry))
            if toast:
                on_toast(toast)
            render_sidebar()

        editor_slot.append(pane)

    render_sidebar()
    render_editor()
    return panel


def build_macro_panel(state: dict) -> Gtk.Widget:
    panel = Gtk.Box(spacing=16)
    sidebar = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4, css_classes=["sidebar"])
    sidebar.set_size_request(200, -1)
    sidebar.append(Gtk.Label(label="Macros", xalign=0, css_classes=["heading"]))
    panel.append(sidebar)
    editor_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, hexpand=True)
    panel.append(editor_slot)

    selected = {"id": state["macros"][0]["id"] if state["macros"] else None}

    def find(id_):
        return next(m for m in state["macros"] if m["id"] == id_)

    def render_sidebar():
        for child in list(sidebar)[1:]:
            sidebar.remove(child)
        for m in state["macros"]:
            row = Gtk.Box(spacing=4)
            btn = Gtk.Button(label=m["name"], hexpand=True)
            if selected["id"] == m["id"]:
                btn.add_css_class("suggested-action")

            def on_open(b, id_=m["id"]):
                selected["id"] = id_
                render_sidebar()
                render_editor()

            btn.connect("clicked", on_open)
            row.append(btn)

            rename_btn = Gtk.MenuButton(label="✎")

            def on_renamed(new_name, id_=m["id"]):
                find(id_)["name"] = new_name
                render_sidebar()
                render_editor()

            rename_btn.set_popover(build_name_prompt_popover(m["name"], "Rename", on_renamed))
            row.append(rename_btn)

            delete_btn = Gtk.Button(label="×")
            if m["used_by"]:
                delete_btn.set_sensitive(False)
                delete_btn.set_tooltip_text(f"Used by {len(m['used_by'])} Binding(s) — can't delete")
            else:
                delete_btn.set_tooltip_text(f"Delete {m['name']!r}")

            def on_delete(b, id_=m["id"]):
                state["macros"][:] = [e for e in state["macros"] if e["id"] != id_]
                if selected["id"] == id_:
                    selected["id"] = state["macros"][0]["id"] if state["macros"] else None
                render_sidebar()
                render_editor()

            delete_btn.connect("clicked", on_delete)
            row.append(delete_btn)
            sidebar.append(row)

        new_btn = Gtk.Button(label="+ New Macro")

        def on_new(b):
            new_id = f"macro-{len(state['macros']) + 1}"
            state["macros"].append({"id": new_id, "name": "New Macro", "steps": [], "used_by": []})
            selected["id"] = new_id
            render_sidebar()
            render_editor()

        new_btn.connect("clicked", on_new)
        sidebar.append(new_btn)

    def render_editor():
        clear_children(editor_slot)
        if selected["id"] is None:
            editor_slot.append(Gtk.Label(label="No Macros yet.", css_classes=["dim"]))
            return
        entry = find(selected["id"])
        pane = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["editor-pane"])
        pane.append(Gtk.Label(label=entry["name"], xalign=0, css_classes=["heading"]))
        pane.append(
            Gtk.Label(
                label="Changes save automatically as you edit — there's no Save button here.",
                xalign=0,
                css_classes=["dim"],
            )
        )
        pane.append(build_macro_step_list(entry["steps"], lambda: None))
        if entry["used_by"]:
            pane.append(
                Gtk.Label(label="Used by: " + ", ".join(entry["used_by"]), xalign=0, wrap=True, css_classes=["used-by"])
            )
        else:
            pane.append(Gtk.Label(label="Not currently used by any Binding.", xalign=0, css_classes=["dim"]))
        editor_slot.append(pane)

    render_sidebar()
    render_editor()
    return panel


# =====================================================================
# Variant C — Inline-first: assign at the Binding, manage from a corner menu
# =====================================================================


def build_variant_c(state: dict, on_toast) -> Gtk.Widget:
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
    root.set_margin_top(12)
    root.set_margin_bottom(12)
    root.set_margin_start(12)
    root.set_margin_end(12)

    top_bar = Gtk.Box(spacing=8)
    top_bar.append(Gtk.Label(label="Device Overview (mock)", css_classes=["dim"], hexpand=True))
    manage_btn = Gtk.MenuButton(label="Manage Library ▾")
    manage_popover = Gtk.Popover()
    manage_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    for setter in (
        manage_box.set_margin_top,
        manage_box.set_margin_bottom,
        manage_box.set_margin_start,
        manage_box.set_margin_end,
    ):
        setter(8)
    manage_box.set_size_request(260, -1)

    def render_manage():
        clear_children(manage_box)
        manage_box.append(Gtk.Label(label="Housekeeping only — assign from a Binding", xalign=0, css_classes=["dim"]))
        for kind, pool in (("stepper", state["steppers"]), ("macro", state["macros"])):
            for entry in pool:
                row = Gtk.Box(spacing=4)
                badge = Gtk.Label(label="Step" if kind == "stepper" else "Macro", css_classes=["badge", "badge-stepper" if kind == "stepper" else "badge-macro"])
                row.append(badge)
                row.append(Gtk.Label(label=entry["name"], hexpand=True, xalign=0))
                rename_btn = Gtk.MenuButton(label="✎")

                def on_renamed(new_name, entry=entry):
                    entry["name"] = new_name
                    render_manage()

                rename_btn.set_popover(build_name_prompt_popover(entry["name"], "Rename", on_renamed))
                row.append(rename_btn)
                delete_btn = Gtk.Button(label="×")
                blocking = entry.get("used_by") if kind == "macro" else None
                if blocking:
                    delete_btn.set_sensitive(False)
                    delete_btn.set_tooltip_text(f"Used by {len(blocking)} Binding(s)")

                def on_delete(b, kind=kind, id_=entry["id"]):
                    tgt = state["steppers"] if kind == "stepper" else state["macros"]
                    tgt[:] = [e for e in tgt if e["id"] != id_]
                    render_manage()

                delete_btn.connect("clicked", on_delete)
                row.append(delete_btn)
                manage_box.append(row)

    render_manage()
    manage_popover.set_child(manage_box)
    manage_btn.set_popover(manage_popover)
    top_bar.append(manage_btn)
    root.append(top_bar)

    root.append(Gtk.Separator())

    editor = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["editor-pane"])
    editor.append(Gtk.Label(label="Mock Binding editor — Input: Grid 7", xalign=0, css_classes=["heading"]))
    root.append(editor)

    action_dd = Gtk.DropDown(model=Gtk.StringList.new(["Keypress", "Macro", "Stepper"]))
    action_dd.set_selected(1)
    editor.append(action_dd)

    slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    editor.append(slot)

    draft = {"assigned": state["macros"][0]["id"] if state["macros"] else None}

    def open_item_editor_popover(kind: str, entry: dict, anchor: Gtk.Widget) -> None:
        popover = Gtk.Popover()
        popover.set_parent(anchor)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        for setter in (box.set_margin_top, box.set_margin_bottom, box.set_margin_start, box.set_margin_end):
            setter(8)
        box.set_size_request(320, -1)
        box.append(Gtk.Label(label=f"Editing {entry['name']!r}", xalign=0, css_classes=["heading"]))
        if kind == "macro":
            box.append(build_macro_step_list(entry["steps"], lambda: None))
        else:
            box.append(build_stepper_item_list(entry["items"], lambda: None))
        close_btn = Gtk.Button(label="Done")
        close_btn.connect("clicked", lambda b: popover.popdown())
        box.append(close_btn)
        popover.set_child(box)
        popover.popup()

    def render_slot():
        clear_children(slot)
        kind_i = action_dd.get_selected()
        if kind_i == 0:
            entry_row = Gtk.Entry(text="KEY_A")
            slot.append(entry_row)
            return

        kind = "macro" if kind_i == 1 else "stepper"
        pool = state["macros"] if kind == "macro" else state["steppers"]

        row = Gtk.Box(spacing=6)
        options = [e["name"] for e in pool] + ["+ Create new…"]
        combo = Gtk.DropDown(model=Gtk.StringList.new(options))
        if draft["assigned"] and any(e["id"] == draft["assigned"] for e in pool):
            combo.set_selected([e["id"] for e in pool].index(draft["assigned"]))
        else:
            combo.set_selected(len(options) - 1 if pool else len(options) - 1)
        row.append(combo)

        edit_btn = Gtk.Button(label="Edit…")
        edit_btn.set_sensitive(bool(pool))

        def on_combo(*_a):
            idx = combo.get_selected()
            if idx == len(options) - 1:
                new_id = f"{kind}-{len(pool) + 1}"
                if kind == "macro":
                    pool.append({"id": new_id, "name": "New Macro", "steps": [], "used_by": []})
                else:
                    pool.append({"id": new_id, "name": "New Stepper", "items": [], "forward": None, "backward": None})
                draft["assigned"] = new_id
                render_manage()
                render_slot()
            else:
                draft["assigned"] = pool[idx]["id"]
                edit_btn.set_sensitive(True)

        combo.connect("notify::selected", on_combo)
        row.append(edit_btn)

        def on_edit(b):
            entry = next(e for e in pool if e["id"] == draft["assigned"])
            open_item_editor_popover(kind, entry, edit_btn)

        edit_btn.connect("clicked", on_edit)
        slot.append(row)

        if kind == "stepper" and draft["assigned"]:
            entry = next(e for e in pool if e["id"] == draft["assigned"])
            assign_row = Gtk.Box(spacing=8)
            assign_row.append(Gtk.Label(label="Forward"))
            assign_row.append(build_input_dropdown(entry["forward"], lambda inp: on_assign(entry, inp, entry["backward"])))
            assign_row.append(Gtk.Label(label="Backward"))
            assign_row.append(build_input_dropdown(entry["backward"], lambda inp: on_assign(entry, entry["forward"], inp)))
            slot.append(assign_row)
            note = Gtk.Label(label=describe_assignment(entry), xalign=0, css_classes=["dim"])
            slot.append(note)

            def on_assign(entry, fwd, bwd):
                toast = assign_stepper(state, entry["id"], fwd, bwd)
                note.set_label(describe_assignment(entry))
                if toast:
                    on_toast(toast)

    action_dd.connect("notify::selected", lambda *_: render_slot())
    render_slot()

    root.append(
        Gtk.Label(
            label="Fold-in note: this variant's 'Manage Library' popover duplicates the row-rendering "
            "logic variant A's sidebar already has — a real build would share one row-builder.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )
    return root


# --- Switcher chrome ---

VARIANTS = [
    ("A", "Unified library · one list, type badges, one editor pane"),
    ("B", "Two adjacent panels · tab switch, never mixed"),
    ("C", "Inline-first · assign at the Binding, manage from a corner menu"),
]


def build_window(app: Gtk.Application) -> Gtk.ApplicationWindow:
    provider = Gtk.CssProvider()
    provider.load_from_data(CSS.encode())
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
    )

    win = Gtk.ApplicationWindow(application=app, title="Ticket 31 prototype — Stepper/Macro library UX")
    win.set_default_size(760, 620)

    state = {"variant_states": {"A": make_seed_state(), "B": make_seed_state(), "C": make_seed_state()}}

    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
    win.set_child(outer)

    toast_label = Gtk.Label(css_classes=["toast"], xalign=0, wrap=True)
    toast_label.set_visible(False)
    outer.append(toast_label)

    def show_toast(msg: str) -> None:
        toast_label.set_label(msg)
        toast_label.set_visible(True)
        GLib.timeout_add(4000, lambda: (toast_label.set_visible(False), False)[1])

    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    for setter in (content.set_margin_top, content.set_margin_bottom, content.set_margin_start, content.set_margin_end):
        setter(4)
    outer.append(content)

    index = {"i": 0}

    def render():
        clear_children(content)
        key, label = VARIANTS[index["i"]]
        vstate = state["variant_states"][key]
        if key == "A":
            content.append(build_variant_a(vstate, show_toast))
        elif key == "B":
            content.append(build_variant_b(vstate, show_toast))
        else:
            content.append(build_variant_c(vstate, show_toast))
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
    app = Gtk.Application(application_id="com.acheron.prototype.ticket31")
    app.connect("activate", lambda a: build_window(a).present())
    app.run(None)


if __name__ == "__main__":
    main()
