// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use acheron_daemon::capture::supervisor;
use acheron_daemon::config;
use acheron_daemon::dbus::Daemon;
use acheron_daemon::{dispatch, injector};
use tokio::task::JoinError;

/// Loads `config.toml` (seeding it on first run, per issue 11), assembles
/// the capture -> dispatch -> injector pipeline (dispatch resolving each
/// event against the active Profile's active Layer, ticket 18), and exposes
/// the `com.acheron.Daemon` D-Bus surface on the session bus (ticket 15) —
/// GUI-originated calls reach the dispatch task as `Command`s over the same
/// channel `PhysicalEvent`s already flow through (issue 07's "D-Bus
/// interleaving"), so it stays the sole owner of `Config`.
///
/// Capture runs inside `supervisor::run` (ticket 23) rather than a bare
/// `EvdevCaptureSource::ALL.run(...)` call — it owns *which* `CaptureSource`
/// (`AnalogCaptureSource`/`EvdevCaptureSource`) is currently live and swaps
/// between them per `Config.force_digital`/a `SetForceDigital` call/a device
/// reconnect, per ticket 18 §6. Any of the three top-level tasks (capture,
/// injector, dispatch) finishing is still treated as fatal — per issue
/// 07/10, a genuine capture or injection error must exit the process so
/// systemd's `Restart=on-failure` can recover it, rather than leaving the
/// daemon silently inert. A clean SIGTERM/SIGINT instead relocks the device
/// (best-effort, harmless if it was never unlocked — ticket 16) and exits
/// directly, without waiting on the capture task's blocking I/O threads to
/// notice (ticket 24's tooling note: they have no shutdown signal of their
/// own reason to join on a real process exit).
#[tokio::main]
async fn main() -> io::Result<()> {
    let config_path = config::config_path();
    let config = config::load_or_seed(&config_path).unwrap_or_else(|err| {
        eprintln!(
            "acheron-daemon: refusing to start: {} ({err})",
            config_path.display()
        );
        std::process::exit(1);
    });

    // /dev/uinput's access comes from udev's `uaccess` tag, applied when the
    // login session becomes active — not ordered against
    // `graphical-session.target`, so the very first `acheron-daemon` start
    // of a fresh session reliably lost this race on real hardware (ticket
    // 28), turning `build_device()`'s `PermissionDenied` into a hard
    // `main()` exit before the capture layer's own, already-soft handling
    // of the *Tartarus's* permission gap ever got a chance to run.
    let device = injector::retry_on_permission_denied(
        "/dev/uinput",
        injector::build_device,
        Duration::from_millis(200),
        25,
    )
    .await?;
    // Ticket 14/43's second uinput device, for Action::ControllerButton —
    // opened right alongside the keyboard device so the same startup-race
    // retry (ticket 28) covers it too.
    let gamepad_device = injector::retry_on_permission_denied(
        "/dev/uinput (gamepad)",
        injector::build_gamepad_device,
        Duration::from_millis(200),
        25,
    )
    .await?;
    let (inj, inj_handle) = injector::spawn(device, gamepad_device);

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(256);
    let (connection_tx, connection_rx) = tokio::sync::mpsc::channel(8);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(256);
    // The Actuation-point snapshot seam (ticket 18 §5/ticket 22): dispatch
    // publishes into `actuation_tx` on every mutation that touches the
    // active Profile's Actuation points; the supervisor hands
    // `actuation_rx` to every `AnalogCaptureSource` attempt it spawns
    // (ticket 23).
    let (actuation_tx, actuation_rx) = tokio::sync::watch::channel(HashMap::new());
    // The live-depth seam (ticket 26): the Analog grid task publishes every
    // key's current depth into `depth_tx` on every incoming report; the
    // D-Bus layer holds `depth_rx` and throttles it down to `DepthChanged`'s
    // ~30Hz for whichever connection has an active `StartDepthStream`. Never
    // touches `Config` or dispatch, mirroring `SetOutputSuppressed`'s
    // Config-free bypass.
    let (depth_tx, depth_rx) = tokio::sync::watch::channel(HashMap::new());
    // The capture-mode seam (ticket 23): the supervisor pushes its current
    // `CaptureMode` on every transition; dispatch consumes it for
    // `GetState`/`CaptureModeChanged`.
    let (capture_mode_tx, capture_mode_rx) = tokio::sync::mpsc::channel(8);
    // The capture-control seam (ticket 23): dispatch forwards every
    // successful `SetForceDigital` value here so the supervisor can act on
    // it live, without dispatch knowing anything about `CaptureSource`s.
    let (capture_control_tx, capture_control_rx) = tokio::sync::mpsc::channel(8);
    // The device-info seam (ticket 101): the supervisor reads the connected
    // Tartarus Pro's firmware/serial over the Interface-2 control channel
    // once per connect and pushes `Some(info)` here (`None` on disconnect);
    // dispatch owns the value `GetState()`'s two optional keys report from.
    let (device_info_tx, device_info_rx) = tokio::sync::mpsc::channel(8);

    // Built before the dispatch task so a real `SignalEmitter` (ticket 18's
    // `ActiveLayerChanged`, pushed directly from the dispatch task on every
    // Mode-key transition) can be handed to it. Held for the process's
    // lifetime: dropping it would take the D-Bus surface down while the
    // other three tasks keep running.
    let connection = zbus::connection::Builder::session()
        .map_err(io::Error::other)?
        .name("com.acheron.Daemon")
        .map_err(io::Error::other)?
        .serve_at(
            "/com/acheron/Daemon",
            // Ticket 71: dispatch also holds its own clone of `depth_rx`
            // below — the continuous half of Axis-assignment resolution
            // (`(Depth, edge_event) -> axis_value`) — alongside the D-Bus
            // layer's own pre-existing `DepthChanged` streaming use of it.
            Daemon::new(cmd_tx, inj.clone(), depth_rx.clone()),
        )
        .map_err(io::Error::other)?
        .build()
        .await
        .map_err(io::Error::other)?;
    let signal_emitter =
        zbus::object_server::SignalEmitter::new(&connection, "/com/acheron/Daemon")
            .map_err(io::Error::other)?
            .into_owned();

    let force_digital = config.force_digital;
    // Ticket 68: read once, here, before dispatch's event loop starts — the
    // kernel autorepeat rate this reflects never changes while the Daemon
    // is running, so there is no reason to re-read it on every Toggle press.
    let toggle_lap_target = acheron_daemon::executor::resolve_toggle_lap_target().await;
    let dispatch_handle = tokio::spawn(dispatch::run(
        event_rx,
        connection_rx,
        cmd_rx,
        inj,
        config,
        config_path,
        Some(signal_emitter),
        actuation_tx,
        capture_mode_rx,
        capture_control_tx,
        toggle_lap_target,
        depth_rx,
        device_info_rx,
    ));

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    let result = tokio::select! {
        result = supervisor::run(event_tx, connection_tx, actuation_rx, depth_tx, capture_mode_tx, capture_control_rx, device_info_tx, force_digital) => report("capture", result),
        result = inj_handle => report("injector", flatten(result)),
        result = dispatch_handle => report("dispatch", flatten(result)),
        _ = sigterm.recv() => relock_and_exit("SIGTERM"),
        _ = tokio::signal::ctrl_c() => relock_and_exit("SIGINT"),
    };
    result
}

/// The standing map decision (map Notes: "The Daemon also re-locks the
/// device to mode 0x00 on clean shutdown"): best-effort — `analog::relock`
/// is a no-op-safe idempotent send (ticket 16: a relock from a fresh handle
/// that never sent the unlock still succeeds), so this doesn't need to know
/// whether the supervisor's current source was actually Analog. A failure
/// here (device unplugged, no `hidraw` access) is reported, not fatal —
/// there's nothing left to clean up in that case anyway.
///
/// Ends the process with `std::process::exit` rather than returning —
/// confirmed live (ticket 23) that returning normally is *not* harmless:
/// `#[tokio::main]`'s generated wrapper drops the multi-thread `Runtime` at
/// the end of `main`, and that `Drop` blocks waiting for every outstanding
/// task — including the supervisor's `tokio::spawn`ed capture-source
/// `JoinHandle` and its own `spawn_blocking` node/grid tasks, none of which
/// SIGTERM/SIGINT ever asked to stop — to finish on their own. A grid task
/// silently retrying a denied `hidraw` open forever (exactly ticket 23's own
/// "udev rule missing" scenario) never finishes on its own, so the process
/// hung indefinitely instead of exiting. `process::exit` skips Rust's
/// destructors and the runtime's task-draining entirely, terminating
/// immediately regardless — matching ticket 24's "harmless under a real
/// process-signal shutdown" note, which only holds for an actual immediate
/// exit, not a graceful drain nothing was told to start.
fn relock_and_exit(signal_name: &str) -> ! {
    eprintln!("acheron-daemon: received {signal_name}, relocking device and exiting");
    if let Err(err) = acheron_daemon::capture::analog::relock() {
        eprintln!(
            "acheron-daemon: relock on shutdown failed (harmless if the device was never unlocked): {err}"
        );
    }
    std::process::exit(0)
}

fn flatten(result: Result<io::Result<()>, JoinError>) -> io::Result<()> {
    result.unwrap_or_else(|join_err| Err(io::Error::other(join_err)))
}

fn report(task: &str, result: io::Result<()>) -> io::Result<()> {
    if let Err(ref err) = result {
        eprintln!("acheron-daemon: {task} task exited with a fatal error: {err}");
    }
    result
}
