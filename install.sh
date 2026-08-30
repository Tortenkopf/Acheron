#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz
# Idempotent install/rebuild path for the Acheron Daemon (ticket 21/spec.md
# "Packaging and lifecycle"): builds the release binary, installs it and the
# systemd --user unit, then (re)enables the unit so it's running afterward.
# Safe to re-run on every rebuild — every step below is either a plain
# overwrite or an already-idempotent systemctl call.
#
# Ticket 23: also installs the udev rule Analog Capture mode needs to open
# the Tartarus Pro's `hidraw` interfaces without root (ticket 18 §8) — the
# first "nothing forces packaging complexity" property this project gives
# up, per the map's Destination. This one step needs `sudo`; its own
# failure is caught and reported with manual recovery instructions rather
# than aborting the rest of the install, since the Daemon still runs and
# still works (degraded to Digital Capture, per the map's standing
# automatic-fallback discipline) without it.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
daemon_dir="$script_dir/daemon"
unit_src="$script_dir/packaging/acheron-daemon.service"
udev_rule_src="$script_dir/packaging/60-acheron-tartarus-pro.rules"
udev_rule_dest="/etc/udev/rules.d/60-acheron-tartarus-pro.rules"

# Ticket 90: the GUI's desktop-app launch path — a launcher script, a
# freedesktop .desktop entry, and the app icon, all installed under $HOME.
launcher_src="$script_dir/packaging/acheron-gui"
desktop_src="$script_dir/packaging/acheron.desktop"
icons_src="$script_dir/packaging/icons/hicolor"

bin_dir="$HOME/.local/bin"
unit_dir="$HOME/.config/systemd/user"
gui_lib_dir="$HOME/.local/lib/acheron"
apps_dir="$HOME/.local/share/applications"
icons_dir="$HOME/.local/share/icons/hicolor"

# Ticket 97: the tray indicator's three status-dot SVGs live here, NOT in
# the GUI package dir — the SNI host keeps a live file-watch on this path
# and overwriting an SVG in a git checkout while the GUI runs has crashed
# the desktop session. The GUI also self-heals this dir on launch; this
# step just puts them in place before the first launch.
tray_icons_src="$script_dir/gui/acheron_gui/icons"
tray_icons_dir="$HOME/.local/share/acheron/tray-icons"

echo "==> Building acheron-daemon (release)"
cargo build --release --manifest-path "$daemon_dir/Cargo.toml"

echo "==> Installing binary to $bin_dir/acheron-daemon"
mkdir -p "$bin_dir"
# `rm -f` then `install` (not a plain `cp` over the top): if the daemon is
# already running, its binary is a busy text file and an in-place write
# fails `ETXTBSY` ("Text file busy"). Unlinking first always succeeds (the
# running process keeps its own open inode); `install` then creates a fresh
# file. The running daemon keeps executing the old build until it's
# restarted — `systemctl --user restart acheron-daemon` picks up the new one.
rm -f "$bin_dir/acheron-daemon"
install -m 755 "$daemon_dir/target/release/acheron-daemon" "$bin_dir/acheron-daemon"

echo "==> Installing systemd --user unit to $unit_dir/acheron-daemon.service"
mkdir -p "$unit_dir"
cp "$unit_src" "$unit_dir/acheron-daemon.service"

echo "==> Installing udev rule for Analog Capture mode (needs sudo)"
if sudo cp "$udev_rule_src" "$udev_rule_dest" \
    && sudo udevadm control --reload-rules \
    && sudo udevadm trigger; then
    echo "    Installed $udev_rule_dest and reloaded udev rules."
else
    echo "    Could not install the udev rule automatically — the Daemon will still"
    echo "    run and work, degraded to Digital Capture mode (every grid key still"
    echo "    fully functional). To enable Analog Capture mode, run manually:"
    echo "        sudo cp \"$udev_rule_src\" \"$udev_rule_dest\""
    echo "        sudo udevadm control --reload-rules"
    echo "        sudo udevadm trigger"
    echo "    then unplug and replug the Tartarus Pro (or reboot)."
fi

echo "==> Reloading systemd --user and enabling acheron-daemon"
systemctl --user daemon-reload
systemctl --user enable --now acheron-daemon

# --- GUI desktop-app launch path (ticket 90) ----------------------------
# All under $HOME, no sudo. The GUI source is copied to a fixed installed
# location ($gui_lib_dir) so the launcher and .desktop entry never point
# back into this git checkout — moving or deleting the checkout afterward
# doesn't break the app grid entry. No venv / pip / Python packaging: the
# launcher runs the system python3 with $gui_lib_dir on PYTHONPATH, same
# GTK4 / PyGObject requirement as `python3 gui/main.py` from a checkout.
echo "==> Installing GUI package to $gui_lib_dir/acheron_gui"
rm -rf "$gui_lib_dir/acheron_gui"
mkdir -p "$gui_lib_dir"
cp -r "$script_dir/gui/acheron_gui" "$gui_lib_dir/acheron_gui"
find "$gui_lib_dir/acheron_gui" -name '__pycache__' -type d -prune -exec rm -rf {} +
# Ticket 102: bundle the GPLv3 text next to the package so the About
# dialog's "View Licence" button works with no git checkout around it
# (a dev checkout falls back to the repo-root LICENSE two levels up).
cp "$script_dir/LICENSE" "$gui_lib_dir/acheron_gui/LICENSE"

echo "==> Installing GUI launcher to $bin_dir/acheron-gui"
install -m 755 "$launcher_src" "$bin_dir/acheron-gui"

echo "==> Installing desktop entry to $apps_dir/acheron.desktop"
mkdir -p "$apps_dir"
install -m 644 "$desktop_src" "$apps_dir/acheron.desktop"

echo "==> Installing app icons to $icons_dir"
mkdir -p "$icons_dir"
cp -r "$icons_src/." "$icons_dir/"

echo "==> Installing tray status icons to $tray_icons_dir"
mkdir -p "$tray_icons_dir"
cp "$tray_icons_src"/*.svg "$tray_icons_dir/"

echo "==> Refreshing desktop database and icon cache (best-effort)"
# Both are pure caches: GNOME/KDE read the loose files above directly, so a
# failure here just means the entry/icon may not appear until the next
# login. Guarded like the udev step — a note, not an abort.
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$apps_dir" 2>/dev/null \
        || echo "    update-desktop-database failed — the entry still works; re-login if it's missing."
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    # gtk-update-icon-cache needs an index.theme in the target dir. Seed it
    # from the system hicolor theme if the user's own copy doesn't have one.
    if [[ ! -f "$icons_dir/index.theme" && -f /usr/share/icons/hicolor/index.theme ]]; then
        cp /usr/share/icons/hicolor/index.theme "$icons_dir/index.theme"
    fi
    gtk-update-icon-cache -f -t "$icons_dir" 2>/dev/null \
        || echo "    gtk-update-icon-cache failed — the icon still resolves from loose files; re-login if it's missing."
fi

echo "==> Done."
echo "    Daemon:  systemctl --user status acheron-daemon"
echo "    GUI:     acheron-gui   (or launch \"Acheron\" from your app grid)"
