# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

from gi.repository import Gtk

from acheron_gui.about_dialog import (
    GPL_URL,
    LEGAL_NOTICE,
    MATT_POCOCK_URL,
    REPO_URL,
    RIVER_NOTE,
    ULTRAMONAKA_URL,
    WIKIPEDIA_URL,
    _license_text,
    build_about_dialog,
    build_license_window,
)

from .widget_tree import find_all, find_one

CONNECTED = {
    "daemon_version": "1.0.0",
    "firmware_version": "v1.2",
    "serial_number": "PM2443F36300141",
}
DISCONNECTED = {"daemon_version": "1.0.0"}


def _texts(root):
    return [w.get_label() for w in find_all(root, lambda w: isinstance(w, Gtk.Label)) if w.get_label()]


def _link_uris(root):
    return {w.get_uri() for w in find_all(root, lambda w: isinstance(w, Gtk.LinkButton))}


def test_connected_state_shows_the_device_values_and_the_daemon_version():
    dialog = build_about_dialog(gui_version="1.0.0-dev+abc1234", state=CONNECTED)
    texts = _texts(dialog)

    assert "Acheron 1.0.0-dev+abc1234" in texts
    assert "Daemon 1.0.0" in texts
    assert "Firmware: v1.2" in texts
    assert "Serial: PM2443F36300141" in texts


def test_device_disconnected_shows_not_connected_for_both_device_rows():
    dialog = build_about_dialog(gui_version="1.0.0", state=DISCONNECTED)
    texts = _texts(dialog)

    assert "Firmware: Not connected" in texts
    assert "Serial: Not connected" in texts
    # The Daemon itself is still up, so its version line stays.
    assert "Daemon 1.0.0" in texts


def test_daemon_down_shows_not_running_and_not_connected():
    dialog = build_about_dialog(gui_version="1.0.0", state=None)
    texts = _texts(dialog)

    assert "Daemon: not running" in texts
    assert "Firmware: Not connected" in texts
    assert "Serial: Not connected" in texts


def test_gui_version_line_uses_the_passed_version_verbatim():
    # Dev builds render the `1.0.0-dev+<hash>` string as-is (ticket 99/102).
    dialog = build_about_dialog(gui_version="9.9.9-dev+deadbee", state=None)
    assert "Acheron 9.9.9-dev+deadbee" in _texts(dialog)


def test_river_note_is_reproduced_verbatim_including_the_ellipses():
    dialog = build_about_dialog(state=None)
    note = find_one(dialog, lambda w: isinstance(w, Gtk.Label) and (w.get_label() or "").startswith("The Acheron"))

    assert note.get_label() == RIVER_NOTE
    assert " ... " in note.get_label()  # the deliberate abridgement markers
    assert note.get_label().count("...") == 2


def test_legal_notice_block_reads_exactly_as_specified():
    dialog = build_about_dialog(state=None)
    assert LEGAL_NOTICE in _texts(dialog)
    assert LEGAL_NOTICE.startswith("Copyright © 2026 Justin Milatz")
    assert "ABSOLUTELY NO WARRANTY" in LEGAL_NOTICE


def test_placeholder_rows_are_shown_with_visible_tbd():
    dialog = build_about_dialog(state=None)
    texts = _texts(dialog)
    assert "Project email: TBD" in texts
    assert "Website: TBD" in texts


def test_repository_row_links_to_the_public_github_repo():
    dialog = build_about_dialog(state=None)
    assert "Repository:" in _texts(dialog)
    assert REPO_URL in _link_uris(dialog)


def test_author_line_credits_claude_code_as_co_author():
    dialog = build_about_dialog(state=None)
    assert "Author: Justin Milatz, with Claude Code as co-author" in _texts(dialog)


def test_every_external_link_is_wired():
    dialog = build_about_dialog(state=None)
    assert _link_uris(dialog) == {
        WIKIPEDIA_URL,
        GPL_URL,
        REPO_URL,
        ULTRAMONAKA_URL,
        MATT_POCOCK_URL,
    }


def test_view_licence_button_is_present():
    dialog = build_about_dialog(state=None)
    assert find_all(dialog, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "View Licence")


def test_dialog_is_modal():
    dialog = build_about_dialog(state=None)
    assert dialog.get_modal() is True


def test_license_window_shows_the_full_text_in_a_scrollable_view():
    text = "GNU GENERAL PUBLIC LICENSE\nVersion 3\n" + ("body line\n" * 500)
    window = build_license_window(license_text=text)

    view = find_one(window, lambda w: isinstance(w, Gtk.TextView))
    buffer = view.get_buffer()
    got = buffer.get_text(buffer.get_start_iter(), buffer.get_end_iter(), False)
    assert got == text
    assert view.get_editable() is False
    assert find_all(window, lambda w: isinstance(w, Gtk.ScrolledWindow))


def test_license_window_without_bundled_text_falls_back_to_the_online_link():
    window = build_license_window(license_text=None)

    assert find_all(window, lambda w: isinstance(w, Gtk.TextView)) == []
    assert GPL_URL in {w.get_uri() for w in find_all(window, lambda w: isinstance(w, Gtk.LinkButton))}


def test_license_text_resolves_from_the_repo_root_in_a_dev_checkout():
    # No `acheron_gui/LICENSE` is committed (install.sh places it); the
    # fallback two levels up is the repo-root GPLv3 file.
    text = _license_text()
    assert text is not None
    assert "GNU GENERAL PUBLIC LICENSE" in text
    assert "Version 3" in text
