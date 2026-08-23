Type: task
Status: resolved
Blocked by: 54, 52

## Question

Build the real Stepper library GUI against [ticket 54](./54-task-land-stepper-library-daemon.md)'s landed Daemon shape and [ticket 31](./31-prototype-stepper-library-ux.md)'s settled variant B — split off [ticket 41](./41-task-build-stepper-macro-library-ux.md). Blocked on [ticket 52](./52-task-build-macro-library-gui.md) because it fills in the Steppers tab within the tab-switched shell that ticket built (the shell itself is not rebuilt here). No open design questions remain.

Scope:

- **Steppers panel**: fills in the Steppers tab of the shell [ticket 52](./52-task-build-macro-library-gui.md) built. List chrome (name / rename "✎" / delete "×" / "+ New" — no delete gate, unlike Macro: ticket 03 never specified one, since reassignment already silently moves a list off its pair rather than something being "in use" to protect against), an item editor with ↑/↓/× (add via a Key/Mouse-button kind selector + the real picker below).
- **Assignment row**: Forward/Backward Input dropdowns beneath the item list. Assigning a pair already claimed by another list silently steals it and surfaces a toast ("Moved off '<name>' (it no longer has an assigned pair)"), per ticket 31's settled specifics.
- **Autosave note**: the pane states upfront that edits save automatically, matching the Macros panel's pattern from ticket 52.
- **Item entry**: reuses the real key/mouse-button picker (`key_picker.py`, ticket 42/44) — not a redesign, same reuse as ticket 52's Macro step editor.
- **`binding_editor.py`**: the Action dropdown gains "Stepper" as a third option alongside Keypress/Macro, assigning a library entry rather than authoring one inline.

Live-hardware verification is deliberately deferred to [Verify the Stepper library on hardware](./56-task-verify-stepper-library-on-hardware.md), not done in this ticket.

## Answer

Built as scoped, GUI-only (no Daemon/D-Bus changes needed — ticket 54 already left `daemon_client.py`/`daemon_stub.py` fully wired for Stepper). GUI test count went from 207 to 235 (28 net new tests, after also rewriting 3 obsolete ones — see below). `.venv/bin/python -m pytest tests` all green; Daemon suite (258 tests), `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check` all still clean, confirming this ticket touched nothing under `daemon/`.

### `gui/acheron_gui/library_view.py`

`build_steppers_panel` replaces the ticket-52-era `build_steppers_stub`, mirroring `build_macros_panel`'s list-plus-editor shape with three settled differences (ticket 31 round 2's Answer):

- **No used-by gate on delete, but the Daemon's own refusal still applies.** Re-reading `dispatch.rs`'s `DeleteStepper` handler (landed by ticket 54) found it refuses while referenced, mirroring `DeleteMacro` exactly — this ticket's own "no delete gate... since ticket 03 never specified one" text turned out to describe the *GUI* treatment only, not the Daemon's. Resolved by not pre-emptively greying "×" (unlike Macro's `used_by`-gated button/tooltip), but still wrapping the click in the same `DaemonError`→`show_error` pattern every other mutation here uses, so a referenced Stepper's delete attempt surfaces the Daemon's real rejection instead of either silently failing or crashing.
- **No item-kind selector.** `StepperItem` has exactly one wire variant (`Key`, covering both keyboard keys and mouse buttons via `key_picker`'s one already-unified picker) unlike Macro's three (KeyDown/KeyUp/Delay), so there's nothing for a kind dropdown to choose between — item entry is just the inline key picker plus "+ Add item".
- **An assignment row** (Forward/Backward `Gtk.DropDown`s, populated from `inputs.ALL_INPUTS` plus a leading "— Unassigned —") sits below the item list, scoped to the same Profile/Layer `device_overview.build_main_view` already threads through everywhere else (`build_library_view` gained `profile`/`layer` parameters to reach it). Reassigning the *same* list off its own old pair needs no client-side logic — `SetBinding`'s `take_stepper_direction_elsewhere` (ticket 54) already does that server-side. What the Daemon doesn't announce back is a plain overwrite silently stealing a *different* list's Binding at the newly-picked Input; this module detects that itself (reading the target Input's existing Binding before calling `SetBinding`) and leaves a one-shot "Moved off '<name>'" notice in `ui_state["stepper_toast"]`, popped and shown once by the editor pane's next render (a plain `Gtk.Label`, new `.toast` CSS class in `app.py` — no toast-widget mechanism exists anywhere in this GTK4-only, non-libadwaita GUI, so this follows the codebase's existing `error_label`-style inline-notice convention rather than inventing one).

### Code review findings, fixed

- **The steal-toast missed the case where a list steals from *itself*.** `take_stepper_direction_elsewhere` only guards the *same* direction living on two Inputs — it has no opinion on reassigning Forward onto the Input that's currently this same list's own Backward. That's a plain Binding-slot overwrite (no server-side signal at all), so with only the cross-*list* steal check, Backward silently vanished with zero notice — a real, reachable case (Forward=A/Backward=B already assigned, reassign Forward to B), not a hypothetical. Fixed by widening `build_stepper_assignment_row`'s steal detection to also compare direction when the existing Binding names the *same* stepper, with its own distinct toast wording ("Also cleared this list's own Backward assignment…"). New regression test.
- **The item-entry key picker showed an inapplicable modifier warning.** `build_stepper_editor`'s "+ Add item" picker used `build_inline_key_picker`'s default `warn_predicate` (always warn on a bare modifier), which recommends "Use Toggle with a single KeyDown-only Macro step" — impossible advice here, since a Stepper item always fires as a bare KeyDown/KeyUp pair and Toggle is disallowed outright for a Stepper Binding. The Macro step editor already suppresses this warning for its own KeyDown-only steps (for a different reason — there the workaround *is* what a KeyDown-only step already is), but the Stepper item picker was never given the same override. Fixed with `warn_predicate=lambda: False`. New regression test.

### `gui/acheron_gui/binding_editor.py`

`ACTION_TYPES` (`inputs.py`) gains `"Stepper"` as the third option, per the ticket's own text. Its `render_action_editor`/`on_save` branch mirrors the Macro branch closely — a dropdown of existing library entries by display name plus "+ New Stepper" to create-and-assign inline — but carries a second field Macro's doesn't: a Forward/Backward `Gtk.Direction` dropdown, since `Action::Step` needs a `direction` alongside its `stepper_id`. Unlike Profile Switch's Trigger-mode lock (exactly one legal value), Stepper allows two of the three Trigger modes (Fire-once/Hold-to-repeat, not Toggle) — `Gtk.DropDown` has no per-item sensitivity, so rather than build custom item-disabling, this relies on the Daemon's own existing Toggle-rejection (`daemon_stub.py`'s `set_binding`, landed by ticket 54) and surfaces it through the ordinary `DaemonError`→`show_error` path Save already has, confirmed by a new test that actually round-trips a Toggle+Stepper save and checks the rejection renders. `action_summary` now takes an optional `steppers` dict and resolves the display name (falling back to the raw `stepper_id` when omitted/missing), closing the gap ticket 54's Answer left open — mirroring ticket 52's identical closing of ticket 51's Macro-name gap. `device_overview.py`'s one call site threads `config.get("steppers", {})` through.

### Superseded tests, rewritten rather than deleted

Ticket 54's own regression tests for the "unknown Action kind" crash-guard (`build_binding_editor` opening a Binding type not in `ACTION_TYPES`) were written *against* `Action::Step` specifically, since Step was the only real example of the class at the time. Now that Step has a real editor, those two tests would silently start asserting the *wrong* thing (a Step Binding no longer hits the guard at all) — rewritten against a synthetic `"future_action_kind"` type instead, keeping the crash-guard mechanism itself covered against whatever the next net-new Action variant turns out to be, independent of Stepper. `test_device_overview.py`/`test_library_view.py`'s ticket-55-stub-existence tests were replaced with tests asserting the real panel renders through both entry points (`build_main_view` end-to-end and `build_library_view` directly).

### Deliberately out of scope

- Live-hardware verification — [ticket 56](./56-task-verify-stepper-library-on-hardware.md).
