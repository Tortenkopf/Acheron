Type: grilling

## Question

Right-clicking the Key textfield in the keybinding popup (`gui/acheron_gui/binding_editor.py`'s `key_entry`) shows a context menu with "Change Direction" and "Insert Emoji" — neither ever specified in `spec.md`, and neither added by Acheron's own code: `key_entry` is a plain, uncustomized `Gtk.Entry(text=...)` (confirmed by reading `binding_editor.py` — no context-menu/`extra_menu`/action-group wiring anywhere near it). These are almost certainly GTK4's own built-in default context-menu items for any `Gtk.Text`-backed widget, not something intentional:

- **"Insert Emoji"** opens GTK's built-in emoji picker and inserts the chosen Unicode emoji character as text at the cursor — GTK stock behavior, not Acheron code.
- **"Change Direction"** toggles the widget's paragraph text direction (RTL/LTR) — also GTK stock behavior, relevant for bidi text entry, not something Acheron wired up.

Confirm this reading live (right-click the field in the running GUI, try "Insert Emoji" and see what actually lands in the field) rather than trusting the code-reading alone, then decide:

- **Keep, suppress, or repurpose each independently.** For "Change Direction": Acheron's key field only ever holds short LTR evdev-code tokens (`KEY_F1`, `BTN_LEFT`, …) — is there any real use case, or is this pure clutter worth suppressing (`Gtk.Text`/`Gtk.Entry` exposes ways to trim the default context menu)?
- **For "Insert Emoji"**: does typing/inserting an emoji into this *specific* field currently do anything meaningful given the field expects an evdev key-code string (an emoji is not a valid `KeyCode` and would presumably fail validation/parsing)? Or is this purely an artifact of the field being a generic `Gtk.Entry` with no input constraint?
- **The use case the user raised**: binding an emoji so pressing the physical Input types that emoji. If wanted, this is a materially different feature than "the stock GTK entry happens to have an unrelated emoji-insert menu item" — it would need the Daemon's injection path to support emitting arbitrary Unicode (evdev/uinput has no native "type this codepoint" primitive; the usual approach is a compose-sequence or platform input-method trick), which the current keyboard-`KeyCode`-only `Action`/`MacroStep` model doesn't have room for. Decide whether that's worth scoping in here, spinning off as its own future idea, or ruled out of scope.

**Cross-reference**: [Decide mouse-button output + GUI picker](./02-decide-mouse-button-output-and-picker.md) is independently redesigning this same text field into a graphical picker. Whichever of the two tickets resolves first should check the other for invalidation — e.g. if the picker replaces the free-text `Gtk.Entry` entirely, the "which context-menu items to suppress" question here may become moot.
