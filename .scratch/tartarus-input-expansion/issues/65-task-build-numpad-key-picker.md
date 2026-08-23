Type: task
Blocked by: 64
Status: resolved

## Question

Build numpad support into the key picker for real, in `gui/acheron_gui/key_picker.py`, against [Decide whether the key picker should expose the numpad](./64-decide-numpad-key-picker-support.md)'s settled shape. No open design questions remain — this is implementation + live verification, per this map's "resolving a ticket means actually building and testing against the real, connected Tartarus Pro" discipline.

Scope, per ticket 64's Answer:

- **Catalog**: add the core 17 numpad entries to `_ALL_ENTRIES`/`LABEL_BY_CODE` — `KEY_KP0`–`KEY_KP9`, `KEY_KPDOT`, `KEY_KPENTER`, `KEY_KPPLUS`, `KEY_KPMINUS`, `KEY_KPASTERISK`, `KEY_KPSLASH` — with nice display labels that disambiguate from their main-block twins (e.g. "Num 7", "Num Enter", "Num +", "Num .", "Num /", "Num *", "Num -"), following the same disambiguation convention `_MODIFIERS_NICE` already uses for Left/Right Ctrl etc.
- **Layout**: a new `_NUMPAD_BLOCK` (or equivalent row set) in the physical grid shape ticket 64 settled (`/ * -` row, `7 8 9` + tall `+`, `4 5 6`, `1 2 3` + tall `Enter`, wide `0` + `.`). `Gtk.Grid` or nested row `Gtk.Box`es — whichever fits the existing `_keycap_row`/spacer-cell conventions most directly; a tall/wide key (`+`, `Enter`, `0`) needs either a real grid span or a stacked-box composition, since today's `_keycap_row` only does horizontal single-row strips — check what's simplest against the existing code rather than assuming.
- **Toggle**: mirror `_FN_ROW_HI`'s exact `fn_hi_state`/`toggle_hi`/`show_hi_btn` pattern in `_keyboard_grid()`, independently — its own local state, a "Show Numpad ▸" / "Hide Numpad ▾" button, no shared state with the F13-F24 toggle.
- **Placement**: insert directly after `_SPACE_ROW` and before the `clusters` Gtk.Box (Navigation/Arrows/Lock+Misc), per ticket 64's settled placement.
- **No Daemon changes** — confirm live rather than re-deriving: numpad codes are already in `all_injectable_key_codes()`'s sweep (ticket 64 confirmed this by reading `daemon/src/input.rs`, but this ticket's live-verification pass should still exercise a couple of them end to end, same as ticket 42 did for its own catalog).
- **No CONTEXT.md changes** — purely a GUI catalog/layout addition, per ticket 64.

Live-hardware verification: confirm a handful of numpad keys (at least one digit, `KEY_KPENTER`, and one operator like `KEY_KPPLUS`) round-trip through `config.toml`/D-Bus/real output against the real Tartarus Pro — same bar ticket 42/44 used for the rest of the catalog.

## Answer

Built in `gui/acheron_gui/key_picker.py`, no Daemon changes:

- **Catalog**: `_NUMPAD_NICE` adds the core 17 entries (`KEY_KP0`-`KEY_KP9`, `KEY_KPDOT`, `KEY_KPENTER`, `KEY_KPPLUS`, `KEY_KPMINUS`, `KEY_KPASTERISK`, `KEY_KPSLASH`) to `_ALL_ENTRIES`/`LABEL_BY_CODE`, labels disambiguated as "Num 7", "Num Enter", "Num +", etc. `key_css_class` gives them no special class — same as any other plain `KEY_*` entry, consistent with letters/digits.
- **Layout**: `_NUMPAD_TOP_ROW` (a plain `_keycap_row` strip: `/ * -`) plus a new `_numpad_block()` builder using a real `Gtk.Grid` (not `_keycap_row`, which can't span) for the 4x4 area — `Num +` spans two rows (7/8/9 through 4/5/6), `Num Enter` spans two rows (1/2/3 through 0/.), `Num 0` spans two columns, matching ticket 64's settled diagram exactly.
- **Toggle**: `numpad_state`/`toggle_numpad` in `_keyboard_grid()` mirrors `fn_hi_state`/`toggle_hi` exactly but independently — its own local dict, its own "Show Numpad ▸"/"Hide Numpad ▾" button, no shared state with the F13-F24 toggle. Verified independent in a dedicated test.
- **Placement**: inserted directly after `_SPACE_ROW`'s row and before the `clusters` box, per ticket 64.
- **Daemon confirmed clean, not re-derived**: read `daemon/src/input.rs::all_injectable_key_codes()` — it sweeps `0..=0x2ff` unconditionally, so every numpad code is already advertised (`KEY_KP7` = 0x47, `KEY_KPENTER` = 0x60, `KEY_KPSLASH` = 0x62, all well under the ceiling). Cross-checked all 13 numeric/operator codes exist in the real `evdev` crate's `scancodes.rs` (0.13.2) at the expected values. Zero Daemon changes.
- **No CONTEXT.md changes** — purely a GUI catalog/layout addition, matching ticket 64's call.

Added 6 new tests to `gui/tests/test_key_picker.py` (toggle show/hide, toggle independence from F13-F24, picking a numpad key reports its `KEY_KP*` code and updates the summary label, `LABEL_BY_CODE` coverage including confirming `KEY_KPEQUAL` was deliberately excluded). Full suite: 244 Python tests green (no regressions). No Rust changes, so no Rust tests to run.

**Live-hardware verification not done this session** — no physical access to the Tartarus Pro from this environment. Everything checkable without the device (catalog values, evdev code validity, Daemon advertising range, full test suite) is confirmed; spawned [Verify the numpad key picker on hardware](./66-task-verify-numpad-key-picker-on-hardware.md) for the actual round-trip, mirroring the ticket 42→44/43→45/48→49/52→53/55→56 build/verify split this map has used throughout.
