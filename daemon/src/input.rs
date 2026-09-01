// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! The Tartarus Pro's `Input` domain model and its evdev code table.
//!
//! `Input` is the composite enum from the data-model decision (issue 06):
//! `ModeKey`, `Grid(row, col)`, `Thumbstick(Direction)`, `Wheel(WheelEvent)`.
//! This module also holds the Input <-> (node, evdev code) table captured
//! live in issue 01, used by the evdev `CaptureSource` to normalize incoming
//! events and by the injector to translate a `PhysicalEvent` back into the
//! identical raw evdev output for passthrough.

use std::fmt;
use std::str::FromStr;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WheelEvent {
    ScrollUp,
    ScrollDown,
    MiddleClick,
}

/// One physical control on the Tartarus Pro that can be bound (CONTEXT.md: Input).
///
/// `PartialOrd`/`Ord` (ticket 40) exist solely so a Chord's membership can
/// live in a `BTreeSet<Input>` (`config::ChordKey`) — the derived order is
/// otherwise arbitrary and carries no meaning of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Input {
    ModeKey,
    /// 1-indexed: row 1-4, column 1-5.
    Grid(u8, u8),
    Thumbstick(Direction),
    Wheel(WheelEvent),
}

/// The flat snake_case string form of an `Input`, matching issue 01's table
/// exactly (`mode_key`, `grid_r1c1`, `thumbstick_up`, `wheel_scroll_up`,
/// `wheel_middle`, …). Used identically in TOML (ticket 14) and, later, on
/// the D-Bus wire (issue 08).
impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Input::ModeKey => write!(f, "mode_key"),
            Input::Grid(row, col) => write!(f, "grid_r{row}c{col}"),
            Input::Thumbstick(Direction::Up) => write!(f, "thumbstick_up"),
            Input::Thumbstick(Direction::Down) => write!(f, "thumbstick_down"),
            Input::Thumbstick(Direction::Left) => write!(f, "thumbstick_left"),
            Input::Thumbstick(Direction::Right) => write!(f, "thumbstick_right"),
            Input::Wheel(WheelEvent::ScrollUp) => write!(f, "wheel_scroll_up"),
            Input::Wheel(WheelEvent::ScrollDown) => write!(f, "wheel_scroll_down"),
            Input::Wheel(WheelEvent::MiddleClick) => write!(f, "wheel_middle"),
        }
    }
}

/// An `Input` string that doesn't match any of issue 01's flat snake_case
/// forms (e.g. a hand-edit typo in `config.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseInputError(String);

impl fmt::Display for ParseInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a valid Input", self.0)
    }
}

impl std::error::Error for ParseInputError {}

impl FromStr for Input {
    type Err = ParseInputError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mode_key" => return Ok(Input::ModeKey),
            "thumbstick_up" => return Ok(Input::Thumbstick(Direction::Up)),
            "thumbstick_down" => return Ok(Input::Thumbstick(Direction::Down)),
            "thumbstick_left" => return Ok(Input::Thumbstick(Direction::Left)),
            "thumbstick_right" => return Ok(Input::Thumbstick(Direction::Right)),
            "wheel_scroll_up" => return Ok(Input::Wheel(WheelEvent::ScrollUp)),
            "wheel_scroll_down" => return Ok(Input::Wheel(WheelEvent::ScrollDown)),
            "wheel_middle" => return Ok(Input::Wheel(WheelEvent::MiddleClick)),
            _ => {}
        }
        if let Some((row, col)) = s
            .strip_prefix("grid_r")
            .and_then(|rest| rest.split_once('c'))
            && let (Ok(row), Ok(col)) = (row.parse::<u8>(), col.parse::<u8>())
            && (1..=4).contains(&row)
            && (1..=5).contains(&col)
        {
            return Ok(Input::Grid(row, col));
        }
        Err(ParseInputError(s.to_string()))
    }
}

/// Serializes as the same flat string `Display` produces, so `Input` can be
/// used directly as a TOML table key (ticket 14) without a wrapper type.
impl serde::Serialize for Input {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for Input {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
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

/// Linux's `KEY_MAX` (`linux/input-event-codes.h`) — the highest valid
/// `EV_KEY` code. `BTN_*` codes share this same numeric space.
const KEY_CODE_MAX: u16 = 0x2ff;

/// All key codes the virtual uinput device must declare support for. Ticket
/// 14 onward lets a Binding's `Action::Keypress` target *any* key (the remap
/// target, not just this device's own physical Inputs), so rather than
/// tracking a curated allow-list, the virtual device declares the whole
/// standard `EV_KEY` range up front — same approach other Linux uinput
/// remappers use, and harmless for the codes that go unused.
pub fn all_injectable_key_codes() -> Vec<KeyCode> {
    (0..=KEY_CODE_MAX).map(KeyCode::new).collect()
}

/// Whether `code` is one of the curated 57-entry gamepad-button allowlist
/// `Action::ControllerButton` validates against (ticket 43, per ticket 14's
/// settled device-advertising scope): the standard Linux Gamepad Spec's
/// named `BTN_GAMEPAD` range, the four `BTN_DPAD_*` directions, and the full
/// `BTN_TRIGGER_HAPPY1`-`40` extra range. Also the injector's
/// keyboard-vs-gamepad routing decision: a `KeyCode` never carries which
/// `uinput` device it targets, so the injector infers it from the code
/// itself rather than threading a device tag through `executor.rs` (which
/// stays unchanged, exactly as ticket 14 intended — "only the target uinput
/// device differs, an executor/injector-level distinction").
pub fn is_gamepad_button(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::BTN_SOUTH
            | KeyCode::BTN_EAST
            | KeyCode::BTN_NORTH
            | KeyCode::BTN_WEST
            | KeyCode::BTN_TL
            | KeyCode::BTN_TR
            | KeyCode::BTN_TL2
            | KeyCode::BTN_TR2
            | KeyCode::BTN_SELECT
            | KeyCode::BTN_START
            | KeyCode::BTN_MODE
            | KeyCode::BTN_THUMBL
            | KeyCode::BTN_THUMBR
            | KeyCode::BTN_DPAD_UP
            | KeyCode::BTN_DPAD_DOWN
            | KeyCode::BTN_DPAD_LEFT
            | KeyCode::BTN_DPAD_RIGHT
    ) || (KeyCode::BTN_TRIGGER_HAPPY1.code()..=KeyCode::BTN_TRIGGER_HAPPY40.code())
        .contains(&code.code())
}

/// The curated gamepad-button allowlist as a list — the second `uinput`
/// device's own advertised capability set (`injector::build_gamepad_device`),
/// so the device's declared codes and `is_gamepad_button`'s validation/
/// routing decision can never drift apart.
pub fn gamepad_button_codes() -> Vec<KeyCode> {
    (0..=KEY_CODE_MAX)
        .map(KeyCode::new)
        .filter(|&code| is_gamepad_button(code))
        .collect()
}

/// Whether `code` is a mouse button, per evdev's own `BTN_LEFT..=BTN_TASK`
/// block (`0x110`-`0x117`: Left/Right/Middle/Side/Extra/Forward/Back/Task) —
/// ticket 79/80's carve-out predicate, `trigger::decide`'s way
/// of telling a mouse-button `Action::Keypress` apart from a keyboard-key
/// one so Hold-to-repeat can give it sustained-hold-for-drag instead of a
/// repeat-tap train. Deliberately wider than the 5 codes the GUI picker
/// (ticket 02) actually emits (Left/Right/Middle/Side-as-Back/Extra-as-
/// Forward): `Action::Keypress.key` has zero allowlist validation (see the
/// map's Notes), so a hand-edited `config.toml` using `BTN_FORWARD`/
/// `BTN_BACK`/`BTN_TASK` gets the same treatment as a picker-built Binding.
/// No overlap with `is_gamepad_button`'s range — `BTN_SOUTH` starts at
/// `0x130`.
pub fn is_mouse_button(code: KeyCode) -> bool {
    (KeyCode::BTN_LEFT.code()..=KeyCode::BTN_TASK.code()).contains(&code.code())
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

    #[test]
    fn display_matches_issue_01s_flat_strings() {
        assert_eq!(Input::ModeKey.to_string(), "mode_key");
        assert_eq!(Input::Grid(1, 1).to_string(), "grid_r1c1");
        assert_eq!(Input::Grid(4, 5).to_string(), "grid_r4c5");
        assert_eq!(
            Input::Thumbstick(Direction::Up).to_string(),
            "thumbstick_up"
        );
        assert_eq!(
            Input::Wheel(WheelEvent::ScrollUp).to_string(),
            "wheel_scroll_up"
        );
        assert_eq!(
            Input::Wheel(WheelEvent::MiddleClick).to_string(),
            "wheel_middle"
        );
    }

    #[test]
    fn from_str_round_trips_every_input_through_its_display_form() {
        let mut inputs = vec![
            Input::ModeKey,
            Input::Thumbstick(Direction::Up),
            Input::Thumbstick(Direction::Down),
            Input::Thumbstick(Direction::Left),
            Input::Thumbstick(Direction::Right),
            Input::Wheel(WheelEvent::ScrollUp),
            Input::Wheel(WheelEvent::ScrollDown),
            Input::Wheel(WheelEvent::MiddleClick),
        ];
        for row in 1..=4u8 {
            for col in 1..=5u8 {
                inputs.push(Input::Grid(row, col));
            }
        }

        for input in inputs {
            let parsed: Input = input
                .to_string()
                .parse()
                .expect("must parse its own Display form");
            assert_eq!(parsed, input);
        }
    }

    #[test]
    fn from_str_rejects_unknown_and_out_of_range_strings() {
        assert!("not_an_input".parse::<Input>().is_err());
        assert!("grid_r5c1".parse::<Input>().is_err());
        assert!("grid_r1c6".parse::<Input>().is_err());
        assert!("grid_r0c1".parse::<Input>().is_err());
    }

    #[test]
    fn gamepad_button_codes_has_exactly_57_entries() {
        assert_eq!(gamepad_button_codes().len(), 57);
    }

    #[test]
    fn is_gamepad_button_accepts_every_named_button_and_the_full_trigger_happy_range() {
        for code in [
            KeyCode::BTN_SOUTH,
            KeyCode::BTN_EAST,
            KeyCode::BTN_NORTH,
            KeyCode::BTN_WEST,
            KeyCode::BTN_TL,
            KeyCode::BTN_TR,
            KeyCode::BTN_TL2,
            KeyCode::BTN_TR2,
            KeyCode::BTN_SELECT,
            KeyCode::BTN_START,
            KeyCode::BTN_MODE,
            KeyCode::BTN_THUMBL,
            KeyCode::BTN_THUMBR,
            KeyCode::BTN_DPAD_UP,
            KeyCode::BTN_DPAD_DOWN,
            KeyCode::BTN_DPAD_LEFT,
            KeyCode::BTN_DPAD_RIGHT,
        ] {
            assert!(is_gamepad_button(code), "{code:?} must be a gamepad button");
        }
        for i in KeyCode::BTN_TRIGGER_HAPPY1.code()..=KeyCode::BTN_TRIGGER_HAPPY40.code() {
            assert!(is_gamepad_button(KeyCode::new(i)));
        }
    }

    #[test]
    fn is_gamepad_button_rejects_keyboard_and_mouse_codes() {
        assert!(!is_gamepad_button(KeyCode::KEY_A));
        assert!(!is_gamepad_button(KeyCode::BTN_LEFT));
        assert!(!is_gamepad_button(KeyCode::BTN_MIDDLE));
        assert!(!is_gamepad_button(KeyCode::new(
            KeyCode::BTN_TRIGGER_HAPPY40.code() + 1
        )));
    }

    #[test]
    fn is_mouse_button_accepts_the_full_btn_left_to_btn_task_range() {
        for code in [
            KeyCode::BTN_LEFT,
            KeyCode::BTN_RIGHT,
            KeyCode::BTN_MIDDLE,
            KeyCode::BTN_SIDE,
            KeyCode::BTN_EXTRA,
            KeyCode::BTN_FORWARD,
            KeyCode::BTN_BACK,
            KeyCode::BTN_TASK,
        ] {
            assert!(is_mouse_button(code), "{code:?} must be a mouse button");
        }
    }

    #[test]
    fn is_mouse_button_rejects_keyboard_gamepad_and_just_out_of_range_codes() {
        assert!(!is_mouse_button(KeyCode::KEY_A));
        assert!(!is_mouse_button(KeyCode::BTN_SOUTH));
        for i in KeyCode::BTN_0.code()..=KeyCode::BTN_9.code() {
            assert!(!is_mouse_button(KeyCode::new(i)));
        }
        assert!(!is_mouse_button(KeyCode::new(KeyCode::BTN_TASK.code() + 1)));
    }

    #[test]
    fn serde_round_trips_input_as_a_toml_string() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Wrapper {
            input: Input,
        }

        let wrapper = Wrapper {
            input: Input::Grid(2, 3),
        };
        let toml = toml::to_string(&wrapper).unwrap();
        assert_eq!(toml.trim(), r#"input = "grid_r2c3""#);

        let parsed: Wrapper = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.input, Input::Grid(2, 3));
    }
}
