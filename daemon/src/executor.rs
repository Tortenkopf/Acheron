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

use crate::capture::analog::{self, RepeatSchedule};
use crate::config::{Action, MacroDef, MacroId, MacroStepDto, Modifiers, StepperItem};
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
/// this now only fires via `trigger::compile_action`'s Digital-Capture-mode
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

/// The Down→Up dwell spliced into a canned one-shot keyboard press
/// (`.scratch/humane-output-rate/` ticket 12): a Fire-once `Action::Keypress`,
/// and its single-key `Macro` / Stepper-`Key`-step / Chord equivalents, otherwise
/// compile to `[KeyDown, KeyUp]` with no `Delay` at all — the near-zero-dwell
/// shape `docs/anti-cheat-input-heuristics.md` and ADR-0008 name the clearest
/// synthetic tell, which the `value=2` rebuild erased everywhere it holds or
/// repeats but not on a plain one-shot press. `trigger::Slots::perform` swaps the
/// plain compiled steps for `fire_once_key_steps` when a Fire-once firing's steps
/// satisfy `single_held_key`.
///
/// 40 ms: ~2.5 frames at 60 Hz, clears the CS2 "0 ms overlap/neutral" macro
/// shape, sits inside the ~30–500 ms keystroke-dynamics typing band, and keeps
/// ~2× headroom under the ~80 ms human same-key double-tap floor so
/// `trigger::decide`'s `FiringUnfinished` overlap guard never drops a real user's
/// second press. This is a rate-plausibility *consistency* follow-up, not a
/// detection defense — the `uinput` origin stays visible, a Macro fired once
/// keeps its author's cadence, and Analog-repeat's tap pulses are untouched.
///
/// Deliberately **not** shared with `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`: that
/// constant (35 ms) targets single-poll-frame coverage on a receiving game
/// (≈ 2 frames at 60 fps); this one targets a human-plausible keystroke dwell.
/// Different jobs — one constant would silently couple two unrelated tuning
/// knobs.
pub(crate) const FIRE_ONCE_KEY_DWELL: Duration = Duration::from_millis(40);

/// The down-only prefix of a modified single-key press (spec-kernel-shaped-repeat.md
/// §3.1): each modifier then the key, all `KeyDown`, no matching `KeyUp` — a
/// deliberately *unbalanced* hold that mirrors a physically held `Ctrl+X`. Only
/// the base key goes on to autorepeat (`value=2`, via `Injector::repeat_key`);
/// the modifiers just sit held `value=1`. Every code that actually reached
/// `uinput` lands in the firing's `held` set, so `FiringHandle::force_release_stuck`
/// on the bound Input's physical `Up` balances the whole chord with `value=0`.
/// The `keypress_steps` prefix up to and including the key press.
pub(crate) fn held_key_down_steps(modifiers: Modifiers, key: KeyCode) -> Vec<MacroStep> {
    let mut steps: Vec<MacroStep> = modifier_codes(modifiers)
        .into_iter()
        .map(MacroStep::KeyDown)
        .collect();
    steps.push(MacroStep::KeyDown(key));
    steps
}

/// `pub(crate)` (rather than private) so `compile_stepper_item` can reuse
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

/// `keypress_steps` with a `FIRE_ONCE_KEY_DWELL` `Delay` spliced between the base
/// key's `KeyDown` and `KeyUp` (`.scratch/humane-output-rate/` ticket 12) —
/// `[KeyDown(mods…), KeyDown(key), Delay(FIRE_ONCE_KEY_DWELL), KeyUp(key),
/// KeyUp(mods…)]`. `trigger::Slots::perform` substitutes this for the plain
/// compiled steps of a Fire-once firing whose steps satisfy `single_held_key`,
/// so a canned one-shot press stops emitting the zero-dwell pair.
///
/// The `Delay` sits *between* the key edges, so `single_held_key` rejects the
/// spliced output (`None`) — correct, since the classifier only ever runs on the
/// *pre-splice* steps and a re-classification must not feed the spliced form
/// back onto any `value=2` autorepeat path.
pub(crate) fn fire_once_key_steps(modifiers: Modifiers, key: KeyCode) -> Vec<MacroStep> {
    let mods = modifier_codes(modifiers);
    let mut steps = Vec::with_capacity(mods.len() * 2 + 3);
    steps.extend(mods.iter().map(|&m| MacroStep::KeyDown(m)));
    steps.push(MacroStep::KeyDown(key));
    steps.push(MacroStep::Delay(FIRE_ONCE_KEY_DWELL));
    steps.push(MacroStep::KeyUp(key));
    steps.extend(mods.iter().rev().map(|&m| MacroStep::KeyUp(m)));
    steps
}

/// `Some((mods, key))` iff `steps` is a single held keyboard key — the exact
/// shape a physically held key produces, so the decision layer can route it
/// through the kernel-autorepeat (`value=2`) path instead of a `[Down, Up]`
/// loop (spec-kernel-shaped-repeat.md §6). Pure over `&[MacroStep]` and
/// unit-tested standalone; ticket 03 wires `trigger::hold_repeat_kind` to it.
///
/// **Qualifies:**
/// - `[KeyDown(k), KeyUp(k)]` — a plain unmodified key;
/// - `[KeyDown(m0)…KeyDown(mN), KeyDown(k), KeyUp(k), KeyUp(mN)…KeyUp(m0)]` —
///   a modifier-wrapped single key, `m*` being modifier codes in
///   `keypress_steps`' fixed ctrl/shift/alt/super order (released in reverse);
/// - either of the above followed by at most one trailing `MacroStep::Delay`
///   (ignored — ticket 03's "turbo macro written with a trailing pause").
///
/// **Disqualifies (`None`):** more than one distinct non-modifier key; any
/// `Delay` *between* the key steps; two or more trailing `Delay`s; an
/// out-of-order modifier chord; any other shape.
pub(crate) fn single_held_key(steps: &[MacroStep]) -> Option<(Modifiers, KeyCode)> {
    // Strip at most one trailing Delay (ticket 03's turbo macro written with a
    // trailing pause). Two or more trailing Delays are an authored cadence.
    let core = match steps.split_last() {
        Some((MacroStep::Delay(_), rest)) => {
            if matches!(rest.last(), Some(MacroStep::Delay(_))) {
                return None;
            }
            rest
        }
        _ => steps,
    };

    // A Delay between the key edges is an authored cadence — a real macro.
    if core.iter().any(|s| matches!(s, MacroStep::Delay(_))) {
        return None;
    }
    if core.len() < 2 || core.len() % 2 != 0 {
        return None;
    }
    let (downs, ups) = core.split_at(core.len() / 2);

    let down_codes: Vec<KeyCode> = downs
        .iter()
        .map(|s| match s {
            MacroStep::KeyDown(k) => Some(*k),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let up_codes: Vec<KeyCode> = ups
        .iter()
        .map(|s| match s {
            MacroStep::KeyUp(k) => Some(*k),
            _ => None,
        })
        .collect::<Option<_>>()?;

    // A physically held key releases its modifiers in reverse order.
    if !up_codes.iter().rev().eq(down_codes.iter()) {
        return None;
    }

    let (key, mods_prefix) = down_codes.split_last()?;

    // The modifier prefix must be codes in `keypress_steps`' fixed
    // ctrl/shift/alt/super order — an out-of-order (or duplicated) chord is a
    // real macro, not a held key.
    const ORDER: [KeyCode; 4] = [
        KeyCode::KEY_LEFTCTRL,
        KeyCode::KEY_LEFTSHIFT,
        KeyCode::KEY_LEFTALT,
        KeyCode::KEY_LEFTMETA,
    ];
    let mut next = 0;
    let mut modifiers = Modifiers::default();
    for code in mods_prefix {
        let pos = ORDER.iter().position(|c| c == code)?;
        if pos < next {
            return None;
        }
        next = pos + 1;
        match *code {
            KeyCode::KEY_LEFTCTRL => modifiers.ctrl = true,
            KeyCode::KEY_LEFTSHIFT => modifiers.shift = true,
            KeyCode::KEY_LEFTALT => modifiers.alt = true,
            KeyCode::KEY_LEFTMETA => modifiers.super_key = true,
            _ => unreachable!("guarded by the ORDER lookup above"),
        }
    }

    Some((modifiers, *key))
}

/// The down/dwell/up triple a single atomic controller-button press
/// compiles to (ticket 75/76's `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`
/// dwell between the edges, so a same-poll-frame game doesn't swallow the
/// press). Shared by `compile`'s `Action::ControllerButton` arm and
/// `compile_stepper_item`'s `StepperItem::ControllerButton` arm (ticket
/// 92) — a Stepper item is always an atomic one-shot press, so it hits the
/// same polled-input risk and reuses the same constant rather than
/// hand-inlining the triple.
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
                "Action::Step's steps depend on Daemon-owned runtime cursor state, resolved by trigger::compile_action before this generic compile is ever reached for it"
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

/// Compiles one already-selected Stepper list item into the flat step
/// sequence the shared executor runs (ticket 62 / 92, post-release ticket
/// 12) — `trigger::compile_action` calls this on the item
/// `stepper::Cursors::step` returns. A `Key` item reuses `Action::Keypress`'s
/// mods-down/key/mods-up path, carrying its own modifier combination if it
/// has one (ticket 62); a `ControllerButton` item reuses
/// `Action::ControllerButton`'s down/dwell/up triple (ticket 92), routed to
/// the gamepad `uinput` device by the injector's `input::is_gamepad_button`
/// check. Both helpers are shared with `compile`'s own arms, so a Stepper
/// item and the matching Action stay byte-identical.
pub(crate) fn compile_stepper_item(item: StepperItem) -> Vec<MacroStep> {
    match item {
        StepperItem::Key { key, modifiers } => keypress_steps(modifiers, key),
        StepperItem::ControllerButton { button } => controller_button_steps(button),
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
    /// `trigger::Slots::slot` / `snapshot`'s `FiringUnfinished` /
    /// `FiringFinished` split for `trigger::decide`'s overlap guard, unchanged
    /// from the old bare `JoinHandle<()>` check.
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

    /// Ticket 05 / spec-kernel-shaped-repeat.md §3.2, §5.2: a Toggle whose
    /// held target is a single keyboard key (`AutorepeatKey`) — plain
    /// `Keypress` or single-key `Macro` — holds a genuine Linux autorepeat
    /// rather than looping `[Down, Up]` through `run_toggle_loop`. `value=1`
    /// on the first press (each modifier, then the base key, all tracked in a
    /// loop-private `held`), then one `value=2` per `schedule` tick at the
    /// full `REP_DELAY`→`REP_PERIOD` envelope — a Toggle-held key *is* a held
    /// key — and `value=0` for the key and every modifier on `stop()` /
    /// `StopAllToggles` / Layer-Profile switch. `schedule` is resolved once at
    /// Daemon startup (`capture::analog::resolve_toggle_autorepeat_schedule`)
    /// and threaded down as a plain value, same as `spawn`'s `target_lap`.
    /// Same `{cancel, handle}` shape as `spawn` / `spawn_held`, so `stop()`
    /// and every caller work unchanged.
    pub fn spawn_autorepeat(
        injector: Injector,
        modifiers: Modifiers,
        key: KeyCode,
        schedule: RepeatSchedule,
    ) -> Self {
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(run_toggle_autorepeat(
            injector,
            modifiers,
            key,
            schedule,
            cancel.clone(),
        ));
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

/// Ticket 05: `run_toggle_loop` + `target_lap` + `MIN_TOGGLE_LAP` +
/// `combine_toggle_lap_target` + `resolve_toggle_lap_target` now survive
/// **only** for the multi-step Macro Toggle (`D::StartToggleLoop`, surface 9)
/// — the sole remaining looping Toggle. A Toggle whose held target is a
/// single keyboard key routes through `spawn_autorepeat` /
/// `run_toggle_autorepeat` instead (spec-kernel-shaped-repeat.md §3.2).
///
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
/// that produces `kernel_period` in production (ticket 68). Ticket 05: only
/// the multi-step Macro Toggle (`run_toggle_loop`) is paced this way now.
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
///
/// Ticket 05: paces only the surviving multi-step Macro Toggle
/// (`D::StartToggleLoop`); a single-key Toggle holds a kernel autorepeat via
/// `capture::analog::resolve_toggle_autorepeat_schedule` instead.
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

/// The looping Toggle body — ticket 05 leaves this reached **only** by
/// `D::StartToggleLoop`, i.e. a Toggle wrapping a *multi-step* Macro
/// (surface 9). Every single-key Toggle now runs `run_toggle_autorepeat`.
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

/// Ticket 05's sustained-autorepeat Toggle body (spec-kernel-shaped-repeat.md
/// §5.2): press each modifier then the base key (`value=1`, tracked in a
/// loop-private `held`), then emit one `injector.repeat_key(key)` (`value=2`)
/// per `schedule` tick — the Nth repeat due at `started + delay_ms +
/// N*period_ms`, the **full** kernel envelope, because a Toggle-held key is a
/// held key. `RepeatSchedule::advance_fired` carries the missed-deadline
/// clamp for free: after a stall the loop emits exactly one `value=2` on
/// resume and re-bases, never a catch-up burst (mirrors the kernel's
/// `input_repeat_key`). On `cancel` — second press, `StopAllToggles`,
/// Layer/Profile switch, Analog→Digital flip — `force_release` drains `held`,
/// so the base key and every modifier go `value=0` together. The `value=2`
/// stream itself is stateless and needs no teardown (§7).
async fn run_toggle_autorepeat(
    injector: Injector,
    modifiers: Modifiers,
    key: KeyCode,
    schedule: RepeatSchedule,
    cancel: CancellationToken,
) {
    let mut held: HashSet<KeyCode> = HashSet::new();
    for step in held_key_down_steps(modifiers, key) {
        if execute_step(&injector, &mut held, step).await.is_err() {
            // The injector task has died — the whole Daemon is going down,
            // so there's no one left to force-release to.
            return;
        }
    }

    let started = tokio::time::Instant::now();
    let mut fired = 0u32;
    loop {
        let due = started + schedule.due_offset(fired);
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep_until(due) => {}
        }
        let held_for = started.elapsed();
        if !schedule.repeat_due(held_for, fired) {
            continue;
        }
        fired = schedule.advance_fired(held_for, fired);
        if injector.repeat_key(key).await.is_err() {
            break;
        }
    }

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
    use crate::config::{Action, MacroStepDto, Modifiers, StepperItem};
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
        // Ticket 92: `compile_stepper_item` reuses this helper for a
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

    /// Ticket 63 (moved here from `dispatch::tests` by post-release ticket
    /// 12): a modifier-bearing Stepper `Key` item compiles through the same
    /// canned mods-down/key/mods-up sequence as `Action::Keypress`.
    #[test]
    fn compile_stepper_item_with_modifiers_is_the_canned_mods_down_key_up_sequence() {
        let steps = compile_stepper_item(StepperItem::Key {
            key: KeyCode::KEY_3,
            modifiers: Modifiers {
                ctrl: true,
                shift: true,
                alt: false,
                super_key: false,
            },
        });

        assert_eq!(
            steps,
            vec![
                MacroStep::KeyDown(KeyCode::KEY_LEFTCTRL),
                MacroStep::KeyDown(KeyCode::KEY_LEFTSHIFT),
                MacroStep::KeyDown(KeyCode::KEY_3),
                MacroStep::KeyUp(KeyCode::KEY_3),
                MacroStep::KeyUp(KeyCode::KEY_LEFTSHIFT),
                MacroStep::KeyUp(KeyCode::KEY_LEFTCTRL),
            ]
        );
    }

    /// Ticket 92 (moved here from `dispatch::tests` by post-release ticket
    /// 12): a `StepperItem::ControllerButton` compiles to the same
    /// down/dwell/up triple as `Action::ControllerButton`'s digital path.
    #[test]
    fn compile_stepper_item_controller_button_is_the_dwell_triple() {
        let steps = compile_stepper_item(StepperItem::ControllerButton {
            button: KeyCode::BTN_SOUTH,
        });

        assert_eq!(
            steps,
            vec![
                MacroStep::KeyDown(KeyCode::BTN_SOUTH),
                MacroStep::Delay(CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD),
                MacroStep::KeyUp(KeyCode::BTN_SOUTH),
            ]
        );
    }

    const CTRL: KeyCode = KeyCode::KEY_LEFTCTRL;
    const SHIFT: KeyCode = KeyCode::KEY_LEFTSHIFT;
    const ALT: KeyCode = KeyCode::KEY_LEFTALT;
    const SUPER: KeyCode = KeyCode::KEY_LEFTMETA;

    #[test]
    fn single_held_key_accepts_a_plain_unmodified_key() {
        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_X),
            MacroStep::KeyUp(KeyCode::KEY_X),
        ];
        assert_eq!(
            single_held_key(&steps),
            Some((Modifiers::default(), KeyCode::KEY_X))
        );
    }

    #[test]
    fn single_held_key_accepts_a_modifier_wrapped_key() {
        let steps = vec![
            MacroStep::KeyDown(CTRL),
            MacroStep::KeyDown(KeyCode::KEY_X),
            MacroStep::KeyUp(KeyCode::KEY_X),
            MacroStep::KeyUp(CTRL),
        ];
        assert_eq!(
            single_held_key(&steps),
            Some((
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
                KeyCode::KEY_X
            ))
        );
    }

    #[test]
    fn single_held_key_matches_keypress_steps_for_every_modifier_combination() {
        // The predicate is the inverse of `keypress_steps` — anything that
        // helper builds must round-trip back to its own `(mods, key)`.
        for mods in [
            Modifiers::default(),
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            Modifiers {
                shift: true,
                alt: true,
                ..Modifiers::default()
            },
            Modifiers {
                ctrl: true,
                shift: true,
                alt: true,
                super_key: true,
            },
        ] {
            let steps = keypress_steps(mods, KeyCode::KEY_J);
            assert_eq!(
                single_held_key(&steps),
                Some((mods, KeyCode::KEY_J)),
                "keypress_steps({mods:?}) must round-trip"
            );
        }
    }

    #[test]
    fn single_held_key_ignores_one_trailing_delay() {
        let steps = vec![
            MacroStep::KeyDown(SHIFT),
            MacroStep::KeyDown(KeyCode::KEY_X),
            MacroStep::KeyUp(KeyCode::KEY_X),
            MacroStep::KeyUp(SHIFT),
            MacroStep::Delay(Duration::from_millis(30)),
        ];
        assert_eq!(
            single_held_key(&steps),
            Some((
                Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                KeyCode::KEY_X
            ))
        );
    }

    #[test]
    fn single_held_key_rejects_a_delay_between_the_key_steps() {
        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_X),
            MacroStep::Delay(Duration::from_millis(30)),
            MacroStep::KeyUp(KeyCode::KEY_X),
        ];
        assert_eq!(single_held_key(&steps), None);
    }

    #[test]
    fn single_held_key_rejects_two_trailing_delays() {
        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_X),
            MacroStep::KeyUp(KeyCode::KEY_X),
            MacroStep::Delay(Duration::from_millis(30)),
            MacroStep::Delay(Duration::from_millis(30)),
        ];
        assert_eq!(single_held_key(&steps), None);
    }

    #[test]
    fn single_held_key_rejects_modifiers_out_of_keypress_order() {
        // shift-down before ctrl-down — `keypress_steps` always emits ctrl first.
        let steps = vec![
            MacroStep::KeyDown(SHIFT),
            MacroStep::KeyDown(CTRL),
            MacroStep::KeyDown(KeyCode::KEY_X),
            MacroStep::KeyUp(KeyCode::KEY_X),
            MacroStep::KeyUp(CTRL),
            MacroStep::KeyUp(SHIFT),
        ];
        assert_eq!(single_held_key(&steps), None);
    }

    #[test]
    fn single_held_key_accepts_modifiers_in_keypress_order() {
        let steps = vec![
            MacroStep::KeyDown(CTRL),
            MacroStep::KeyDown(ALT),
            MacroStep::KeyDown(SUPER),
            MacroStep::KeyDown(KeyCode::KEY_X),
            MacroStep::KeyUp(KeyCode::KEY_X),
            MacroStep::KeyUp(SUPER),
            MacroStep::KeyUp(ALT),
            MacroStep::KeyUp(CTRL),
        ];
        assert_eq!(
            single_held_key(&steps),
            Some((
                Modifiers {
                    ctrl: true,
                    alt: true,
                    super_key: true,
                    ..Modifiers::default()
                },
                KeyCode::KEY_X
            ))
        );
    }

    #[test]
    fn single_held_key_rejects_more_than_one_non_modifier_key() {
        let steps = vec![
            MacroStep::KeyDown(KeyCode::KEY_X),
            MacroStep::KeyDown(KeyCode::KEY_Y),
            MacroStep::KeyUp(KeyCode::KEY_Y),
            MacroStep::KeyUp(KeyCode::KEY_X),
        ];
        assert_eq!(single_held_key(&steps), None);
    }

    #[test]
    fn single_held_key_rejects_modifiers_not_released_in_reverse() {
        let steps = vec![
            MacroStep::KeyDown(CTRL),
            MacroStep::KeyDown(SHIFT),
            MacroStep::KeyDown(KeyCode::KEY_X),
            MacroStep::KeyUp(KeyCode::KEY_X),
            MacroStep::KeyUp(CTRL),
            MacroStep::KeyUp(SHIFT),
        ];
        assert_eq!(single_held_key(&steps), None);
    }

    #[test]
    fn single_held_key_rejects_unbalanced_and_empty_shapes() {
        assert_eq!(single_held_key(&[]), None);
        assert_eq!(single_held_key(&[MacroStep::KeyDown(KeyCode::KEY_X)]), None);
        assert_eq!(
            single_held_key(&[
                MacroStep::KeyDown(KeyCode::KEY_X),
                MacroStep::KeyDown(KeyCode::KEY_X),
            ]),
            None,
            "two KeyDowns is not a balanced hold"
        );
        assert_eq!(
            single_held_key(&[MacroStep::Delay(Duration::from_millis(5))]),
            None
        );
    }

    #[test]
    fn fire_once_key_steps_splices_one_dwell_between_the_edges() {
        // Plain unmodified key: exactly `[KeyDown(k), Delay, KeyUp(k)]`.
        assert_eq!(
            fire_once_key_steps(Modifiers::default(), KeyCode::KEY_X),
            vec![
                MacroStep::KeyDown(KeyCode::KEY_X),
                MacroStep::Delay(FIRE_ONCE_KEY_DWELL),
                MacroStep::KeyUp(KeyCode::KEY_X),
            ]
        );

        // Modifier-wrapped: the dwell sits between the *base key's* edges,
        // the modifiers still wrap the whole thing and release in reverse.
        assert_eq!(
            fire_once_key_steps(
                Modifiers {
                    ctrl: true,
                    shift: true,
                    ..Modifiers::default()
                },
                KeyCode::KEY_X,
            ),
            vec![
                MacroStep::KeyDown(CTRL),
                MacroStep::KeyDown(SHIFT),
                MacroStep::KeyDown(KeyCode::KEY_X),
                MacroStep::Delay(FIRE_ONCE_KEY_DWELL),
                MacroStep::KeyUp(KeyCode::KEY_X),
                MacroStep::KeyUp(SHIFT),
                MacroStep::KeyUp(CTRL),
            ]
        );
    }

    #[test]
    fn fire_once_key_steps_output_is_not_re_classified_as_a_single_held_key() {
        // Ticket 12 §4: the spliced `Delay` sits *between* the key edges, so
        // `single_held_key` rejects the spliced form — a re-classification
        // must never feed it back onto the `value=2` autorepeat path. The
        // classifier only ever runs on the pre-splice steps.
        for mods in [
            Modifiers::default(),
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        ] {
            let pre = keypress_steps(mods, KeyCode::KEY_X);
            assert_eq!(single_held_key(&pre), Some((mods, KeyCode::KEY_X)));
            let spliced = fire_once_key_steps(mods, KeyCode::KEY_X);
            assert_eq!(single_held_key(&spliced), None);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fire_once_key_dwell_actually_elapses_before_the_up_write() {
        // The dwell must be a genuine blocking sleep, not just a step in the
        // compiled sequence — the Up write must not reach the sink until the
        // full `FIRE_ONCE_KEY_DWELL` has elapsed.
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());

        tokio::task::yield_now().await;
        spawn_fire_once(
            inj.clone(),
            fire_once_key_steps(Modifiers::default(), KeyCode::KEY_X),
        );
        tokio::task::yield_now().await;

        assert_eq!(sink.batches().len(), 1, "the Down fires immediately");

        tokio::time::advance(FIRE_ONCE_KEY_DWELL - Duration::from_millis(1)).await;
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
        assert_eq!(batches.len(), 2, "the Up fires once the dwell elapses");
        assert_eq!(key_and_value(batches[0][0]), (KeyCode::KEY_X, 1));
        assert_eq!(key_and_value(batches[1][0]), (KeyCode::KEY_X, 0));
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

    /// Ticket 05 / spec-kernel-shaped-repeat.md §5.2: a single-key Toggle
    /// holds a genuine kernel autorepeat — `value=1` on press, the first
    /// `value=2` a full `delay_ms` later, then `value=2` every `period_ms`,
    /// `value=0` on `stop()`.
    #[tokio::test(start_paused = true)]
    async fn toggle_autorepeat_holds_the_full_kernel_envelope_then_releases() {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let schedule = RepeatSchedule::new(250, 33);

        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn_autorepeat(
            inj.clone(),
            Modifiers::default(),
            KeyCode::KEY_A,
            schedule,
        );
        tokio::task::yield_now().await;

        // The press: exactly one value=1, nothing else yet.
        assert_eq!(sink.batches().len(), 1, "one value=1 on the first press");
        assert_eq!(key_and_value(sink.batches()[0][0]), (KeyCode::KEY_A, 1));

        // Just before the full delay: still no autorepeat.
        tokio::time::advance(Duration::from_millis(249)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            sink.batches().len(),
            1,
            "no value=2 before the full REP_DELAY elapses"
        );

        // Crossing the delay: the first value=2.
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(sink.batches().len(), 2, "first value=2 at the full delay");
        assert_eq!(key_and_value(sink.batches()[1][0]), (KeyCode::KEY_A, 2));

        // One period later: the second value=2.
        tokio::time::advance(Duration::from_millis(33)).await;
        tokio::task::yield_now().await;
        assert_eq!(sink.batches().len(), 3, "second value=2 one period later");
        assert_eq!(key_and_value(sink.batches()[2][0]), (KeyCode::KEY_A, 2));

        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let batches = sink.batches();
        assert_eq!(
            key_and_value(*batches.last().unwrap().last().unwrap()),
            (KeyCode::KEY_A, 0),
            "stop() releases the held key with value=0"
        );
    }

    /// Ticket 05 / §5.4: a stall past several due times emits exactly one
    /// `value=2` on resume and re-bases — no catch-up burst (reuses
    /// `RepeatSchedule::advance_fired`, same as surface 2).
    #[tokio::test(start_paused = true)]
    async fn toggle_autorepeat_does_not_burst_after_a_long_stall() {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let schedule = RepeatSchedule::new(250, 33);

        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn_autorepeat(
            inj.clone(),
            Modifiers::default(),
            KeyCode::KEY_A,
            schedule,
        );
        tokio::task::yield_now().await;

        // Jump 5s in one go — ~144 repeats' worth of due times slipped past.
        tokio::time::advance(Duration::from_millis(5_000)).await;
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        let after_stall = sink.batches().len();
        assert_eq!(
            after_stall, 2,
            "one value=1 + exactly one value=2 on resume, not a burst of the missed repeats"
        );
        assert_eq!(key_and_value(sink.batches()[1][0]), (KeyCode::KEY_A, 2));

        // Thereafter it re-bases to a steady one-per-period cadence — one
        // more value=2 per period elapsed, never a catch-up burst.
        tokio::time::advance(Duration::from_millis(33)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            sink.batches().len(),
            3,
            "one value=2 per period after re-basing"
        );
        tokio::time::advance(Duration::from_millis(33)).await;
        tokio::task::yield_now().await;
        assert_eq!(sink.batches().len(), 4);

        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();
    }

    /// Ticket 05 / §3.1: the modifier-wrapped variant holds the modifier
    /// `value=1` alongside the base key, autorepeats only the base key, and
    /// releases the modifier with the key on `stop()`.
    #[tokio::test(start_paused = true)]
    async fn toggle_autorepeat_holds_the_modifier_and_repeats_only_the_base_key() {
        let sink = RecordingSink::new();
        let (inj, inj_handle) = injector::spawn(sink.clone(), sink.clone());
        let schedule = RepeatSchedule::new(250, 33);
        let mods = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };

        tokio::task::yield_now().await;
        let toggle = ActiveToggle::spawn_autorepeat(inj.clone(), mods, KeyCode::KEY_X, schedule);
        tokio::task::yield_now().await;

        assert_eq!(
            sink.batches()
                .iter()
                .map(|b| key_and_value(b[0]))
                .collect::<Vec<_>>(),
            vec![(KeyCode::KEY_LEFTCTRL, 1), (KeyCode::KEY_X, 1)],
            "modifier held value=1, then the base key value=1"
        );

        // Two autorepeat ticks — only the base key, never the modifier.
        tokio::time::advance(Duration::from_millis(250)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(33)).await;
        tokio::task::yield_now().await;
        let repeats: Vec<_> = sink.batches()[2..]
            .iter()
            .map(|b| key_and_value(b[0]))
            .collect();
        assert_eq!(repeats, vec![(KeyCode::KEY_X, 2), (KeyCode::KEY_X, 2)]);

        toggle.stop().await;
        drop(inj);
        inj_handle.await.unwrap().unwrap();

        let released: std::collections::HashSet<_> = sink
            .batches()
            .iter()
            .filter(|b| key_and_value(b[0]).1 == 0)
            .map(|b| key_and_value(b[0]).0)
            .collect();
        assert!(
            released.contains(&KeyCode::KEY_X) && released.contains(&KeyCode::KEY_LEFTCTRL),
            "both the base key and its modifier are released on stop: {released:?}"
        );
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
