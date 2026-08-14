//! The Tartarus Pro's `Input` domain model and its evdev code table.
//!
//! `Input` is the composite enum from the data-model decision (issue 06):
//! `ModeKey`, `Grid(row, col)`, `Thumbstick(Direction)`, `Wheel(WheelEvent)`.
//! This module also holds the Input <-> (node, evdev code) table captured
//! live in issue 01, used by the evdev `CaptureSource` to normalize incoming
//! events and by the injector to translate a `PhysicalEvent` back into the
//! identical raw evdev output for passthrough.

use evdev::KeyCode;

/// Which of the Tartarus Pro's three evdev nodes an `Input` is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    Main,
    If01,
    If02,
}

impl Node {
    /// The three nodes' fixed `/dev/input/by-id` paths on this device.
    pub const fn device_path(self) -> &'static str {
        match self {
            Node::Main => "/dev/input/by-id/usb-Razer_Razer_Tartarus_Pro-event-kbd",
            Node::If01 => "/dev/input/by-id/usb-Razer_Razer_Tartarus_Pro-if01-event-kbd",
            Node::If02 => "/dev/input/by-id/usb-Razer_Razer_Tartarus_Pro-if02-event-mouse",
        }
    }

    pub const ALL: [Node; 3] = [Node::Main, Node::If01, Node::If02];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WheelEvent {
    ScrollUp,
    ScrollDown,
    MiddleClick,
}

/// One physical control on the Tartarus Pro that can be bound (CONTEXT.md: Input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Input {
    ModeKey,
    /// 1-indexed: row 1-4, column 1-5.
    Grid(u8, u8),
    Thumbstick(Direction),
    Wheel(WheelEvent),
}

/// The 4x5 grid's evdev key codes, top-left to bottom-right, per issue 01's table.
const GRID_KEYS: [[KeyCode; 5]; 4] = [
    [
        KeyCode::KEY_1,
        KeyCode::KEY_2,
        KeyCode::KEY_3,
        KeyCode::KEY_4,
        KeyCode::KEY_5,
    ],
    [
        KeyCode::KEY_TAB,
        KeyCode::KEY_Q,
        KeyCode::KEY_W,
        KeyCode::KEY_E,
        KeyCode::KEY_R,
    ],
    [
        KeyCode::KEY_CAPSLOCK,
        KeyCode::KEY_A,
        KeyCode::KEY_S,
        KeyCode::KEY_D,
        KeyCode::KEY_F,
    ],
    [
        KeyCode::KEY_LEFTSHIFT,
        KeyCode::KEY_Z,
        KeyCode::KEY_X,
        KeyCode::KEY_C,
        KeyCode::KEY_SPACE,
    ],
];

/// Maps an `EV_KEY` code observed on `node` to the `Input` it represents.
/// Returns `None` for codes the Tartarus Pro doesn't emit (should not happen
/// on a real device grabbed by node, but keeps capture non-panicking).
pub fn input_for_key(node: Node, code: KeyCode) -> Option<Input> {
    match node {
        Node::Main => match code {
            KeyCode::KEY_LEFTALT => Some(Input::ModeKey),
            KeyCode::KEY_UP => Some(Input::Thumbstick(Direction::Up)),
            KeyCode::KEY_DOWN => Some(Input::Thumbstick(Direction::Down)),
            KeyCode::KEY_LEFT => Some(Input::Thumbstick(Direction::Left)),
            KeyCode::KEY_RIGHT => Some(Input::Thumbstick(Direction::Right)),
            _ => None,
        },
        Node::If01 => {
            for (row_idx, row) in GRID_KEYS.iter().enumerate() {
                if let Some(col_idx) = row.iter().position(|&k| k == code) {
                    return Some(Input::Grid(row_idx as u8 + 1, col_idx as u8 + 1));
                }
            }
            None
        }
        Node::If02 => match code {
            KeyCode::BTN_MIDDLE => Some(Input::Wheel(WheelEvent::MiddleClick)),
            _ => None,
        },
    }
}

/// The evdev key code an `Input` maps back onto for passthrough injection.
/// Returns `None` for `Wheel(ScrollUp | ScrollDown)`, which inject as
/// `EV_REL` events instead (see `injector::wheel_scroll_events`).
pub fn key_code_for_input(input: Input) -> Option<KeyCode> {
    match input {
        Input::ModeKey => Some(KeyCode::KEY_LEFTALT),
        Input::Grid(row, col) => {
            let row_idx = usize::from(row).checked_sub(1)?;
            let col_idx = usize::from(col).checked_sub(1)?;
            GRID_KEYS.get(row_idx).and_then(|r| r.get(col_idx)).copied()
        }
        Input::Thumbstick(Direction::Up) => Some(KeyCode::KEY_UP),
        Input::Thumbstick(Direction::Down) => Some(KeyCode::KEY_DOWN),
        Input::Thumbstick(Direction::Left) => Some(KeyCode::KEY_LEFT),
        Input::Thumbstick(Direction::Right) => Some(KeyCode::KEY_RIGHT),
        Input::Wheel(WheelEvent::MiddleClick) => Some(KeyCode::BTN_MIDDLE),
        Input::Wheel(WheelEvent::ScrollUp | WheelEvent::ScrollDown) => None,
    }
}

/// All key codes the virtual uinput device must declare support for, so that
/// every `Input` other than wheel-scroll can be replayed through it.
pub fn all_injectable_key_codes() -> Vec<KeyCode> {
    let mut codes = vec![KeyCode::KEY_LEFTALT, KeyCode::BTN_MIDDLE];
    codes.extend([
        KeyCode::KEY_UP,
        KeyCode::KEY_DOWN,
        KeyCode::KEY_LEFT,
        KeyCode::KEY_RIGHT,
    ]);
    for row in GRID_KEYS {
        codes.extend(row);
    }
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_round_trips_through_key_codes() {
        for row in 1..=4u8 {
            for col in 1..=5u8 {
                let input = Input::Grid(row, col);
                let code = key_code_for_input(input).expect("grid input must map to a key code");
                assert_eq!(input_for_key(Node::If01, code), Some(input));
            }
        }
    }

    #[test]
    fn mode_key_and_thumbstick_are_on_main() {
        assert_eq!(
            input_for_key(Node::Main, KeyCode::KEY_LEFTALT),
            Some(Input::ModeKey)
        );
        assert_eq!(
            input_for_key(Node::Main, KeyCode::KEY_UP),
            Some(Input::Thumbstick(Direction::Up))
        );
    }

    #[test]
    fn middle_click_is_on_if02() {
        assert_eq!(
            input_for_key(Node::If02, KeyCode::BTN_MIDDLE),
            Some(Input::Wheel(WheelEvent::MiddleClick))
        );
        assert_eq!(input_for_key(Node::If01, KeyCode::BTN_MIDDLE), None);
    }

    #[test]
    fn wheel_scroll_has_no_key_code() {
        assert_eq!(key_code_for_input(Input::Wheel(WheelEvent::ScrollUp)), None);
        assert_eq!(
            key_code_for_input(Input::Wheel(WheelEvent::ScrollDown)),
            None
        );
    }

    #[test]
    fn out_of_range_grid_coordinates_return_none_instead_of_panicking() {
        assert_eq!(key_code_for_input(Input::Grid(0, 1)), None);
        assert_eq!(key_code_for_input(Input::Grid(1, 0)), None);
        assert_eq!(key_code_for_input(Input::Grid(5, 1)), None);
        assert_eq!(key_code_for_input(Input::Grid(1, 6)), None);
    }
}
