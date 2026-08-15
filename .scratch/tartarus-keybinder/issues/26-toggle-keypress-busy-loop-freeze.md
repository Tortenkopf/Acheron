# 26 — Toggle trigger mode busy-loops when the Action is a plain Keypress

**What to build:** `TriggerMode::Toggle` must never loop its compiled steps with zero pacing. A live-hardware incident (2026-08-15) showed that binding a button to `trigger = "toggle"` with `action.type = "keypress"` (no Macro, no explicit `Delay` step) causes `run_toggle_loop` (`daemon/src/executor.rs`) to hammer `uinput` with a synthetic KeyDown/KeyUp flood at effectively unbounded speed — freezing the focused application, then the whole system's input handling, hard enough to require a hard power cycle to regain control. Every Toggle lap needs a floor pacing regardless of what its compiled `Action` contains.

Toggle+Keypress itself is not the problem and must keep working exactly as it does today — it's the intended, quick way for a user to get "hold this key down until I toggle it off" without hand-building a Macro just to add a no-op `Delay` step. The fix is purely a safety-net floor on `run_toggle_loop` itself, small enough to be unnoticeable for any Macro that already paces itself with real `Delay` steps, and it applies uniformly to every Toggle regardless of whether it came from a Keypress or a Macro — a Toggle Macro of a single `KeyDown` with no `Delay` at all would hit the exact same busy-loop failure today, so a fix scoped to Keypress specifically would leave that case just as dangerous.

**Status:** resolved

- [x] `run_toggle_loop` enforces a minimum inter-lap delay, applied unconditionally after every lap regardless of the compiled steps' own `Delay` total — covers a bare `Action::Keypress` (`keypress_steps` never emits a `Delay`) and an under-paced or delay-free `Action::Macro` alike.
- [x] The floor must not break cancellation responsiveness — `ActiveToggle::stop()` should still return promptly (bounded by the floor, not by a full lap) even mid-pause.
- [x] Regression test: a Toggle compiled from a plain `Action::Keypress` (assert its compiled steps contain no `MacroStep::Delay`) is paced by the floor, not left to run at whatever rate the injector channel allows.
- [x] Regression test: a Toggle Macro with a single delay-free `KeyDown` step hits the same floor — confirms the fix isn't accidentally scoped to Keypress only.
- [x] Existing Toggle/executor tests (`daemon/src/executor.rs`, `daemon/src/dispatch.rs`) stay green — in particular the ones already using tight explicit delays (e.g. 10ms) close to whatever floor value is chosen.

**Resolved 2026-08-15.** Implemented exactly the drafted fix: `MIN_TOGGLE_LAP = Duration::from_millis(20)` in `daemon/src/executor.rs`, measured from `lap_start` and slept (via `tokio::select!` against the cancellation token, so `stop()` still returns promptly) only for the shortfall. Two new regression tests added — one driving a compiled `Action::Keypress` (asserting zero `MacroStep::Delay` in its compiled steps) and one driving a hand-built single-`KeyDown` Toggle Macro — both confirming laps are paced one-per-floor-tick under a paused `tokio` clock rather than free-running. Full `cargo test` (108 tests), `cargo clippy --all-targets`, and `cargo check --all-targets` all clean; the existing `toggle_loops_the_steps_until_stopped` test (two explicit 10ms delays, 20ms/lap) was unperturbed as predicted. `/code-review` returned no findings.

Not yet done: the live daemon rebuild + systemd restart this ticket's comments flagged as still outstanding (the running binary is still unpatched; the live D-Bus binding-clear is the only thing currently protecting the machine). That's an operational follow-up, not a code change.

## Comments

**2026-08-15 — filed after a live-hardware incident.** User's `Gaming` profile had `grid_r1c3` (button 3) bound to `trigger = "toggle"`, `action = { type = "keypress", key = "KEY_C" }`. Pressing it froze the focused text editor immediately; the editor eventually crashed, and all system input (including a terminal) stayed unresponsive until the machine was power-cycled — no stop-press or shortcut reached the daemon in time to help, because the physical repro itself made the whole desktop unresponsive.

Root cause, read directly from the running code (`daemon/src/executor.rs`):

- `Action::Keypress` compiles via `keypress_steps` (`executor.rs:55`) to exactly `[KeyDown(key), KeyUp(key)]` (plus modifier steps if any are set) — **never** a `MacroStep::Delay`.
- `run_toggle_loop` (`executor.rs:141`) only ever sleeps when a compiled step is itself `MacroStep::Delay`. For a Toggle+Keypress binding, that means the `'running: loop` body has no `await` point that yields real wall-clock time beyond the injector channel's own round-trip — a busy loop writing to `uinput` as fast as the async runtime and the device allow.
- This is a genuine gap in ticket 17's original design/testing: the ticket's live-hardware demo (`.scratch/tartarus-keybinder/issues/17-full-action-trigger-mode-support.md`, "Live-hardware demo done 2026-08-15") only ever exercised Toggle with a **Macro** that had an explicit `Delay` step (`KeyDown A`, 700ms delay, `KeyUp A`, 700ms delay). Toggle wrapping a bare Keypress was never tried before now.

**Immediate live mitigation already applied (no code change):** `busctl --user call com.acheron.Daemon /com/acheron/Daemon com.acheron.Daemon ClearBinding ss "grid_r1c3" "base"` — cleared the dangerous binding in the running daemon and persisted the clear to `config.toml`. Button 3 is passthrough/unbound again; the machine is not currently at risk from this profile. This does **not** fix the underlying bug — any future Toggle+Keypress binding (or, more generally, any Toggle whose compiled steps sum to zero delay) will reproduce it.

**Suggested fix (drafted, not applied — pending this ticket):** give `run_toggle_loop` an unconditional floor between laps, measured from the start of each lap so it only adds sleep when the lap's own steps didn't already take long enough:

```rust
/// Floor pacing between Toggle laps, independent of the compiled steps' own
/// `Delay` total. Found live (2026-08-15): a Toggle wrapping a plain
/// `Action::Keypress` compiles (`keypress_steps`) to `[KeyDown, KeyUp]` with
/// no `Delay` step at all, so without this floor a lap ran as fast as the
/// injector channel + `uinput` write allowed — an unbounded flood of
/// synthetic keystrokes that froze the focused app and then the whole input
/// pipeline, hard enough to require a power cycle. Every lap, however it's
/// compiled, now takes at least this long.
const MIN_TOGGLE_LAP: Duration = Duration::from_millis(20);

async fn run_toggle_loop(injector: Injector, steps: Vec<MacroStep>, cancel: CancellationToken) {
    let mut held: HashSet<KeyCode> = HashSet::new();
    'running: loop {
        if steps.is_empty() {
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
                return;
            }
        }
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
```

20ms was chosen (by hand-tracing, not yet run) so it doesn't perturb the existing `toggle_loops_the_steps_until_stopped` test — its steps already total exactly 20ms/lap via two explicit 10ms `Delay`s, so `elapsed < MIN_TOGGLE_LAP` is false there and no extra sleep gets added — and so it stays well under the threshold of perceptible lag for any real Macro's own pacing. The other Toggle tests in `executor.rs`/`dispatch.rs` all stop mid-lap (before a full lap's `for` loop completes), so they never reach the new floor check at all. Whoever picks this up should still run the full `cargo test` to confirm that holds. The daemon itself is currently running unpatched (systemd service still has the old binary in memory; only the live D-Bus binding-clear above is protecting it), so this needs an actual rebuild+restart once implemented, not just a source change.
