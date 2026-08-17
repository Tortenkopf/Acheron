use std::collections::HashMap;
use std::io;

use acheron_daemon::capture::CaptureSource;
use acheron_daemon::capture::evdev_source::EvdevCaptureSource;
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
/// No graceful-shutdown story exists yet either, so any of the three tasks
/// finishing is treated as fatal: per issue 07/10, a genuine capture or
/// injection error must exit the process so systemd's `Restart=on-failure`
/// can recover it, rather than leaving the daemon silently inert.
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

    let device = injector::build_device()?;
    let (inj, inj_handle) = injector::spawn(device);

    let (event_tx, event_rx) = tokio::sync::mpsc::channel(256);
    let (connection_tx, connection_rx) = tokio::sync::mpsc::channel(8);
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(256);
    // The Actuation-point snapshot seam (ticket 18 §5/ticket 22): dispatch
    // publishes into `actuation_tx` on every mutation that touches the
    // active Profile's Actuation points. `actuation_rx` is unused here for
    // now — wiring it into a live `AnalogCaptureSource` grid task is ticket
    // 23's job, once that source actually exists.
    let (actuation_tx, _actuation_rx) = tokio::sync::watch::channel(HashMap::new());

    // Built before the dispatch task so a real `SignalEmitter` (ticket 18's
    // `ActiveLayerChanged`, pushed directly from the dispatch task on every
    // Mode-key transition) can be handed to it. Held for the process's
    // lifetime: dropping it would take the D-Bus surface down while the
    // other three tasks keep running.
    let connection = zbus::connection::Builder::session()
        .map_err(io::Error::other)?
        .name("com.acheron.Daemon")
        .map_err(io::Error::other)?
        .serve_at("/com/acheron/Daemon", Daemon::new(cmd_tx, inj.clone()))
        .map_err(io::Error::other)?
        .build()
        .await
        .map_err(io::Error::other)?;
    let signal_emitter =
        zbus::object_server::SignalEmitter::new(&connection, "/com/acheron/Daemon")
            .map_err(io::Error::other)?
            .into_owned();

    let dispatch_handle = tokio::spawn(dispatch::run(
        event_rx,
        connection_rx,
        cmd_rx,
        inj,
        config,
        config_path,
        Some(signal_emitter),
        actuation_tx,
    ));

    let result = tokio::select! {
        result = EvdevCaptureSource::ALL.run(event_tx, connection_tx) => report("capture", result),
        result = inj_handle => report("injector", flatten(result)),
        result = dispatch_handle => report("dispatch", flatten(result)),
    };
    result
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
