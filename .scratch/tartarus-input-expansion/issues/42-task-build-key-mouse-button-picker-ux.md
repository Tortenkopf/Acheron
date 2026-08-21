Type: task

## Question

Build the key/mouse-button picker for real in `binding_editor.py`, replacing the free-text `Gtk.Entry` key field, against [Design the look and feel of the key/mouse-button picker GUI](./32-prototype-key-mouse-button-picker-ux.md)'s settled shape (variant A — Inline Keyboard Panel). No open design questions remain — this is implementation + live verification, per this map's "resolving a ticket means actually building and testing against the real, connected Tartarus Pro" discipline.

Scope, per ticket 32's Answer:

- **Inline picker widget**: a reusable component — a collapsed "`<key label>` ▸ Change" summary button that expands into the full keyboard grid in place (function row with F13–F24 behind a "Show F13-F24" toggle, number row, QWERTY/home/bottom rows, nav cluster, lock/misc strips, multimedia strip, and a mouse-button strip laid out Left/Middle/Right + gap + Back/Forward) — driven off the real `evdev::KeyCode` set, not the prototype's hand-listed ~112-entry catalog. Confirm live which real key names the Daemon actually round-trips (ticket 02 already verified this for a sample; this ticket's job is presenting the *full* set correctly, not re-verifying output).
- **Wire into `render_action_editor`**: replaces the current bare `Gtk.Entry` for the Keypress `key` field, alongside the existing Modifiers checkboxes, at the sizing settled in the prototype (grid unit ≈28.5px, 12px font — re-tune against the real popover's actual space budget rather than copying the numbers blindly).
- **Modifier warning**: surface ticket 02's settled note when the selected key is a modifier and Trigger mode isn't Toggle.
- **Reuse**: mount the same component for Macro step `KeyDown`/`KeyUp` values (today's step editor in `binding_editor.py`), and leave it ready for [Stepper item entry](./31-prototype-stepper-library-ux.md) and [ticket 41](./41-task-build-stepper-macro-library-ux.md) to consume once/if built after this ticket lands.
- **Window/popover sizing discipline**: apply ticket 32's two live-verified GTK4 findings if this picker (or anything nearby) ends up inside a `Gtk.ScrolledWindow` or grows any `wrap=True` label — `propagate_natural_width(True)` and `max_width_chars` are not optional extras, they're what makes natural sizing actually track content.

Live-hardware verification: confirm the full key range (not just ticket 02's sample) round-trips through `config.toml`/D-Bus/real output for a handful of keys per category, especially the multimedia set and F13–F24 (never live-tested before, only asserted reachable via the same `all_injectable_key_codes()` sweep).
