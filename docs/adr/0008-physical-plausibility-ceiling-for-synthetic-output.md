# Synthetic output holds to a physical-plausibility ceiling; held keys present as genuine kernel autorepeat

Some games and anti-cheat systems flag input no hand could produce — inhuman event
rates, and perfectly regular intervals ([`docs/anti-cheat-input-heuristics.md`](../anti-cheat-input-heuristics.md):
osu! circleguard's ~5 ms tap SD, CS2's "0 ms overlap/neutral", published
keystroke-per-second ceilings). Acheron's `uinput` device is *already* fully visible —
it enumerates under `/sys/devices/virtual/input/` as "Acheron Virtual Tartarus Pro"
with a blank `phys` — and we accept that. This decision is about the **plausibility of
the output rate**, not disguise of its origin: nothing here hides the virtual device,
and we deliberately do **not** inject jitter or anti-regularity noise (the kernel's own
autorepeat is timer-regular, and so is ours).

**The ceiling.** No holding or repeating Action emits key/button events faster than the
Linux input stack would for a physically held key or button: for keys, the machine's
configured kernel autorepeat delay/period, read live via `analog::read_kernel_auto_repeat`
(fallback 250 ms / 33 ms); for a held mouse or gamepad button, exactly one Down/Up with
no repeat, since the kernel never autorepeats `BTN_*`. Steady-state *rate* is the
invariant; a key's ~250 ms initial-repeat delay is judged per surface (Analog-repeat,
which simulates hand-interlaced tapping rather than a hold, deliberately has none). This
is enforced by existing mechanism, not a central gate: `MIN_TOGGLE_LAP` /
`combine_toggle_lap_target` floor the Toggle loop lap, `RepeatSchedule` seeds its
envelope from the live kernel rate, `run_toggle_held` / `spawn_held` give a held button
a single sustained press, `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD` floors the digital
pulse, `FIRE_ONCE_KEY_DWELL` floors the one-shot keyboard press below.

A canned one-shot keyboard press — a Fire-once Keypress, and its single-key Macro /
Stepper-step / Chord equivalents — carries a fixed 40 ms dwell between Down and Up
(`FIRE_ONCE_KEY_DWELL`), rather than the near-zero-dwell pair the research names the
clearest synthetic tell. This is a rate-plausibility consistency follow-up
(`.scratch/humane-output-rate/` ticket 12), not a detection defense: a Macro fired once
still keeps its author's cadence, and Analog-repeat's tap pulses are unchanged. A
dual-stage key's **primary stage** is excluded — its press is machine-sequenced input,
its timing already subject to depth interpretation, so it is dwelled nowhere
`stage::Engine` drives a firing (`.scratch/post-release-development/` ticket 17: the
primary path now builds `PerformDeps::new_machine_sequenced`, matching every other
`stage::Engine`-driven edge — deep-stage fire, `RepressPrimary`, retroactive re-press,
deep repeat, all already dwell-free before ticket 17).

This carve-out is narrow. The dwell splice only ever engaged for a Fire-once binding
whose compiled action is a *single held key* (`executor::single_held_key` — plain /
modifier Keypress, single-key Macro, single-key Stepper `Key` step), so the exclusion
touches only a primary binding that is **both** Fire-once **and** a single-key press,
and only on its initial `Down`. A Toggle, Hold-to-repeat, multi-step-Macro, mouse- or
gamepad-button primary never carried the dwell and is unaffected. Within that case the
change is visible only when the key is tapped **without** crossing into the deep band —
if it crosses, `ReleasePrimary` tears the key down regardless, which is the ~0 ms-dwell
residual ticket 12 could not clean up and the reason this exclusion exists. The loss —
one such primary, tapped shallow, forgoes the ~40 ms dwell on that one press — is
negligible: dual-stage keys are niche and the primary is the light half of an
escalating pair.

**The audit.** `.scratch/humane-output-rate/` ticket 01 traced all twelve
holding/repeating surfaces to their constants and ratified a verdict table. Eight
passed. Two repeat pace loops (analog-synth Hold-to-repeat, Analog-repeat) advanced
their "next fire" bookkeeping one step per emitted event with no missed-deadline clamp,
so a scheduler or injector stall longer than one period produced a bunched catch-up
burst on resume — fixed inline (ticket 06) by re-basing `fired` from real elapsed time,
matching the kernel's `input_repeat_key`, which re-arms from *now* and never bursts.

**Kernel-shaped repeat (`value=2`).** Before this change every Hold-to-repeat path
emitted `[KeyDown, KeyUp]` pairs with ~0 ms dwell — rate-compliant, but the clearest
synthetic tell in the research, with an implied hold time no physical key produces. The
rebuild (ticket 07, specced in `.scratch/humane-output-rate/spec-kernel-shaped-repeat.md`,
carried out in `.scratch/kernel-shaped-repeat-impl/`) makes a held single key instead
emit one `value=1` then `value=2` autorepeat events at the live `REP_DELAY`→`REP_PERIOD`
envelope, then `value=0`. We inject `value=2` ourselves rather than advertise `EV_REP`
on the virtual device — `evdev` 0.13.2 cannot enable kernel softrepeat on a
`VirtualDevice`, and `injector::translate` already emits `value=2` for passthrough. This
converts Digital and analog-synth Hold-to-repeat, single-key Chord, single-key deep-stage
Hold-to-repeat, Analog-repeat hold-solid, and Toggle / Hold-to-repeat of a single key or
single-key Macro. It does **not** touch Stepper Hold-to-repeat (each repeat targets a
different item), button Toggles (`BTN_*` does not autorepeat), or multi-step Macro loops.
Acheron's held and repeated keyboard output is now timing-indistinguishable from a
physical hold, and its one dynamic mode — the Analog-repeat depth ramp — is continuously
hand-driven.

One surface could not be reshaped to fit and was **deleted** instead: the Dual-stage
**Additive** staging mode held both a primary and a deep key in autorepeat at once — two
concurrent `value=2` streams from one press, a shape the kernel (single `repeat_key`)
never produces. It is removed (ADR-0009); after the cut no staging mode emits two
concurrent autorepeat streams.

**The Macro exception.** A Macro is the sole deliberate exception: its author may
sequence Keypresses at any cadence. A Macro fired once, and the keystroke cadence
*within* one run of a multi-step Macro, are unrestricted — a within-run floor would cap
legitimate fast combos, and the once-fired burst is bounded by step count and injector
backpressure (not an unbounded loop). Only trigger-driven Macro *repetition* is floored:
the Toggle→Macro loop lap and the Hold-to-repeat→Macro re-fire cadence are both already
held to `max(kernel period, MIN_TOGGLE_LAP)` by `run_toggle_loop`'s `target_lap` and the
`FiringUnfinished` overlap guard, so no new floor was added; ticket 08 locks the
invariant with regression tests. Analog-repeat cannot wrap a Macro at all (ticket 09 —
rejected at `SetBinding` and at config load).

**Not guarded.** A pathologically fast kernel autorepeat config (`kbdrate` at 100/s+) is
not defended against: following the live kernel rate *is* the ceiling as written, and a
misconfigured OS would flood identically from the user's own physical keys (ticket 01,
Q4).

This decision is the first place the "`uinput` origin is always detectable, and that is
accepted" premise is recorded. ADR-0002 (direct evdev/uinput instead of OpenRazer)
concerns the capture/injection transport only and says nothing about detectability; it
is neither the basis for nor refined by this decision.
