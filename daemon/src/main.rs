use std::io;

use acheron_daemon::capture::CaptureSource;
use acheron_daemon::capture::evdev_source::EvdevCaptureSource;
use acheron_daemon::config;
use acheron_daemon::{dispatch, injector};
use tokio::task::JoinError;

/// No D-Bus surface yet (ticket 15) — this loads `config.toml` (seeding it
/// on first run, per issue 11) and assembles the capture -> dispatch ->
/// injector pipeline, with dispatch resolving each event against the
/// active Profile's Base Layer (ticket 14).
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
    let bindings = config
        .active_profile()
        .expect("load_or_seed validates active_profile names a real profile")
        .base
        .clone();

    let device = injector::build_device()?;
    let (inj, inj_handle) = injector::spawn(device);

    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let dispatch_handle = tokio::spawn(dispatch::run(rx, inj, bindings));

    let result = tokio::select! {
        result = EvdevCaptureSource.run(tx) => report("capture", result),
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
