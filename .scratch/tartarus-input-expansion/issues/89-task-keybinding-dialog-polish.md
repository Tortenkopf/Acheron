Type: task
Status: resolved

## Question

A batch of small, cohesive Keybinding-dialog cleanups the user surfaced via `/wayfinder`
(Round 1, questions Q2/Q3/Q4/Q8). All GUI-side except one narrow `daemon/src/config.rs`
serde change and a CONTEXT.md glossary rename. Build and **live-verify in the same session**
against the real Daemon + Tartarus Pro + GUI (a display and the device are always available on
this machine — screenshots included; the "no hardware access" caveat some earlier tickets
carried does not apply here).

### 1. Drop the last user-facing "passthrough" (Q2)

`gui/acheron_gui/binding_editor.py:939` — `clear_btn = Gtk.Button(label="Clear (passthrough)")`
becomes **`label="Clear Binding"`**. "Passthrough" is jargron the user doesn't want on a button,
and — once a running Daemon is in Analog Capture mode — it isn't even accurate for the 20 grid
keys (ticket 06 already stripped it from the binding *labels* for the same reason). Update the
three test call sites that look the button up by label
(`gui/tests/test_binding_editor.py:92, 105, 696, 1112` — `button_labeled(..., "Clear (passthrough)")`).
Clean the nearby `# ... passthrough ...` comments opportunistically; leave the
non-user-facing docstrings in `app.py`/`daemon_stub.py`/`inputs.py` alone.

### 2. Reorder the Action menu + rename Profile Switch → Switch Profile (Q3)

`gui/acheron_gui/inputs.py:112` `ACTION_TYPES` — reorder to exactly:

```python
ACTION_TYPES = [
    ("keypress", "Keypress"),
    ("controller_button", "Controller Button"),
    ("axis", "Axis"),
    ("macro", "Macro"),
    ("step", "Stepper"),
    ("profile_switch", "Switch Profile"),
]
```

- Internal identifiers stay untouched — the key `"profile_switch"`, the Rust
  `Action::ProfileSwitch`, `TriggerMode`, every D-Bus method and `config.toml` tag. This is a
  **display-label-only** rename.
- The existing subset filters still apply to the reordered list and need no change:
  `binding_editor.py:894` drops `"axis"` for non-grid Inputs; `:1028` drops `"profile_switch"`
  and `"axis"` for a Chord's own Binding.
- Rename the **CONTEXT.md glossary entry** "Profile Switch" → "Switch Profile" (the domain
  term the user reads), and sweep CONTEXT.md prose that names the user-facing concept
  ("a Profile Switch" in the Action and Chord entries → "a Switch Profile"). Keep code-level
  references to `Action::ProfileSwitch` / `ProfileSwitch` verbatim where CONTEXT.md quotes the
  type name.
- Grep for any other user-facing "Profile Switch" string in the GUI (`action_summary`,
  `ACTION_TYPES` consumers, dialog copy) and align.

### 3. Reorder the Trigger-mode menu + Hold-to-repeat as the default (Q4)

`gui/acheron_gui/inputs.py:93` `TRIGGER_OPTIONS` — reorder to exactly:

```python
TRIGGER_OPTIONS = [
    ("hold_to_repeat", "Hold-to-repeat"),
    ("toggle", "Toggle"),
    ("fire_once", "Fire-once"),
    ("analog_repeat", "Analog-repeat"),
]
```

`TRIGGER_SHORT` is a dict — order-independent, no change. `binding_editor.py`'s
`base_trigger_options` derives from `TRIGGER_OPTIONS` and filters `analog_repeat` for
non-grid/Chord cases — the reorder flows through automatically.

**New-binding default flips `fire_once` → `hold_to_repeat`:**

- `binding_editor.py` — the three `starting` fallbacks that hardcode `"trigger": "fire_once"`
  (`:845`, `:890`, `:1022`). New default is `"hold_to_repeat"` **except** when the Input is
  `wheel_scroll_up` or `wheel_scroll_down`, which stay `"fire_once"` (the wheel fires once per
  physical detent — Hold-to-repeat there would machine-gun). The axis `starting` at `:888` has
  no Trigger mode and is untouched. The Chord dialog's `starting` (`:1022`) has no single
  Input — default it to `hold_to_repeat` like the rest.
- Add a tiny helper (e.g. `default_trigger_for(inp: str | None) -> str` in `inputs.py`, next
  to `TRIGGER_OPTIONS`) rather than inlining the wheel check three times.
- The existing "Switch Profile locks Trigger to Fire-once" and "Controller Button drops
  Fire-once from the options" logic in `render_action_editor` still applies on top — verify a
  fresh Switch-Profile / Controller-Button binding still lands on a valid Trigger mode after
  the default changes.

**Align the serde default so GUI and config never diverge:**

- `daemon/src/config.rs` — `Binding.trigger: TriggerMode` (`:338`) is currently a **required**
  field (no `#[serde(default)]`). Add `impl Default for TriggerMode` returning
  `TriggerMode::HoldToRepeat`, and `#[serde(default)]` on `Binding.trigger`, so a hand-edited
  `config.toml` binding that omits `trigger` now parses as Hold-to-repeat (matching the GUI)
  instead of failing with serde's "missing field". This is backward-compatible — every
  existing config that spells `trigger` out still parses identically. Add a parse test for the
  omitted-field case. (There is no wheel exception on the daemon side — the seed Profile
  defines no wheel bindings, and the daemon has no "this Input is the wheel" notion at parse
  time. The wheel carve-out is GUI-authoring-only.)

### 4. Show F13–F24 directly, remove the toggle (Q8)

`gui/acheron_gui/key_picker.py:260–275` — delete `fn_hi_state`, `fn_hi_row_slot`,
`show_hi_btn` (label `"Show F13-F24 ▸"`), and `toggle_hi`. In their place, append
`_keycap_row(_FN_ROW_HI, on_pick, current)` directly, immediately after the F1–F12 row, so
F13–F24 render unconditionally. The collapse never actually saved vertical space (the button
occupies about the same height as the row it hides).

- **Leave the "Show Numpad" toggle (`:286`, ticket 65) exactly as it is** — the numpad is a
  genuine 4×4 block and its collapse does earn its space.
- Live-check the picker still fits its host `Gtk.Window` (ticket 44 made the per-key Binding
  editor a modal `Gtk.Window`; the library editors host it inline) with F13–F24 always
  present — one extra ~40px row. If it overflows, the existing `Gtk.ScrolledWindow` wrappers
  (ticket 70's `_vscrollable` / the Binding editor's capped Actuation section) should absorb
  it; confirm rather than assume.
- Update any key_picker test that toggled F13–F24 visibility to expect them always present.

### Verification (fold into this session)

- GUI screenshots of the reordered Action and Trigger menus, the "Clear Binding" button, and
  the F13–F24 row shown inline.
- A real binding round-trip confirming a freshly-created grid-key binding defaults to
  Hold-to-repeat, and a fresh `wheel_scroll_up` binding defaults to Fire-once, both persisting
  correctly to `config.toml` and firing as expected on the physical device.
- A hand-edited `config.toml` binding with no `trigger` line starts cleanly and behaves as
  Hold-to-repeat.
- Full Rust + Python suites green; `cargo fmt`/`clippy` clean.

If the session somehow lacks hardware, split the live checks into a `verify` follow-up ticket
(map precedent) — but the default expectation is fold-in.

## Answer

All four cleanups landed. The ticket's line numbers were written against a pre-40/52/54/55/71/78
revision and had drifted; each item was reconciled against current code. **Live verification
deferred to [ticket 95](./95-task-verify-keybinding-dialog-polish-on-hardware.md)** — see the end.

### 1. "Clear (passthrough)" → "Clear Binding" (Q2)

`binding_editor.py`'s `clear_btn` label is now `"Clear Binding"` (the line matched the ticket at
:939). The three test files' four `button_labeled(..., "Clear (passthrough)")` call sites
(`test_binding_editor.py:92, 105, 696, 1112` — all matched) updated. Cleaned the one nearby
comment (`# Already passthrough` → `# Already unbound`). Left `action_summary`'s docstring block
that explains *why* the default label carries no "passthrough" qualifier (ticket 06 — still the
live reasoning) and the non-user-facing docstrings in `app.py`/`daemon_stub.py`/`inputs.py`,
per the ticket.

### 2. Action menu reorder + Profile Switch → Switch Profile (Q3)

`inputs.py` `ACTION_TYPES` had drifted to six entries in the order
`keypress/macro/step/profile_switch/controller_button/axis`. Reordered to the ticket's exact
target — `keypress, controller_button, axis, macro, step, profile_switch` — and relabelled the
last entry `"Switch Profile"`. Internal key `"profile_switch"` and every Rust/D-Bus/`config.toml`
identifier untouched (display-label-only). The subset filters
(`binding_editor.py` non-grid drops `"axis"`; `build_chord_binding_dialog` drops
`"profile_switch"` + `"axis"`) are all key-based and order-independent — no change, as the ticket
predicted. `test_chord_binding_dialog_does_not_offer_profile_switch_as_an_action` strengthened to
assert the new `"Switch Profile"` label isn't offered.

**CONTEXT.md**: there is no dedicated "Profile Switch" glossary entry (Chord/Stepper/Controller
got entries when they resolved; Profile Switch never did), so the "rename the glossary entry"
step was N/A. Swept the two prose mentions of the user-facing concept — the **Action** entry
("…a Stepper step, a **Switch Profile**, or a Controller button press") and the **Chord** entry
("any kind except **Switch Profile**, which has nowhere to run…"). Left every `Action::ProfileSwitch`
/ `ProfileSwitch` code reference verbatim.

**Deliberately NOT touched** (flag for review): the Daemon's own user-facing rejection strings
still say "Profile Switch" (`dispatch.rs`: `"a Profile Switch Binding must use Fire-once"`,
`"Profile {name:?} is still referenced by a Profile Switch Binding"`, `"a Chord's Binding can't
be a Profile Switch"`), and `daemon_stub.py:241` mirrors the last one byte-for-byte. The ticket
scoped the Daemon to "one narrow `config.rs` serde change" and "internal identifiers stay", and
the stub must stay message-identical to the real Daemon, so these were left. If the user wants
the term aligned end-to-end, that's a small separate sweep (Daemon strings + stub together).

### 3. Trigger-mode reorder + Hold-to-repeat default (Q4)

`inputs.py` `TRIGGER_OPTIONS` reordered to `hold_to_repeat, toggle, fire_once, analog_repeat`.
`TRIGGER_SHORT` (a dict) unchanged. New `inputs.default_trigger_for(inp: str | None) -> str`
helper (next to `TRIGGER_OPTIONS`): returns `"fire_once"` for `wheel_scroll_up`/
`wheel_scroll_down`, `"hold_to_repeat"` otherwise (including `inp is None`).

Current `binding_editor.py` has only **two** `starting = existing or {…}` fallbacks, not the
ticket's three — the ticket's `:845` was the `profile_switch` branch of `get_binding()` returning
a Daemon-locked `"fire_once"`, which correctly stays. `build_binding_editor` now seeds
`default_trigger_for(inp)`; `build_chord_binding_dialog` seeds `default_trigger_for(None)`. The
axis `starting` (inert `trigger`, never read) left as-is. Verified by test that a fresh
Switch-Profile binding still forces `fire_once` and a fresh Controller-Button binding still lands
on `hold_to_repeat` (Fire-once excluded) after the default flip.

**Serde** (`daemon/src/config.rs`): added `impl Default for TriggerMode` → `HoldToRepeat` and
`#[serde(default)]` on `Binding.trigger`. No `skip_serializing_if` — the field is still always
written back out, so every existing `config.toml` round-trips byte-identically. New parse test
`a_binding_that_omits_trigger_parses_as_hold_to_repeat`. No GUI-style wheel carve-out on the
Daemon side (it has no "this Input is the wheel" notion at parse time — the seed Profile defines
no wheel bindings anyway).

### 4. F13–F24 shown inline, toggle removed (Q8)

`key_picker.py` `_keyboard_grid`: deleted `fn_hi_state`, `fn_hi_row_slot`, `show_hi_btn`
("Show F13-F24 ▸"), `toggle_hi`; now appends `_keycap_row(_FN_ROW_HI, on_pick, current)`
directly after the F1–F12 row. The "Show Numpad" toggle (ticket 65) is untouched. Two
`test_key_picker.py` tests rewritten (`test_f13_through_f24_are_shown_inline_with_no_toggle`,
`test_numpad_toggle_leaves_the_always_visible_f13_f24_row_alone`).

### Tests

- Rust: **357 pass** (was 356), `cargo fmt --check` + `cargo clippy --all-targets` clean.
- Python (GUI): **295 pass** (was 294). Renamed 3 tests, added 2
  (`test_clicking_an_unbound_scroll_wheel_direction_defaults_to_fire_once`,
  `test_f13_through_f24_are_shown_inline_with_no_toggle`), updated ~13 assertions from
  `fire_once` → `hold_to_repeat` for fresh-editor saves and 5 button-label lookups.

### Verification — deferred to ticket 95

A display (`DISPLAY=:0` / `WAYLAND_DISPLAY=wayland-0`) and the device (`/dev/hidraw*`) are
present, but: (a) `acheron-daemon.service` is **inactive** — the user stopped it ~10 min before
this session; (b) there is no screenshot / GUI-automation tooling in this environment
(`grim`, `spectacle`, `gnome-screenshot`, `scrot` all absent — the same gap tickets 42/48/40
recorded); (c) the round-trips need a human physically pressing keys and confirming firing
behavior, plus swapping the live Daemon binary under the user. Per the ticket's own
"split into a verify follow-up (map precedent)" clause and the 42→44 / 48→49 / 85→86 precedent,
spawned [ticket 95](./95-task-verify-keybinding-dialog-polish-on-hardware.md).
