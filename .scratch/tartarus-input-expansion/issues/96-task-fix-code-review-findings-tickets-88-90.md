Type: task
Status: resolved

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

## Answer

Both defects fixed, GUI-only (no daemon change), tests green throughout. Defect 2
(the tooltip) is **fully verified in this session** via the screenshot harness
(ticket 91/95 precedent — drives the real `AcheronApplication` against `DaemonStub`,
no daemon, no hardware). Defect 1 (the launcher) is verified as far as this machine
allows: its sys.path sanitization works under the system `python3` (3.14) and the
packaging suite now executes the installed launcher — but the actual failure mode
(a `python3` **older than 3.11**) can't be reproduced here (no docker/podman/pyenv/uv,
and the system `python3` is 3.14), and re-running `install.sh` to check the real
app-grid `.desktop` launch would rebuild the daemon and restart the user's live
service. Those two remain, spawned as
[Verify the ticket-88/90 code-review fixes](./104-task-verify-ticket-88-90-code-review-fixes.md).

### 1. Launcher `python3 -P` → portable sys.path sanitization (ticket 90)

`packaging/acheron-gui` — replaced `exec python3 -P -m acheron_gui "$@"` with:

```bash
cd "$acheron_lib"
exec python3 -m acheron_gui "$@"
```

`python3 -m` prepends the process cwd (`""`) to `sys.path` *ahead of* `PYTHONPATH`, so
the original reason for `-P` (a checkout's `./acheron_gui` shadowing the installed copy
when `acheron-gui` is run from inside `gui/`) is handled by `cd`ing into the installed
package dir first — then the implicit `""` entry resolves to `$acheron_lib`, which
holds the installed package, and nothing a checkout provides is on the path at all.
No version-specific interpreter flags remain, so Ubuntu 22.04 (3.10), Debian 11 (3.9)
and RHEL/Alma/Rocky 9 (3.9) are unblocked.

Verified live on this machine (system `python3` is 3.14): built a throwaway installed
layout, `cd`'d into a fake checkout's `gui/` containing a decoy `acheron_gui/__init__.py`
that `raise SystemExit`s if imported, ran the real `packaging/acheron-gui --version` from
there → printed `acheron-gui 1.0.0` (the *installed* package), decoy never touched.

**`packaging/test_install.sh`** — the finding was that it only diffed the launcher's
text, never ran it. Added:
- a regression guard that the installed launcher contains no `python3 -P` and still
  `cd`s into `$acheron_lib`;
- a real smoke step that **executes** the installed launcher under the system `python3`
  (`acheron-gui --version` and `--help`, asserting exit 0 + output shape) — a launcher
  that can't start Python now fails the packaging suite.

**`gui/acheron_gui/app.py`** — `main()` grew a tiny pre-`Gtk.Application` arg surface
(`--version` / `-V`, `--help` / `-h`) so that smoke step has something to run that needs
no display and acquires no bus name. `--version` prints `acheron-gui <__version__>`
(reuses ticket 99's `__version__`); anything else launches the GUI exactly as before
(`app.run([sys.argv[0]])` still drops argv, unchanged). This is the "or equivalent" the
ticket's own verification line allowed for `acheron-gui --help`.

No Python floor was hard-stated in code or a `main.py` version gate — the launcher no
longer needs one, and stating a supported-`python3` range is left to
[ticket 35](./35-task-write-release-documentation.md)'s install docs (a forward note is
already there). `install.sh` unchanged (it already `install -m 755`s the launcher).

### 2. Chord-member tooltip loses the individual binding (ticket 88)

`gui/acheron_gui/device_overview.py`, `make_input_button` — the old override chain
(`full text` → `insensitive_reason` → `chord_tooltip`, last wins) meant a grid key that
is **both** a Chord member **and** individually bound showed only the Chord's
membership+action on hover, and (post-ticket-88) its own summary was ellipsized on the
face — so the individual binding was readable nowhere. Rewritten to stack rather than
override:

- disabled key (`not sensitive` + reason) → still just the reason, nothing else is
  actionable;
- Chord member **with** its own Binding → `"{full_text}\n\n{chord_tooltip}"` — face text
  first, Chord membership below;
- Chord-only member (no individual Binding) → `chord_tooltip` alone, unchanged from
  before;
- otherwise → `full_text`, unchanged.

The lower-confidence `chars = 8 if w <= 100 else 14` item: added the requested comment
explaining the two width buckets (100px buttons → 8, key 20's 150px paddle → 14) and
that they're ticket 88's eyeball-tuned values against the real Yaru font, not derived
from `w`/metrics — a wider button added later needs its own bucket. Not deriving from
font metrics: out of proportion to the payoff for a fixed 2-width layout, and ticket 88
already rejected the floaty-width approach for a real GTK warning.

New regression test `test_chord_member_with_its_own_binding_shows_both_in_the_tooltip`
in `test_device_overview.py`: a key that is in a `{1,2}` Chord *and* bound to
`Ctrl+Shift+Alt+F9 [1x]` → tooltip `"1  Ctrl+Shift+Alt+F9  [1x]\n\nPart of Chord:\n1 + 2 → C  [1x]"`;
the Chord-only sibling key still reads `"Part of Chord:\n1 + 2 → C  [1x]"`.

**Live-verified via the screenshot harness.** New
`gui/tools/shot_device_overview.py` (sibling of `shot_library.py` /
`shot_binding_editor.py`) seeds grid key 1 as both a `{1,2}`-Chord member and a
`Ctrl+Shift+Alt+F9 [1x]` individual Keypress, key 3 as an individual-only binding,
renders the real Device Overview against `DaemonStub`, screenshots it, and dumps
every grid button's `get_tooltip_text()`. Assets in
[`assets/96-tooltip-shot/`](../assets/96-tooltip-shot/) — `tooltips.txt`:

| key | tooltip |
|---|---|
| 1 (Chord + own binding) | `1  Ctrl+Shift+Alt+F9  [1x]` ⏎⏎ `Part of Chord:` ⏎ `1 + 2 → C  [1x]` |
| 2 (Chord-only) | `Part of Chord:` ⏎ `1 + 2 → C  [1x]` |
| 3 (individual-only) | `3  Super+K  [hold]` |
| Mode (insensitive) | `Layer-shift Mode key: switch it to Bound above …` |

`device_overview.png` shows the grid rendering cleanly with the fix in place (no
crash, the Chords panel reads `1 + 2 → C  [1x]`). Note: in the harness's default
Adwaita theme key 1's face wraps rather than ellipsizes, so the "readable nowhere"
symptom is font/theme-dependent — but the tooltip **content loss** the finding
describes was unconditional (the old override dropped the individual summary for
*every* Chord-member-with-a-binding regardless of theme), and that is what's fixed.

### Suites

- **Python (GUI)**: `332 passed` (was 331; +1 new test). `.venv/bin/pytest`.
- **Rust (daemon)**: `369 passed` — unchanged, no daemon code touched, run as a baseline.
- **`packaging/test_install.sh`**: green, including the two new launcher checks.
- New file `gui/tools/shot_device_overview.py` (screenshot harness, not test-run in CI —
  matches `shot_library.py`/`shot_binding_editor.py`).
- `config.toml` never touched this session (no daemon run, no GUI run).

### For ticket 104 (what this session couldn't reach)

- The launcher run under a real `python3` **≤ 3.10** (a container) — the exact
  regression the `-P` removal targets, unreproducible on this box.
- `install.sh` re-run + the real app-grid `.desktop` click on this machine (ticket 90's
  `gtk-launch acheron.desktop` check), since only the launcher's internals changed and
  the `.desktop` file itself is untouched.

The tooltip fix needs no further verification — done above.
