Label: wayfinder:map

## Destination

Three new capabilities added on top of the shipped Acheron MVP ([Tartarus keybinder](../tartarus-keybinder/map.md)):

1. **Chord** Bindings — an Action triggered by two-or-more physical Inputs pressed simultaneously, distinct from any Binding on those same Inputs pressed individually.
2. **Mouse-button output** — a Keypress/Macro-step Action can target a mouse button (Left/Right/Middle/Side/Extra click) as well as a keyboard key, surfaced through a proper GUI picker instead of a hand-typed evdev code string.
3. **Stepper** — a generic construct pairing two physical Inputs ("forward"/"backward") to step-and-fire through a user-defined ordered list of fire-once Keypress/mouse-button items in one motion, intended primarily for the scroll wheel.

Like the previous map, this one carries execution.

## Notes

**This map carries execution** — resolving a ticket means actually building and testing against the real, connected Tartarus Pro, not only deciding (same discipline as the previous map).

**Grounding facts found before charting this map:**
- `Action::Keypress`'s `key` field is already a bare `evdev::KeyCode`, not a curated keyboard-only enum — nothing in the Daemon validates it against a keyboard allow-list.
- The virtual `uinput` device (`daemon/src/injector.rs::build_device`) already declares the *entire* standard `EV_KEY` range via `input::all_injectable_key_codes()`, and `BTN_*` codes (`BTN_LEFT`, `BTN_RIGHT`, `BTN_MIDDLE`, …) share the same numeric space as `KEY_*` in evdev — they're already advertised.
- evdev's `KeyCode::FromStr`/`Display` (the `evdev_enum!` macro) parses `"BTN_LEFT"` the same way it parses `"KEY_A"` — so `config.toml` can already represent a mouse-button target with zero Daemon changes.
- The GUI's key field (`gui/acheron_gui/binding_editor.py`) is a bare `Gtk.Entry` — no picker, no validation. This is the actual gap for mouse-button support, not the Daemon.
- Given the above, mouse-button *output* likely already works Daemon-side and mostly needs verification + a GUI picker, not new Daemon capability — confirm this empirically rather than assuming, since untested claims from reading code have been wrong before on this project (see the `ResetFailed`/`ResetFailedUnit` correction on the last map).

**Terminology (settled during charting, see CONTEXT.md):**
- **Chord** is reserved for the new simultaneous-Input concept. Keypress's existing modifier combination (Ctrl+Shift+T) is now called exactly that — "a modifier combination" — not "a chord," to avoid collision. CONTEXT.md's Keypress entry has been updated; full Chord/Stepper glossary entries are deliberately *not* added yet — each is a bare reserved name until its own ticket below settles the actual model, per domain-modeling's "update lazily, only when resolved" discipline.

**Skills to consult**: default to `/grilling` and `/domain-modeling` for the three decision tickets below. `/prototype` is likely warranted for the mouse-button/key GUI picker and the Stepper list-editing UX — each ticket's own grilling session should decide whether to spin one up, per the "how should it look/behave" test.

**Scope boundary volunteered directly by the user, not re-litigated in a ticket**: mouse-wheel motion is *not* an output Action — the Tartarus Pro's own wheel already passes through scroll natively, so nothing needs to synthesize scroll output. Mouse-button output is clicks only, never cursor movement.

## Decisions so far

(empty — charting session only; no tickets resolved yet)

## Not yet specified

- **Composition between the three features** — e.g. can a Chord's Action be a Stepper step; can a Stepper's forward/backward pair include a Chord as one side; can a Chord itself be one of a Stepper's two Inputs. Not sharp enough to ticket until the three individual tickets below have settled their own shape.
- **Design/prototype/build tickets** for each of Chord, mouse-button-output, and Stepper — expected to graduate once their respective grilling ticket below resolves (mirrors the previous map's shape: decide first, then design/prototype, then build against real hardware).

## Out of scope

- Mouse cursor movement (pointer X/Y motion) as an output Action — mouse-button support is clicks only.
- A synthetic mouse-scroll-wheel output Action — the Tartarus Pro's own wheel already passes through scroll natively (see Notes).
- Capturing input from a real external mouse device — Acheron's capture surface remains the Tartarus Pro's three evdev nodes only; "mouse buttons" here is output-side exclusively.
