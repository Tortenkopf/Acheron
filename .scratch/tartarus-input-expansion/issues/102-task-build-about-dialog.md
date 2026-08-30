Type: task
Status: resolved
Assignee: Charon (2026-08-30)
Blocked by: 99, 101

## Question

Build the **About dialog** and the header-bar menu entry point that opens it — the last
required-floor GUI item for the v1.0 release.

Settled in the charting grilling (2026-08-29) — do not re-litigate:

- **Entry point**: add a `Gtk.HeaderBar` as the titlebar of the main window
  (`gui/acheron_gui/app.py` — today a bare `Gtk.ApplicationWindow(title="Acheron")` with the
  WM's default titlebar). Right-aligned `Gtk.MenuButton` (`open-menu-symbolic`) with a
  `Gio.Menu` whose one item is **"About Acheron"** → an `app.about` action. The menu is the
  intended home for future global actions (Quit, Preferences) — build it as a menu, not a
  bare button.
- **Implementation**: a **hand-built plain `Gtk.Window`** (new file
  `gui/acheron_gui/about_dialog.py`), modal and `transient-for` the main window. **No
  libadwaita** — the grilling explicitly ruled out the app-wide Adwaita restyle that
  `Adw.AboutDialog` would drag in over a GUI that's had a dozen polish tickets (87–95).
- **Content** (concise, well-formatted — labeled rows / sections):
  - **Name** "Acheron", **subtitle** "An open keybinding tool for the Razer Tartarus Pro".
  - **Background note** — reproduce the river quote **verbatim, including the `...`
    ellipses** (they are deliberate abridgements):
    > The Acheron (/ˈækərən/ or /ˈækərɒn/; Ancient Greek: Ἀχέρων Acheron or Ἀχερούσιος
    > Acherousios; Greek: Αχέροντας Acherontas) is a river in the Epirus region of northwest
    > Greece. ... Ancient Greek mythology saw the Acheron, sometimes known as the "river of
    > woe", as one of the five rivers of the Greek underworld. ... The Suda describes the
    > river as "a place of healing, not a place of punishment, cleansing and purging the
    > sins of humans".

    Followed by "Source: Wikipedia as of August 2026" with a hyperlink to
    <https://en.wikipedia.org/wiki/Acheron> (we are quoting it, so link it).
  - **Version**: "Acheron {gui `__version__`}" prominent; "Daemon {the Daemon-version key
    ticket 99 adds to `GetState()`}" as a secondary line. Dev builds render the
    `1.0.0-dev+<hash>` string as-is. If the Daemon isn't running, show the GUI version alone
    (or "Daemon: not running").
  - **Author**: "Justin Milatz, with Claude Code as co-author".
  - **Placeholder rows, shown explicitly** (a visible `TBD` beats a forgotten row):
    "Project email: TBD", "Website: TBD", "Repository: TBD" (the eventual GitHub repo).
  - **Device** (from ticket 101's `GetState()` keys): "Firmware: {`firmware_version`}" and
    "Serial: {`serial_number`}", each showing **"Not connected"** (or "—") when its key is
    absent.
  - **Acknowledgements** section:
    - **ultramonaka** — for the reverse-engineering of the Tartarus Pro's hardware protocol,
      without which Acheron would not have been possible.
      <https://github.com/ultramonaka/open-tartarus-driver>
    - **Matt Pocock** — for the skills for LLM-assisted software development that were
      invaluable in building Acheron. <https://github.com/mattpocock/skills>
  - **Legal** section (GPLv3 "Appropriate Legal Notices" — §5(d) + §0):
    > Copyright © 2026 Justin Milatz
    > Acheron comes with ABSOLUTELY NO WARRANTY.
    > This is free software: you are free to change and redistribute it under the terms of
    > the GNU General Public License, version 3 or (at your option) any later version.

    A **"View Licence"** button opening the bundled full `LICENSE` text in a scrollable view,
    plus a link to <https://www.gnu.org/licenses/gpl-3.0.html>.

Watch / decide during the build:

- **Ticket 36 minimize-to-tray interaction** (`app.py` ~L275): the tray design intercepts
  the titlebar close button to hide the window. That's a `close-request` handler on the
  window and should be unaffected by moving to a `Gtk.HeaderBar`, but **verify it** — the
  close button is now inside our header bar.
- **`LICENSE` file access**: the GUI is installed to `~/.local/lib/acheron/` with no repo
  around it. Bundle `LICENSE` into the GUI package (and have `install.sh` place it), with a
  dev-checkout fallback to the repo-root file. Hand the install-path addition to
  [ticket 35](./35-task-write-release-documentation.md).
- Link-opening: use `Gtk.LinkButton` / `Gtk.UriLauncher` (or `xdg-open`) — no new deps.

Build a screenshot harness for the dialog (`gui/tools/shot_about.py`), following
`shot_library.py` / `shot_binding_editor.py`. Add `gui/tests/test_about_dialog.py`. No Daemon
change in this ticket — the Daemon version and firmware/serial come from tickets 99 and 101.
GUI suite green.

## Answer

Built AFK against `DaemonStub`; every dialog state is screenshot-verified from
inside the process. The one thing left for **hardware** — the connected unit's real
firmware/serial in the device rows, and the tray minimize-to-tray still working with the
new header bar — is [ticket 103](./103-task-verify-about-dialog-on-hardware.md)'s job.
GUI suite **355 passed / 0 failed** (was 337; +15 `test_about_dialog.py`, +3
`test_app.py`). No Daemon change.

### What was built

- **`gui/acheron_gui/about_dialog.py`** (new) — `build_about_dialog(parent, *, gui_version,
  state)` returns a hand-built modal `Gtk.Window`, `transient-for` the parent. **No
  libadwaita** (per the map's cluster notes). Content in a vertical-scrolling
  `Gtk.ScrolledWindow` with a fixed bottom action bar (Close). All sections present:
  name + subtitle; **Version** ("Acheron {gui `__version__`}" prominent, "Daemon
  {`daemon_version`}" secondary — or **"Daemon: not running"** when `state is None`);
  **Background** (the river quote reproduced **verbatim including both `...`**, a module
  constant `RIVER_NOTE` with a test asserting exact equality + `count("...") == 2`, then
  the Wikipedia link); **Device** ("Firmware: …" / "Serial: …", each **"Not connected"**
  when its optional `GetState()` key is absent or `state is None`); **Project** (author
  "Justin Milatz, with Claude Code as co-author" + the three visible-`TBD` placeholder
  rows); **Acknowledgements** (ultramonaka, Matt Pocock, each with its GitHub link);
  **Licence** (the GPLv3 §5(d) copyright/no-warranty/redistribution block as the
  `LEGAL_NOTICE` constant, a **"View Licence"** button, a gnu.org link).
- **`build_license_window(parent, *, license_text)`** in the same module — a scrollable
  read-only monospace `Gtk.TextView` of the full bundled `LICENSE`. `license_text=None`
  (file genuinely not found) degrades to a short message + the gnu.org link rather than an
  empty window. `_license_text()` reads `acheron_gui/LICENSE` (installed copy) first, then
  falls back to the repo-root `LICENSE` two levels up (dev checkout).
- **Links** use `Gtk.LinkButton` — its default `activate-link` handler opens via
  `gtk_show_uri` (portal / `xdg-open`), **no new dependency**.
- **`gui/acheron_gui/app.py`** — `_build_main_window` now sets a `Gtk.HeaderBar` titlebar
  on the main window with a right-packed `Gtk.MenuButton` (`open-menu-symbolic`) whose
  `Gio.Menu` has one item, **"About Acheron" → `app.about`**. The `app.about`
  `Gio.SimpleAction` opens a fresh dialog reading one `GetState()` snapshot. Two testable
  helpers factored out (same compromise as the file's other helpers — the real
  `_build_main_window` needs a registered app + session bus to drive): `_build_primary_menu()`
  and `_about_dialog_state(client, daemon_running)` (returns the snapshot, or `None` when
  the Daemon's down / the call raises).
- **Ticket 36 interaction** — `_wire_window_close_to_hide` connects to the window's
  `close-request` signal, which is emitted by `GtkWindow` regardless of whether a titlebar
  widget is set via `set_titlebar()`. Structurally unaffected; the header bar's own close
  button routes through the same signal. Verified the whole app still builds/runs with the
  header bar via the `shot_device_overview.py` / `shot_binding_editor.py` harnesses. Live
  tray behaviour confirmation is ticket 103.
- **`LICENSE` bundling** — `install.sh` gained `cp "$script_dir/LICENSE"
  "$gui_lib_dir/acheron_gui/LICENSE"` after the package copy; `packaging/test_install.sh`
  copies `LICENSE` into its sandbox repo and asserts the installed package contains it.
  Installed-path addition handed to [ticket 35](./35-task-write-release-documentation.md).
  No committed `gui/acheron_gui/LICENSE` — install.sh places it; the checkout uses the
  repo-root fallback.
- **`gui/tools/shot_about.py`** (new) — screenshot harness (sibling of
  `shot_binding_editor.py`): renders the connected / disconnected / daemon-down dialog
  states and the licence window, dumps every label's text to `about_states.txt`. Run this
  session; output confirms the verbatim quote, all four links, the legal block, and the
  "Not connected" / "not running" fallbacks.
- **`gui/tests/test_about_dialog.py`** (new, 15 tests) + 3 in `test_app.py`
  (`_build_primary_menu` shape, `_about_dialog_state` running / not-running / raises).

### Not done (out of scope / handed off)

- Live firmware/serial + tray verification → ticket 103.
- Per-file GPL headers, README license section, documenting the new installed path → ticket
  35 (handoff note appended there).
- `acheron-daemon --version` CLI flag — still not added (ticket 99 already noted it; not
  needed by this dialog, which reads the D-Bus `daemon_version` key).
