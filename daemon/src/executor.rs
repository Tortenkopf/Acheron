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

/// Walks `steps` once, in order, sleeping between `Delay` steps. Shared by
/// a one-shot firing and each lap of a Toggle's loop. Returns `Err` only
/// when the injector task itself has died (a genuine, fatal Daemon
/// condition, not something this firing should retry).
async fn run_once(injector: &Injector, steps: &[MacroStep]) -> Result<(), InjectorClosed> {
    for step in steps {
        match step {
            MacroStep::KeyDown(key) => injector.set_key_state(*key, true).await?,
            MacroStep::KeyUp(key) => injector.set_key_state(*key, false).await?,
            MacroStep::Delay(duration) => tokio::time::sleep(*duration).await,
        }
    }
    Ok(())
}

/// Spawns a one-shot firing: walks `steps` exactly once. Used for Fire-once
/// (on `Down`) and Hold-to-repeat (on `Down` and every subsequent `Repeat`)
/// — both fire-and-forget from the dispatch task's point of view.
pub fn spawn_fire_once(injector: Injector, steps: Vec<MacroStep>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let _ = run_once(&injector, &steps).await;
    })
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

async fn run_toggle_loop(injector: Injector, steps: Vec<MacroStep>, cancel: CancellationToken) {
    let mut held: HashSet<KeyCode> = HashSet::new();
    'running: loop {
        if steps.is_empty() {
            // A degenerate empty Macro has nothing to loop over; wait for
            // the stop signal instead of spinning.
            cancel.cancelled().await;
            break 'running;
        }
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
    }
    force_release(&injector, held).await;
}

async fn execute_step(
    injector: &Injector,
    held: &mut HashSet<KeyCode>,
    step: MacroStep,
) -> Result<(), InjectorClosed> {
    match step {
        MacroStep::KeyDown(key) => {
            held.insert(key);
            injector.set_key_state(key, true).await
        }
        MacroStep::KeyUp(key) => {
            held.remove(&key);
            injector.set_key_state(key, false).await
        }
        MacroStep::Delay(duration) => {
            tokio::time::sleep(duration).await;
            Ok(())
        }
    }
}

async fn force_release(injector: &Injector, held: HashSet<KeyCode>) {
    for key in held {
        // Best-effort: if the injector is already gone the Daemon is
        // shutting down anyway.
        let _ = injector.set_key_state(key, false).await;
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
        spawn_fire_once(inj.clone(), steps).await.unwrap();

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
}
