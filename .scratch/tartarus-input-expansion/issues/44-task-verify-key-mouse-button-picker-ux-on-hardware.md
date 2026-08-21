Type: task

## Question

Live-verify [Build the key/mouse-button picker UX for real](./42-task-build-key-mouse-button-picker-ux.md)'s build against the real, connected Tartarus Pro and GUI — the joint HITL session that ticket 42 itself skipped (no physical device access from that session, and per this map's ticket 26 precedent, swapping the user's live input/GUI state unasked is out of scope for an unattended session).

Checklist:

- Install the new binary and open the real GUI against the real Daemon; open a Grid key's Binding editor and confirm the Key field renders as the collapsed picker, not a text entry.
- Expand the picker and visually sanity-check the real popover's space budget against ticket 32's settled sizing (`_UNIT_PX = 28.5`, 12px font) — re-tune if the real popover clips or the window grows unexpectedly wide, per ticket 32's own two live-verified GTK4 findings (`propagate_natural_width`/`max_width_chars`) if either turns out to be needed here after all.
- Pick a handful of keys per category (letters, a function key, a multimedia key, a lock key, `F13`-`F24` behind the toggle, all five mouse buttons) and confirm each one round-trips through `config.toml`/D-Bus and produces the correct real output — the multimedia set and F13–F24 have never been live-tested before (only asserted reachable via `all_injectable_key_codes()`'s blanket sweep).
- Select a bare modifier (e.g. Left Ctrl) as the Key with Trigger mode at Fire-once — confirm the warning renders; switch Trigger mode to Toggle live and confirm it disappears without reselecting the key.
- Switch the Action to Macro, add a KeyDown step via the picker — confirm the step is added correctly and that no modifier warning appears even when the step's key is a bare modifier.
- Confirm the Device Overview grid button and Action Table row both show a readable label ("Mouse Left", not "BTN_LEFT") for a saved mouse-button Binding.
