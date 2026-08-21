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

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use evdev::KeyCode;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::{Action, MacroStepDto, Modifiers};
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

fn keypress_steps(modifiers: Modifiers, key: KeyCode) -> Vec<MacroStep> {
    let mods = modifier_codes(modifiers);
    let mut steps = Vec::with_capacity(mods.len() * 2 + 2);
    steps.extend(mods.iter().map(|&m| MacroStep::KeyDown(m)));
    steps.push(MacroStep::KeyDown(key));
    steps.push(MacroStep::KeyUp(key));
    steps.extend(mods.iter().rev().map(|&m| MacroStep::KeyUp(m)));
    steps
}

/// Compiles a Binding's `Action` into the flat step sequence the shared
/// executor runs (spec.md: "both Action kinds compile ... into one steps:
/// Vec<MacroStep> ... run by one shared executor").
pub fn compile(action: &Action) -> Vec<MacroStep> {
    match action {
        Action::Keypress { modifiers, key } => keypress_steps(*modifiers, *key),
        Action::Macro { steps } => steps
            .iter()
            .map(|step| match step {
                MacroStepDto::KeyDown(key) => MacroStep::KeyDown(*key),
                MacroStepDto::KeyUp(key) => MacroStep::KeyUp(*key),
                MacroStepDto::Delay(ms) => MacroStep::Delay(Duration::from_millis(*ms)),
            })
            .collect(),
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
    /// Whether the firing's steps have finished walking — used by `fire()`'s
    /// existing same-Input overlap guard, unchanged from the old bare
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
    /// force-releases exactly the keys it was still holding.
    pub fn spawn(injector: Injector, steps: Vec<MacroStep>) -> Self {
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(run_toggle_loop(injector, steps, cancel.clone()));
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

/// Floor pacing between Toggle laps, independent of the compiled steps' own
/// `Delay` total. Found live (ticket 26, 2026-08-15): a Toggle wrapping a
/// plain `Action::Keypress` compiles (`keypress_steps`) to `[KeyDown,
/// KeyUp]` with no `Delay` step at all, so without this floor a lap ran as
/// fast as the injector channel + `uinput` write allowed — an unbounded
/// flood of synthetic keystrokes that froze the focused app and then the
/// whole input pipeline, hard enough to require a power cycle. Every lap,
/// however it's compiled, now takes at least this long.
const MIN_TOGGLE_LAP: Duration = Duration::from_millis(20);

async fn run_toggle_loop(injector: Injector, steps: Vec<MacroStep>, cancel: CancellationToken) {
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
        // no-op for any Macro that already paces itself past the floor.
        let elapsed = lap_start.elapsed();
        if elapsed < MIN_TOGGLE_LAP {
            tokio::select! {
                _ = cancel.cancelled() => break 'running,
                _ = tokio::time::sleep(MIN_TOGGLE_LAP - elapsed) => {}
            }
        }
    }
    force_release(&injector, held).await;
}

async fn execute_step(
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

async fn force_release(injector: &Injector, held: HashSet<KeyCode>) {
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

        let steps = compile(&action);

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
        let action = Action::Macro {
            steps: vec![
                MacroStepDto::KeyDown(KeyCode::KEY_A),
                MacroStepDto::Delay(50),
                MacroStepDto::KeyUp(KeyCode::KEY_A),
            ],
        };

        let steps = compile(&action);

        assert_eq!(
            steps,
            vec![
                MacroStep::KeyDown(KeyCode::KEY_A),
                MacroStep::Delay(Duration::from_millis(50)),
                MacroStep::KeyUp(KeyCode::KEY_A),
            ]
        );
    }

    #[tokio::test]
    async fn spawn_fire_once_runs_the_steps_exactly_once() {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone());

        let steps = compile(&Action::Keypress {
            modifiers: Modifiers::default(),
            key: KeyCode::KEY_F1,
        });
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
        let (inj, inj_handle) = injector::spawn(sink.clone());

        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(10)),
            MacroStep::KeyUp(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(10)),
        ];
        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn(inj.clone(), steps);
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
    async fn toggle_stop_force_releases_a_key_left_held_by_an_unbalanced_macro() {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone());

        // An unbalanced Macro: KeyDown with no matching KeyUp before the
        // loop's Delay — the Toggle should still force-release it on stop.
        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(50)),
        ];
        let toggle = ActiveToggle::spawn(inj.clone(), steps);

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
        let (inj, inj_handle) = injector::spawn(sink.clone());

        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(50)),
        ];
        let toggle = ActiveToggle::spawn(inj.clone(), steps);

        // KEY_A's KeyDown is sent for real while unsuppressed.
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(sink.batches().len(), 1, "the initial KeyDown must have reached the sink");

        inj.set_suppressed(true).await.unwrap();
        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        let evdev::EventSummary::Key(_, code, value) = batches.last().unwrap()[0].destructure()
        else {
            panic!("expected a key event");
        };
        assert_eq!(code, KeyCode::KEY_A, "the held key must still be force-released");
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
        let (inj, inj_handle) = injector::spawn(sink.clone());

        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(10)),
            MacroStep::KeyUp(KeyCode::KEY_A),
            MacroStep::Delay(Duration::from_millis(10)),
        ];
        let toggle = ActiveToggle::spawn(inj.clone(), steps);

        // KEY_A's KeyDown is sent for real while unsuppressed.
        tokio::time::advance(Duration::from_millis(10)).await;
        tokio::task::yield_now().await;
        assert_eq!(sink.batches().len(), 1, "the initial KeyDown must have reached the sink");

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
        let (inj, inj_handle) = injector::spawn(sink.clone());

        let toggle = ActiveToggle::spawn(inj.clone(), Vec::new());
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
        // busy-spin as fast as the injector channel allowed.
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone());

        let action = Action::Keypress {
            modifiers: Modifiers::default(),
            key: KeyCode::KEY_C,
        };
        let steps = compile(&action);
        assert!(
            !steps.iter().any(|s| matches!(s, MacroStep::Delay(_))),
            "a plain Keypress must compile to zero Delay steps"
        );

        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn(inj.clone(), steps);
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
        let (inj, inj_handle) = injector::spawn(sink.clone());

        let steps = vec![MacroStep::KeyDown(KeyCode::KEY_A)];

        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn(inj.clone(), steps);
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
}
