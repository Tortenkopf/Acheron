Type: grilling

## Question

Finalize mouse-button output support and design the GUI's key/action picker. Settle at least:

- **Verify, don't assume**: the map's Notes record that the Daemon likely already supports targeting a mouse button (`BTN_LEFT`/`BTN_RIGHT`/`BTN_MIDDLE`/etc.) from `Action::Keypress`/Macro steps with zero code changes, based on reading `injector.rs`/`input.rs` and the `evdev` crate's `FromStr`. Confirm this empirically against the real, connected Tartarus Pro/Daemon before treating it as settled — hand-edit a `config.toml` Binding to `key = "BTN_LEFT"` (or via `busctl SetBinding`) and confirm a real left-click actually lands.
- **Modifier-combination interaction**: does holding a modifier while a mouse-button Action fires make sense (e.g. Ctrl+Click), and if so does it reuse the exact same `Modifiers` mechanism as a keyboard Keypress?
- **Macro steps**: do `MacroStepDto::KeyDown`/`KeyUp` already work identically for mouse buttons (they should, per the same `evdev::KeyCode` typing), or is there a wrinkle worth surfacing?
- **Scope button set**: which mouse buttons does the GUI actually expose — Left/Right/Middle plus Side/Extra (`BTN_SIDE`/`BTN_EXTRA`), or a narrower set?
- **GUI picker design**: replace (or augment) the free-text `Gtk.Entry` key field in `binding_editor.py` with a graphical picker covering both keyboard keys and mouse buttons. This is very likely a "how should it look/behave" question — use `/prototype`.

Out of scope for this ticket (per the map): cursor movement, synthetic scroll output, capturing from a real external mouse.
