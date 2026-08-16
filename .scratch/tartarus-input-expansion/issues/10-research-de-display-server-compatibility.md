Type: research
Status: resolved

## Question

Determine what in Acheron's codebase actually assumes GNOME Shell / Wayland specifically, versus what's simply untested on anything else. The MVP map scoped "GNOME Shell 50.1, Wayland only" as a fact about the one development/test machine, not a verified architectural constraint — but a public release aimed at "the Linux gaming community" broadly will run into KDE, other DEs, and X11 users.

Investigate, reading the codebase directly (no external sources needed):

- Does the Daemon (`daemon/`) — evdev capture, `uinput` injection, D-Bus — touch anything DE- or display-server-specific at all, or does it operate entirely below that layer (kernel input devices + system D-Bus), making it inherently portable?
- Does the GUI's tray icon (`gui/`) genuinely require `ubuntu-appindicators` / a GNOME Shell extension, or does it use a more portable tray mechanism (e.g. `StatusNotifierItem` via a standard library) that just happens to also work under GNOME because that extension is installed? Check what tray-icon API/library is actually called.
- Any other GTK4 usage, packaging assumption (`install.sh`, the systemd user unit), or hardcoded path/dependency that's GNOME- or Wayland-specific rather than generically-Linux.
- Bottom line: is "other DEs/display-servers" likely to just work today (untested but not blocked), mostly work with one specific gap (name it), or genuinely require new work?

This almost certainly doesn't change anything about the Daemon (kernel-level, DE-agnostic by construction) but may surface a real gap in the GUI's tray integration — record findings either way so the map can decide whether "verify/support beyond GNOME" becomes a real ticket or stays a documented assumption.

## Answer

**Daemon — entirely below the DE/display-server layer, verified not assumed.** `daemon/Cargo.toml` depends only on `evdev`, `zbus`, `tokio`, `serde`/`toml`, `dirs`. A repo-wide grep of `daemon/src` for `wayland|x11|gnome|kde|desktop|display.?server|session` turns up nothing but "session bus" (the freedesktop D-Bus concept). `main.rs` opens `zbus::connection::Builder::session()` — the standard per-user D-Bus session bus, identical under GNOME, KDE, Sway, or bare X11 with `dbus-user-session`. `capture/evdev_source.rs` grabs the three device nodes via `EVIOCGRAB`; `injector.rs::build_device` creates a `uinput` `VirtualDevice`. Both are kernel-level mechanisms neither X11's input stack nor Wayland compositors' `libinput` gate — no compositor is ever in the path. **Zero DE/display-server dependency.**

**GUI tray icon — doesn't exist yet; the *planned* mechanism is already the portable one.** There is no real tray icon in the codebase today. `gui/acheron_gui/device_overview.py:264-332` (`build_tray_mock`) is an in-window mock `Gtk.Box`, built because `AppIndicator3`/`AyatanaAppIndicator3` "isn't installed in this dev sandbox" (see `.scratch/tartarus-keybinder/issues/09-design-gui-information-architecture.md:20`); the mock literally comments `note = Gtk.Label(label="real tray uses AppIndicator3")` (`device_overview.py:328`), and `spec.md:147` records the same decision. No `gi.require_version("AppIndicator3", ...)` call exists anywhere — the real tray has never been built or tested, GNOME included.

`AppIndicator3`/`AyatanaAppIndicator3` is itself a binding over `libayatana-appindicator`, which implements the freedesktop `StatusNotifierItem` D-Bus protocol — natively hosted by KDE Plasma and XFCE with no extra extension. GNOME Shell dropped native SNI hosting years ago, so the `ubuntu-appindicators` extension is a **GNOME-specific workaround for GNOME's own gap**, not something Acheron's chosen library needs elsewhere. The design already on file is the portable/standard one.

**Everything else checked clean.** GTK4/GLib imports (`app.py`, `daemon_client.py`, `wire.py`, `systemd_client.py`) are version-pinned only, no `Gdk.Wayland`/`Gdk.X11` backend calls, no `layer-shell` usage. D-Bus calls use `Gio.BusType.SESSION` throughout. `install.sh` and `packaging/acheron-daemon.service` use plain `systemd --user` (`~/.local/bin`, `~/.config/systemd/user`, `WantedBy=default.target`) — standard on any systemd distro, not GNOME-specific. No `.desktop` launcher exists yet (GUI launches via `python3 gui/main.py`) — an absence, not a GNOME-specific gap. No hardcoded `gnome`/`ubuntu` paths or package names anywhere in `daemon/src`, `gui/`, `install.sh`, or the unit file.

**Bottom line: mostly works, with one specific named gap — and it isn't a portability gap.** The Daemon is DE/display-server-agnostic by construction; untested on KDE/X11 but architecturally unblocked, no work needed. The real tray icon simply hasn't been built (only an in-window mock exists) — the fix isn't "make it portable," it's "build the already-designed `AppIndicator3`/SNI tray," which then works natively on KDE/XFCE and needs the `ubuntu-appindicators` extension only under GNOME specifically (worth a line in release docs). See [Decide the tray icon's look and behavior](./issues/11-decide-tray-icon-look-and-behavior.md), spawned by this finding.

## Answer

