# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Ticket 102: Acheron's About dialog — a hand-built plain `Gtk.Window`.

Deliberately **no libadwaita**: `Adw.AboutDialog` needs libadwaita
initialised, which applies the Adwaita stylesheet app-wide and would
silently restyle the whole hardware-verified GUI (a dozen polish tickets,
87–95). See the map's About-dialog cluster notes — revisiting libadwaita
is a separate, deliberately-verified change if ever wanted.

Opened from the main window's header-bar primary menu ("About Acheron" ->
the `app.about` action wired in `app.py`). The dialog reads a single
`GetState()` snapshot, passed in as `state`:

- `state is None` -> the Daemon isn't running: the Daemon version line
  reads "Daemon: not running" and the device rows read "Not connected".
- `firmware_version` / `serial_number` are *optional* keys (ticket 101),
  absent whenever no device is connected — each renders as an explicit
  "Not connected", never a blank row.

`build_license_window` is split out and takes its text as a parameter so
it's testable without the bundled-file lookup `_license_text` does.
"""

from __future__ import annotations

from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk

from . import __version__

WIKIPEDIA_URL = "https://en.wikipedia.org/wiki/Acheron"
GPL_URL = "https://www.gnu.org/licenses/gpl-3.0.html"
REPO_URL = "https://github.com/Tortenkopf/Acheron"
ULTRAMONAKA_URL = "https://github.com/ultramonaka/open-tartarus-driver"
MATT_POCOCK_URL = "https://github.com/mattpocock/skills"

SUBTITLE = "An open keybinding tool for the Razer Tartarus Pro"

# Reproduced verbatim, including the `...` ellipses — those are deliberate
# abridgements of the source article, not something to tidy away. Ticket 102.
RIVER_NOTE = (
    "The Acheron (/ˈækərən/ or /ˈækərɒn/; Ancient Greek: Ἀχέρων Acheron or "
    "Ἀχερούσιος Acherousios; Greek: Αχέροντας Acherontas) is a river in the "
    "Epirus region of northwest Greece. ... Ancient Greek mythology saw the "
    'Acheron, sometimes known as the "river of woe", as one of the five '
    'rivers of the Greek underworld. ... The Suda describes the river as "a '
    'place of healing, not a place of punishment, cleansing and purging the '
    'sins of humans".'
)

# GPLv3 §5(d) "Appropriate Legal Notices" (+ §0). Releasing under the GPL
# does not waive copyright, so the copyright line is required, not optional.
LEGAL_NOTICE = (
    "Copyright © 2026 Justin Milatz\n"
    "Acheron comes with ABSOLUTELY NO WARRANTY.\n"
    "This is free software: you are free to change and redistribute it under "
    "the terms of the GNU General Public License, version 3 or (at your "
    "option) any later version."
)


def _line(text: str, *, css: str | None = None) -> Gtk.Label:
    label = Gtk.Label(label=text, xalign=0, wrap=True)
    if css:
        label.add_css_class(css)
    return label


def _section(title: str) -> Gtk.Label:
    label = Gtk.Label(label=title, xalign=0)
    label.add_css_class("sub-heading")
    label.set_margin_top(8)
    return label


def _link(uri: str, label: str) -> Gtk.LinkButton:
    """A `Gtk.LinkButton` — its default `activate-link` handler opens the URI
    via `gtk_show_uri` (the portal / `xdg-open` path), so no new dependency
    and no explicit `Gtk.UriLauncher` wiring is needed here."""
    button = Gtk.LinkButton.new_with_label(uri, label)
    button.set_halign(Gtk.Align.START)
    return button


def _margins(widget: Gtk.Widget, px: int) -> None:
    for setter in (
        widget.set_margin_top,
        widget.set_margin_bottom,
        widget.set_margin_start,
        widget.set_margin_end,
    ):
        setter(px)


def _license_text() -> str | None:
    """The bundled GPLv3 text. `install.sh` places a copy of the repo-root
    `LICENSE` next to this package (`acheron_gui/LICENSE`) when it installs
    to `~/.local/lib/acheron`; a dev checkout has no such copy, so fall back
    to the repo-root file two levels up from this module."""
    here = Path(__file__).resolve().parent
    for candidate in (here / "LICENSE", here.parent.parent / "LICENSE"):
        try:
            return candidate.read_text(encoding="utf-8")
        except OSError:
            continue
    return None


def build_license_window(
    parent: Gtk.Window | None = None, *, license_text: str | None
) -> Gtk.Window:
    """The scrollable full-GPLv3 view behind the About dialog's "View
    Licence" button. `license_text is None` (the bundled file genuinely
    couldn't be found) degrades to a message plus the gnu.org link rather
    than an empty window."""
    window = Gtk.Window(title="Acheron — GNU General Public License v3", modal=True)
    if parent is not None:
        window.set_transient_for(parent)
    window.set_default_size(720, 600)

    if license_text is None:
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        _margins(box, 16)
        box.append(
            _line(
                "The bundled licence file could not be found. The full GNU "
                "General Public License, version 3, is available online:"
            )
        )
        box.append(_link(GPL_URL, "gnu.org/licenses/gpl-3.0.html"))
        window.set_child(box)
        return window

    scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.AUTOMATIC)
    view = Gtk.TextView(editable=False, cursor_visible=False, monospace=True)
    view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
    for setter in (
        view.set_left_margin,
        view.set_right_margin,
        view.set_top_margin,
        view.set_bottom_margin,
    ):
        setter(8)
    view.get_buffer().set_text(license_text)
    scroller.set_child(view)
    window.set_child(scroller)
    return window


def build_about_dialog(
    parent: Gtk.Window | None = None,
    *,
    gui_version: str = __version__,
    state: dict | None = None,
) -> Gtk.Window:
    """The About dialog. `parent` is the main window it's `transient-for`
    and modal against; `state` is a `GetState()` snapshot or `None` (Daemon
    not running). A fresh window is built per call — it's cheap and opened
    rarely, so there's nothing to cache (unlike ticket 44's per-key
    editors)."""
    window = Gtk.Window(title="About Acheron", modal=True)
    if parent is not None:
        window.set_transient_for(parent)
    window.set_default_size(500, 640)

    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)

    scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER, vexpand=True)
    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    _margins(content, 16)
    scroller.set_child(content)
    root.append(scroller)

    title = Gtk.Label(label="Acheron", xalign=0)
    title.add_css_class("about-title")
    content.append(title)
    content.append(_line(SUBTITLE, css="dim"))

    content.append(_section("Version"))
    content.append(_line(f"Acheron {gui_version}", css="heading"))
    if state is None:
        content.append(_line("Daemon: not running", css="dim"))
    else:
        content.append(_line(f"Daemon {state.get('daemon_version', '—')}", css="dim"))

    content.append(_section("Background"))
    content.append(_line(RIVER_NOTE, css="quote"))
    content.append(_line("Source: Wikipedia as of August 2026"))
    content.append(_link(WIKIPEDIA_URL, "en.wikipedia.org/wiki/Acheron"))

    device = state or {}
    content.append(_section("Device"))
    content.append(_line(f"Firmware: {device.get('firmware_version') or 'Not connected'}"))
    content.append(_line(f"Serial: {device.get('serial_number') or 'Not connected'}"))

    content.append(_section("Project"))
    content.append(_line("Author: Justin Milatz, with Claude Code as co-author"))
    content.append(_line("Project email: TBD"))
    content.append(_line("Website: TBD"))
    content.append(_line("Repository:"))
    content.append(_link(REPO_URL, "github.com/Tortenkopf/Acheron"))

    content.append(_section("Acknowledgements"))
    content.append(
        _line(
            "ultramonaka — for the reverse-engineering of the Tartarus Pro's "
            "hardware protocol, without which Acheron would not have been possible."
        )
    )
    content.append(_link(ULTRAMONAKA_URL, "github.com/ultramonaka/open-tartarus-driver"))
    content.append(
        _line(
            "Matt Pocock — for the skills for LLM-assisted software development "
            "that were invaluable in building Acheron."
        )
    )
    content.append(_link(MATT_POCOCK_URL, "github.com/mattpocock/skills"))

    content.append(_section("Licence"))
    content.append(_line(LEGAL_NOTICE))
    legal_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
    view_licence = Gtk.Button(label="View Licence")
    view_licence.connect(
        "clicked",
        lambda _b: build_license_window(window, license_text=_license_text()).present(),
    )
    legal_row.append(view_licence)
    legal_row.append(_link(GPL_URL, "gnu.org/licenses/gpl-3.0.html"))
    content.append(legal_row)

    action_bar = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, halign=Gtk.Align.END, spacing=8)
    _margins(action_bar, 10)
    close_button = Gtk.Button(label="Close")
    close_button.connect("clicked", lambda _b: window.close())
    action_bar.append(close_button)
    root.append(action_bar)

    window.set_child(root)
    return window
