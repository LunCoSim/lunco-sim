//! Configurable command chords and cross-layer keyboard ownership.

use crate::InputBindingsSettings;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A scalar command parameter. The command owner validates its actual field type.
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub enum CommandBindingValue {
    Text(String),
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct CommandBindingArgument {
    pub name: String,
    pub value: CommandBindingValue,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct CommandKeyBinding {
    /// One simultaneous chord. Empty explicitly disables this binding id.
    pub keys: Vec<KeyCode>,
    pub command: String,
    #[serde(default)]
    pub arguments: Vec<CommandBindingArgument>,
    /// Modified application shortcuts may opt into text-editor focus.
    #[serde(default)]
    pub allow_in_text: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Reflect)]
pub struct InputBindingCollision {
    pub chord: String,
    pub displaced: String,
    pub winner: String,
}

#[derive(Resource, Default, Clone, Debug, PartialEq, Reflect)]
#[reflect(Resource)]
pub struct InputBindingConflicts(pub Vec<InputBindingCollision>);
impl lunco_core::SceneResource for InputBindingConflicts {}

fn normalized_key(key: KeyCode) -> KeyCode {
    match key {
        KeyCode::ControlRight | KeyCode::SuperLeft | KeyCode::SuperRight => KeyCode::ControlLeft,
        KeyCode::ShiftRight => KeyCode::ShiftLeft,
        KeyCode::AltRight => KeyCode::AltLeft,
        key => key,
    }
}

pub fn normalized_chord(keys: &[KeyCode]) -> Vec<KeyCode> {
    let mut keys: Vec<_> = keys.iter().copied().map(normalized_key).collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

pub fn shortcut_label(keys: &[KeyCode]) -> String {
    keys.iter()
        .map(|key| match normalized_key(*key) {
            KeyCode::ControlLeft => "Ctrl".into(),
            KeyCode::ShiftLeft => "Shift".into(),
            KeyCode::AltLeft => "Alt".into(),
            key => crate::key_label(&[key]),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Parse friendly shortcut spelling through Bevy's own KeyCode deserializer.
pub fn parse_shortcut(text: &str) -> Result<Vec<KeyCode>, String> {
    let mut keys = Vec::new();
    if text.trim().is_empty() {
        return Ok(keys);
    }
    for part in text.split('+').map(str::trim) {
        let name = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "command" => "ControlLeft".into(),
            "shift" => "ShiftLeft".into(),
            "alt" | "option" => "AltLeft".into(),
            _ if part.len() == 1 && part.as_bytes()[0].is_ascii_alphabetic() => {
                format!("Key{}", part.to_ascii_uppercase())
            }
            _ if part.len() == 1 && part.as_bytes()[0].is_ascii_digit() => format!("Digit{part}"),
            _ => part.to_owned(),
        };
        let key = KeyCode::deserialize(
            serde::de::value::StrDeserializer::<serde::de::value::Error>::new(&name),
        )
        .map_err(|error| error.to_string())?;
        keys.push(key);
    }
    if normalized_chord(&keys).len() != keys.len() {
        return Err("A shortcut cannot repeat a key or modifier".into());
    }
    Ok(keys)
}

pub fn chord_matches(chord: &[KeyCode], keys: &ButtonInput<KeyCode>) -> bool {
    if chord.is_empty() {
        return false;
    }
    let modifiers = [KeyCode::ControlLeft, KeyCode::ShiftLeft, KeyCode::AltLeft];
    let has = |key| {
        chord
            .iter()
            .any(|candidate| normalized_key(*candidate) == key)
    };
    let held = |key| {
        keys.get_pressed()
            .any(|candidate| normalized_key(*candidate) == key)
    };
    chord.iter().all(|key| held(normalized_key(*key)))
        && modifiers.iter().all(|key| has(*key) == held(*key))
}

pub fn chord_just_pressed(chord: &[KeyCode], keys: &ButtonInput<KeyCode>) -> bool {
    chord_matches(chord, keys)
        && keys.get_just_pressed().any(|key| {
            chord
                .iter()
                .any(|candidate| normalized_key(*candidate) == normalized_key(*key))
        })
}

fn claims(settings: &InputBindingsSettings) -> BTreeMap<Vec<KeyCode>, String> {
    let mut claims = BTreeMap::new();
    for (intent, keys) in &settings.bindings {
        for key in keys {
            claims.insert(normalized_chord(&[*key]), format!("intent:{intent}"));
        }
    }
    for (intent, chords) in &settings.key_chords {
        for chord in chords {
            claims.insert(normalized_chord(chord), format!("intent:{intent}"));
        }
    }
    for (id, binding) in &settings.command_bindings {
        if !binding.keys.is_empty() {
            claims.insert(normalized_chord(&binding.keys), format!("command:{id}"));
        }
    }
    claims
}

impl InputBindingsSettings {
    /// Higher-layer explicit physical chords displace lower-layer actions.
    /// The lower settings resource itself is never modified by composition.
    pub fn resolve_overrides(&self, overrides: &Self) -> (Self, Vec<InputBindingCollision>) {
        let claims = claims(overrides);
        let mut resolved = self.clone();
        let mut collisions = Vec::new();
        let mut retain = |keys: &[KeyCode], displaced: &str| {
            let Some(winner) = claims.get(&normalized_chord(keys)) else {
                return true;
            };
            if winner == displaced {
                return true;
            }
            collisions.push(InputBindingCollision {
                chord: shortcut_label(keys),
                displaced: displaced.into(),
                winner: winner.clone(),
            });
            false
        };
        for (intent, keys) in &mut resolved.bindings {
            keys.retain(|key| retain(&[*key], &format!("intent:{intent}")));
        }
        for (intent, chords) in &mut resolved.key_chords {
            chords.retain(|chord| retain(chord, &format!("intent:{intent}")));
        }
        for (id, binding) in &mut resolved.command_bindings {
            if !binding.keys.is_empty() && !retain(&binding.keys, &format!("command:{id}")) {
                binding.keys.clear();
            }
        }
        // An explicit intent override replaces both forms of that intent.
        for intent in overrides.bindings.keys().chain(overrides.key_chords.keys()) {
            resolved.bindings.remove(intent);
            resolved.key_chords.remove(intent);
        }
        resolved.bindings.extend(overrides.bindings.clone());
        resolved.key_chords.extend(overrides.key_chords.clone());
        resolved
            .command_bindings
            .extend(overrides.command_bindings.clone());
        resolved
            .pointer_bindings
            .extend(overrides.pointer_bindings.clone());
        resolved.look_button = overrides.look_button.clone().or(resolved.look_button);
        (resolved, collisions)
    }
}

pub fn report_binding_collisions(
    prefix: &str,
    collisions: &[InputBindingCollision],
    commands: &mut Commands,
) {
    for collision in collisions {
        commands.trigger(lunco_telemetry_core::TelemetryEvent {
            name: "input-binding-collision".into(),
            source: 0,
            severity: lunco_telemetry_core::Severity::Warning,
            data: lunco_telemetry_core::TelemetryValue::String(format!(
                "{prefix}: {} — {} overrides {}",
                collision.chord, collision.winner, collision.displaced
            )),
            timestamp: 0.0,
            sim_secs: 0.0,
            sim_tick: 0,
        });
    }
}
