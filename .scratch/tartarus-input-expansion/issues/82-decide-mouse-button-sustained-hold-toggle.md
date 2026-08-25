Type: grilling
Status: resolved

## Question

Surfaced directly by the user after [ticket 81](./81-task-verify-mouse-button-sustained-hold-on-hardware.md)
closed the Hold-to-repeat strand: Toggle mode has the exact same gap for
mouse-button `Action::Keypress` output that Hold-to-repeat had before
[ticket 79](./79-decide-mouse-button-sustained-hold-drag.md).

Confirmed in code (`daemon/src/dispatch.rs`, both `ActiveToggle::spawn` call
sites — the plain-Input Toggle arm and the Chord Toggle arm): Toggle on
`Down` compiles the Action via `compile_action`/`executor::compile` (a
balanced `KeyDown`+`KeyUp` pair for a bare Keypress) and loops that pair
repeatedly via `run_toggle_loop` at `toggle_lap_target` cadence (ticket 68 —
tuned to match the kernel's own autorepeat rate) until the second `Down`
cancels it. For a mouse-button Keypress this means "toggled on" mash-clicks
rather than holding the button down — the same drag-breaking shape ticket 79
fixed for Hold-to-repeat, just via a persistent loop instead of a
Repeat-driven one.

Open questions for this session:

- Should Toggle mirror ticket 79's unconditional call exactly — a mouse-button
  Toggle becomes a real sustained hold (`KeyDown` once on the first `Down`,
  `KeyUp` once on the second `Down`/stop, no loop) — or does Toggle's own
  shape change the tradeoff? Ticket 79 dismissed mash-click's "legitimate use
  case" argument for Hold-to-repeat because holding a physical key to sustain
  it is a weak ask; **Toggle-driven mash-click is a genuine auto-clicker with
  no physical-hold cost**, arguably a stronger, more plausible want than
  Hold-to-repeat's ever was. Does that change the answer, or does drag still
  strictly dominate?
- If sustained-hold wins unconditionally: mechanism. `ActiveToggle` is
  presently loop-only (`run_toggle_loop`); a held Toggle needs a variant that
  fires one `KeyDown`, waits on the cancellation token, then releases with one
  `KeyUp` — likely a new `ActiveToggle` constructor alongside `spawn`, chosen
  by `is_mouse_button` the same way ticket 80 chose `fire()`'s carve-out.
  Confirm this is the right shape given `ActiveToggle::stop`'s existing
  contract (force-release whatever's still held) needs to keep working
  unchanged for both loop and held variants.
- Blast radius: both `ActiveToggle::spawn` call sites (plain Input Toggle in
  `handle_event`/`handle_command`, and Chord Toggle in `handle_chord_event`)
  need the same carve-out, mirroring ticket 80's Chord coverage for
  Hold-to-repeat. Keyboard-key Toggle (the ticket-68-tuned walking-game case)
  and `Action::ControllerButton` Toggle must stay on the existing loop
  unchanged — only a mouse-button Keypress's Toggle changes.
- Any interaction with a Macro under Toggle worth calling out? (Ticket 79's
  own comment on [ticket 78](./78-decide-controller-button-trigger-mode-applicability.md)
  already established a Macro under Toggle is untouched — arbitrary
  already-explicit steps, no Trigger-mode repeat logic in the path.)

Record the settled design as this ticket's Answer; spawn build/verify tasks
per this map's standing precedent if it changes shipped behavior.

## Answer

Settled directly with the user: **Toggle mirrors ticket 79's `HoldToRepeat` fix exactly** —
unconditional, no mode split. A mouse-button Toggle becomes a real sustained hold (`KeyDown`
once on the toggle-on `Down`, `KeyUp` once on the toggle-off `Down`/stop, no loop), same as
Hold-to-repeat. The user weighed the auto-clicker angle directly (Toggle-driven mash-click
costs no physical hold, unlike Hold-to-repeat's) and still chose drag-support over preserving
it as a built-in behavior — no separate auto-clicker mechanism spawned.

- **Mechanism**: `ActiveToggle` gains a second constructor, `spawn_held(injector, key)`,
  alongside the existing `spawn(injector, steps, target_lap)` — fires one `KeyDown`, awaits
  the cancellation token, then force-releases via the existing `force_release` helper. Same
  `{cancel, handle}` shape as the loop variant, so `ActiveToggle::stop()` (and every caller of
  it — `StopAllToggles`, profile switch, the Mode key, a Toggle Chord's "full member set
  again" stop, a plain Input's own second `Down`) works unchanged for both variants with zero
  call-site changes beyond the two spawn sites themselves.
- **Where the carve-out lives**: both `ActiveToggle::spawn` call sites in `dispatch.rs` — the
  plain-Input Toggle arm (`fire()`) and the Chord Toggle arm (`fire_chord()`) — gain a new
  match arm ahead of the general Toggle arm, guarded on
  `matches!(binding.action, Action::Keypress { key, .. } if is_mouse_button(key))`,
  structurally identical to ticket 80's `HoldToRepeat` mouse-button carve-out. Keyboard-key
  Toggle (ticket 68's walking-game case) and `Action::ControllerButton` Toggle keep the
  existing loop unchanged — only a mouse-button Keypress's Toggle changes.
- **Macro under Toggle**: untouched, per ticket 79's own comment on
  [ticket 78](./78-decide-controller-button-trigger-mode-applicability.md) — a Macro's steps
  are already explicit and unrelated to this loop-vs-held distinction.

Spawned [Build the mouse-button sustained-hold Toggle fix](./83-task-build-mouse-button-sustained-hold-toggle.md)
and [Verify the mouse-button sustained-hold Toggle fix on hardware](./84-task-verify-mouse-button-sustained-hold-toggle-on-hardware.md).
