#!/usr/bin/env bash
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

bin_dir="$HOME/.local/bin"
unit_dir="$HOME/.config/systemd/user"

echo "==> Building acheron-daemon (release)"
cargo build --release --manifest-path "$daemon_dir/Cargo.toml"

echo "==> Installing binary to $bin_dir/acheron-daemon"
mkdir -p "$bin_dir"
cp "$daemon_dir/target/release/acheron-daemon" "$bin_dir/acheron-daemon"

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

echo "==> Done. Check status with: systemctl --user status acheron-daemon"
