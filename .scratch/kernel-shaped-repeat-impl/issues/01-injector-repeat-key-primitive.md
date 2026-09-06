# 01 — Injector `repeat_key` primitive

**What to build:** The one new output capability every other ticket in this effort
builds on — a way to emit a single Linux autorepeat event (`value=2`) for a key,
through the same one channel / one task / one fd every other write already goes
through. Nothing in production calls it yet after this ticket; it is exercised only
by its own unit tests. A caller (added in tickets 04–06) will drive it from the code
that already decides when a repeat is due.

Source of truth: [`spec-kernel-shaped-repeat.md`](../../humane-output-rate/spec-kernel-shaped-repeat.md)
§2.1 (the `InjectorMessage::RepeatKey` / `Injector::repeat_key` rows) and §5.1
(timestamp).

**Blocked by:** None — can start immediately.

**Status:** done

- [x] `InjectorMessage::RepeatKey(KeyCode)` added to the enum, and `Injector::repeat_key(&self, KeyCode)`
      added beside `force_release_key` — same `tx.send(...).await.map_err(|_| InjectorClosed)`
      shape, fire-and-forget (no reply channel, like `Physical` / `AxisValue`).
- [x] The `injector_loop` handler arm emits `KeyEvent::new(key, 2)` via
      `sink_for(&mut sink, &mut gamepad_sink, key)` — so a keyboard code lands on the
      keyboard/mouse device and a gamepad code on the gamepad device, exactly like
      `KeyState` / `ForceRelease`.
- [x] **Subject to `suppressed`** — unlike `ForceRelease`. While suppression is on,
      `repeat_key` emits nothing; when it turns back off, subsequent `repeat_key`
      calls emit again. (§2.1: "a suppressed hold emits nothing and resumes on
      unsuppress".) `repeat_key` touches no `held` set — `value=2` events are
      stateless (§7).
- [x] No explicit `input_event` timestamp is set — `evdev`'s `.emit()` default is
      used, so the kernel stamps at handling time with its monotonic clock, matching
      real `input_repeat_key` (§5.1). Add a brief comment saying so and why (a
      userspace timestamp would put any regularity into a field a detector reads).
- [x] `.emit()` frames the event with its own `SYN_REPORT` (one frame per call),
      same as `set_key_state`.
- [x] Unit tests in `injector::tests`, mirroring the existing `set_key_state` /
      `force_release` / `set_axis_value` tests:
  - one call → one batch of one `KeyEvent` with `value == 2`;
  - a keyboard code never reaches the gamepad sink and vice versa;
  - a `repeat_key` issued while `set_suppressed(true)` produces no batch, and one
    issued after `set_suppressed(false)` does.
- [x] `cargo fmt --check`, `cargo clippy --all-targets`, and the daemon test suite
      all clean.
