Type: task

## Question

Two defects a `/code-review` (high) pass on the ticket-93 branch surfaced in
**already-resolved** tickets 88 and 90 — noted here so they can be fixed as their own
batch rather than blocking ticket 93. Both are GUI-side (`gui/`), no daemon change.
Build and **live-verify in the same session** against the real GUI (a display is always
available on this machine); ticket 90's item also wants a check on an older `python3`.

### 1. GUI launcher uses `python3 -P`, which is Python 3.11+ only (ticket 90)

`packaging/acheron-gui:27` — the launcher `install.sh` drops at `~/.local/bin/acheron-gui`
and wires into the `Acheron` app-grid entry ends with:

```bash
exec python3 -P -m acheron_gui "$@"
```

The `-P` interpreter flag (keep `$CWD`/`""` off `sys.path`) **only exists on CPython
3.11+**. On any distro whose system `python3` is older it dies immediately with
`Unknown option: -P` (exit 2) and the GUI never opens:

- Ubuntu 22.04 LTS → 3.10
- Debian 11 (bullseye) → 3.9
- RHEL/Alma/Rocky 9 → 3.9

`install.sh` still prints `==> Done`, and `packaging/test_install.sh` only diffs the
generated script text (never executes it), so nothing catches this. The desktop-launch
feature ticket 90 shipped is effectively dead on the most common LTS targets.

Options to weigh:
- Replace `-P` with the portable equivalent: `exec python3 -m acheron_gui "$@"` after
  `export PYTHONSAFEPATH=1` (also 3.11+, no help) — so instead explicitly sanitize:
  run from a directory that has no `acheron_gui/` (e.g. `cd /` before the `exec`), or
  `exec python3 -c 'import sys; sys.path[:] = [p for p in sys.path if p not in ("", ".")]; from acheron_gui.__main__ import ...'`.
  The cleanest is probably `cd "$acheron_lib" && exec python3 -m acheron_gui "$@"` — the
  installed package dir is already first on `PYTHONPATH`, and `cd`ing there makes the
  implicit `""` entry resolve to the installed copy too, so a checkout's `./acheron_gui`
  can't shadow it.
- Decide whether to state a Python floor (3.9? 3.10?) in the install docs (ticket 35) and
  test `main.py` / the package against it.
- Give `packaging/test_install.sh` (or a new test) a smoke step that actually *runs*
  `acheron-gui --help` or equivalent under the system `python3`, so a launcher that can't
  start fails CI.

### 2. A truncated chord-member binding summary is unreadable anywhere (ticket 88)

`gui/acheron_gui/device_overview.py` — ticket 88 made every Device Overview button a
fixed 100×100 (grid) with the label `inner` capped at `set_width_chars(chars)` /
`set_max_width_chars(chars)` / `set_lines(3)` / `set_ellipsize(END)` where
`chars = 8 if w <= 100 else 14` (`:385`). Ticket 88 also added an unconditional full-text
tooltip so an ellipsized face is still readable on hover (`:431`):

```python
btn.set_tooltip_text(f"{label_line}  {summary_line}")
if not sensitive and insensitive_reason:
    btn.set_tooltip_text(insensitive_reason)
elif chord_tooltip:
    btn.set_tooltip_text(chord_tooltip)          # <-- overwrites the full-text tooltip
```

For a grid key that is **both** a Chord member **and** carries its own individual Binding,
`chord_tooltip` (from `_chord_button_style`, `:308`) wins — and it shows the *Chord's*
membership + the *Chord's* action, never this key's own individual Binding summary. So a
key bound to e.g. `Ctrl+Shift+Alt+F9  [1x]` that is also in a Chord:

- face: ellipsized to ~8 chars (`Ctrl+Sh…`)
- hover: "Part of Chord: … → …" — the individual binding text appears nowhere

Before ticket 88 the 76px button wrapped the full text with no ellipsis, so it was always
visible. Decide the fix: append the individual-binding line to `chord_tooltip` when both
exist, or build one combined tooltip (label + individual summary + chord membership) and
set it once instead of the current override chain.

Also flagged in the same review, lower confidence — fold in if cheap:
- `chars = 8 if w <= 100 else 14` (`:385`) is a magic two-bucket lookup not derived from
  `w` or font metrics; a short key label like `Q` wastes the width while a real binding
  summary can't use it. Worth a comment explaining the two buckets at minimum, or deriving
  `chars` from `w` and the label font's average advance.

### Verification

- Launcher: run `acheron-gui` (or the sanitized `python3 … -m acheron_gui`) under a
  3.10-or-older `python3` if one can be obtained (a container is fine), confirm it starts;
  confirm the app-grid `.desktop` entry still launches on this machine.
- Chord tooltip: bind a grid key to a long modifier-combination Keypress, add it to a
  Chord, screenshot the button and confirm both the individual binding and the Chord
  membership are reachable (face or hover). Regression-check a Chord-only member (no
  individual binding) and an individual-binding-only key still read correctly.
- Full Rust + Python suites green; `config.toml` restored byte-identical.
