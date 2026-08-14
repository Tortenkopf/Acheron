//! The `CaptureSource` seam: "produces a stream of normalized `PhysicalEvent`s
//! into a shared channel" is the only contract anything downstream of the
//! channel relies on (see issue 07 / ticket 13). `evdev_source` is the real
//! implementation; `fake` is the scripted stand-in used by tests.

pub mod evdev_source;
pub mod fake;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalEvent {
    pub input: Input,
    pub state: EventState,
}

/// Produces a stream of normalized `PhysicalEvent`s into `tx`. Nothing
/// downstream of the channel knows or cares which implementation produced
/// an event — this is the swappable capture layer the map's standing
/// architectural discipline calls for.
pub trait CaptureSource: Send + 'static {
    fn run(
        self,
        tx: mpsc::Sender<PhysicalEvent>,
    ) -> impl std::future::Future<Output = std::io::Result<()>> + Send;
}
