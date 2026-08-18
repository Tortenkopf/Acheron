Type: task
Status: resolved

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

## Answer

Built end to end; all four scope items landed, tests green, HITL verification deferred (see
below, same pattern as ticket 22 → ticket 24).

**Daemon D-Bus** (`daemon/src/dbus/mod.rs`): `StartDepthStream(input)`/`StopDepthStream(input)`
plus a `DepthChanged(input, depth)` signal at ~30Hz (`DEPTH_STREAM_INTERVAL`). Modeled as a
single current stream target with an epoch/disconnect-watcher, mirroring `SetOutputSuppressed`'s
shape exactly rather than a per-connection subscriber set — last-write-wins, auto-stopped on
disconnect, Config-free and bypassing dispatch entirely (same bypass `SetOutputSuppressed`
already uses). `capture::analog::relay_grid_blocking` gained a `depth_tx: watch::Sender<HashMap
<Input, u8>>` publishing all 20 keys' current depth on every incoming report (not just
Down/Up/Repeat transitions, since the live bar needs travel between the release and actuation
points too); threaded through `grid_task_blocking`/`AnalogCaptureSource`/`supervisor::run`/
`main.rs` alongside the existing `actuation_tx`/`actuation_rx` pair, in the opposite direction.
`examples/analog_probe.rs` updated for the new constructor parameter (receiver dropped, no
consumer needed there). One real bug caught by its own test: `tokio::time::interval`'s first
tick resolves immediately rather than after one interval, which let a superseded
`StartDepthStream` race one stray signal out before its abort took effect — fixed with
`interval_at`, first tick one full `DEPTH_STREAM_INTERVAL` out.

**`GetConfig()`'s wire dict** (`daemon/src/dbus/wire.rs`): `profile_to_dict` now serializes
`default_actuation`/`actuation_overrides` (flat `{"actuation": u8, "release": u8}` dicts,
Input-keyed for the overrides map) — the gap ticket 21 deliberately deferred.

**`binding_editor.py`**: a new Actuation & release section, shown only for Grid Inputs
(`inputs.is_grid_input`), porting ticket 19's prototype variant B's `DepthTrack` widget
(plain `Gtk.Box`es, not `Gtk.DrawingArea` — this environment's pycairo still has no
`gi._gi_cairo` bridge) almost unchanged, plus a real `on_drag_end` callback the prototype had
no persist path to call. Wired to all five existing Set/Clear/Reset D-Bus methods
(`SetActuationPoint` on drag-end, `ClearActuationPoint` for "Reset to Profile default",
`SetDefaultActuation`/`ResetActuationPoints` for two smaller "push this as the new Profile
default" / "reset every key" affordances the map's Notes didn't otherwise have a home for, and
`SetForceDigital` via a checkbox — the standing "explicit user-facing override that forces
digital" the map's Notes call for). Depth streaming and the capture-mode badge needed two
different fixes for the same underlying hazard: `build_binding_editor` runs eagerly for *every*
Grid key on *every* app `rebuild()`, not lazily on popover open, so anything that subscribes to
a D-Bus signal at construction time leaks one registration per rebuild.
- **Depth** (~30Hz, must stay scoped to whichever one popover is actually open):
  `client.start_depth_stream(input, on_depth)`/`stop_depth_stream(input)`, called from the
  section's own `box`'s `map`/`unmap` signals — real per-popover start/stop — backed by a
  client-side single-current-target routing seam (`DBusDaemonClient._depth_callback`/
  `_depth_target`) that wires the underlying `"g-signal"` listener at most once, lazily, ever;
  `DaemonStub` mirrors it (`simulate_depth`).
- **Capture mode** (rare transitions, badge only needs its live value while a popover happens
  to be open): threaded as a plain parameter instead
  (`build_status_wrapped_view`/`build_main_view`/`make_input_button`/`build_binding_editor` all
  gained a `capture_mode` parameter), fed by a new `CaptureModeChanged` subscription at the
  *app* level (`app.py`'s `_wire_status_tracking`, alongside the existing daemon-running/
  device-connected signals) that drives the same full `rebuild()` every other live status
  transition there already does. Accepted tradeoff, not a new one: a mode flip while an editor
  is open closes it, exactly like every other status-driven rebuild already does today.

`daemon_client.py`/`daemon_stub.py` both gained the five actuation methods plus the depth pair
and `subscribe_capture_mode_changed`, mirroring every other D-Bus surface's real/stub pair.
`_connect_signal`'s callback invocation generalized from `(value,) = parameters.unpack();
callback(value)` to `callback(*parameters.unpack())` so the same helper covers `DepthChanged`'s
two args without a parallel implementation.

**Tests**: 176 Rust + 79 Python, all green (`cargo test`, `cargo clippy --all-targets`, `cargo
fmt --check` clean on every file this ticket touched — pre-existing formatting debt in
`executor.rs`, untouched by this ticket, was left alone rather than swept in by a blanket
`cargo fmt`). New coverage: four `StartDepthStream`/`StopDepthStream` D-Bus tests (push,
retarget, stop, non-Grid rejection), a `config_to_dict` actuation-serialization test, and eleven
`binding_editor.py` tests (section presence/absence, all five button/checkbox wirings, badge
per capture_mode, the construction-time no-leak invariant, `DepthTrack.set_live_value`
directly). Four pre-existing tests updated for the now-real seed/wire shape (`DaemonStub`'s
`default_actuation`/`actuation_overrides` seed, `_wire_status_tracking`'s `capture_mode` key,
a CSS-class collision between this section's own sub-heading and the main editor's heading, and
`test_running_connected_status_enables_the_grid_with_no_dim_overlay`'s over-broad `Gtk.Overlay`
check — now scoped to the `"dim-overlay"` CSS class specifically, since this section legitimately
builds its own `Gtk.Overlay`s).

**Not exercised this session**: forcing GTK's `"map"`/`"unmap"` signals on an unrealized widget
aborts the process outright in this headless test environment (confirmed live, mid-session) —
so the map/unmap-triggered `StartDepthStream`/`StopDepthStream` calls themselves, and the whole
live-hardware checklist ticket 19 asked for (real depth driving the bar, real persistence,
reaching the digital fallback via `SetForceDigital`, the badge flipping on a real capture-mode
transition), are unverified by this session. This machine does have a real Daemon running
against a real, connected Tartarus Pro (`systemctl --user status acheron-daemon`,
`/dev/hidraw0-5` present) — but it's the pre-ticket-26 binary, and swapping the user's live
input-device driver out from under them without asking first isn't a call this session should
make solo. Follow-up: [Verify the trigger-point UX and live-depth channel on hardware]
(./27-task-verify-trigger-point-depth-ux-on-hardware.md), same shape as ticket 22 → ticket 24.
