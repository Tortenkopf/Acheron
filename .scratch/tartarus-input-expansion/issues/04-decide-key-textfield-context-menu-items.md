Type: grilling
Status: resolved

## Question

Right-clicking the Key textfield in the keybinding popup (`gui/acheron_gui/binding_editor.py`'s `key_entry`) shows a context menu with "Change Direction" and "Insert Emoji" — neither ever specified in `spec.md`, and neither added by Acheron's own code: `key_entry` is a plain, uncustomized `Gtk.Entry(text=...)` (confirmed by reading `binding_editor.py` — no context-menu/`extra_menu`/action-group wiring anywhere near it). These are almost certainly GTK4's own built-in default context-menu items for any `Gtk.Text`-backed widget, not something intentional:

- **"Insert Emoji"** opens GTK's built-in emoji picker and inserts the chosen Unicode emoji character as text at the cursor — GTK stock behavior, not Acheron code.
- **"Change Direction"** toggles the widget's paragraph text direction (RTL/LTR) — also GTK stock behavior, relevant for bidi text entry, not something Acheron wired up.

Confirm this reading live (right-click the field in the running GUI, try "Insert Emoji" and see what actually lands in the field) rather than trusting the code-reading alone, then decide:

- **Keep, suppress, or repurpose each independently.** For "Change Direction": Acheron's key field only ever holds short LTR evdev-code tokens (`KEY_F1`, `BTN_LEFT`, …) — is there any real use case, or is this pure clutter worth suppressing (`Gtk.Text`/`Gtk.Entry` exposes ways to trim the default context menu)?
- **For "Insert Emoji"**: does typing/inserting an emoji into this *specific* field currently do anything meaningful given the field expects an evdev key-code string (an emoji is not a valid `KeyCode` and would presumably fail validation/parsing)? Or is this purely an artifact of the field being a generic `Gtk.Entry` with no input constraint?
- **The use case the user raised**: binding an emoji so pressing the physical Input types that emoji. If wanted, this is a materially different feature than "the stock GTK entry happens to have an unrelated emoji-insert menu item" — it would need the Daemon's injection path to support emitting arbitrary Unicode (evdev/uinput has no native "type this codepoint" primitive; the usual approach is a compose-sequence or platform input-method trick), which the current keyboard-`KeyCode`-only `Action`/`MacroStep` model doesn't have room for. Decide whether that's worth scoping in here, spinning off as its own future idea, or ruled out of scope.

**Cross-reference**: [Decide mouse-button output + GUI picker](./02-decide-mouse-button-output-and-picker.md) is independently redesigning this same text field into a graphical picker. Whichever of the two tickets resolves first should check the other for invalidation — e.g. if the picker replaces the free-text `Gtk.Entry` entirely, the "which context-menu items to suppress" question here may become moot.

## Answer

**Moot, superseded by [Prototype the key/mouse-button picker UX](./32-prototype-key-mouse-button-picker-ux.md).** Ticket 02 resolved (picker exposes the full keyboard range + Left/Right/Middle/Back/Forward mouse buttons, no exclusions) and spawned ticket 32, whose own scope states it plainly: the picker is "replacing `binding_editor.py`'s free-text `Gtk.Entry` key field," with full coverage and no text-entry fallback for an uncovered case. Once that build lands, `key_entry` stops being an editable `Gtk.Text`-backed widget — it becomes a picker (graphical keyboard grid and/or category list, inline or via popover/dialog, per ticket 32's own open question) — so GTK's stock "Insert Emoji"/"Change Direction" context menu, which only attaches to editable text widgets, no longer applies. There is nothing left to keep, suppress, or repurpose on the current `Gtk.Entry`, and the emoji-as-Action idea raised in this ticket's third bullet is unaddressed by the picker (it only picks existing `KeyCode`/`BTN_*` targets) — if still wanted, it should be raised as a fresh, separate idea against the picker's eventual shape, not resurrected here.

Confirmed with the user rather than independently re-deciding: closed without the live GTK right-click verification, since the widget being verified is slated for replacement regardless of what stock GTK does today.
