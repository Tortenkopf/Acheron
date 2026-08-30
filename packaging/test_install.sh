#!/usr/bin/env bash
# Automated coverage for ticket 21's packaging: unit-file content and
# install.sh idempotency — the parts that don't require a real systemd
# --user session (per the ticket, the login-autostart and crash-recovery
# demos are manual/live-hardware verification, not covered here). cargo and
# systemctl are stubbed so this runs without a real build or a real systemd.
# Ticket 23: sudo is stubbed too, for the same reason — install.sh's udev
# step is real-system-modifying (a fixed /etc/udev/rules.d path, not
# redirectable via $HOME like the rest of the script), so this test must
# never invoke a real sudo, successful or not.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

fail() {
  echo "FAIL: $1" >&2
  exit 1
}

# --- unit-file content ---------------------------------------------------
unit_file="$repo_root/packaging/acheron-daemon.service"
[[ -f "$unit_file" ]] || fail "unit file missing: $unit_file"

assert_contains() {
  grep -qxF "$1" "$unit_file" || fail "unit file missing line: $1"
}

assert_contains "Type=simple"
assert_contains "ExecStart=%h/.local/bin/acheron-daemon"
assert_contains "After=graphical-session.target"
assert_contains "WantedBy=default.target"
assert_contains "Restart=no"
echo "PASS: unit file contains all required directives"

# --- udev rule content (ticket 23, ticket 18 §8) --------------------------
udev_rule_file="$repo_root/packaging/60-acheron-tartarus-pro.rules"
[[ -f "$udev_rule_file" ]] || fail "udev rule file missing: $udev_rule_file"
grep -q 'idVendor}=="1532"' "$udev_rule_file" || fail "udev rule missing the Tartarus Pro's idVendor match"
grep -q 'idProduct}=="0244"' "$udev_rule_file" || fail "udev rule missing the Tartarus Pro's idProduct match"
grep -q 'MODE="0660"' "$udev_rule_file" || fail "udev rule missing MODE=\"0660\""
grep -q 'GROUP="plugdev"' "$udev_rule_file" || fail "udev rule missing GROUP=\"plugdev\""
echo "PASS: udev rule matches the Tartarus Pro and grants plugdev group access"

# --- install.sh idempotency, with cargo/systemctl stubbed ----------------
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

fake_home="$work/home"
fake_bin="$work/stub-bin"
call_log="$work/calls.log"
mkdir -p "$fake_home" "$fake_bin"
: > "$call_log"

# Run install.sh from a throwaway copy of the repo, never the checkout this
# test lives in. The fake `cargo` below writes its stub binary to
# `<script_dir>/daemon/target/release/acheron-daemon`, where `<script_dir>`
# is wherever the install.sh it runs is located — point that at the real
# checkout and the stub silently overwrites a real release binary, which a
# later `install.sh` then copies straight into ~/.local/bin (observed:
# a daemon that "starts" and immediately exits 0 printing "fake-acheron-daemon").
# The sandbox copy carries only what install.sh actually reads.
sandbox_repo="$work/repo"
mkdir -p "$sandbox_repo/daemon" "$sandbox_repo/gui"
cp "$repo_root/install.sh" "$sandbox_repo/"
cp "$repo_root/daemon/Cargo.toml" "$sandbox_repo/daemon/"
cp -r "$repo_root/packaging" "$sandbox_repo/"
cp -r "$repo_root/gui/acheron_gui" "$sandbox_repo/gui/"
# Ticket 102: install.sh bundles the repo-root LICENSE into the GUI package.
cp "$repo_root/LICENSE" "$sandbox_repo/"

# Stub cargo: records its invocation, then drops a fake release binary where
# install.sh expects to find one, so the rest of the script has something
# real to `cp` without an actual Rust build. Guarded so it can only ever
# write inside $work — a regression that runs it against a real checkout
# fails loudly instead of clobbering a real binary.
cat > "$fake_bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -u
echo "cargo $*" >> "$CALL_LOG"
manifest=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--manifest-path" ]]; then
    manifest="$arg"
  fi
  prev="$arg"
done
daemon_dir="$(cd "$(dirname "$manifest")" && pwd)"
if [[ -z "${SANDBOX_ROOT:-}" || "$daemon_dir/" != "$SANDBOX_ROOT"/* ]]; then
  echo "fake cargo: refusing to write outside the test sandbox: $daemon_dir" >&2
  exit 1
fi
mkdir -p "$daemon_dir/target/release"
printf '#!/usr/bin/env bash\necho fake-acheron-daemon\n' > "$daemon_dir/target/release/acheron-daemon"
chmod +x "$daemon_dir/target/release/acheron-daemon"
EOF
chmod +x "$fake_bin/cargo"

cat > "$fake_bin/systemctl" <<'EOF'
#!/usr/bin/env bash
echo "systemctl $*" >> "$CALL_LOG"
EOF
chmod +x "$fake_bin/systemctl"

# Stub sudo: records its invocation and exits 0 (the happy path — the udev
# rule "installs" successfully) without ever touching the real filesystem.
cat > "$fake_bin/sudo" <<'EOF'
#!/usr/bin/env bash
echo "sudo $*" >> "$CALL_LOG"
EOF
chmod +x "$fake_bin/sudo"

# Stub the two cache-refresh tools (ticket 90): record the call, do nothing.
# The real ones are pure caches and already guarded in install.sh, but
# stubbing keeps this test hermetic and independent of whether they're
# installed on the machine running it.
for tool in update-desktop-database gtk-update-icon-cache; do
  cat > "$fake_bin/$tool" <<EOF
#!/usr/bin/env bash
echo "$tool \$*" >> "\$CALL_LOG"
EOF
  chmod +x "$fake_bin/$tool"
done

run_install() {
  HOME="$fake_home" CALL_LOG="$call_log" SANDBOX_ROOT="$work" PATH="$fake_bin:$PATH" \
    bash "$sandbox_repo/install.sh"
}

run_install
run_install # second run must succeed the same way, not error or diverge

[[ -x "$fake_home/.local/bin/acheron-daemon" ]] || fail "binary not installed to ~/.local/bin"
[[ -f "$fake_home/.config/systemd/user/acheron-daemon.service" ]] || fail "unit file not installed"
diff -q "$unit_file" "$fake_home/.config/systemd/user/acheron-daemon.service" >/dev/null \
  || fail "installed unit file content differs from packaging/acheron-daemon.service"

reload_count="$(grep -c '^systemctl --user daemon-reload$' "$call_log" || true)"
enable_count="$(grep -c '^systemctl --user enable --now acheron-daemon$' "$call_log" || true)"
[[ "$reload_count" -eq 2 ]] || fail "expected daemon-reload once per run (2 total), got $reload_count"
[[ "$enable_count" -eq 2 ]] || fail "expected enable --now once per run (2 total), got $enable_count"

echo "PASS: install.sh is idempotent across two runs, with correct binary/unit placement"

# --- udev rule install step (ticket 23) -----------------------------------
udev_cp_count="$(grep -c "^sudo cp .*60-acheron-tartarus-pro.rules /etc/udev/rules.d/60-acheron-tartarus-pro.rules\$" "$call_log" || true)"
reload_rules_count="$(grep -c '^sudo udevadm control --reload-rules$' "$call_log" || true)"
trigger_count="$(grep -c '^sudo udevadm trigger$' "$call_log" || true)"
[[ "$udev_cp_count" -eq 2 ]] || fail "expected the udev rule to be sudo-copied once per run (2 total), got $udev_cp_count"
[[ "$reload_rules_count" -eq 2 ]] || fail "expected udevadm control --reload-rules once per run (2 total), got $reload_rules_count"
[[ "$trigger_count" -eq 2 ]] || fail "expected udevadm trigger once per run (2 total), got $trigger_count"

echo "PASS: install.sh installs the udev rule and reloads udev on every run"

# --- GUI desktop-app launch path (ticket 90) ----------------------------
gui_lib="$fake_home/.local/lib/acheron/acheron_gui"
[[ -f "$gui_lib/__main__.py" ]] || fail "GUI package not installed to ~/.local/lib/acheron (missing __main__.py)"
[[ -f "$gui_lib/app.py" ]] || fail "GUI package not installed to ~/.local/lib/acheron (missing app.py)"
[[ -d "$gui_lib/__pycache__" ]] && fail "__pycache__ leaked into the installed GUI package"
# Ticket 102: the About dialog's "View Licence" reads this bundled copy.
diff -q "$repo_root/LICENSE" "$gui_lib/LICENSE" >/dev/null \
  || fail "GPLv3 text not bundled into the installed GUI package (about_dialog.py needs it)"

launcher="$fake_home/.local/bin/acheron-gui"
[[ -x "$launcher" ]] || fail "launcher not installed to ~/.local/bin/acheron-gui"
diff -q "$repo_root/packaging/acheron-gui" "$launcher" >/dev/null \
  || fail "installed launcher content differs from packaging/acheron-gui"

# Ticket 96: the launcher must not use `python3 -P` — that flag is CPython
# 3.11+ only and aborts with "Unknown option: -P" on the 3.9/3.10 that the
# common LTS targets ship, so the GUI never opens. It sanitizes sys.path by
# cd'ing into the installed package dir instead.
grep -qE '(^|[[:space:]])python3[[:space:]]+-P' "$launcher" \
  && fail "launcher uses 'python3 -P' — 3.11+ only, breaks on Ubuntu 22.04 / Debian 11 / RHEL 9"
grep -qxF 'cd "$acheron_lib"' "$launcher" \
  || fail "launcher no longer cd's into \$acheron_lib to sanitize sys.path"
echo "PASS: launcher avoids the 3.11-only python3 -P flag"

# Smoke check: actually *run* the installed launcher under the system
# python3 (the finding: nothing here ever executed it, only diffed its
# text). `--version` is handled before Gtk.Application, so it needs no
# display and no bus name — a launcher that can't start Python fails here.
launcher_out="$(HOME="$fake_home" "$launcher" --version)" \
  || fail "installed launcher exited non-zero on 'acheron-gui --version'"
[[ "$launcher_out" == acheron-gui\ * ]] \
  || fail "launcher --version printed unexpected output: $launcher_out"
HOME="$fake_home" "$launcher" --help >/dev/null \
  || fail "installed launcher exited non-zero on 'acheron-gui --help'"
echo "PASS: installed launcher runs under the system python3 (--version / --help)"

desktop="$fake_home/.local/share/applications/acheron.desktop"
[[ -f "$desktop" ]] || fail "desktop entry not installed to ~/.local/share/applications/acheron.desktop"
diff -q "$repo_root/packaging/acheron.desktop" "$desktop" >/dev/null \
  || fail "installed desktop entry differs from packaging/acheron.desktop"
grep -qxF "Exec=acheron-gui" "$desktop" || fail "desktop entry missing Exec=acheron-gui"
grep -qxF "Icon=acheron" "$desktop" || fail "desktop entry missing Icon=acheron"

for size in 16 24 32 48 64 128 256 512; do
  installed="$fake_home/.local/share/icons/hicolor/${size}x${size}/apps/acheron.png"
  [[ -f "$installed" ]] || fail "icon not installed at hicolor/${size}x${size}/apps/acheron.png"
done

# --- tray status icons (ticket 97) --------------------------------------
# The SNI host reads these from a stable per-user dir, never the git
# checkout — install.sh must place them at ~/.local/share/acheron/tray-icons.
tray_icons_dir="$fake_home/.local/share/acheron/tray-icons"
for name in acheron-running-connected acheron-running-disconnected acheron-not-running; do
  src="$repo_root/gui/acheron_gui/icons/$name.svg"
  dest="$tray_icons_dir/$name.svg"
  [[ -f "$dest" ]] || fail "tray status icon not installed at acheron/tray-icons/$name.svg"
  diff -q "$src" "$dest" >/dev/null \
    || fail "installed tray status icon $name.svg differs from gui/acheron_gui/icons/$name.svg"
done
echo "PASS: install.sh installs the three tray status icons outside the checkout"

if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$desktop" || fail "desktop-file-validate rejected the installed entry"
  echo "PASS: desktop entry validates"
fi

ddb_count="$(grep -c "^update-desktop-database .*/applications\$" "$call_log" || true)"
[[ "$ddb_count" -eq 2 ]] || fail "expected update-desktop-database once per run (2 total), got $ddb_count"

echo "PASS: install.sh installs the GUI launcher, desktop entry, and icons on every run"
