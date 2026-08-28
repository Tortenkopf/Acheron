Type: task
Blocked by: 89
Status: open

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
