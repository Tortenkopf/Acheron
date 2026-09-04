Type: grilling
Blocked by: 02
Status: resolved (Charon, 2026-09-04)

## Question

Sweep the **interactions between a held/mid-press dual-stage key and the rest of the
runtime**. Grilling + `/domain-modeling` against `dispatch.rs`, `trigger.rs`, the
`capture` supervisor, and `spec.md`'s existing "Toggle behavior across Layer/Profile
switches" / "Daemon output suppression" sections. Decision only.

### Settled inputs (do not re-open)

- The four modes and their event sequences (ticket 02).
- Deep stage inert in Digital mode (Q7); `force_release_stuck` is the precedent for
  releasing a held Binding on a switch.

### Settle at least

- **Layer switch mid-press** (Mode key pressed/released while a deep stage is held) —
  which stage(s) get force-released, and does the *other* Layer's stage for that Input
  then engage at the current Depth, or stay dormant until the next full release? (Compare
  how a plain held Binding behaves across a Layer switch today.)
- **Profile switch mid-press** — same question; plus a Profile-switch *fired by the deep
  stage itself* (allowed per Q11) while that stage is held.
- **Capture-mode flip mid-press** (Analog→Digital because the unlock dropped, or the user
  forced Digital) while a deep stage is held — deep stage must release cleanly; does the
  primary then take over at current Depth?
- **Output suppression / `StopAllToggles`** — a suppressed dual-stage key: does the
  staging state machine keep running (like Trigger-mode firing does under suppression)?
  Does `StopAllToggles` on GUI focus stop a Toggle that is on the deep stage?
- **Cascade-delete** — deleting the primary Binding removes the deep stage: confirmed at
  the `Edit` layer (ticket 03 made a dangling deep stage unrepresentable) — here, decide
  the GUI-side warning and whether a live delete while the key is held force-releases
  both.
- **Reconnect / stuck-key reset** — `relay_grid_blocking`'s reset-to-`Up`-on-reopen
  (`analog.rs:829`) must also reset any staging state so a replug can't leave a deep stage
  half-held.
- **Chord suppression window vs a dual-stage key that is *not* a Chord member but sits
  near one** — confirm no interaction (a Chord member can't have a deep stage per Q11, so
  this should be a non-issue — verify).

### Output

An `## Answer` resolving each interaction, ready for ticket 06 to fold into `spec.md`'s
runtime-behavior sections.

## Answer

Resolved by a grilling session against the real pipeline (`dispatch.rs`'s
`handle_layer_switch`/`handle_capture_mode_change`/`handle_connection_change`/
`run_effects`/`commit_input_edits`, `edit.rs`'s `SwitchProfile` plan, `trigger.rs`'s
`Slots::stop_all_toggles`, `chord.rs`'s `feed` gating, `capture/analog.rs`'s
`relay_grid_blocking` dropout handling, `gui/acheron_gui/app.py`'s focus-in
`stop_all_toggles()` call, and `binding_editor.py`'s delete path), 2026-09-04. Builds on
[ticket 01](./01-staged-event-pipeline.md) (`stage::Engine`, `stop_all()`) and
[ticket 02](./02-dual-stage-state-machine.md) (per-mode event sequences). Decisions only —
no build.

### Corrections to tickets 01/02's grounding, established by inspection

Two of the "grounding facts" earlier tickets leaned on turned out not to hold as literal
precedent once checked against the real code:

- **Layer switch force-releases *nothing* today.** `handle_layer_switch` only resets Axis
  outputs and stops Analog-repeat — it never touches `self.individual`. The Binding for a
  Repeat/Up event is **re-resolved every time** from `profile.layer(self.active_layer)`, so
  a live Slot from the old Layer's Binding is silently re-interpreted against whatever sits
  at that Input on the *new* Layer (or dangles, unbound, if nothing does). This is a live
  re-bind, not release-then-dormancy. The `force_release_stuck`-as-Layer-switch-precedent
  claim in this ticket's own "Settled inputs" and in the map's charting notes is **not
  accurate** — real force-release-on-switch precedent exists only for `SwitchProfile`
  (`Effect::StopAllToggles`/`StopAllAnalogRepeats`) and the Chord retroactive-miss path
  (`ForceReleaseIndividual`).
- **There is no generic disconnect hook.** `handle_connection_change` only flips a bool and
  emits a signal. Today's only dropout robustness is `relay_grid_blocking`'s own trick
  (`analog.rs:829`): it synthesizes a real `Up` `PhysicalEvent` for every key its local
  `holds` array shows mid-press, before `key_states` resets fresh on reopen. This covers
  only the **primary** band — a dispatch-side deep-stage engine gets no such synthetic
  event and needs its own explicit hook (§6 below). Ticket 01's "wired to... the disconnect
  path" was aspirational; no such site exists yet for anything, including today's
  Analog-repeat (which has no dropout handling at all and would otherwise sit stale on
  frozen Depth across a replug).
- **Chord suppression window vs. a non-member dual-stage key: confirmed non-issue by
  inspection, not by decision.** `chord::feed` only diverts an event when
  `chords_with_member(chords, event.input).next().is_some()`. Since a dual-stage key can
  never be a Chord member (Q11), its events never enter the Chord window machinery
  regardless of physical proximity to a real Chord. No mechanism needed, nothing to design.

### 1. Layer switch mid-press

**Force-release both stages, deliberately stricter than the existing plain-key precedent.**
Extend `stage::Engine::stop_all()`'s call sites to include `handle_layer_switch` (as ticket
01 already planned, even though the literal "precedent" it cited doesn't hold). The other
Layer's stage for that Input — if it has one — starts fresh from Up on the next physical
depth crossing; it never "picks up" at the current Depth. Chosen over mirroring the plain
key's live-rebind-with-dangling-Slot behavior: that behavior is an accepted historical quirk
of the single-stage system, not a model worth propagating into a new feature, and force-release
matches what a user watching the Depth bar would actually expect.

### 2. Profile switch mid-press

Same mechanism: `stage::Engine::stop_all()` joins `SwitchProfile`'s effect list (a new
`Effect::StopAllStages`, alongside `Effect::StopAllToggles`/`Effect::StopAllAnalogRepeats`).
For a Profile switch **fired by the deep stage's own Action** (Q11): mirrors the existing
Chord-member-fires-`ProfileSwitch` precedent exactly. The deep stage's firing is committed
through `commit_input_edits` the same way Chord's `FireIndividual` already routes a member's
`ProfileSwitch` — the returned `Edit::SwitchProfile` is applied *after* this tick's firing
already ran against the pre-switch `Config`, so `stop_all()` (part of applying that edit)
only tears down state *after* the triggering firing completed. No special-casing beyond
routing deep-stage-originated `ProfileSwitch` edits through the same `commit_input_edits`
path.

### 3. Capture-mode flip mid-press (Analog → Digital)

Extend the same `stop_all()` call into `handle_capture_mode_change`'s existing
Digital-transition branch (this one *does* have real precedent — it's the same branch that
already calls `analog_repeat.stop_all()` today). After the deep stage is force-released, the
primary is untouched by the flip and simply inherits whatever Digital-sourced Down/Repeat/Up
the key produces next — exactly the existing behavior a plain key already has on this
transition (per the code's own doc comment on that branch). No new mechanism for the
primary's side of this.

### 4. Output suppression / `StopAllToggles`

- **Staging state machine under Chord-style suppression: moot.** A dual-stage key can never
  be a Chord member (Q11), so it is never subject to Chord-style output suppression in the
  first place — nothing to design.
- **GUI-focus `StopAllToggles` (`app.py`'s `stop_all_toggles()` on every focus-in): extend
  it to the deep stage.** `Command::StopAllToggles`'s handler drains the deep
  `Slots<StageKey>`'s toggles too, alongside `self.individual.stop_all_toggles()`. This is
  deliberately more aggressive than the Chord-toggle-survives-a-Profile-switch precedent —
  it's a manual escape hatch for "the user's attention just moved to the GUI," not an
  automatic teardown, and should reach every live Toggle regardless of which stage it's on.

### 5. Cascade-delete

- **Live force-release, immediately.** `edit::apply` pushes a new per-key
  `Effect::StopStage(input)` whenever an edit removes a primary Binding that had a live
  `deep_base`/`deep_held` entry (both a direct `ClearBinding`/`SetBinding`-overwrite on the
  primary, and the pre-existing cascade ticket 03 already makes structurally required).
  `run_effects`' handler calls the same per-key teardown §1–3 use. Leaving it to "release
  naturally on the next Up" was rejected — there's no guarantee a next Up ever arrives
  (the physical key may not move again before the user notices stray output).
- **No GUI confirmation dialog.** Verified against `binding_editor.py`: no
  confirm/`MessageDialog` machinery exists anywhere around its `clear_binding` call today —
  no Binding delete in this GUI is confirmed, primary or otherwise. A special-case dialog
  just for cascade-deleting a deep stage would be inconsistent with the rest of the editor,
  so this stays a silent delete like every other one.

### 6. Reconnect / stuck-key reset

**New, explicit — there's nothing to piggyback on.** Add a disconnect hook in
`handle_connection_change`'s `connected == false` branch (or directly in the `rx_connection`
arm) that force-releases every live deep slot and resets the engine's per-key deep-band
`KeyState` and Quick-Skip runtime state, independent of whatever capture does for the
primary band via its own synthetic-Up trick. This is new machinery, not a reuse of an
existing "disconnect path" (§ Corrections above — no such generic path exists today).

**Out of scope, not fixed here:** Analog-repeat's own pre-existing lack of any dropout
handling (it would sit stale on frozen Depth across a replug today, with or without
dual-stage keys). Fixing that is a separate effort; this ticket only guarantees the
*new* deep-stage engine doesn't inherit the same gap.

### 7. Chord suppression window vs. non-member proximity

Confirmed non-issue by inspection (see Corrections above) — no ticket, no design.

### New mechanisms named for ticket 06

- `Effect::StopAllStages` — global per-connection/per-Layer/per-Profile teardown, wired into
  `handle_layer_switch`, `handle_capture_mode_change`'s Digital branch, `SwitchProfile`'s
  effect list, and the new disconnect hook. Calls `stage::Engine::stop_all()`.
- `Effect::StopStage(Input)` — single-key teardown for cascade-delete, pushed by
  `edit::apply` whenever a primary-Binding edit removes a Binding with a live deep stage.
- `Command::StopAllToggles`'s handler extended to also drain the deep `Slots<StageKey>`.
- A new disconnect hook in `handle_connection_change`'s `connected == false` branch,
  resetting `stage::Engine`'s per-key `KeyState` + Quick-Skip runtime state.
