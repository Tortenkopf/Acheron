Type: task
Status: open
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
