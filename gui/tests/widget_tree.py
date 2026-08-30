# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Test-only helper for walking a built Gtk4 widget tree, including into
`Gtk.Popover`/`Gtk.MenuButton` contents. A `Gtk.MenuButton`'s popover is
explicitly walked below since it isn't guaranteed reachable through
`get_first_child()` alone — but in this GTK4 environment it turns out
`get_first_child()` *does* also reach it (as one of the MenuButton's normal
internal children), so without deduplication a popover's contents get
yielded twice from a root-rooted walk. `_seen` guards against that on
either GTK4 behavior, so callers' `find_all`/`find_one` counts stay correct
regardless.

`_seen` holds the widget objects themselves (relying on PyGObject wrappers'
identity-based `__eq__`/`__hash__` on the underlying GObject), not their
`id()` — `get_first_child()`/`get_next_sibling()` mint a fresh, short-lived
Python wrapper on every call, and once a `child` local is reassigned the
previous wrapper's refcount can drop to zero and its memory get reused for
the *next* freshly-minted wrapper. `id()` would then alias two genuinely
different widgets together, under-counting real matches; keeping the
objects in the set instead keeps them alive for the rest of this walk and
compares them correctly.
"""

from __future__ import annotations

from collections.abc import Callable, Iterator

from gi.repository import Gtk


def walk(widget: Gtk.Widget, _seen: set[Gtk.Widget] | None = None) -> Iterator[Gtk.Widget]:
    if _seen is None:
        _seen = set()
    if widget in _seen:
        return
    _seen.add(widget)
    yield widget
    if isinstance(widget, Gtk.Popover):
        child = widget.get_child()
        if child is not None:
            yield from walk(child, _seen)
        return
    if isinstance(widget, Gtk.MenuButton):
        popover = widget.get_popover()
        if popover is not None:
            yield from walk(popover, _seen)
    child = widget.get_first_child()
    while child is not None:
        yield from walk(child, _seen)
        child = child.get_next_sibling()


def find_all(root: Gtk.Widget, predicate: Callable[[Gtk.Widget], bool]) -> list[Gtk.Widget]:
    return [w for w in walk(root) if predicate(w)]


def find_one(root: Gtk.Widget, predicate: Callable[[Gtk.Widget], bool]) -> Gtk.Widget:
    matches = find_all(root, predicate)
    assert len(matches) == 1, f"expected exactly one match, found {len(matches)}"
    return matches[0]


def button_labeled(root: Gtk.Widget, label: str) -> Gtk.Button:
    return find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == label)


def editor_content(btn: Gtk.Widget) -> Gtk.Widget:
    """The Binding-editor content of a Device Overview grid button. Since
    ticket 44's Popover->Window conversion, that's `btn.binding_editor_window
    .get_child()` rather than a `Gtk.Popover`; centralized here so the
    accessor only needs updating in one place if it changes again."""
    return btn.binding_editor_window.get_child()
