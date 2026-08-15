#!/usr/bin/env bash
# Idempotent install/rebuild path for the Acheron Daemon (ticket 21/spec.md
# "Packaging and lifecycle"): builds the release binary, installs it and the
# systemd --user unit, then (re)enables the unit so it's running afterward.
# Safe to re-run on every rebuild — every step below is either a plain
# overwrite or an already-idempotent systemctl call.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
daemon_dir="$script_dir/daemon"
unit_src="$script_dir/packaging/acheron-daemon.service"

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

echo "==> Reloading systemd --user and enabling acheron-daemon"
systemctl --user daemon-reload
systemctl --user enable --now acheron-daemon

echo "==> Done. Check status with: systemctl --user status acheron-daemon"
