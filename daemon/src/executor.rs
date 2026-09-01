// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The shared Action executor (ticket 17): compiles a Binding's `Action`
//! into one flat `Vec<MacroStep>` — a Keypress becomes a canned
//! modifier-down/key-down/key-up/modifier-up sequence, a Macro's steps map
//! straight across (spec.md's "Runtime Binding") — and runs that sequence
//! through the one shared step-walker, regardless of which `Action` variant
//! produced it. This is the only place Trigger-mode firing/stopping logic
//! lives.
//!
//! Every firing spawns its own `tokio` task (issue 07: a Macro's `Delay`
//! steps must never block the dispatch task), and every step's key
//! up/down goes through the one shared `Injector` channel so concurrently
//! running Toggles never interleave raw `uinput` writes.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use evdev::KeyCode;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::capture::analog;
use crate::config::{Action, MacroDef, MacroId, MacroStepDto, Modifiers};
use crate::injector::{Injector, InjectorClosed};

/// The runtime step sequence a compiled `Action` runs as, per spec.md's
/// "Runtime `Binding`". `MacroStepDto::Delay`'s milliseconds become a
/// `Duration` up front so the executor never re-derives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroStep {
    KeyDown(KeyCode),
    KeyUp(KeyCode),
    Delay(Duration),
}

/// The modifier key codes a chord presses, in a fixed ctrl/shift/alt/super
/// order (released in reverse) — moved here from the old `injector::
/// fire_keypress` path now that a compiled Keypress runs through the same
/// executor as a Macro (ticket 17).
fn modifier_codes(modifiers: Modifiers) -> Vec<KeyCode> {
    let mut codes = Vec::with_capacity(4);
    if modifiers.ctrl {
        codes.push(KeyCode::KEY_LEFTCTRL);
    }
    if modifiers.shift {
        codes.push(KeyCode::KEY_LEFTSHIFT);
    }
    if modifiers.alt {
        codes.push(KeyCode::KEY_LEFTALT);
    }
    if modifiers.super_key {
        codes.push(KeyCode::KEY_LEFTMETA);
    }
    codes
}

/// `compile()`'s dwell floor for `Action::ControllerButton` output (ticket
/// 74/75/76): a bare zero-artificial-dwell `KeyDown`/`KeyUp` pair can land
/// both edges inside the same input-poll frame on the receiving game, so the
/// whole press is silently swallowed. Originally tuned for Fire-once, but
/// ticket 78 locked Fire-once out for `Action::ControllerButton` entirely —
/// this now only fires via `dispatch::compile_action`'s Digital-Capture-mode
/// Analog-repeat fallback (a Digital-sourced Analog-repeat Binding resolves
/// to `trigger::TriggerDecision::SpawnFireOnce`, which calls straight through
/// to `compile()`, per ticket 20's Answer), so the same single-poll-swallow risk still
/// applies there. Deliberately *not* shared with
/// `dispatch::ANALOG_REPEAT_PULSE_HOLD`/`ANALOG_REPEAT_CONTROLLER_PULSE_HOLD`
/// — the dwells are tuned for unrelated jobs (15ms was tuned against
/// Analog-repeat's own rate-curve/cadence on real hardware; 35ms targets true
/// phase-independent single-poll coverage against a 60fps frame interval, per
/// ticket 74's research §6) — sharing the constant would silently couple
/// unrelated tuning knobs. Not final-tuned against a real game yet.
pub(crate) const CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD: Duration = Duration::from_millis(35);

/// `pub(crate)` (rather than private) so `dispatch::resolve_step` can reuse
/// the same canned mods-down/key/mods-up sequence for a Stepper item's
/// modifier combination (ticket 63) — the two callers share the exact
/// balanced-firing semantics `Action::Keypress` established.
pub(crate) fn keypress_steps(modifiers: Modifiers, key: KeyCode) -> Vec<MacroStep> {
    let mods = modifier_codes(modifiers);
    let mut steps = Vec::with_capacity(mods.len() * 2 + 2);
    steps.extend(mods.iter().map(|&m| MacroStep::KeyDown(m)));
    steps.push(MacroStep::KeyDown(key));
    steps.push(MacroStep::KeyUp(key));
    steps.extend(mods.iter().rev().map(|&m| MacroStep::KeyUp(m)));
    steps
}

/// The down/dwell/up triple a single atomic controller-button press
/// compiles to (ticket 75/76's `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`
/// dwell between the edges, so a same-poll-frame game doesn't swallow the
/// press). Shared by `compile`'s `Action::ControllerButton` arm and
/// `dispatch::resolve_step`'s `StepperItem::ControllerButton` arm (ticket
/// 92) — a Stepper item is always an atomic one-shot press, so it hits the
/// same polled-input risk and reuses the same constant rather than
/// hand-inlining the triple in `dispatch`.
pub(crate) fn controller_button_steps(button: KeyCode) -> Vec<MacroStep> {
    vec![
        MacroStep::KeyDown(button),
        MacroStep::Delay(CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD),
        MacroStep::KeyUp(button),
    ]
}

/// Compiles a Binding's `Action` into the flat step sequence the shared
/// executor runs (spec.md: "both Action kinds compile ... into one steps:
/// Vec<MacroStep> ... run by one shared executor"). `macros` resolves a
/// Macro Action's `macro_id` into its `MacroDef` (ticket 51 — a Binding no
/// longer carries step content directly, only a reference into the shared
/// library).
pub fn compile(action: &Action, macros: &HashMap<MacroId, MacroDef>) -> Vec<MacroStep> {
    match action {
        Action::Keypress { modifiers, key } => keypress_steps(*modifiers, *key),
        Action::Macro { macro_id } => {
            // Every `macro_id` reaching here is structurally guaranteed to
            // resolve: `SetBinding` rejects an unknown one at write time, and
            // `config::parse` refuses to even start the Daemon on a
            // `config.toml` with a dangling reference (ticket 51).
            let def = macros.get(macro_id).expect(
                "SetBinding/config::parse validate every macro_id references an existing MacroDef",
            );
            def.steps
                .iter()
                .map(|step| match step {
                    MacroStepDto::KeyDown(key) => MacroStep::KeyDown(*key),
                    MacroStepDto::KeyUp(key) => MacroStep::KeyUp(*key),
                    MacroStepDto::Delay(ms) => MacroStep::Delay(Duration::from_millis(*ms)),
                })
                .collect()
        }
        Action::ProfileSwitch { .. } => {
            unreachable!(
                "Action::ProfileSwitch is intercepted in dispatch::handle_event before compile is ever called"
            )
        }
        Action::Step { .. } => {
            unreachable!(
                "Action::Step's steps depend on Daemon-owned runtime cursor state, resolved by dispatch::compile_action before this generic compile is ever reached for it"
            )
        }
        // Almost the same shape as a plain, unmodified Keypress (ticket 14's
        // Answer: "a controller-button press is the same shape as a
        // Keypress: compile a down/up pair, inject it") — only the target
        // uinput device differs, which the injector alone decides
        // (`input::is_gamepad_button`) — plus a genuine dwell between the
        // two (ticket 75/76): a bare zero-artificial-dwell pair can land
        // both edges inside the same input-poll frame on the receiving
        // game, silently swallowing the press. Fire-once is locked out for
        // this Action (ticket 78), and both Hold-to-repeat and Toggle are
        // carved out ahead of `compile_action` in `trigger::decide`
        // (ticket 75/76's bare-KeyDown hold, ticket 78's Toggle mirror of
        // it) — the only caller still reaching this arm is the
        // Digital-Capture-mode Analog-repeat fallback (ticket 20).
        Action::ControllerButton { button } => controller_button_steps(*button),
    }
}

/// Walks `steps` once, in order, sleeping between `Delay` steps — shared
/// shape with `execute_step`'s Toggle-loop version, but tracking `held` in a
/// cross-task-visible `Mutex` rather than a loop-private `&mut` since a
/// one-shot firing's `held` set must survive the spawned task to be readable
/// from the dispatch task later (ticket 33's stuck-key fix: force-released
/// on the bound Input's physical `Up`, not just on an explicit stop).
/// `held` only ever mirrors reality (a write suppression withheld), same
/// rationale as `execute_step`. Returns `Err` only when the injector task
/// itself has died (a genuine, fatal Daemon condition, not something this
/// firing should retry).
async fn run_once(
    injector: &Injector,
    steps: &[MacroStep],
    held: &Mutex<HashSet<KeyCode>>,
) -> Result<(), InjectorClosed> {
    for step in steps {
        match step {
            MacroStep::KeyDown(key) => {
                let applied = injector.set_key_state(*key, true).await?;
                if applied {
                    held.lock().expect("held mutex poisoned").insert(*key);
                }
            }
            MacroStep::KeyUp(key) => {
                let applied = injector.set_key_state(*key, false).await?;
                if applied {
                    held.lock().expect("held mutex poisoned").remove(key);
                }
            }
            MacroStep::Delay(duration) => tokio::time::sleep(*duration).await,
        }
    }
    Ok(())
}

/// A spawned Fire-once/Hold-to-repeat firing, as tracked in dispatch's
/// `HashMap<Input, FiringHandle>`. `held` mirrors `ActiveToggle`'s own
/// `held: HashSet<Key>` discipline, just shared with the dispatch task
/// instead of kept loop-private, since ticket 33's fix needs to read (and
/// force-release) it from the outside, on the bound Input's physical `Up`.
pub struct FiringHandle {
    handle: JoinHandle<()>,
    held: Arc<Mutex<HashSet<KeyCode>>>,
}

impl FiringHandle {
    /// Whether the firing's steps have finished walking — feeds
    /// `dispatch::slot_for`'s `FiringUnfinished` / `FiringFinished` split for
    /// `trigger::decide`'s overlap guard, unchanged from the old bare
    /// `JoinHandle<()>` check.
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Awaits the firing's own task to completion — a test-only convenience
    /// (dispatch never awaits a firing directly; it only ever polls
    /// `is_finished()` or force-releases via `force_release_stuck`).
    #[cfg(test)]
    async fn join(self) {
        let _ = self.handle.await;
    }

    /// Ticket 33's fix: force-releases (bypassing suppression, same as
    /// `ActiveToggle::stop`'s force-release) every key this firing still has
    /// down, then forgets them. A normal *balanced* Fire-once/Hold-to-repeat
    /// Macro has already self-released by the time a physical `Up` calls
    /// this (`held` is empty — a no-op); an *unbalanced* one (a bare
    /// `KeyDown` with no matching `KeyUp`, used to fake a sustained "hold")
    /// is exactly what this releases, instead of leaving it stuck at the OS
    /// level until reboot.
    pub async fn force_release_stuck(&self, injector: &Injector) {
        let stuck: Vec<KeyCode> = self
            .held
            .lock()
            .expect("held mutex poisoned")
            .drain()
            .collect();
        for key in stuck {
            let _ = injector.force_release_key(key).await;
        }
    }
}

/// Spawns a one-shot firing: walks `steps` exactly once. Used for Fire-once
/// (on `Down`) and Hold-to-repeat (on `Down` and every subsequent `Repeat`)
/// — fire-and-forget from the dispatch task's point of view, except for the
/// `held` handle it hands back so a later physical `Up` can force-release
/// anything left stuck (ticket 33).
pub fn spawn_fire_once(injector: Injector, steps: Vec<MacroStep>) -> FiringHandle {
    let held = Arc::new(Mutex::new(HashSet::new()));
    let held_task = held.clone();
    let handle = tokio::spawn(async move {
        let _ = run_once(&injector, &steps, &held_task).await;
    });
    FiringHandle { handle, held }
}

/// A running Toggle, as tracked in dispatch's `HashMap<Input, ActiveToggle>`
/// (spec.md). Stopping only ever needs to (a) tell the loop to stop and (b)
/// wait for it to have force-released whatever it was holding — the loop
/// task itself owns the live `HashSet<Key>` of currently-down keys (it's the
/// only task that ever mutates it, so there's nothing to race against by
/// keeping it there rather than mirroring it into a second, shared copy).
pub struct ActiveToggle {
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl ActiveToggle {
    /// Spawns the Toggle's own task: loops `steps` indefinitely
    /// (`tokio::time::sleep` between `Delay` steps) until cancelled, then
    /// force-releases exactly the keys it was still holding. `target_lap`
    /// (ticket 68) is resolved once by the caller — at Daemon startup, via
    /// `resolve_toggle_lap_target` — and passed down as a plain value here,
    /// rather than read fresh on every Toggle press: a per-press blocking
    /// device read would put a real (if small) async hop ahead of every
    /// Toggle's very first fire, on dispatch's own hot path, for a kernel
    /// setting that in practice never changes while the Daemon is running.
    pub fn spawn(injector: Injector, steps: Vec<MacroStep>, target_lap: Duration) -> Self {
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(run_toggle_loop(injector, steps, cancel.clone(), target_lap));
        ActiveToggle { cancel, handle }
    }

    /// Ticket 82's mouse-button Toggle fix: a single sustained hold rather
    /// than `spawn`'s repeat-tap loop — one `KeyDown` on start, released by
    /// one `KeyUp` on `stop()`, nothing in between. Same `{cancel, handle}`
    /// shape as the loop variant, so `stop()` and every one of its existing
    /// callers (`StopAllToggles`, profile switch, the Mode key, a Toggle
    /// Chord's own "full member set again" stop, a plain Input's second
    /// `Down`) work unchanged for both variants.
    pub fn spawn_held(injector: Injector, key: KeyCode) -> Self {
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(run_toggle_held(injector, key, cancel.clone()));
        ActiveToggle { cancel, handle }
    }

    /// Stops the Toggle and waits for its force-release to complete, so a
    /// caller that awaits this knows every held key is already released
    /// before doing anything else (e.g. resuming normal evaluation of the
    /// next press on the same Input).
    pub async fn stop(self) {
        self.cancel.cancel();
        let _ = self.handle.await;
    }
}

/// Hard safety floor beneath the live-cadence target `resolve_toggle_lap_
/// target` resolves (ticket 68) — no longer the pacing target itself. Found
/// live (ticket 26, 2026-08-15): a Toggle wrapping a plain `Action::Keypress`
/// compiles (`keypress_steps`) to `[KeyDown, KeyUp]` with no `Delay` step at
/// all, so without a floor a lap ran as fast as the injector channel +
/// `uinput` write allowed — an unbounded flood of synthetic keystrokes that
/// froze the focused app and then the whole input pipeline, hard enough to
/// require a power cycle. Kept as a guard against a degenerate live-read
/// cadence (e.g. an unusually fast configured kernel repeat rate) rather
/// than tuned to feel right on its own.
pub(crate) const MIN_TOGGLE_LAP: Duration = Duration::from_millis(20);

/// Combines the live kernel-repeat period with `MIN_TOGGLE_LAP`'s hard
/// floor — pure and unit-tested on its own, independent of the device read
/// that produces `kernel_period` in production (ticket 68).
fn combine_toggle_lap_target(kernel_period: Duration) -> Duration {
    kernel_period.max(MIN_TOGGLE_LAP)
}

/// The impure boundary ticket 68 adds, called once at Daemon startup
/// (`main.rs`, before dispatch's event loop starts) rather than per Toggle
/// spawn: reads the same live kernel-autorepeat cadence Hold-to-repeat
/// already sources from (`analog::read_kernel_auto_repeat`, off
/// `Node::If01`), off the blocking pool so the device open/ioctl never runs
/// on an async task's own thread. No device (no udev access, no real
/// Tartarus Pro attached, or a `spawn_blocking` panic — the last never
/// observed, only theoretically possible) falls back to `MIN_TOGGLE_LAP`
/// directly, which is also this module's own pre-ticket-68 pacing constant —
/// so a sandboxed/hardware-less run resolves to exactly the old hardcoded
/// behavior.
pub async fn resolve_toggle_lap_target() -> Duration {
    let kernel_period = tokio::task::spawn_blocking(|| {
        analog::read_kernel_auto_repeat()
            .map(|repeat| Duration::from_millis(u64::from(repeat.period)))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(MIN_TOGGLE_LAP);
    combine_toggle_lap_target(kernel_period)
}

async fn run_toggle_loop(
    injector: Injector,
    steps: Vec<MacroStep>,
    cancel: CancellationToken,
    target_lap: Duration,
) {
    let mut held: HashSet<KeyCode> = HashSet::new();
    'running: loop {
        if steps.is_empty() {
            // A degenerate empty Macro has nothing to loop over; wait for
            // the stop signal instead of spinning.
            cancel.cancelled().await;
            break 'running;
        }
        let lap_start = tokio::time::Instant::now();
        for step in &steps {
            let outcome = tokio::select! {
                _ = cancel.cancelled() => break 'running,
                outcome = execute_step(&injector, &mut held, *step) => outcome,
            };
            if outcome.is_err() {
                // The injector task has died — the whole Daemon is going
                // down, so there's no one left to force-release to.
                return;
            }
        }
        // Unconditional floor, measured from the start of the lap so it
        // only adds sleep when the lap's own steps (its own Delay total)
        // didn't already take this long — covers a bare Action::Keypress
        // (no Delay at all) and an under-paced Macro alike, and stays a
        // no-op for any Macro that already paces itself past the target.
        let elapsed = lap_start.elapsed();
        if elapsed < target_lap {
            tokio::select! {
                _ = cancel.cancelled() => break 'running,
                _ = tokio::time::sleep(target_lap - elapsed) => {}
            }
        }
    }
    force_release(&injector, held).await;
}

/// Ticket 82's held (non-looping) Toggle body: a single `KeyDown`, then wait
/// for the stop signal, then release — the mouse-button counterpart to
/// `run_toggle_loop`'s repeat-tap shape. A write suppressed at the injector
/// (nothing actually went down) leaves `held` empty, so the eventual
/// `force_release` below is correctly a no-op, same discipline as
/// `execute_step`'s own suppression handling.
async fn run_toggle_held(injector: Injector, key: KeyCode, cancel: CancellationToken) {
    let mut held: HashSet<KeyCode> = HashSet::new();
    if execute_step(&injector, &mut held, MacroStep::KeyDown(key))
        .await
        .is_err()
    {
        // The injector task has died — the whole Daemon is going down, so
        // there's no one left to force-release to.
        return;
    }
    cancel.cancelled().await;
    force_release(&injector, held).await;
}

/// `pub(crate)` (rather than private) so `analog_repeat::run_analog_repeat_loop`
/// can reuse the same step-walking primitive `run_toggle_loop` uses,
/// pacing it against a live-Depth-driven interval instead of a fixed lap
/// (ticket 20/39) — mirrors `keypress_steps`'s own promotion precedent
/// (ticket 62).
pub(crate) async fn execute_step(
    injector: &Injector,
    held: &mut HashSet<KeyCode>,
    step: MacroStep,
) -> Result<(), InjectorClosed> {
    match step {
        // `held` must mirror reality, not intent: only update it once we
        // know the write actually reached `uinput` (ticket 25's
        // live-hardware finding). A `KeyUp` step whose write suppression
        // silently withheld must NOT drop `key` from `held` — the key is
        // still genuinely down, and `force_release` on stop only
        // re-releases what's still listed here (bypassing suppression, but
        // only for keys it still knows about).
        MacroStep::KeyDown(key) => {
            let applied = injector.set_key_state(key, true).await?;
            if applied {
                held.insert(key);
            }
            Ok(())
        }
        MacroStep::KeyUp(key) => {
            let applied = injector.set_key_state(key, false).await?;
            if applied {
                held.remove(&key);
            }
            Ok(())
        }
        MacroStep::Delay(duration) => {
            tokio::time::sleep(duration).await;
            Ok(())
        }
    }
}

/// `pub(crate)` for the same reason as `execute_step` above —
/// `analog_repeat::run_analog_repeat_loop` needs its own force-release-on-stop,
/// same bypasses-suppression semantics.
pub(crate) async fn force_release(injector: &Injector, held: HashSet<KeyCode>) {
    for key in held {
        // Bypasses suppression (ticket 25's live-hardware test caught the
        // stuck-key bug from gating this the same as `set_key_state`): a key
        // this loop thinks it's holding may have gone down for real before
        // suppression turned on, so releasing it must never be withheld —
        // best-effort otherwise, since if the injector is already gone the
        // Daemon is shutting down anyway.
        let _ = injector.force_release_key(key).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, MacroStepDto, Modifiers};
    use crate::injector::{self, testing::RecordingSink};

    /// An empty macro library for tests exercising a non-Macro `Action` —
    /// `compile` requires the map unconditionally, but Keypress/
    /// ControllerButton/ProfileSwitch never consult it.
    fn empty_macros() -> HashMap<MacroId, MacroDef> {
        HashMap::new()
    }

    fn key_and_value(event: evdev::InputEvent) -> (KeyCode, i32) {
        match event.destructure() {
            evdev::EventSummary::Key(_, code, value) => (code, value),
            other => panic!("expected a key event, got {other:?}"),
        }
    }

    #[test]
    fn compile_keypress_is_a_canned_modifier_key_sequence() {
        let action = Action::Keypress {
            modifiers: Modifiers {
                ctrl: true,
                shift: true,
                alt: false,
                super_key: false,
            },
            key: KeyCode::KEY_T,
        };

        let steps = compile(&action, &empty_macros());

        assert_eq!(
            steps,
            vec![
                MacroStep::KeyDown(KeyCode::KEY_LEFTCTRL),
                MacroStep::KeyDown(KeyCode::KEY_LEFTSHIFT),
                MacroStep::KeyDown(KeyCode::KEY_T),
                MacroStep::KeyUp(KeyCode::KEY_T),
                MacroStep::KeyUp(KeyCode::KEY_LEFTSHIFT),
                MacroStep::KeyUp(KeyCode::KEY_LEFTCTRL),
            ]
        );
    }

    #[test]
    fn compile_macro_maps_steps_straight_across() {
        let mut macros = empty_macros();
        macros.insert(
            MacroId::from("test-macro"),
            MacroDef {
                name: "Test macro".to_string(),
                steps: vec![
                    MacroStepDto::KeyDown(KeyCode::KEY_A),
                    MacroStepDto::Delay(50),
                    MacroStepDto::KeyUp(KeyCode::KEY_A),
                ],
            },
        );
        let action = Action::Macro {
            macro_id: MacroId::from("test-macro"),
        };

        let steps = compile(&action, &macros);

        assert_eq!(
            steps,
            vec![
                MacroStep::KeyDown(KeyCode::KEY_A),
                MacroStep::Delay(Duration::from_millis(50)),
                MacroStep::KeyUp(KeyCode::KEY_A),
            ]
        );
    }

    #[test]
    fn compile_controller_button_is_a_down_up_pair_with_a_dwell() {
        let action = Action::ControllerButton {
            button: KeyCode::BTN_SOUTH,
        };

        let steps = compile(&action, &empty_macros());

        assert_eq!(
            steps,
            vec![
                MacroStep::KeyDown(KeyCode::BTN_SOUTH),
                MacroStep::Delay(CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD),
                MacroStep::KeyUp(KeyCode::BTN_SOUTH),
            ]
        );
    }

    #[test]
    fn controller_button_steps_helper_matches_the_compile_arm() {
        // Ticket 92: `dispatch::resolve_step` reuses this helper for a
        // `StepperItem::ControllerButton`, so it must stay identical to
        // what `compile(Action::ControllerButton)` produces.
        assert_eq!(
            controller_button_steps(KeyCode::BTN_TL),
            compile(
                &Action::ControllerButton {
                    button: KeyCode::BTN_TL,
                },
                &empty_macros(),
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn fire_once_controller_button_dwell_actually_elapses_before_the_up_write() {
        // Ticket 75/76: the dwell must be a genuine blocking sleep, not just
        // a step present in the compiled sequence — the Up write must not
        // reach the sink until the full dwell has actually elapsed.
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        let steps = compile(
            &Action::ControllerButton {
                button: KeyCode::BTN_SOUTH,
            },
            &empty_macros(),
        );
        tokio::task::yield_now().await;
        spawn_fire_once(inj.clone(), steps);
        tokio::task::yield_now().await;

        assert_eq!(sink.batches().len(), 1, "the Down must fire immediately");

        tokio::time::advance(CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD - Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            sink.batches().len(),
            1,
            "the Up must not fire before the dwell elapses"
        );

        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;

        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(batches.len(), 2, "the Up must fire once the dwell elapses");
        assert_eq!(key_and_value(batches[0][0]), (KeyCode::BTN_SOUTH, 1));
        assert_eq!(key_and_value(batches[1][0]), (KeyCode::BTN_SOUTH, 0));
    }

    #[tokio::test]
    async fn spawn_fire_once_runs_the_steps_exactly_once() {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        let steps = compile(
            &Action::Keypress {
                modifiers: Modifiers::default(),
                key: KeyCode::KEY_F1,
            },
            &empty_macros(),
        );
        spawn_fire_once(inj.clone(), steps).join().await;

        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(batches.len(), 2, "one KeyDown batch + one KeyUp batch");
        assert_eq!(key_and_value(batches[0][0]), (KeyCode::KEY_F1, 1));
        assert_eq!(key_and_value(batches[1][0]), (KeyCode::KEY_F1, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_loops_the_steps_until_stopped() {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(10)),
            MacroStep::KeyUp(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(10)),
        ];
        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn(inj.clone(), steps, MIN_TOGGLE_LAP);
        tokio::task::yield_now().await;

        // Let a few full laps run — advancing in steps matching each
        // Delay, so the loop task is actually polled (and gets to register
        // its next sleep) between each timer firing.
        for _ in 0..7 {
            tokio::time::advance(Duration::from_millis(10)).await;
            tokio::task::yield_now().await;
        }

        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        // At least 3 full down/up laps ran (65ms / 20ms-per-lap), and the
        // loop always ends on a matched KeyUp — no extra force-release
        // event tacked on for an already-balanced stop point.
        assert!(batches.len() >= 6, "expected several laps, got {batches:?}");
        for (i, batch) in batches.iter().enumerate() {
            let expected_value = if i % 2 == 0 { 1 } else { 0 };
            assert_eq!(key_and_value(batch[0]), (KeyCode::KEY_A, expected_value));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn spawn_held_holds_a_single_keydown_until_stopped() {
        // Ticket 82: the mouse-button Toggle fix — one KeyDown, nothing
        // else, no matter how long it's left running, until `stop()`
        // releases it with exactly one KeyUp.
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn_held(inj.clone(), KeyCode::BTN_LEFT);
        tokio::task::yield_now().await;

        // Advance well past several ordinary Toggle laps' worth of time —
        // a looping Toggle would have re-pressed several times by now.
        for _ in 0..7 {
            tokio::time::advance(MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(
            sink.batches().len(),
            1,
            "a held Toggle must never re-fire, unlike the looping variant"
        );
        assert_eq!(key_and_value(sink.batches()[0][0]), (KeyCode::BTN_LEFT, 1));

        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(batches.len(), 2, "exactly one KeyDown, one KeyUp");
        assert_eq!(key_and_value(batches[1][0]), (KeyCode::BTN_LEFT, 0));
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_stop_force_releases_a_key_left_held_by_an_unbalanced_macro() {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        // An unbalanced Macro: KeyDown with no matching KeyUp before the
        // loop's Delay — the Toggle should still force-release it on stop.
        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(50)),
        ];
        let toggle = ActiveToggle::spawn(inj.clone(), steps, MIN_TOGGLE_LAP);

        // Advance into the middle of the Delay, so KEY_A is definitely held
        // (KeyDown sent) and not yet due for another KeyDown.
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;

        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(key_and_value(batches[0][0]), (KeyCode::KEY_A, 1));
        let evdev::EventSummary::Key(_, code, value) = batches.last().unwrap()[0].destructure()
        else {
            panic!("expected a key event");
        };
        assert_eq!(code, KeyCode::KEY_A, "the held key must be force-released");
        assert_eq!(value, 0, "force-release must be a KeyUp");

        // No key is left down: KeyDown count must equal KeyUp count.
        let downs = batches
            .iter()
            .filter(|b| key_and_value(b[0]).1 == 1)
            .count();
        let ups = batches
            .iter()
            .filter(|b| key_and_value(b[0]).1 == 0)
            .count();
        assert_eq!(
            downs, ups,
            "every KeyDown must have a matching KeyUp after stop"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_stop_force_releases_a_held_key_even_while_output_is_suppressed() {
        // Regression test for ticket 25's live-hardware finding: a key can
        // go down for real (unsuppressed), then suppression turns on (e.g.
        // the GUI gains focus) before the same physical key stops the
        // Toggle — the force-release on stop must still reach the sink, or
        // the key is left stuck down at the OS level with `active_toggles`
        // wrongly implying it was released.
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(50)),
        ];
        let toggle = ActiveToggle::spawn(inj.clone(), steps, MIN_TOGGLE_LAP);

        // KEY_A's KeyDown is sent for real while unsuppressed.
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            sink.batches().len(),
            1,
            "the initial KeyDown must have reached the sink"
        );

        inj.set_suppressed(true).await.unwrap();
        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        let evdev::EventSummary::Key(_, code, value) = batches.last().unwrap()[0].destructure()
        else {
            panic!("expected a key event");
        };
        assert_eq!(
            code,
            KeyCode::KEY_A,
            "the held key must still be force-released"
        );
        assert_eq!(
            value, 0,
            "force-release must bypass suppression, not be silently dropped"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_suppressed_keyup_step_keeps_the_key_in_held_for_a_later_force_release() {
        // Regression test for a `/code-review` finding on the fix above: a
        // Toggle's own *normal* loop step (not the stop path) can also hit
        // suppression mid-lap. `execute_step`'s KeyUp arm used to remove the
        // key from `held` unconditionally, even when the matching write was
        // silently withheld by suppression — so if the Toggle was stopped
        // shortly after, `force_release` no longer knew that key was still
        // genuinely down, and it was never actually released.
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(10)),
            MacroStep::KeyUp(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(10)),
        ];
        let toggle = ActiveToggle::spawn(inj.clone(), steps, MIN_TOGGLE_LAP);

        // KEY_A's KeyDown is sent for real while unsuppressed.
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            sink.batches().len(),
            1,
            "the initial KeyDown must have reached the sink"
        );

        // Suppression turns on (the GUI gains focus) before the loop's own
        // KeyUp step runs. That KeyUp's write is withheld — no new batch —
        // but the key is still genuinely down at the OS level.
        inj.set_suppressed(true).await.unwrap();
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            sink.batches().len(),
            1,
            "the suppressed KeyUp step must not have reached the sink"
        );

        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(
            batches.len(),
            2,
            "stop must still force-release the key the suppressed KeyUp step never actually released"
        );
        let evdev::EventSummary::Key(_, code, value) = batches[1][0].destructure() else {
            panic!("expected a key event");
        };
        assert_eq!(
            (code, value),
            (KeyCode::KEY_A, 0),
            "the key must not be left stuck down just because a normal loop step got suppressed"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_with_an_empty_macro_stops_cleanly_with_no_output() {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        let toggle = ActiveToggle::spawn(inj.clone(), Vec::new(), MIN_TOGGLE_LAP);
        tokio::task::yield_now().await;
        toggle.stop().await;

        drop(inj);
        inj_handle.await.unwrap().unwrap();

        assert!(sink.batches().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_with_a_keypress_action_is_paced_by_the_floor() {
        // Regression test for ticket 26's live-hardware incident: a Toggle
        // wrapping a plain Action::Keypress compiles to [KeyDown, KeyUp]
        // with no Delay step at all, so without a floor the loop would
        // busy-spin as fast as the injector channel allowed. Passes
        // `MIN_TOGGLE_LAP` directly as `target_lap` (ticket 68) — this test
        // is about the floor mechanism itself, not about whatever cadence a
        // live device read would resolve to in production.
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        let action = Action::Keypress {
            modifiers: Modifiers::default(),
            key: KeyCode::KEY_C,
        };
        let steps = compile(&action, &empty_macros());
        assert!(
            !steps.iter().any(|s| matches!(s, MacroStep::Delay(_))),
            "a plain Keypress must compile to zero Delay steps"
        );

        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn(inj.clone(), steps, MIN_TOGGLE_LAP);
        tokio::task::yield_now().await;

        // Advance in floor-sized ticks. Without a floor, every lap would
        // complete without ever waiting on simulated time, so the loop
        // would never yield to these advances at all — it would just spin
        // forever inside the task, and this test would hang instead of
        // completing.
        for _ in 0..7 {
            tokio::time::advance(MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }

        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        // One lap (2 batches) fires immediately (no sim time needed for a
        // zero-Delay lap), then roughly one more lap per floor tick — a
        // small, bounded number, nowhere near a busy loop's output.
        assert!(
            (8..=18).contains(&batches.len()),
            "expected laps paced one-per-floor-tick, got {} batches: {batches:?}",
            batches.len()
        );
        for (i, batch) in batches.iter().enumerate() {
            let expected_value = if i % 2 == 0 { 1 } else { 0 };
            assert_eq!(key_and_value(batch[0]), (KeyCode::KEY_C, expected_value));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_macro_with_a_delay_free_keydown_hits_the_same_floor() {
        // Regression test for ticket 26: the floor must apply uniformly to
        // every Toggle, not just ones compiled from Action::Keypress — a
        // hand-built Toggle Macro of a single delay-free KeyDown is just as
        // dangerous and must hit the same floor.
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        let steps = vec![MacroStep::KeyDown(KeyCode::KEY_A)];

        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn(inj.clone(), steps, MIN_TOGGLE_LAP);
        tokio::task::yield_now().await;

        for _ in 0..3 {
            tokio::time::advance(MIN_TOGGLE_LAP).await;
            tokio::task::yield_now().await;
        }

        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        // A small, bounded number of laps, each a KeyDown re-press (this
        // Macro never itself sends a KeyUp), plus stop()'s trailing
        // force-release of whatever was still held.
        assert!(
            (3..=7).contains(&batches.len()),
            "expected a few floor-paced laps, got {} batches: {batches:?}",
            batches.len()
        );
        for batch in &batches[..batches.len() - 1] {
            assert_eq!(key_and_value(batch[0]), (KeyCode::KEY_A, 1));
        }
        assert_eq!(
            key_and_value(batches.last().unwrap()[0]),
            (KeyCode::KEY_A, 0),
            "stop() must force-release the key still held"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn toggle_paces_to_an_arbitrary_target_lap_not_just_min_toggle_lap() {
        // Ticket 68: `run_toggle_loop` must actually pace off whatever
        // `target_lap` it's given (in production, the live kernel-autorepeat
        // period), not a hardcoded constant — this is the regression guard
        // for that parameterization, independent of `combine_toggle_lap_
        // target`'s own pure floor test below.
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        let target_lap = Duration::from_millis(50);
        let steps = vec![MacroStep::KeyDown(KeyCode::KEY_A)];

        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn(inj.clone(), steps, target_lap);
        tokio::task::yield_now().await;

        // 150ms at a 50ms target should complete exactly 3 laps; at the old
        // hardcoded 20ms floor it would have completed 7+.
        for _ in 0..3 {
            tokio::time::advance(target_lap).await;
            tokio::task::yield_now().await;
        }

        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert!(
            (4..=6).contains(&batches.len()),
            "expected laps paced to the given 50ms target, got {} batches: {batches:?}",
            batches.len()
        );
    }

    #[test]
    fn toggle_lap_target_floors_an_implausibly_fast_kernel_period() {
        assert_eq!(
            combine_toggle_lap_target(Duration::from_millis(5)),
            MIN_TOGGLE_LAP
        );
    }

    #[test]
    fn toggle_lap_target_uses_a_slower_kernel_period_unfloored() {
        let slower = Duration::from_millis(40);
        assert_eq!(combine_toggle_lap_target(slower), slower);
    }

    #[test]
    fn toggle_lap_target_at_exactly_the_floor_is_unchanged() {
        assert_eq!(combine_toggle_lap_target(MIN_TOGGLE_LAP), MIN_TOGGLE_LAP);
    }
}
