use std::io;

use acheron_daemon::capture::CaptureSource;
use acheron_daemon::capture::evdev_source::EvdevCaptureSource;
use acheron_daemon::{dispatch, injector};
use tokio::task::JoinError;

/// No config/Profiles/Bindings yet (ticket 13) — this assembles just the
/// capture -> dispatch -> injector passthrough pipeline the rest of the
/// Daemon will build on.
///
/// No graceful-shutdown story exists yet either, so any of the three tasks
/// finishing is treated as fatal: per issue 07/10, a genuine capture or
/// injection error must exit the process so systemd's `Restart=on-failure`
/// can recover it, rather than leaving the daemon silently inert.
#[tokio::main]
async fn main() -> io::Result<()> {
    let device = injector::build_device()?;
    let (inj, inj_handle) = injector::spawn(device);

    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let dispatch_handle = tokio::spawn(dispatch::run(rx, inj));

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
