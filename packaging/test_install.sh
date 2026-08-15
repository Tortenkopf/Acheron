#!/usr/bin/env bash
# Automated coverage for ticket 21's packaging: unit-file content and
# install.sh idempotency — the parts that don't require a real systemd
# --user session (per the ticket, the login-autostart and crash-recovery
# demos are manual/live-hardware verification, not covered here). cargo and
# systemctl are stubbed so this runs without a real build or a real systemd.
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
assert_contains "Restart=on-failure"
assert_contains "RestartSec=1"
assert_contains "StartLimitIntervalSec=60"
assert_contains "StartLimitBurst=5"
echo "PASS: unit file contains all required directives"

# --- install.sh idempotency, with cargo/systemctl stubbed ----------------
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

fake_home="$work/home"
fake_bin="$work/stub-bin"
call_log="$work/calls.log"
mkdir -p "$fake_home" "$fake_bin"
: > "$call_log"

# Stub cargo: records its invocation, then drops a fake release binary where
# install.sh expects to find one, so the rest of the script has something
# real to `cp` without an actual Rust build.
cat > "$fake_bin/cargo" <<'EOF'
#!/usr/bin/env bash
echo "cargo $*" >> "$CALL_LOG"
manifest=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--manifest-path" ]]; then
    manifest="$arg"
  fi
  prev="$arg"
done
daemon_dir="$(dirname "$manifest")"
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

run_install() {
  HOME="$fake_home" CALL_LOG="$call_log" PATH="$fake_bin:$PATH" bash "$repo_root/install.sh"
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
