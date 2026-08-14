# 13 — Daemon skeleton: capture → passthrough injection

**What to build:** The foundational Daemon process and event loop — grabs the Tartarus Pro's three evdev nodes exclusively and re-emits every physical input unchanged via a `uinput` virtual device, so the device behaves exactly like stock hardware while the Daemon owns it. No config, Profiles, or Bindings yet — this ticket proves and builds the mechanism everything else runs on. See `.scratch/tartarus-keybinder/spec.md` ("Daemon event loop and concurrency") for the full design.

**Blocked by:** None — can start immediately

**Status:** resolved

- [x] One `tokio` runtime hosts the whole Daemon process.
- [x] A `CaptureSource` trait is defined whose contract is "produces a stream of normalized `PhysicalEvent { input: Input, state: Down | Repeat | Up }` into a shared channel" — nothing downstream of the channel knows or cares which implementation produced an event.
- [x] A real evdev `CaptureSource` implementation grabs all three nodes (`main`, `if01`, `if02`) exclusively (`EVIOCGRAB`) via `spawn_blocking` background tasks, using the Input↔(node, evdev code) table already captured in `.scratch/tartarus-keybinder/issues/01-enumerate-physical-inputs.md`, and normalizes evdev's raw `EV_KEY` value (1/2/0) onto Down/Repeat/Up.
- [x] A fake/scripted `CaptureSource` implementation exists for tests — feeds a synthetic sequence of `PhysicalEvent`s into the same channel type, with no real device involved.
- [x] A single injector task owns one `uinput` virtual device, created once at Daemon startup and held for the process lifetime; all output writes go through this one task via a channel (not directly to the fd).
- [x] With no config/Bindings in play, every captured `PhysicalEvent` results in the injector re-emitting the identical input unchanged (pure passthrough) — verified against the exclusive-grab-and-relay mechanism already proven in `.scratch/tartarus-keybinder/issues/02-prove-evdev-uinput-pipeline.md`.
- [x] Live-hardware demo: with the Daemon running, the physical Tartarus Pro produces identical output to plugging it in with no Daemon running at all (grid keys, Mode key, thumbstick, wheel scroll/middle-click all pass through).
- [x] Automated tests exercise the dispatch/injection path via the fake `CaptureSource`, asserting on injected output, not on private struct fields.

## Comments

Implemented in `daemon/` (new Cargo binary+lib crate `acheron-daemon`, Rust edition 2024, `evdev = "0.13"` + `tokio` with `rt-multi-thread`/`macros`/`sync`).

- `src/input.rs` — the `Input` domain enum (`ModeKey`/`Grid(row,col)`/`Thumbstick(Direction)`/`Wheel(WheelEvent)`) plus the bidirectional Input↔evdev-KeyCode table from issue 01 (`input_for_key`, `key_code_for_input`). No `Display`/`FromStr` yet — that's ticket 14's concern (TOML/wire strings), not needed for pure passthrough.
- `src/capture/` — `CaptureSource` trait (native `async fn` in trait, `-> impl Future<..> + Send` for dyn-free Send futures); `evdev_source::EvdevCaptureSource` spawns one `spawn_blocking` task per node (`Node::ALL`), grabs each via `Device::grab()`, and normalizes `EV_KEY` values and `REL_WHEEL` ticks (wheel scroll has no natural release, so it normalizes to a single `Down`, matching how `FireOnce` will later treat it); `fake::FakeCaptureSource` replays a scripted `Vec<PhysicalEvent>`.
- `src/injector.rs` — `InjectSink` trait mirroring `VirtualDevice::emit`'s signature, so tests substitute a `RecordingSink` instead of needing real `uinput` access; `build_device()` declares every key/button an `Input` can inject plus `REL_WHEEL`/`REL_WHEEL_HI_RES`; the injector task owns the sink and is the sole writer.
- `src/dispatch.rs` — pure passthrough for this ticket: forwards every `PhysicalEvent` from the capture channel straight to the injector, unchanged. Binding lookup/Trigger-mode logic is explicitly deferred to ticket 14+.
- Capture-failure handling matches issue 07's *original* (pre-ticket-10-correction) "any capture failure is fatal" call — the ~2s device-absent poll loop from ticket 10's correction is out of scope here and left for whichever later ticket owns Daemon/device status (issue 12/20 territory), since ticket 13 has no config/D-Bus/status surface to report through yet.
- Tests: 10 unit/integration tests (`cargo test`), all via the fake `CaptureSource` + `RecordingSink`, asserting on injected evdev event batches (code/value/axis), not private struct fields. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` both clean.
- Live-hardware demo done 2026-08-14: Daemon run as `target/release/acheron-daemon` against the real, connected Tartarus Pro — grabbed all three real nodes, virtual device `Acheron Virtual Tartarus Pro` appeared in `/proc/bus/input/devices`, user confirmed grid/thumbstick output in a real text field matched stock (no-Daemon) behavior.
- `/code-review` caught three real issues, all fixed: (1) `main.rs` discarded the injector/dispatch `JoinHandle`s, so a genuine `uinput` write failure or a task panic would leave the daemon silently inert instead of fatal-exiting per issue 07/10 — `main` now `select!`s across capture/injector/dispatch, logs to stderr, and exits non-zero on any of them ending; (2) `dispatch::run` now propagates an injector-channel-closed failure as `io::Result<()>` instead of swallowing it via a bare `break`; (3) `key_code_for_input` did unchecked `usize - 1` on `Grid(row, col)`, panicking on `row`/`col` `== 0` — switched to `checked_sub` returning `None`, since `Input::Grid` is a public constructor future config-parsing tickets will feed untrusted values into.
