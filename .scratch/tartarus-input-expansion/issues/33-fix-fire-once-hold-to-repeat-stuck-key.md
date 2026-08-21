Type: grilling
Status: resolved

## Question

Fix the stuck-key/stuck-button gap in Fire-once and Hold-to-repeat, surfaced live during [Finalize mouse-button/full-keyboard output and design the picker](./02-decide-mouse-button-output-and-picker.md): a Macro whose steps include a `KeyDown` with no matching `KeyUp` (e.g. a single-step Macro used to fake a "hold" — the exact technique tried with `KEY_LEFTCTRL` under Hold-to-repeat) leaves that key held down at the OS level forever.

Root cause, confirmed by reading `dispatch.rs::fire()`: Fire-once and Hold-to-repeat only ever fire-and-forget their compiled step sequence — neither reacts to the physical Input's `Up` event at all (only `Down`/`Repeat` are handled), and neither tracks which keys a firing left down the way `Toggle`'s `run_toggle_loop` tracks `held: HashSet<KeyCode>` and force-releases it via `injector::force_release_key` when the Toggle is cancelled. Reproduced live: recovering from the stuck `KEY_LEFTCTRL` required a full reboot — releasing the grid key, re-pressing it, and even changing the binding away all did nothing.

Toggle itself is unaffected and already the correct, proven pattern for "hold until pressed again" — confirmed via the user's own working mouse-look toggle on the MnM profile's thumbstick, `Action::Macro{steps: [KeyDown(BTN_RIGHT)]}` under `TriggerMode::Toggle`, which force-releases correctly on the second press.

Settle and build:

- Give Fire-once/Hold-to-repeat the same held-key tracking + force-release `Toggle` already has — most likely: track keys left down by an in-flight Fire-once/Hold-to-repeat firing per `Input` (mirroring `dispatch.rs`'s existing `in_flight: HashMap<Input, JoinHandle<()>>`), and force-release them on that Input's physical `Up` (currently ignored entirely for these two Trigger modes).
- Decide whether this is purely a Daemon-side fix, or whether the GUI's Macro-step editor should also warn/block saving an unbalanced Macro under Fire-once/Hold-to-repeat (belt-and-suspenders, per ticket 02's README footgun-list note) — or both.
- Confirm the fix doesn't regress Toggle's own existing force-release path, and doesn't break a normal *balanced* Fire-once/Hold-to-repeat Macro (which already self-releases correctly today).

This map carries execution: build the fix and verify live against the real Daemon/Tartarus Pro — reproduce the original stuck-key scenario first (confirm it still reproduces pre-fix), then confirm release now happens correctly on physical key-up, without needing a reboot.

## Answer

Built as a Daemon-side-only fix, no GUI warning — see rationale below.

**Mechanism**: `executor::spawn_fire_once` now returns a new `FiringHandle` (replacing the bare `JoinHandle<()>` `in_flight` used to hold) wrapping the task's `JoinHandle` plus an `Arc<Mutex<HashSet<KeyCode>>>` tracking exactly which keys the firing currently has down — the same `held`-mirrors-reality bookkeeping `ActiveToggle`'s loop already does (only applying a write to `held` once `injector.set_key_state` confirms it actually reached `uinput`, per ticket 25's suppression finding), just shared with the dispatch task via the `Mutex` instead of kept loop-private, since this fix needs to read it from outside the spawned task. `fire()` gains a new match arm, `(FireOnce | HoldToRepeat, Up)` — previously part of the catch-all no-op — that calls `FiringHandle::force_release_stuck`, which drains the shared `held` set and force-releases (bypassing suppression, same call `ActiveToggle::stop` already uses) whatever's still in it. `is_finished()` (the existing same-Input overlap guard) is unaffected — the entry is never removed from `in_flight` by this fix, only its `held` set is drained.

**Why Daemon-only, no GUI warning**: the fix doesn't just patch the bug, it changes what an unbalanced Macro *means* under Fire-once/Hold-to-repeat — instead of "stuck forever, needs a reboot," it now behaves as "held for as long as the physical Input stays down," which is an intuitive, arguably useful pattern (a lightweight alternative to Toggle, tracking physical hold-duration instead of a second press) rather than a footgun. Since the hazard the warning would have guarded against (permanent stuck key) no longer exists, a save-time GUI warning isn't needed — settled without a `/prototype` session, since this is a straightforward "does the hazard still exist" call, not a look/feel question.

**Verified**:
- 181 Rust tests green, including a new regression test (`hold_to_repeats_unbalanced_macro_is_force_released_on_physical_up`, `daemon/src/dispatch.rs`) reproducing the ticket's exact scenario (`Macro{steps: [KeyDown(KEY_LEFTCTRL)]}` under Hold-to-repeat) and asserting the KeyUp now happens on physical Up.
- `cargo clippy --all-targets` clean.
- Live-verified against the real Daemon/Tartarus Pro (release build installed via `systemctl --user restart`, temporary bindings added directly to `config.toml` then reverted): the exact repro (grid key bound to `Macro{KeyDown(KEY_LEFTCTRL)}` under Hold-to-repeat) — pressed and released once, `python3-evdev`'s `active_keys()` on the Acheron virtual device confirmed `[]` (no stuck key) immediately after, no reboot needed. A normal balanced Fire-once Macro (`KeyDown`+`KeyUp` of `KEY_LEFTSHIFT`) on a second grid key was also pressed and released live and behaved exactly as before (a quick Shift blip, not a sustained hold, nothing stuck or doubled) — the user's own words: "does not break anything and as expected holding it does not act like holding a real shift key." Skipped the *pre-fix* repro specifically (the user's choice, to avoid genuinely stranding a stuck Ctrl requiring a reboot) — ticket 33's own original write-up already established that reproduction once.
- The pre-fix reproduction of the deliberately-not-repeated original bug was itself already established live when this ticket was opened (see the ticket's own description); not re-run here.

No Rust source changes outside `daemon/src/executor.rs` (`FiringHandle`, `run_once`) and `daemon/src/dispatch.rs` (`fire()`'s new match arm, `in_flight`'s type, a new regression test). CONTEXT.md unchanged — no new domain terms, this is a bug fix to existing Trigger-mode behavior, not a new concept.
