// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The `CaptureSource` seam: "produces a stream of normalized `PhysicalEvent`s
//! into a shared channel" is the only contract anything downstream of the
//! channel relies on (see issue 07 / ticket 13). `evdev_source` is the
//! Digital Capture mode implementation (evdev passthrough, no Depth);
//! `analog` is the Analog Capture mode implementation (ticket 22: the same
//! evdev nodes minus the grid, plus a `hidraw` grid task carrying Depth);
//! `fake` is the scripted stand-in used by tests.

pub mod analog;
pub mod evdev_source;
pub mod fake;
pub mod supervisor;

use crate::input::Input;
use tokio::sync::mpsc;

/// The three-state normalization of evdev's raw `EV_KEY` value (1/2/0),
/// per issue 07's dispatch design. A wheel-scroll tick (no natural
/// press/release) is normalized as a single `Down`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventState {
    Down,
    Repeat,
    Up,
}

/// Which physical capture path is currently live (ticket 17 §4 /
/// `command::State::capture_mode`): `Analog` reads the 20 grid keys' Depth
/// over `hidraw` (`capture::analog::AnalogCaptureSource`); `Digital` reads
/// every Input, grid included, over evdev (`capture::evdev_source::
/// EvdevCaptureSource`) — the automatic degradation path ticket 23's
/// supervisor falls back to when Analog can't unlock, and the explicit
/// `force_digital` override always selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    Analog,
    Digital,
}

impl CaptureMode {
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureMode::Analog => "analog",
            CaptureMode::Digital => "digital",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalEvent {
    pub input: Input,
    pub state: EventState,
    /// The Depth (0-255) this event was captured at (ticket 17 §1). `None`
    /// for every evdev-sourced event — including a `Grid` `Input` while
    /// degraded to Digital Capture mode — and for every non-`Grid` `Input`
    /// regardless of Capture mode; `Some` only for an Analog-sourced `Grid`
    /// event. How an analog source synthesizes `Down`/`Repeat`/`Up` from
    /// depth thresholds is ticket 18's job, not this field's.
    pub depth: Option<u8>,
}

/// Produces a stream of normalized `PhysicalEvent`s into `tx`, and a live
/// device-connection view into `connection_tx` (ticket 20: `true`/`false`
/// sent on every transition, redundant sends are harmless — the dispatch
/// task consuming it only reacts to an actual value change). Nothing
/// downstream of either channel knows or cares which implementation produced
/// an event — this is the swappable capture layer the map's standing
/// architectural discipline calls for.
pub trait CaptureSource: Send + 'static {
    fn run(
        self,
        tx: mpsc::Sender<PhysicalEvent>,
        connection_tx: mpsc::Sender<bool>,
    ) -> impl std::future::Future<Output = std::io::Result<()>> + Send;
}
