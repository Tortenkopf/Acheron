"""Small Gtk4 helpers shared across widget-building modules."""

from __future__ import annotations

from gi.repository import Gtk


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
