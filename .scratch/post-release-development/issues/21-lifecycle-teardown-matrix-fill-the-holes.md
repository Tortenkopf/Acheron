<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 21 — Lifecycle-teardown matrix: fill the six holes (ticket 20 cases A1–A6)

**What to build:** Six additions to `DispatchState::tear_down`
(`dispatch.rs`), one new `chord::ChordMachine::reset()`, and one dual-stage
`spec.md` Out-of-Scope edit. Every `//`-marked skip that ticket 20's grilling
decided to close, in one behaviour-changing diff, with the
`dispatch::tests::tear_down_*` matrix tests updated to match.

Ticket 19 made the matrix explicit without deciding it; ticket 20 grilled each
skip. This ticket is the six that graduated to a fix.

## The changes, per `tear_down` arm

| skip today (ticket 19 `//` line) | case | becomes |
|---|---|---|
| `axis` NOT torn down on disconnect | **A1** | `self.reset_axis_outputs().await` in `Disconnect` |
| `analog_repeat` NOT torn down on disconnect | **A2** | `self.analog_repeat.stop_all().await` in `Disconnect` |
| `axis` NOT reset on the Digital flip | **A3** | `self.reset_axis_outputs().await` in `CaptureModeToDigital` |
| `chord_machine` NOT reset by any lifecycle event | **A4** | `self.chord_machine.reset()` in **all four** arms |
| `chord_slots` firings NOT drained | **A5** | `self.chord_slots.drain_firings(&self.injector).await` in **all four** arms |
| `chord_slots` Toggles survive a Profile switch | **A6** | `self.chord_slots.stop_all_toggles().await` in `ProfileSwitch` only |

After this ticket, the matrix is:

| situation | axis | analog_repeat | stage | indiv. firings | indiv. toggles | chord_machine | chord firings | chord toggles |
|---|---|---|---|---|---|---|---|---|
| Layer switch | reset | stop_all | stop_all | drain | — survive | **reset** | **drain** | — survive |
| Profile switch | reset | stop_all | stop_all | drain | stop_all | **reset** | **drain** | **stop_all** |
| Disconnect | **reset** | **stop_all** | stop_all | drain | — survive | **reset** | **drain** | — survive |
| → Digital | **reset** | stop_all | stop_all | drain | — survive | **reset** | **drain** | — survive |

Bold = new here. The only surviving asymmetry is deliberate and spec-backed:
individual **and** Chord Toggles survive a Layer switch (keybinder `spec.md`
"Toggle behavior across Layer/Profile switches" — "Layer change never touches
an active Toggle"), both drain on a Profile switch ("Profile switch releases
*every* active Toggle immediately"). Chord teardown now matches individual
teardown exactly (ticket 20 A5/A6 decision: **full match**).

### A1 — `axis` on disconnect

A grid key Axis-assigned and pushed to full travel when the device drops
leaves its last `ABS_*` value asserted on the gamepad `uinput` device with no
key left to release it — a stick or trigger frozen at full deflection until
something else happens to touch that axis. Layer switch and Profile switch
both already `reset_axis_outputs()`. On reconnect the engine re-resolves from
fresh `rx_depth` snapshots, so centring on the drop is free of side effects.

### A2 — `analog_repeat` on disconnect

The dual-stage `spec.md` listed this Out of Scope as "a separate effort."
**Ticket 20 decided this ticket is that effort** (one-liner, symmetric with
the Analog→Digital flip which already calls `analog_repeat.stop_all()`). A live
Analog-repeat task after a device drop either busy-spins — `run_analog_repeat_
loop`'s `_ = depth_rx.changed()` select arm accepts the `Err` a dropped watch
sender returns and re-enters immediately — or, if the sender lingers, taps /
holds-solid forever against a frozen snapshot. `stop_all()` cancels each task;
the loop's `cancel.cancelled()` arm force-releases whatever it held and exits
cleanly. **Confirmed** during the grilling: the `select!` in every arm
(`Idle` / `Tap` / `HoldSolid`) has a `cancel.cancelled()` branch, so
cancellation is clean regardless of the `depth_rx` sender's state.

- **`spec.md` edit** (`tartarus-dual-stage-keys/spec.md`, "Out of Scope"): the
  "Fixing Analog-repeat's pre-existing lack of dropout/disconnect handling"
  bullet is rewritten to record it as **resolved by `post-release-development`
  ticket 21** — `analog_repeat.stop_all()` now runs in
  `tear_down(TeardownReason::Disconnect)` alongside the deep-stage engine's
  own hook. The `.scratch/README.md` dual-stage-spec line's trailing
  "explicitly out of scope as a separate effort" clause updates to match.

### A3 — `axis` on the Digital flip

Digital mode has no Depth. An Axis-assigned key's `contributions[input]` holds
its last analog value; `axis::Engine::step_digital` (the Digital fallback)
only zeroes it on that key's next `Up`. Until then the axis is stuck at the
frozen analog deflection. `analog_repeat` and `stage` are both `stop_all`'d on
this flip; `axis` should reset for the same reason. Post-reset, `step_digital`
starts each contribution fresh from 0 — the correct Digital-mode baseline.
Cross-check resolved: `step_digital` reads `contributions.get(&input)
.unwrap_or(0)`, so a cleared map is exactly what it expects; no interaction
problem.

### A4 — `chord_machine.reset()` (new method)

```rust
// chord.rs
impl ChordMachine {
    /// Drops any open simultaneity window and every claimed Input — a
    /// lifecycle transition (Layer / Profile switch, disconnect, Digital
    /// flip) is a clean-slate event, same as every other engine's teardown.
    /// Pending members do NOT retroactively fire (unlike a window timeout):
    /// the press context they belonged to is gone.
    pub(crate) fn reset(&mut self) {
        self.window = None;
        self.claimed.clear();
    }
}
```

Without it, a <50 ms window open across a switch keeps ticking and
`chord::tick` later fires `FireIndividual` for its pending members resolved
against the *new* Layer's bindings, while their `claimed`-set membership
suppresses their ordinary Down/Up handling in the meantime. Low probability
(needs a chord member physically held mid-window exactly as the Layer
toggles), bounded (≤50 ms), but inconsistent with every other engine
resetting — and the Mode key's own edge already returns early from
`handle_event` before `chord::feed`, so the machine can be left mid-window by
a switch it never saw.

### A5 — `chord_slots.drain_firings`

A live Chord Hold-to-repeat firing (its bare `value=1` `HoldKeyDown`) survives
a Layer switch with no release edge — the member `Up` that would end it is on
the old Layer's suppression path. This is precisely the stuck-key case
`individual.drain_firings` exists for (`trigger.rs` doc: "a bare `HoldKeyDown`
hold now backs *every* single-key Hold-to-repeat … would otherwise be
stranded at the OS level whenever the bound key is released on a Layer/Profile
where it is unbound"). `Slots<ChordKey>::drain_firings` already exists and does
the right thing (force-release + remove, Toggles untouched).

### A6 — `chord_slots.stop_all_toggles` on a Profile switch

`edit.rs` documents "an active Chord Toggle survives a Profile switch today"
as current behaviour, never as a decision. Keybinder `spec.md`: "Profile
switch releases **every** active Toggle immediately, as part of the switch" —
unqualified. Individual Toggles drain (`individual.stop_all_toggles()` in the
`ProfileSwitch` arm); Chord Toggles should too. `Slots<ChordKey>::stop_all_
toggles` already exists (it is what the GUI-focus `StopAllToggles` command
calls via `chord_slots.stop_all_toggles()` at `dispatch.rs`).

## Behaviour-preservation protocol

This ticket **changes behaviour** — that is its point. The protocol is to
change *only* the six cells above and prove the rest of the matrix is
untouched.

- **Diff the four `tear_down` arms against `HEAD` — every non-A1–A6 line
  identical.** The `LayerSwitch`/`ProfileSwitch` axis + analog + stage + indiv
  order is unchanged; only the new calls are inserted.
- **Order within each arm:** put the new `chord_machine.reset()` /
  `chord_slots.*` calls after the existing individual-path calls, mirroring
  the individual → chord order the rest of dispatch uses. `reset_axis_outputs`
  in `Disconnect`/`CaptureModeToDigital` goes where the other arms put it
  (before `analog_repeat` in `LayerSwitch`, so match that).
- **`cargo test -p acheron-daemon` green** — the `dual_stage_*` tests, the
  layer/profile/disconnect/capture integration tests. The four
  `dispatch::tests::tear_down_*` tests **change** (see below); nothing else
  should.
- **`/code-review`** on Standards + Spec axes, as tickets 05–19 did. Spec
  axis checks each new call against the ticket-20 decision it implements.

## Tests

- **`dispatch::tests::tear_down_*` — update all four.** Each already seeds a
  Chord Toggle + a Chord firing on the `Seam` seam (ticket 19 added them as
  survivors). Flip the assertions:
  - `tear_down_layer_switch_*`: Chord firing now **drained**, Chord machine
    window now **reset**, Chord Toggle still survives, individual Toggle still
    survives.
  - `tear_down_profile_switch_*`: Chord firing **drained**, Chord Toggle
    **drained**, Chord machine **reset**.
  - `tear_down_disconnect_*`: axis output now **centred**, analog-repeat task
    now **stopped**, Chord firing **drained**, Chord machine **reset**;
    individual + Chord Toggles still survive.
  - `tear_down_capture_mode_to_digital_*`: axis output now **centred**, Chord
    firing **drained**, Chord machine **reset**.
  - Rename any test whose name encodes the old behaviour
    (`..._but_leaves_axis`, `..._touches_only_the_deep_stage_and_individual_
    firings`).
- **New — `chord.rs` pure test:** `reset()` on a machine with an open window
  and claimed members clears both; `next_deadline` is `None` after; a later
  `feed` for a formerly-claimed Input is `NotMine`.
- **New — one `dispatch.rs` pipeline test per genuinely new stuck-output
  symptom** (the integration net, matching ticket 18's pattern):
  - an Axis-assigned grid key at full deflection, device disconnect → the
    `ABS_*` stream shows it centred (A1).
  - a live Analog-repeat task, device disconnect → task stopped, held key
    released, no further output (A2).
  - a live Chord Hold-to-repeat, Layer switch → deep/chord key released (A5).
  - a live Chord Toggle, Profile switch → released (A6).
- **Kept unchanged:** `dual_stage_*` pipeline tests, `stage.rs` / `axis.rs` /
  `analog_repeat.rs` pure tests, the layer/profile/disconnect/capture wiring
  tests.

## Docs

- **`dispatch.rs`** — the `tear_down` doc comment's "Turning one of the
  `//`-marked skips into a real call is ticket 20's" sentence updates to
  past-tense / points at this ticket; each former `//` skip line that became a
  call is deleted, each surviving `//` line (the Toggle-survival rationales)
  stays.
- **`chord.rs`** — `ChordMachine` doc gains one line naming `reset` as the
  lifecycle-teardown entry point (it currently says "Reset fresh on every
  dispatch task start" — extend to "and on every lifecycle transition, via
  `reset()`").
- **`tartarus-dual-stage-keys/spec.md`** — the "Out of Scope" Analog-repeat-
  dropout bullet rewritten (A2, above).
- **`CONTRIBUTING.md`** — the "Changing lifecycle teardown" bullet ticket 19
  added ("behaviour changes to a `—` cell are ticket 20's, not a drive-by")
  updates: the cells are decided; a *new* engine still adds a line to each
  arm.
- **No ADR** — ticket 20's grilling is the decision record; ADR-0010 already
  covers the mechanism.
- **No `CONTEXT.md`** — no new domain concept (`reset` is dispatch-internal
  plumbing, ticket 10/14/15/17/19 precedent).
- **`.scratch/README.md`** — extend the `post-release-development` line with
  ticket 21.

## Cases from ticket 20 NOT in this ticket

- **B8** (`SetDeepActuation` under a live slot) — decided **keep**; rationale
  comment already added to the `edit.rs` variant doc by ticket 20.
- **B9 / B10 / B11** (config edits orphaning individual / Chord runtime
  state) — **ticket 22**.
- **B7 / B12** (`SetStagingMode` teardown + `stop_stage` reset-and-keep) —
  **ticket 23**.

**Blocked by:** None. (Ticket 19 landed the matrix; this fills it. Independent
of tickets 22 / 23.)

**Status:** done — landed 2026-09-10 on `dev`. All six additions (A1–A6) plus
`ChordMachine::reset()` shipped; `dispatch::tests::tear_down_*` flipped, four
new integration tests added, docs updated. daemon 558 green, clippy clean.

## Comments

**2026-09-10** — Implemented. `chord::ChordMachine::reset()` (new `impl`
block — the `feed`/`tick`/`next_deadline` free-function idiom doesn't fit a
one-line field clear called as `self.chord_machine.reset()` from dispatch) +
a pure `chord::tests` case. All four `tear_down` arms updated: the new
`chord_machine.reset()` / `chord_slots.*` calls sit after the individual-path
calls (mirroring dispatch's individual → chord order), `reset_axis_outputs`
in `Disconnect` / `CaptureModeToDigital` goes first, matching `LayerSwitch`.
The four `tear_down_*` matrix tests: `matrix_config` gained a 3-member Chord
(KEY_H) with one member seeded down so every arm can assert the
`chord_machine` window resets; assertions flipped per the ticket table; two
tests renamed off their old `..._but_leaves_axis` /
`..._touches_only_the_deep_stage_and_individual_firings` names. Four new
pipeline tests (`a_disconnect_centers_any_live_axis_output`,
`a_disconnect_stops_a_live_analog_repeat_task_and_releases_its_held_key`,
`a_layer_switch_drains_a_live_chord_hold_to_repeat_firing`,
`a_profile_switch_drains_a_live_chord_toggle`). Docs: `dispatch.rs` /
`chord.rs` doc comments, dual-stage `spec.md` "Out of Scope" bullet rewritten
as resolved, `CONTRIBUTING.md` "Changing lifecycle teardown" bullet,
`.scratch/README.md`. No ADR (ticket 20's decision table is the record), no
`CONTEXT.md` (dispatch-internal plumbing).

**2026-09-10** — Filed from ticket 20 (the lifecycle-teardown behaviour
review). Ticket 20 grilled all twelve matrix / config-edit gaps; six of them
(A1–A6) are lifecycle-teardown-matrix additions that share a diff and a test
file, so they cluster here rather than as six separate tickets. Decisions
carried from ticket 20's `AskUserQuestion` round: A2 = take the Analog-repeat
disconnect fix here (not a future separate effort); A5 + A6 = full match
between Chord and individual teardown.
