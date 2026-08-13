Type: grilling
Blocked by: 06
Status: resolved

## Question

Design the Daemon's main event loop: how it grabs the three evdev nodes (`main`/`if01`/`if02`, per the Input table in [Enumerate physical inputs](./01-enumerate-physical-inputs.md)), dispatches each event through the active Profile/Layer to resolve a Binding (using the data model from [Decide Daemon data model](./06-decide-daemon-data-model.md)), executes the resulting Action's Trigger mode (including Hold-to-repeat's continuous re-fire and Toggle's start/stop state per the decided toggle-across-switches rule) via `uinput` injection (mechanism proven in [Prove evdev/uinput pipeline](./02-prove-evdev-uinput-pipeline.md)), and handles concurrency (async runtime vs threads, how D-Bus requests from the GUI interleave with input events).

Per the map's standing architectural discipline, keep the capture step behind an internal abstraction so a second capture source (`hidraw` analog) could be added later without a rewrite — decide what that seam looks like concretely.

## Answer

Grilling session, 2026-08-13.

**Runtime** — one `tokio` runtime for the whole Daemon. Three background tasks (via `spawn_blocking`, one per evdev node: `main`/`if01`/`if02`) read blocking evdev events and normalize them into `PhysicalEvent { input: Input, state: Down | Repeat | Up }` — mapping evdev's raw `EV_KEY` `value` (1/2/0) onto the three-state model, using the Input→(node,code) table from [Enumerate physical inputs](./01-enumerate-physical-inputs.md) — and send them into one shared `mpsc` channel.

**Dispatch task** — the single consumer of that channel and sole owner of all mutable Daemon state (active Profile/Layer, the `ActiveToggle` map from [Decide Daemon data model](./06-decide-daemon-data-model.md)). No `Mutex` anywhere — state mutation is serialized by construction, not by locking. Down/Repeat/Up map onto Trigger mode as follows: `FireOnce` fires only on `Down`, ignoring `Repeat`/`Up`. `HoldToRepeat` fires on `Down` *and* every subsequent `Repeat` (each firing runs the full compiled step sequence independently) — this is the concrete mechanics of ticket 06's "driven by native evdev autorepeat" decision. `Toggle` starts/stops only on `Down`, ignoring `Repeat`/`Up` entirely (holding a toggle key down doesn't double-toggle).

**Capture seam** (the map's standing discipline) — a `CaptureSource` abstraction whose only external contract is "produces a stream of normalized `PhysicalEvent`s into the shared channel." The evdev implementation (the three per-node background tasks plus the Input↔(node,code) mapping) lives entirely behind it. A future `hidraw` analog source would be a second implementation feeding the same channel type; the dispatch task never knows or cares which mechanism produced an event.

**Firing execution model** — firing any Action spawns its own `tokio` task that walks the compiled `Vec<MacroStep>` (`tokio::time::sleep` between `Delay` steps, looping indefinitely for `Toggle`), so a Macro's delays never block the dispatch task from processing other events. Since multiple Toggles can run concurrently (different physical keys, per the `HashMap<Input, ActiveToggle>` model), all `uinput` writes are serialized through one dedicated **injector task** that owns the single virtual device (created once at Daemon startup, held for the process lifetime, matching the proven spike in [Prove evdev/uinput pipeline](./02-prove-evdev-uinput-pipeline.md)) — every firing-task sends write-commands to it over a channel rather than writing to the fd directly, preventing two concurrently-running macros from interleaving raw writes into the same `SYN_REPORT` batch. Each `ActiveToggle` carries a `CancellationToken` alongside the live-held-keys `HashSet<Key>` from ticket 06, so the stop-key mechanism (dispatch task, on seeing `Down` for an `Input` with an active toggle) cancels the spawned task and has it force-release exactly those held keys through the injector before exiting.

**D-Bus interleaving** — zbus's async server API runs on the Daemon's same `tokio` runtime (not a second one), and GUI-originated calls push a `Command` variant into the *same* `mpsc` channel the dispatch task already consumes alongside `PhysicalEvent`s. A Profile switch from the GUI and a physical keypress are just two message types serialized through the one state-owning consumer — no separate lock, no second copy of state.

**Failure handling** — any `CaptureSource` read failure (e.g. device unplugged mid-run) is treated as fatal; the Daemon exits rather than attempting an internal reconnect/retry loop, relying on systemd's restart-on-failure policy (deferred to [Decide systemd service packaging](./10-decide-systemd-service-packaging.md)). Reasoned scope call for a personally-used MVP on one fixed device — a reconnect loop is real complexity for a failure the user will notice immediately and can resolve by relaunching.

No new tickets surfaced. [Decide D-Bus interface surface](./08-decide-dbus-interface-surface.md) is now unblocked (its two blockers — this ticket and the data-model ticket — are both resolved).

## Correction (from [Decide systemd service packaging](./10-decide-systemd-service-packaging.md))

The "any `CaptureSource` read failure is fatal" rule above is too broad. Split by cause:

- **Device absent** — the Tartarus Pro's nodes don't exist, whether at Daemon startup (booted before the device was plugged in) or after a mid-run unplug — is **not** fatal. The `CaptureSource` polls for the known `/dev/input/by-id/...` paths every ~2s until they open cleanly, then resumes normal capture. One poll loop covers both the boot-before-plugin and unplug/replug cases. Reasoning: with systemd's `Restart=on-failure` + burst-limit policy (settled in ticket 10), treating device-absence as a process exit means the unit crash-loops and lands in `failed` after 5 attempts in 60s — the opposite of the desired "stay around, pick the device up once it's plugged in" behavior.
- **Genuine capture errors** (e.g. a `uinput` write failure, an unexpected fd error unrelated to the device being unplugged) — still fatal-exit, still deferred to systemd's `Restart=on-failure`, as originally decided. This class is a real bug, not an absent peripheral.
