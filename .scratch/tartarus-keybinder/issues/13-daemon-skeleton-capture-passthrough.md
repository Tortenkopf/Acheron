# 13 — Daemon skeleton: capture → passthrough injection

**What to build:** The foundational Daemon process and event loop — grabs the Tartarus Pro's three evdev nodes exclusively and re-emits every physical input unchanged via a `uinput` virtual device, so the device behaves exactly like stock hardware while the Daemon owns it. No config, Profiles, or Bindings yet — this ticket proves and builds the mechanism everything else runs on. See `.scratch/tartarus-keybinder/spec.md` ("Daemon event loop and concurrency") for the full design.

**Blocked by:** None — can start immediately

**Status:** ready-for-agent

- [ ] One `tokio` runtime hosts the whole Daemon process.
- [ ] A `CaptureSource` trait is defined whose contract is "produces a stream of normalized `PhysicalEvent { input: Input, state: Down | Repeat | Up }` into a shared channel" — nothing downstream of the channel knows or cares which implementation produced an event.
- [ ] A real evdev `CaptureSource` implementation grabs all three nodes (`main`, `if01`, `if02`) exclusively (`EVIOCGRAB`) via `spawn_blocking` background tasks, using the Input↔(node, evdev code) table already captured in `.scratch/tartarus-keybinder/issues/01-enumerate-physical-inputs.md`, and normalizes evdev's raw `EV_KEY` value (1/2/0) onto Down/Repeat/Up.
- [ ] A fake/scripted `CaptureSource` implementation exists for tests — feeds a synthetic sequence of `PhysicalEvent`s into the same channel type, with no real device involved.
- [ ] A single injector task owns one `uinput` virtual device, created once at Daemon startup and held for the process lifetime; all output writes go through this one task via a channel (not directly to the fd).
- [ ] With no config/Bindings in play, every captured `PhysicalEvent` results in the injector re-emitting the identical input unchanged (pure passthrough) — verified against the exclusive-grab-and-relay mechanism already proven in `.scratch/tartarus-keybinder/issues/02-prove-evdev-uinput-pipeline.md`.
- [ ] Live-hardware demo: with the Daemon running, the physical Tartarus Pro produces identical output to plugging it in with no Daemon running at all (grid keys, Mode key, thumbstick, wheel scroll/middle-click all pass through).
- [ ] Automated tests exercise the dispatch/injection path via the fake `CaptureSource`, asserting on injected output, not on private struct fields.
