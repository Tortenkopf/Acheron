Type: task
Blocked by: 89
Status: resolved

## Question

Live-verify [ticket 89](./89-task-keybinding-dialog-polish.md)'s four Keybinding-dialog
cleanups against the real Daemon + Tartarus Pro + GUI. Ticket 89 landed the build (357 Rust /
295 Python tests green, `fmt`/`clippy` clean) but couldn't fold in the live checks: the Daemon
was stopped and this environment has no screenshot/GUI-automation tooling. Needs the new Daemon
binary installed and the live `systemd --user` service restarted mid-session (ticket 34's
precedent), with the user's `config.toml` backed up and diffed byte-identical afterward.

Checklist:

- **Screenshots / visual confirmation** (the user drives the GUI; agent can't screenshot here):
  - The Action dropdown reads, top to bottom: Keypress, Controller Button, Axis, Macro, Stepper,
    Switch Profile (and Axis/Switch-Profile still absent for a non-grid Input / a Chord's own
    Binding).
  - The Trigger-mode dropdown reads: Hold-to-repeat, Toggle, Fire-once, Analog-repeat
    (Analog-repeat still absent for a non-grid Input; Fire-once still absent when Action-kind is
    Controller Button).
  - The per-key editor's clear button reads **"Clear Binding"**, not "Clear (passthrough)".
  - The key picker shows the **F13–F24 row inline** directly under F1–F12, with no
    "Show F13-F24" toggle. The "Show Numpad" toggle is still there and still works.

- **Picker fits its host** (ticket 89 §4's open concern): with F13–F24 always present (one extra
  ~28px row), confirm the key picker still fits inside the per-key modal `Gtk.Window`
  (`device_overview.make_input_button`) and the inline library-editor mounts. If it overflows,
  confirm the existing `Gtk.ScrolledWindow` wrappers (ticket 70's `_vscrollable` / the Binding
  editor's capped Actuation section) absorb it rather than the window growing past the screen.

- **New-binding default, real round-trip**:
  - A freshly-created **grid-key** binding (e.g. `grid_r2c3` → some key) defaults to
    **Hold-to-repeat**, persists to `config.toml` as `trigger = "hold_to_repeat"`, and actually
    re-fires while held on the physical device.
  - A freshly-created **`wheel_scroll_up`** (or `_down`) binding defaults to **Fire-once**,
    persists as `trigger = "fire_once"`, and fires once per physical detent (not machine-gunned).
  - A fresh **Switch Profile** binding still lands on Fire-once (locked) and a fresh
    **Controller Button** binding still lands on Hold-to-repeat — both save and behave correctly.

- **Serde default, hand-edited config**: hand-edit a binding in `config.toml` to omit the
  `trigger` line entirely, restart the Daemon, and confirm it (a) starts cleanly with no
  "missing field `trigger`" error and (b) behaves as Hold-to-repeat on the physical device.
  Then confirm a binding that *does* spell `trigger` out still parses and behaves identically
  (backward-compat).

- **Regression spot-check**: opening an existing binding (any Trigger mode, any Action kind)
  still preselects its real saved values — the reorder/default changes only affect
  *freshly-created* bindings, never existing ones.

- Clean up every throwaway Profile/Binding; diff `config.toml` byte-identical to its
  pre-session backup.

## Answer

Every checklist item verified against the daily-driver `acheron-daemon` (installed binary +
`~/.local/lib/acheron` GUI both confirmed **byte-identical to HEAD** `a326f25` before starting —
`cmp` on a fresh `cargo build --release`, `diff -rq` on the GUI package) + the real Tartarus Pro
in **Analog Capture** mode + the "Acheron Virtual Tartarus Pro" (`event26`) and "Acheron Virtual
Controller" (`event27`) `uinput` nodes. Ticket 89's build needed **no** daemon or GUI change.
Suites: **365 Rust / 317 Python green**, `cargo fmt --check` + `cargo clippy --all-targets` clean.

Ticket 95's premise that "this environment has no screenshot/GUI-automation tooling" was already
known stale (ticket 91's Answer). New reusable harness `gui/tools/shot_binding_editor.py` (sibling
of ticket 91's `shot_library.py`) drives the real `AcheronApplication` against `DaemonStub`,
opens `device_overview.make_input_button`'s modal editor `Gtk.Window`, drives its dropdowns, and
self-screenshots via the toplevel GSK renderer — forcing a `NON_UNIQUE` private application id so
it coexists with the user's running GUI (ticket 94's flagged blocker for 95+, now solved).

### 1. Screenshots / visual confirmation — PASS

Harness screenshots + programmatic dropdown-model dumps (`be_models.txt`):

- **Action menu (grid key)**: `Keypress · Controller Button · Axis · Macro · Stepper · Switch
  Profile` — exactly ticket 89 §2's order, last entry labelled **"Switch Profile"**.
- **Action menu, non-grid Input (`wheel_scroll_up`)**: `Keypress · Controller Button · Macro ·
  Stepper · Switch Profile` — **Axis absent**.
- **Action menu, Chord's own Binding**: `Keypress · Controller Button · Macro · Stepper` — **both
  Axis and Switch Profile absent**.
- **Trigger menu (grid key)**: `Hold-to-repeat · Toggle · Fire-once · Analog-repeat` — exactly
  §3's order.
- **Trigger menu, non-grid Input / Chord**: `Hold-to-repeat · Toggle · Fire-once` — **Analog-
  repeat absent**.
- **Trigger menu, Action-kind = Controller Button**: `Hold-to-repeat · Toggle · Analog-repeat` —
  **Fire-once absent** (ticket 78 restriction, still live).
- **Clear button** reads **"Clear Binding"** (screenshot), not "Clear (passthrough)".
- **F13–F24 row renders inline** directly under F1–F12, **no "Show F13-F24" toggle**. The **"Show
  Numpad ▸"** toggle is still present.
- Fresh-editor Trigger dropdown lands on index 0 = **Hold-to-repeat** (grid key / mode key /
  thumbstick / Chord), index 2 = **Fire-once** (`wheel_scroll_up`/`_down`), index 2 insensitive =
  **Fire-once locked** (Switch Profile).

### 2. Picker fits its host — PASS

- **Per-key modal `Gtk.Window`**: natural size **599 × 1015 px** on the 1920 × 1080 display. The
  entire key picker (Esc row → F13–F24 row → … → Mouse buttons) renders above the Save / Clear
  Binding row; only the `Actuation & release` section sits below, and that already has its own
  internal `Gtk.ScrolledWindow` cap (ticket 70 follow-up), so it — not the picker or the window —
  absorbs any height pressure. No clipping, no overflow past the screen.
- **Inline library-editor mounts**: the F13–F24 row lives in the *shared* `key_picker._keyboard_
  grid`, used by `build_inline_key_picker` too. Ticket 89 landed (`541038d`) **before** ticket 91
  (`ed28015`), so ticket 91's own homogenization screenshots already show the always-inline
  F13–F24 row fitting and pixel-aligned in both the Stepper and Macro editors (`shot_library.py`,
  measured y=228). Nothing regressed since.

### 3. New-binding default, real round-trip — PASS

Driven in a throwaway `Wf95` Profile (`default_actuation` 26/10 so a normal press registers in
Analog mode — ticket 94 precedent); bindings created by feeding the daemon the *exact* dict the
GUI's own `build_binding_editor().get_binding()` produces for a fresh binding (verified
separately — grid → `hold_to_repeat`/keypress, wheel → `fire_once`/keypress, mode
key/thumbstick → `hold_to_repeat`, Controller Button → `hold_to_repeat`, Switch Profile →
`fire_once`). `evtest` on the virtual nodes while the user pressed the physical keys:

| fresh binding | persisted in `config.toml` | physical behaviour |
|---|---|---|
| `grid_r2c3` → KEY_F13 | `trigger = "hold_to_repeat"` | held ~2 s → ~62 discrete re-fires, **~33 ms period** (matches the device's kernel autorepeat), clean stop on release, no stuck key |
| `wheel_scroll_up` → KEY_F14 | `trigger = "fire_once"` | 5 slow detents → **exactly 5** single down+up pulses, one per detent, **no machine-gunning** |
| `grid_r2c4` → BTN_SOUTH | `trigger = "hold_to_repeat"` | held ~2 s → BTN_SOUTH **one down, sustained, one up** on `event27` only, **zero leakage** to `event26` (Controller-Button sustained-hold, ticket 78/86) |
| `grid_r3c2` → Switch Profile `MnM` | `trigger = "fire_once"` (locked) | single tap → active profile `Wf95` → `MnM`, `active_profile` persisted |

### 4. Serde default, hand-edited config — PASS

Daemon stopped, the `trigger = "hold_to_repeat"` line **deleted** from `[profiles.Wf95.base.grid_
r2c3]` in `config.toml`, daemon restarted:

- **Starts cleanly** — `systemctl --user is-active` = `active`, journal shows a normal start, **no
  "missing field `trigger`"** error.
- `GetConfig()` reports `grid_r2c3.trigger` = `"hold_to_repeat"` — `#[serde(default)]` →
  `TriggerMode::default()` filled it, and it re-serializes with the field spelled back out.
- **Behaves as Hold-to-repeat**: user held `grid_r2c3` ~2 s → 216 events, same ~33 ms re-fire
  cadence as check 1.
- **Backward-compat**: every binding that *does* spell `trigger` out parsed and behaved
  identically — `grid_r2c4` (`hold_to_repeat`) in check 3, `grid_r3c2`/`wheel_scroll_up`
  (`fire_once`), plus all of `MnM`/`Testing`.

### 5. Regression spot-check — PASS

`build_action_and_trigger_fields` built against the **live** `GetConfig()` for five existing
bindings — each dropdown preselects the exact saved value after the reorder:

| existing binding | saved | Action dd | Trigger dd |
|---|---|---|---|
| `MnM/base/grid_r1c1` | `fire_once` keypress | Keypress | Fire-once |
| `MnM/base/thumbstick_left` | `toggle` keypress | Keypress | Toggle |
| `MnM/base/wheel_scroll_up` | `fire_once` step | Stepper | Fire-once |
| `MnM/held/grid_r1c1` | `fire_once` keypress | Keypress | Fire-once |
| `Testing/base/grid_r1c1` | `analog_repeat` controller_button | Controller Button | Analog-repeat |

`get_binding()` round-trips each back to its original `trigger`/`type`. The reorder/default
changes touch only freshly-created bindings.

### Cleanup

Daemon stopped, `config.toml` **hard-restored** from the pre-session backup, daemon restarted:
**byte-identical** (`sha256 78b160965d1ba185…`, `cmp` clean), no `Wf95` residue, daemon back on
the original `Default` Profile, device connected in Analog mode. No stray `evtest` processes;
BTN_SOUTH confirmed released, `active_toggles` empty.

### Not in scope (still flagged for the user, from ticket 89's Answer)

The Daemon's own user-facing rejection strings still say "Profile Switch" (`dispatch.rs` ×3 +
`daemon_stub.py` mirror). Ticket 89 scoped the Daemon to one `config.rs` serde change; this
remains a small optional end-to-end term sweep, untouched here.

### Map status

This resolves the last verification ticket of the `/wayfinder` GUI-polish cluster (89–95). Every
ticket 89 cleanup — the "Clear Binding" relabel, the Action + Trigger menu reorders, the
"Switch Profile" label, the Hold-to-repeat new-binding default with the wheel Fire-once carve-out,
the `TriggerMode` serde default, and the always-inline F13–F24 row — is now hardware-verified.
