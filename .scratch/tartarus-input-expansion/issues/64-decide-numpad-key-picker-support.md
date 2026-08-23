Type: grilling
Status: resolved

## Question

Surfaced by the user noticing a real gap in [the key/mouse-button picker](./42-task-build-key-mouse-button-picker-ux.md): `gui/acheron_gui/key_picker.py`'s catalog has no numpad entries at all (`KEY_KP0`–`KEY_KP9`, `KEY_KPENTER`, `KEY_KPPLUS`, `KEY_KPMINUS`, `KEY_KPASTERISK`, `KEY_KPSLASH`, `KEY_KPDOT`, `KEY_KPEQUAL`, …) — confirmed absent from `_ALL_ENTRIES`/every row list in that file. This is not merely a missing-output gap: numpad keys are **distinct evdev `KeyCode`s** from their non-numpad counterparts (`KEY_KP1` vs `KEY_1`, `KEY_KPENTER` vs `KEY_ENTER`) even though, under a normal Num Lock/desktop mapping, they often *produce* the same character. Acheron binds a grid key to a specific simulated physical key so it can match whatever a game's own keybinding screen expects — and some games bind numpad keys distinctly from their main-block twins (e.g. "press 1" vs "press Numpad 1" as different actions). So the missing coverage is a real functional gap, not just a labeling nicety.

The user's own framing, carried into this ticket rather than re-litigated: if added, numpad keys should sit behind a **collapsed toggle**, mirroring the existing "Show F13-F24 ▸" pattern in `_keyboard_grid()` (ticket 32/42's settled precedent) — not every keyboard has a numpad, but per the F13-F24 precedent that's not a reason to withhold access to it.

Settle at least:

- **Which codes**: confirm the exact evdev numpad set against the real `evdev` crate's `scancodes.rs` (same discipline ticket 42 used for its whole catalog) — the digits, `KEY_KPDOT`, `KEY_KPENTER`, `KEY_KPPLUS`, `KEY_KPMINUS`, `KEY_KPASTERISK`, `KEY_KPSLASH`, and whether `KEY_KPEQUAL`/`KEY_KPCOMMA`/`KEY_KPJPCOMMA`/`KEY_KPLEFTPAREN`/`KEY_KPRIGHTPAREN` are real, injectable, and worth exposing or too obscure to bother with.
- **Num Lock's role**: `KEY_NUMLOCK` is already in the picker (misc/lock strip) as a bindable target itself. Does the numpad toggle's block need any relationship to it, or does it stay fully independent (Acheron simulates the physical key press; what the OS/game does with Num Lock state is downstream and none of Acheron's concern)?
- **Toggle mechanics**: exact placement in `_keyboard_grid()` relative to the existing rows/clusters (own collapsed section vs. tucked into an existing cluster like Navigation/Lock), and whether it reuses the F13-F24 toggle's exact code shape (`fn_hi_state`/`toggle_hi` pattern) or needs its own local state — likely just a second instance of the same pattern, confirm rather than assume.
- **Layout inside the toggle**: physical numpad shape (4 rows: `7 8 9` / `4 5 6` / `1 2 3` / `0-spans-two . ` plus a right-hand column `NumLock / * -` above `+` spanning two rows, `Enter` spanning two rows) vs. a flatter single-row/grid listing — decide whether the physical arrangement is worth the layout complexity or a simpler grid reads fine, same "how should it look" question ticket 32 already answered once for the main keyboard.
- **Display labels**: e.g. "Num 7", "Num Enter", "Num +" — confirm naming convention consistent with existing labels (`LABEL_BY_CODE` nice names already distinguish "Left Ctrl"/"Right Ctrl"; numpad needs the same disambiguation from "7"/"Enter"/"+").
- **Reuse**: same component (`build_inline_key_picker`) already backs Binding Keypress, Macro steps, and (per ticket 62) Stepper items — confirm numpad keys need no special-casing at any of those three mount points, same as every other addition to the catalog so far.
- **Scope check**: is this purely a GUI catalog/layout addition (Daemon already advertises the full evdev range per ticket 02/42's precedent — confirm numpad codes are included in `all_injectable_key_codes()`'s sweep, don't just assume), or does anything else need touching?

Out of scope for this ticket: revisiting F13-F24's own settled design: any change to numpad toggle mechanics should follow that precedent, not reopen it.

## Answer

**Facts confirmed before design (same discipline as ticket 42's catalog):** `KEY_KP0`–`KEY_KP9` (0x47–0x52), `KEY_KPDOT` (0x53), `KEY_KPASTERISK` (0x37), `KEY_KPMINUS` (0x4a), `KEY_KPPLUS` (0x4e), `KEY_KPSLASH` (0x62), `KEY_KPENTER` (0x60) are all real entries in the `evdev` crate's `scancodes.rs`. The Daemon's `all_injectable_key_codes()` (`daemon/src/input.rs`) sweeps `0..=KEY_CODE_MAX` (`0x2ff`) unconditionally, so every one of these is already advertised — this is a pure GUI catalog/layout gap, **zero Daemon changes needed**, same shape as every prior picker addition (multimedia, F13-F24).

**Key set — Core 17 only**: `KEY_KP0`–`KEY_KP9`, `KEY_KPDOT`, `KEY_KPENTER`, `KEY_KPPLUS`, `KEY_KPMINUS`, `KEY_KPASTERISK`, `KEY_KPSLASH`. The more obscure evdev-defined extras (`KEY_KPEQUAL`, `KEY_KPCOMMA`, `KEY_KPJPCOMMA`, `KEY_KPLEFTPAREN`/`KEY_KPRIGHTPAREN`, `KEY_KPPLUSMINUS`) are real but JIS/Mac/scientific-calculator-style keys essentially no physical numpad exposes — excluded, consistent with the catalog's existing restraint elsewhere (no `KEY_102ND`, no obscure international variants).

**Layout — physical numpad grid**, matching the graphical-keyboard philosophy ticket 32 already chose for the main keys over a flat category list:
```
┌───┬───┬───┬───┐
│ 7 │ 8 │ 9 │   │
├───┼───┼───┤ + │
│ 4 │ 5 │ 6 │   │
├───┼───┼───┼───┤
│ 1 │ 2 │ 3 │   │
├───┴───┼───┤Ent│
│   0   │ . │   │
└───────┴───┴───┘
```
`/` and `*` and `-` render as a top row above `7 8 9` (three single-unit keys, no NumLock duplicate — `KEY_NUMLOCK` already lives in the existing Lock strip elsewhere in the picker, not re-added here). `+` spans two rows (7/8/9 through 4/5/6), `Enter` spans two rows (1/2/3 through 0/.), `0` spans two columns — mirroring the real hardware shape the same way the main grid mirrors a real keyboard.

**Placement**: directly below the existing keyboard rows (after `_SPACE_ROW`) and above the `clusters` row (Navigation/Arrows/Lock+Misc) and the Multimedia/Mouse-buttons sections — the user's own call, placed with the primary typing keys rather than grouped into the secondary-cluster row or tacked on at the very bottom.

**Toggle mechanics**: reuses the F13-F24 toggle's exact shape (`fn_hi_state`/`toggle_hi` pattern in `_keyboard_grid()`) — its own local collapse state, a "Show Numpad ▸" / "Hide Numpad ▾" button, no shared state with the F13-F24 toggle. Two independent toggles, not one generalized mechanism — consistent with the codebase's existing per-feature-not-generalized style for these collapsible sections (no premature abstraction).

**Num Lock's role — none, deliberately independent**: Acheron simulates the physical keypress; what the OS/game does with Num Lock state downstream is outside Acheron's concern, consistent with ticket 02's settled precedent ("Acheron is a remap tool, not a policy layer, and doesn't guard against other self-inflicted footguns"). No interaction with the existing `KEY_NUMLOCK` picker entry needed.

**Reuse**: no special-casing at any of the three existing mount points (Binding Keypress, Macro step, Stepper item per ticket 62/63) — same as every other catalog addition to date.

**Scope confirmed**: purely `gui/acheron_gui/key_picker.py` (catalog + `_keyboard_grid()` layout) — no Daemon changes, no CONTEXT.md changes (mirrors ticket 32's "no new domain term, purely a GUI affordance" precedent).

No prototype ticket needed — the layout/placement/key-set questions that would normally warrant one were settled directly in this grilling session by direct reuse of ticket 32's already-proven visual language (mirrors ticket 62's "settled by direct precedent, not asked as open questions" shortcut). Spawned [Build numpad support in the key picker](./65-task-build-numpad-key-picker.md).
