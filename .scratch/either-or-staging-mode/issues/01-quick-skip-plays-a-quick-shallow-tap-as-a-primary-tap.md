# 01: Quick-Skip plays a quick shallow tap as a primary tap

**What to build:** A Quick-Skip dual-stage key that is pressed past its primary
Actuation point and released inside the ~50ms window, without ever reaching the deep
band, currently emits nothing: the buffered primary Down is dropped. It should instead
fire the primary as a tap. The primary's Down is performed when the release arrives,
and its Up follows one canned-tap dwell (the Physical-plausibility ceiling's ~40ms
Fire-once dwell) later. The Up has real-Up semantics, not a force-release: the tap
behaves exactly as a physical tap on a single-stage key with the same Binding would,
just shifted in time. Source of truth:
[`spec.md`](../spec.md) §"Early-Up flush" and Decisions Q3/Q7.

The delayed Up is a timed event owned by the stage engine, riding the same
deadline/tick mechanism the Quick-Skip window already uses. It is not a sleep in the
dispatch loop.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [x] A quick shallow tap on a Quick-Skip key emits the primary's Down on release and its Up ~40ms later (paused-time dispatch test)
- [x] The existing test asserting "early Up cancels with nothing emitted" is replaced by the flush behaviour, not left alongside it
- [x] Per Trigger mode: Fire-once fires once; Toggle starts and keeps running after the tap; Hold-to-repeat and mouse-button primaries are held ~40ms then released; a Controller-button primary emits Down, ~40ms, Up on the gamepad device; a ProfileSwitch primary switches Profile
- [x] Pressing the key again before the pending Up lands performs that Up immediately, then begins the new press normally
- [x] A Layer/Profile switch or capture-mode flip with an Up pending cancels it and leaves the primary released (nothing stuck down)
- [x] A Layer/Profile switch or capture-mode flip while still *Armed* still cancels silently: nothing fires
- [x] Skipped and Late behaviour are unchanged (existing Quick-Skip tests stay green)
- [ ] Verified on hardware: a quick tap on a Quick-Skip key produces the primary

## Comments

Implemented in `daemon/src/stage.rs` (new `StageOp::FlushPrimary`, emitted by the
Armed early-Up row; `KeyRuntime::pending_release` rides `Engine::next_deadline` /
`tick`) with dispatch coverage in `daemon/src/dispatch.rs`. Two implementation calls
worth knowing:

- The flush fires with the user-initiated `PerformDeps::new`, not
  `new_machine_sequenced` like `tick`'s `RepressPrimary`, so a Fire-once primary keeps
  its own 40 ms dwell.
- A Fire-once primary's deferred Up is still scheduled, since it marks that the tap owns
  its release so the real Up edge won't force-release it mid-dwell. When it comes due it
  performs nothing: the dwell's own `KeyUp` is the tap's Up, and a `ForceReleaseStuck`
  at the same instant emitted the Up twice.

Hardware verification is still pending.
