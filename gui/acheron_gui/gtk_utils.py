"""Small Gtk4 helpers shared across widget-building modules."""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk

from .daemon_client import DaemonError


def build_pinned_sidebar_box() -> Gtk.Box:
    """The fixed-220px, non-expanding column-1 shape shared by the Profile
    sidebar (Grid destination) and the Library browse column (ticket 70) —
    factored out so both destinations' column 1 stay pixel-identical width,
    per ticket 69's "nothing visibly resizes when flipping destinations."""
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    box.add_css_class("sidebar")
    box.set_size_request(220, -1)
    box.set_hexpand(False)
    return box


def clear_children(container: Gtk.Widget) -> None:
    """Removes every direct child of `container` — the "rebuild this box
    from scratch" pattern used throughout the GUI (Save/Clear re-rendering
    a popover's action editor, a rebuild replacing the whole window
    content, etc). Works for any container whose `remove(child)` takes the
    child widget directly (`Gtk.Box`, `Gtk.ListBox`, …)."""
    child = container.get_first_child()
    while child is not None:
        nxt = child.get_next_sibling()
        container.remove(child)
        child = nxt


def build_name_prompt_popover(
    client_error_context: str, initial_text: str, submit_label: str, on_submit: Callable[[str], None]
) -> Gtk.Popover:
    """A small Entry-plus-submit-button popover shared by every "+ New"/
    rename ("✎") control across the GUI (Profile sidebar — ticket 19 — and
    the Macro library — ticket 52) — the closest existing pattern is
    `binding_editor.build_binding_editor`'s own Save/error handling, reused
    here at a much smaller scale rather than inventing a separate dialog
    mechanism."""
    popover = Gtk.Popover()
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.set_margin_top(8)
    box.set_margin_bottom(8)
    box.set_margin_start(8)
    box.set_margin_end(8)

    entry = Gtk.Entry(text=initial_text)
    entry.set_width_chars(16)
    box.append(entry)

    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.set_visible(False)
    box.append(error_label)

    def on_submit_clicked(_widget):
        name = entry.get_text().strip()
        if not name:
            error_label.set_label("Name can't be empty")
            error_label.set_visible(True)
            return
        try:
            on_submit(name)
        except DaemonError as exc:
            error_label.set_label(f"{client_error_context}: {exc}")
            error_label.set_visible(True)
            return
        popover.popdown()

    submit_btn = Gtk.Button(label=submit_label)
    submit_btn.add_css_class("suggested-action")
    submit_btn.connect("clicked", on_submit_clicked)
    entry.connect("activate", on_submit_clicked)
    box.append(submit_btn)

    popover.set_child(box)
    return popover
