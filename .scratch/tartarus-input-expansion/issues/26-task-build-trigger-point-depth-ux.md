Type: task

## Question

Land [ticket 19](./19-prototype-trigger-point-ux-and-live-depth.md)'s settled design for
real, against the running Daemon and the live Tartarus Pro — variant B from the prototype
(see that ticket's Answer for the full settled shape: two-marker/percent actuation bar,
live analog/digital badge, greyed-bar-with-overlaid-warning digital fallback, colored
green/amber legend), not the throwaway `prototype/19-trigger-point-depth-ux` branch's code,
which stays out of `main`.

Scope:

- **Daemon D-Bus**: add `StartDepthStream(input)` / `StopDepthStream(input)` (scoped to the
  requesting client's own bus connection, auto-stopped on disconnect) and a
  `DepthChanged(input, depth)` signal, rate-limited to ~30Hz, independent of
  `StopAllToggles`/`SetOutputSuppressed`'s lifecycle. Wire it to the real depth values the
  `AnalogCaptureSource` (ticket 22) already produces.
- **`GetConfig()`'s wire dict**: serialize `default_actuation`/`actuation_overrides` — still
  missing since [ticket 21](./21-task-apply-analog-data-model-to-code.md) deliberately
  deferred it.
- **`binding_editor.py`**: the real Actuation & release section per ticket 19's Answer,
  calling the existing `SetActuationPoint`/`ClearActuationPoint`/`SetDefaultActuation`/
  `ResetActuationPoints`/`SetForceDigital` D-Bus methods (already built, ticket 17/21) and
  the new depth-stream pair above. `daemon_client.py`/`daemon_stub.py` both need the new
  calls (mirroring every other D-Bus surface's real/stub pair).
- Live-hardware verification, per this map's standing execution discipline: real depth
  driving the bar, real actuation/release persistence, the digital-mode fallback actually
  reachable (e.g. via `SetForceDigital`), and the live capture-mode badge flipping on a
  real mode change.

HITL — needs the real, connected Tartarus Pro, same as tickets 22/23/24.
