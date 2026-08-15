"""The Action Table — a collapsible sidebar off Device Overview (ticket 09),
closed by default, with no Profile/Layer pickers of its own: it reflects
whatever Device Overview already has selected rather than duplicating those
controls. One expandable row per Input; only bound Inputs show by default,
with a "Show all inputs" checkbox revealing passthrough rows too.
"""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk

from .binding_editor import action_summary, build_binding_editor
from .gtk_utils import clear_children
from .inputs import ALL_INPUTS, input_label


def build_table_row(
    client,
    config: dict,
    profile: str,
    layer: str,
    inp: str,
    on_change: Callable[[], None],
    ui_state: dict,
) -> Gtk.Expander:
    binding = config["profiles"][profile][layer].get(inp)
    expander = Gtk.Expander()
    header = Gtk.Box(spacing=12)
    header.append(Gtk.Label(label=input_label(inp), width_chars=8, xalign=0))
    header.append(Gtk.Label(label=action_summary(binding), hexpand=True, xalign=0))
    expander.set_label_widget(header)
    expander.set_child(build_binding_editor(client, config, profile, layer, inp, on_change))

    # GUI-only view state, same reasoning as ui_state["table_open"] (ticket
    # 09): on_change triggers a full rebuild from scratch, which would
    # otherwise re-collapse every row the user had expanded.
    expander.set_expanded(inp in ui_state["expanded_rows"])

    def on_notify_expanded(exp, _pspec):
        if exp.get_expanded():
            ui_state["expanded_rows"].add(inp)
        else:
            ui_state["expanded_rows"].discard(inp)

    expander.connect("notify::expanded", on_notify_expanded)
    return expander


def build_action_table(
    client, config: dict, profile: str, layer: str, on_change: Callable[[], None], ui_state: dict
) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    show_all = Gtk.CheckButton(label="Show all inputs")
    box.append(show_all)

    scroller = Gtk.ScrolledWindow(vexpand=True)
    listbox = Gtk.ListBox()
    listbox.add_css_class("boxed-list")
    scroller.set_child(listbox)

    bindings = config["profiles"][profile][layer]

    def render_rows():
        clear_children(listbox)
        for inp in ALL_INPUTS:
            if inp not in bindings and not show_all.get_active():
                continue
            listbox.append(build_table_row(client, config, profile, layer, inp, on_change, ui_state))

    show_all.connect("toggled", lambda b: render_rows())
    render_rows()
    box.append(scroller)
    return box
