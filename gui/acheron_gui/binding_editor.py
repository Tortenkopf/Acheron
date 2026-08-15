"""The shared Binding editor — one component used identically from Device
Overview's popovers and the Action Table sidebar's expandable rows (ticket
09's resolved IA). Trigger-mode/Macro-step UI is preserved in full from the
prototype (the wire encoding already round-trips every `TriggerMode`/
`Action::Macro` shape, per ticket 15) even though only Keypress/Fire-once
actually fires in the Daemon yet (ticket 17) — matching ticket 16's
"Macro/other-Trigger-mode UI can exist inert" allowance.

A Binding here is the same *flat* dict `GetConfig()`/`SetBinding` use on the
wire (`{"trigger": ..., "type": ..., "key"/"steps": ...}`), not the ticket
09 prototype's nested `{"trigger": ..., "action": {...}}` shape — this
editor edits exactly what the Daemon will hand back on the next read.
"""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk

from .daemon_client import DaemonError
from .gtk_utils import clear_children
from .inputs import ACTION_TYPES, TRIGGER_OPTIONS, TRIGGER_SHORT, input_label


def action_summary(binding: dict | None) -> str:
    if not binding:
        return "passthrough"
    if binding["type"] == "keypress":
        mods = "+".join(m.capitalize() for m in binding.get("modifiers", []))
        key = binding["key"].replace("KEY_", "")
        chord = f"{mods}+{key}" if mods else key
        return f"{chord}  [{TRIGGER_SHORT[binding['trigger']]}]"
    steps = binding.get("steps", [])
    return f"Macro ({len(steps)} steps)  [{TRIGGER_SHORT[binding['trigger']]}]"


def describe_step(step: dict) -> str:
    kind = step["type"]
    if kind == "key_down":
        return f"KeyDown {step['key']}"
    if kind == "key_up":
        return f"KeyUp {step['key']}"
    if kind == "delay_ms":
        return f"Delay {step['ms']}ms"
    return str(step)


def labeled_row(label: str, widget: Gtk.Widget) -> Gtk.Box:
    row = Gtk.Box(spacing=8)
    lbl = Gtk.Label(label=label, xalign=0)
    lbl.set_size_request(90, -1)
    row.append(lbl)
    widget.set_hexpand(True)
    row.append(widget)
    return row


def build_binding_editor(
    client,
    config: dict,
    profile: str,
    layer: str,
    inp: str,
    on_saved: Callable[[], None],
) -> Gtk.Widget:
    bindings = config["profiles"][profile][layer]
    existing = bindings.get(inp)
    starting = existing or {"trigger": "fire_once", "type": "keypress", "key": "KEY_A", "modifiers": []}

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    box.set_margin_top(10)
    box.set_margin_bottom(10)
    box.set_margin_start(10)
    box.set_margin_end(10)
    heading = Gtk.Label(label=f"{profile} / {layer} / {input_label(inp)}", xalign=0)
    heading.add_css_class("heading")
    box.append(heading)

    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.set_visible(False)
    box.append(error_label)

    trigger_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in TRIGGER_OPTIONS]))
    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index(starting["trigger"]))
    box.append(labeled_row("Trigger mode", trigger_dd))

    action_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in ACTION_TYPES]))
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index(starting["type"]))
    box.append(labeled_row("Action", action_dd))

    editor_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.append(editor_slot)

    draft = {
        "keypress": {"key": starting.get("key", "KEY_A"), "modifiers": list(starting.get("modifiers", []))}
        if starting["type"] == "keypress"
        else {"key": "KEY_A", "modifiers": []},
        "steps": list(starting.get("steps", [])) if starting["type"] == "macro" else [],
    }

    def render_action_editor():
        clear_children(editor_slot)

        kind = ACTION_TYPES[action_dd.get_selected()][0]
        if kind == "keypress":
            key_entry = Gtk.Entry(text=draft["keypress"].get("key", "KEY_A"))
            key_entry.connect("changed", lambda e: draft["keypress"].__setitem__("key", e.get_text()))
            editor_slot.append(labeled_row("Key", key_entry))

            mod_box = Gtk.Box(spacing=8)
            mods = set(draft["keypress"].get("modifiers", []))
            for m in ("ctrl", "shift", "alt", "super"):
                cb = Gtk.CheckButton(label=m)
                cb.set_active(m in mods)

                def on_mod(c, m=m):
                    cur = set(draft["keypress"].get("modifiers", []))
                    if c.get_active():
                        cur.add(m)
                    else:
                        cur.discard(m)
                    draft["keypress"]["modifiers"] = sorted(cur)

                cb.connect("toggled", on_mod)
                mod_box.append(cb)
            editor_slot.append(mod_box)
        else:
            steps_list = Gtk.ListBox()
            steps_list.add_css_class("boxed-list")

            def render_steps():
                clear_children(steps_list)
                for i, step in enumerate(draft["steps"]):
                    row_box = Gtk.Box(spacing=6)
                    row_box.set_margin_top(2)
                    row_box.set_margin_bottom(2)
                    row_box.set_margin_start(4)
                    row_box.set_margin_end(4)
                    row_box.append(Gtk.Label(label=describe_step(step), hexpand=True, xalign=0))
                    rm = Gtk.Button(label="×")

                    def on_remove(b, i=i):
                        draft["steps"].pop(i)
                        render_steps()

                    rm.connect("clicked", on_remove)
                    row_box.append(rm)
                    steps_list.append(row_box)

            render_steps()
            editor_slot.append(steps_list)

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
                draft["steps"].append(step)
                render_steps()

            add_btn.connect("clicked", on_add)
            add_box.append(add_btn)
            editor_slot.append(add_box)

    action_dd.connect("notify::selected", lambda *_: render_action_editor())
    render_action_editor()

    def show_error(exc: Exception):
        error_label.set_label(str(exc))
        error_label.set_visible(True)

    btn_row = Gtk.Box(spacing=8)
    save_btn = Gtk.Button(label="Save")
    save_btn.add_css_class("suggested-action")

    def on_save(b):
        kind = ACTION_TYPES[action_dd.get_selected()][0]
        if kind == "keypress":
            binding = {
                "trigger": TRIGGER_OPTIONS[trigger_dd.get_selected()][0],
                "type": "keypress",
                "key": draft["keypress"].get("key", "KEY_A"),
                "modifiers": draft["keypress"].get("modifiers", []),
            }
        else:
            binding = {
                "trigger": TRIGGER_OPTIONS[trigger_dd.get_selected()][0],
                "type": "macro",
                "steps": draft["steps"],
            }
        try:
            client.set_binding(inp, layer, binding)
        except DaemonError as exc:
            show_error(exc)
            return
        on_saved()

    save_btn.connect("clicked", on_save)
    btn_row.append(save_btn)

    clear_btn = Gtk.Button(label="Clear (passthrough)")

    def on_clear(b):
        if existing is None:
            # Already passthrough — nothing to clear, no D-Bus call needed.
            on_saved()
            return
        try:
            client.clear_binding(inp, layer)
        except DaemonError as exc:
            show_error(exc)
            return
        on_saved()

    clear_btn.connect("clicked", on_clear)
    btn_row.append(clear_btn)
    box.append(btn_row)
    return box
