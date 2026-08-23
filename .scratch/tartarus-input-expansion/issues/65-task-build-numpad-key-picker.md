Type: task
Blocked by: 64
Status: open

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
