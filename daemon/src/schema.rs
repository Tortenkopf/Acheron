// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! Golden-file contract fixture bridging the daemon's device catalogs and
//! `config::validate` verdicts to the GUI's hand-written `rules.py` mirror
//! (post-release ticket 06).
//!
//! ADR 0003 split the stack into a Rust daemon and a Python + GTK GUI talking
//! over a D-Bus process seam, so the GUI necessarily re-implements the *pure*
//! half of the domain model — the device vocabularies and the
//! Binding-legality matrix — as `gui/acheron_gui/rules.py`. This module makes
//! that mirror's faithfulness a **test** rather than a comment: the single
//! `#[test]` below derives the two catalogs and the two verdict matrices by
//! driving the real [`crate::config::validate`], serialises the lot to
//! `daemon/contract/daemon-schema.json`, and fails on any diff against the
//! checked-in file. `gui/tests/test_rules_contract.py` loads the same file
//! and asserts `rules` agrees with it row for row.
//!
//! ## Regenerating the fixture (`ACHERON_BLESS`)
//!
//! This is the only golden / "bless" file in the repo. When a device-catalog
//! entry or a Binding-legality rule changes on the daemon side:
//!
//! ```sh
//! ACHERON_BLESS=1 cargo test --manifest-path daemon/Cargo.toml schema
//! ```
//!
//! rewrites `daemon/contract/daemon-schema.json` in place instead of
//! asserting. Then mirror the change into `gui/acheron_gui/rules.py` and run
//! the GUI suite. See `CONTRIBUTING.md`.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;

use evdev::KeyCode;

use crate::config::{
    self, Action, AxisTarget, Binding, ChordKey, Config, MacroDef, MacroId, Modifiers, Profile,
    SCHEMA_VERSION, StepDirection, StepperDef, StepperId, TriggerMode,
};
use crate::dbus::wire::axis_target_str;
use crate::input::{Direction, Input, WheelEvent, gamepad_button_codes};

/// The `__chord__` sentinel `rules.valid_triggers` / `valid_action_kinds`
/// use for "a Chord's own Binding" (which has no single Input) — matches
/// `binding_editor.py`'s existing `inp is None` convention on the wire.
const CHORD_SENTINEL: &str = "__chord__";

/// The five real Action kinds a Binding can carry — `axis` is deliberately
/// absent (an Axis assignment is not an `Action`; it has no `TriggerMode`).
const BINDING_ACTION_KINDS: [&str; 5] = [
    "keypress",
    "controller_button",
    "macro",
    "step",
    "profile_switch",
];

/// Every Action-*placement* kind, including `axis` — mirrors
/// `rules.ALL_ACTION_KINDS` and `inputs.ACTION_TYPES`' key set.
const ALL_ACTION_KINDS: [&str; 6] = [
    "keypress",
    "controller_button",
    "axis",
    "macro",
    "step",
    "profile_switch",
];

/// Mirrors `rules.ALL_TRIGGERS` and `inputs.TRIGGER_OPTIONS`' key set.
const ALL_TRIGGERS: [&str; 4] = ["fire_once", "hold_to_repeat", "toggle", "analog_repeat"];

/// The 28 real Input strings, in `inputs.ALL_INPUTS` order (Mode key, the
/// 4×5 grid row-major, the four thumbstick directions, the three wheel
/// events).
fn real_inputs() -> Vec<Input> {
    let mut inputs = vec![Input::ModeKey];
    for row in 1..=4u8 {
        for col in 1..=5u8 {
            inputs.push(Input::Grid(row, col));
        }
    }
    inputs.extend([
        Input::Thumbstick(Direction::Up),
        Input::Thumbstick(Direction::Down),
        Input::Thumbstick(Direction::Left),
        Input::Thumbstick(Direction::Right),
        Input::Wheel(WheelEvent::ScrollUp),
        Input::Wheel(WheelEvent::ScrollDown),
        Input::Wheel(WheelEvent::MiddleClick),
    ]);
    inputs
}

/// A minimal two-profile `Config` seeded so every *out-of-scope* check in
/// `validate` (dangling macro/stepper/profile-switch target,
/// `release < actuation`, …) always passes — leaving the combination under
/// test as the only thing that can make `validate` fail.
fn seeded_config() -> Config {
    let mut profiles = HashMap::new();
    profiles.insert("P1".to_string(), Profile::default());
    profiles.insert("P2".to_string(), Profile::default());

    let mut macros = HashMap::new();
    macros.insert(
        MacroId::from("m"),
        MacroDef {
            name: "m".to_string(),
            steps: Vec::new(),
        },
    );

    let mut steppers = HashMap::new();
    steppers.insert(
        StepperId::from("s"),
        StepperDef {
            name: "s".to_string(),
            items: Vec::new(),
        },
    );

    Config {
        schema_version: SCHEMA_VERSION,
        active_profile: "P1".to_string(),
        profiles,
        force_digital: false,
        macros,
        steppers,
    }
}

fn action_for(kind: &str) -> Action {
    match kind {
        "keypress" => Action::Keypress {
            modifiers: Modifiers::default(),
            key: KeyCode::KEY_A,
        },
        "controller_button" => Action::ControllerButton {
            button: KeyCode::BTN_SOUTH,
        },
        "macro" => Action::Macro {
            macro_id: MacroId::from("m"),
        },
        "step" => Action::Step {
            stepper: StepperId::from("s"),
            direction: StepDirection::Forward,
        },
        "profile_switch" => Action::ProfileSwitch {
            target: "P2".to_string(),
        },
        other => panic!("not a Binding Action kind: {other:?}"),
    }
}

fn trigger_for(name: &str) -> TriggerMode {
    match name {
        "fire_once" => TriggerMode::FireOnce,
        "hold_to_repeat" => TriggerMode::HoldToRepeat,
        "toggle" => TriggerMode::Toggle,
        "analog_repeat" => TriggerMode::AnalogRepeat,
        other => panic!("not a TriggerMode: {other:?}"),
    }
}

/// A trigger that is itself legal for `kind`, so an `action_kind_matrix` row
/// isolates "is this Action kind legal on this Input" without a trigger
/// violation confounding the verdict.
fn neutral_trigger_for(kind: &str) -> TriggerMode {
    match kind {
        "profile_switch" => TriggerMode::FireOnce,
        _ => TriggerMode::HoldToRepeat,
    }
}

/// The fixed two-member Chord (`{grid_r1c1, grid_r1c2}`) every `__chord__`
/// row is derived from — one Chord, so the subset/superset and
/// axis-conflict checks never fire on it.
fn chord_members() -> BTreeSet<Input> {
    [Input::Grid(1, 1), Input::Grid(1, 2)].into_iter().collect()
}

/// `Ok(())` from `validate` → `true`; any `Err(_)` → `false`.
fn trigger_verdict(input: &str, kind: &str, trigger: &str) -> bool {
    let binding = Binding {
        trigger: trigger_for(trigger),
        action: action_for(kind),
    };
    let mut config = seeded_config();
    let profile = config.profiles.get_mut("P1").expect("P1 seeded");
    if input == CHORD_SENTINEL {
        profile
            .chords_base
            .insert(ChordKey::new(chord_members()), binding);
    } else {
        let parsed: Input = input.parse().expect("a real Input string");
        profile.base.insert(parsed, binding);
    }
    config::validate(&config).is_ok()
}

fn action_kind_verdict(input: &str, kind: &str) -> bool {
    let mut config = seeded_config();
    let profile = config.profiles.get_mut("P1").expect("P1 seeded");

    if kind == "axis" {
        // An Axis assignment has no `Config` representation on a Chord at
        // all, so `__chord__ + axis` is simply never legal.
        if input == CHORD_SENTINEL {
            return false;
        }
        let parsed: Input = input.parse().expect("a real Input string");
        profile.axis_base.insert(parsed, AxisTarget::LeftTrigger);
        return config::validate(&config).is_ok();
    }

    let binding = Binding {
        trigger: neutral_trigger_for(kind),
        action: action_for(kind),
    };
    if input == CHORD_SENTINEL {
        profile
            .chords_base
            .insert(ChordKey::new(chord_members()), binding);
    } else {
        let parsed: Input = input.parse().expect("a real Input string");
        profile.base.insert(parsed, binding);
    }
    config::validate(&config).is_ok()
}

// --- hand-authored transformation example lists ------------------------------

/// `(name, fallback)` pairs for `config::slug_base` — Unicode, runs of
/// space/punctuation collapsing to one `-`, leading/trailing `-` trim,
/// empty-after-strip falling back.
const SLUG_EXAMPLES: [(&str, &str); 16] = [
    ("My Macro!!", "macro"),
    ("  leading spaces", "macro"),
    ("trailing spaces  ", "macro"),
    ("--dashes--", "macro"),
    ("a___b---c   d", "stepper"),
    ("!!!", "macro"),
    ("", "stepper"),
    ("Café Noir", "macro"),
    ("Ünïcödé", "macro"),
    ("UPPER CASE", "macro"),
    ("Screenshot Combo", "macro"),
    ("weapon wheel 2", "stepper"),
    ("tab\tnewline\n", "macro"),
    ("emoji 🎮 pad", "macro"),
    ("123", "macro"),
    ("mixed—em-dash", "macro"),
];

/// Member lists for `ChordKey`'s `Display` — each `Input` variant's own
/// internal order plus cross-variant mixes a plain alphabetical sort gets
/// wrong.
const CHORD_KEY_EXAMPLES: [&[&str]; 12] = [
    &["grid_r1c2", "grid_r1c1"],
    &["grid_r2c1", "grid_r1c5"],
    &["grid_r1c1", "mode_key"],
    &["thumbstick_down", "thumbstick_up"],
    &["thumbstick_right", "thumbstick_left"],
    &["wheel_scroll_down", "wheel_scroll_up"],
    &["wheel_middle", "wheel_scroll_up"],
    &["mode_key", "wheel_middle"],
    &["thumbstick_up", "grid_r4c5"],
    &["wheel_scroll_up", "thumbstick_left"],
    &["mode_key", "thumbstick_up", "grid_r1c1", "wheel_middle"],
    &["grid_r3c3", "grid_r3c2", "grid_r2c3"],
];

// --- JSON rendering (hand-rolled: serde_json is not a dependency) ------------

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_string_array(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|item| json_string(item)).collect();
    format!("[{}]", inner.join(", "))
}

fn push_section(out: &mut String, key: &str, rows: &[String], last: bool) {
    out.push_str("  ");
    out.push_str(&json_string(key));
    out.push_str(": [\n");
    for (index, row) in rows.iter().enumerate() {
        out.push_str("    ");
        out.push_str(row);
        out.push_str(if index + 1 < rows.len() { ",\n" } else { "\n" });
    }
    out.push_str(if last { "  ]\n" } else { "  ],\n" });
}

/// Builds the full fixture text — deterministic key order, 2-space pretty
/// print, trailing newline.
fn render_schema() -> String {
    let gamepad_buttons: Vec<String> = gamepad_button_codes()
        .iter()
        .map(|code| format!("{code:?}"))
        .collect();
    let axis_targets: Vec<String> = AxisTarget::ALL
        .iter()
        .map(|target| axis_target_str(*target).to_string())
        .collect();

    let inputs: Vec<String> = real_inputs()
        .iter()
        .map(ToString::to_string)
        .chain(std::iter::once(CHORD_SENTINEL.to_string()))
        .collect();

    let mut trigger_rows = Vec::new();
    for kind in BINDING_ACTION_KINDS {
        for input in &inputs {
            for trigger in ALL_TRIGGERS {
                let allowed = trigger_verdict(input, kind, trigger);
                trigger_rows.push(format!(
                    "{{\"action_kind\": {}, \"input\": {}, \"trigger\": {}, \"allowed\": {}}}",
                    json_string(kind),
                    json_string(input),
                    json_string(trigger),
                    allowed
                ));
            }
        }
    }

    let mut action_kind_rows = Vec::new();
    for input in &inputs {
        for kind in ALL_ACTION_KINDS {
            let allowed = action_kind_verdict(input, kind);
            action_kind_rows.push(format!(
                "{{\"input\": {}, \"action_kind\": {}, \"allowed\": {}}}",
                json_string(input),
                json_string(kind),
                allowed
            ));
        }
    }

    let slug_rows: Vec<String> = SLUG_EXAMPLES
        .iter()
        .map(|(name, fallback)| {
            format!(
                "{{\"name\": {}, \"fallback\": {}, \"slug\": {}}}",
                json_string(name),
                json_string(fallback),
                json_string(&config::slug_base(name, fallback))
            )
        })
        .collect();

    let chord_rows: Vec<String> = CHORD_KEY_EXAMPLES
        .iter()
        .map(|members| {
            let set: BTreeSet<Input> = members
                .iter()
                .map(|member| member.parse().expect("a real Input string"))
                .collect();
            let key = ChordKey::new(set).to_string();
            let members_owned: Vec<String> = members.iter().map(ToString::to_string).collect();
            format!(
                "{{\"members\": {}, \"key\": {}}}",
                json_string_array(&members_owned),
                json_string(&key)
            )
        })
        .collect();

    let mut out = String::from("{\n");
    push_section(
        &mut out,
        "gamepad_buttons",
        &gamepad_buttons
            .iter()
            .map(|name| json_string(name))
            .collect::<Vec<_>>(),
        false,
    );
    push_section(
        &mut out,
        "axis_targets",
        &axis_targets
            .iter()
            .map(|name| json_string(name))
            .collect::<Vec<_>>(),
        false,
    );
    push_section(&mut out, "trigger_matrix", &trigger_rows, false);
    push_section(&mut out, "action_kind_matrix", &action_kind_rows, false);
    push_section(&mut out, "slug_examples", &slug_rows, false);
    push_section(&mut out, "chord_key_examples", &chord_rows, true);
    out.push('}');
    out.push('\n');
    out
}

fn fixture_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("contract")
        .join("daemon-schema.json")
}

/// Derives the GUI-mirror contract fixture from the real `config::validate`
/// and asserts `daemon/contract/daemon-schema.json` matches — or, under
/// `ACHERON_BLESS=1`, rewrites it. See this module's doc comment.
#[test]
fn daemon_schema_fixture_is_current() {
    let rendered = render_schema();
    let path = fixture_path();

    if std::env::var("ACHERON_BLESS").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().expect("fixture path has a parent"))
            .expect("create daemon/contract/");
        std::fs::write(&path, &rendered).expect("write daemon-schema.json");
        return;
    }

    let on_disk = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "cannot read {} ({err}) — run `ACHERON_BLESS=1 cargo test --manifest-path daemon/Cargo.toml schema` to generate it",
            path.display()
        )
    });

    assert_eq!(
        rendered, on_disk,
        "daemon/contract/daemon-schema.json is stale — run \
         `ACHERON_BLESS=1 cargo test --manifest-path daemon/Cargo.toml schema` \
         and mirror the change into gui/acheron_gui/rules.py"
    );
}

/// Guard-rails on the fixture's own shape, so a generation bug can't quietly
/// bless a truncated file.
#[test]
fn rendered_schema_has_the_fully_enumerated_matrices() {
    let inputs = real_inputs().len() + 1; // + the `__chord__` sentinel
    assert_eq!(real_inputs().len(), 28);

    let rendered = render_schema();
    assert_eq!(
        rendered.matches("\"trigger\": ").count(),
        BINDING_ACTION_KINDS.len() * inputs * ALL_TRIGGERS.len()
    );
    assert_eq!(
        rendered.matches("\"action_kind\": ").count(),
        // every trigger_matrix row carries one too, plus the action_kind_matrix
        BINDING_ACTION_KINDS.len() * inputs * ALL_TRIGGERS.len() + ALL_ACTION_KINDS.len() * inputs
    );
}
