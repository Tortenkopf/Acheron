//! Manual HITL verification tool for ticket 101's device-info read — not
//! part of the daemon itself and not wired into `main.rs`. Runs
//! `analog::read_device_info()` against the connected Tartarus Pro and
//! prints the result, so a human can confirm ticket 100/101's live-check
//! items:
//!
//!   - the `HIDIOCGFEATURE` round-trip works on this device at all
//!   - which `transaction_id` the read succeeds with — `read_device_info`
//!     logs `transaction_id 0xff` (primary, expected) or `0x1f` (fallback)
//!   - firmware + serial match the known values for the unit
//!     (research §4: firmware `v1.2`, serial `PM2443F36300141`)
//!   - no reset / USB re-enumeration — watch `dmesg -w` or
//!     `udevadm monitor` in another terminal across repeated runs
//!   - keys absent after an unplug, correct again after replug — run this
//!     with the device unplugged (expect `NotFound`), then replug and rerun
//!
//! It performs only *reads* — it never sends `set_device_mode`, so it
//! leaves the device in whatever Capture mode it was already in and needs
//! no relock afterwards.
//!
//! Usage (the Interface-2 `/dev/hidraw*` node needs read/write access —
//! install `packaging/60-acheron-tartarus-pro.rules` via `install.sh` and
//! be in the `plugdev` group, run this under `sudo`, or
//! `sudo chmod 660 /dev/hidraw*` first):
//!
//!     cargo run --example device_info_probe [iterations]
//!
//! `iterations` (default 1) repeats the read with a 1s gap — useful for
//! watching for any delayed reset across several back-to-back exchanges.

use std::time::{Duration, Instant};

use acheron_daemon::capture::analog;

fn main() {
    let iterations: u32 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(1);

    println!(
        "acheron device_info_probe: reading firmware + serial from the connected Tartarus Pro"
    );
    println!(
        "(watch `dmesg -w` / `udevadm monitor` in another terminal for any reset / re-enumeration)"
    );
    println!("expected for our unit (research §4): firmware v1.2, serial PM2443F36300141");
    println!();

    let mut failures = 0u32;
    for i in 1..=iterations {
        let started = Instant::now();
        match analog::read_device_info() {
            Ok(info) => {
                println!(
                    "[{i}/{iterations}] OK in {:?}: firmware_version={:?} serial_number={:?}",
                    started.elapsed(),
                    info.firmware_version,
                    info.serial_number,
                );
            }
            Err(err) => {
                failures += 1;
                eprintln!(
                    "[{i}/{iterations}] FAILED in {:?}: {err}",
                    started.elapsed()
                );
                eprintln!(
                    "    PermissionDenied => the Interface-2 /dev/hidraw* node isn't accessible \
                     (udev rule / `plugdev` / sudo / chmod 660)"
                );
                eprintln!(
                    "    NotFound         => the Tartarus Pro isn't plugged in / not enumerated"
                );
            }
        }
        if i < iterations {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    if failures > 0 {
        std::process::exit(1);
    }
}
