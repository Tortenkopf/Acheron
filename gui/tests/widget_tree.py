"""Test-only helper for walking a built Gtk4 widget tree, including into
`Gtk.Popover`/`Gtk.MenuButton` contents — which `get_first_child()` doesn't
reach on its own, since a Popover's contents realize as a separate surface
rather than a normal child in the tree.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator

from gi.repository import Gtk


def walk(widget: Gtk.Widget) -> Iterator[Gtk.Widget]:
    yield widget
    if isinstance(widget, Gtk.Popover):
        child = widget.get_child()
        if child is not None:
            yield from walk(child)
        return
    if isinstance(widget, Gtk.MenuButton):
        popover = widget.get_popover()
        if popover is not None:
            yield from walk(popover)
    child = widget.get_first_child()
    while child is not None:
        yield from walk(child)
        child = child.get_next_sibling()


def find_all(root: Gtk.Widget, predicate: Callable[[Gtk.Widget], bool]) -> list[Gtk.Widget]:
    return [w for w in walk(root) if predicate(w)]


def find_one(root: Gtk.Widget, predicate: Callable[[Gtk.Widget], bool]) -> Gtk.Widget:
    matches = find_all(root, predicate)
    assert len(matches) == 1, f"expected exactly one match, found {len(matches)}"
    return matches[0]


def button_labeled(root: Gtk.Widget, label: str) -> Gtk.Button:
    return find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == label)
