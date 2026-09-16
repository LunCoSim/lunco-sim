//! Shared semantic input bindings.
//!
//! This package owns the user-editable keymap and its projection into Bevy's
//! semantic [`leafwing_input_manager`] map. Vessel actuation remains in
//! `lunco-controller`; UI/help/tutorial consumers depend on this small input
//! contract instead of pulling the controller's simulation adapter.

use bevy::prelude::*;
use leafwing_input_manager::prelude::InputMap;
use lunco_control_core::UserIntent;
use lunco_settings::{AppSettingsExt, SettingsSection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The bundled default keymap data. It is embedded so the same resolved
/// defaults are available on native and wasm targets without direct file I/O.
const KEYBINDINGS_JSON: &str = include_str!("../../../assets/config/keybindings.json");

fn default_look_button() -> String {
    "Right".into()
}

/// One exact pointer chord in the shared input configuration.
#[derive(Reflect, Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct PointerBinding {
    /// Physical button spelling, such as `Left`, `Right`, or `Middle`.
    pub button: String,
    /// Whether Alt must be held.
    #[serde(default)]
    pub alt: bool,
    /// Whether Shift must be held.
    #[serde(default)]
    pub shift: bool,
    /// Whether Control must be held.
    #[serde(default)]
    pub ctrl: bool,
}

impl PointerBinding {
    /// Check whether this binding matches one physical pointer chord.
    pub fn matches(&self, button: &str, alt: bool, shift: bool, ctrl: bool) -> bool {
        self.alt == alt
            && self.shift == shift
            && self.ctrl == ctrl
            && pointer_button_matches(&self.button, button)
    }
}

fn pointer_button_matches(configured: &str, event: &str) -> bool {
    let configured = configured.trim().to_ascii_lowercase();
    let event = event.trim().to_ascii_lowercase();
    match event.as_str() {
        "primary" => configured == "left" || configured == "primary",
        "secondary" => configured == "right" || configured == "secondary",
        "middle" => configured == "middle",
        _ => configured == event,
    }
}

#[derive(Deserialize)]
struct InputBindingsFile {
    #[serde(flatten)]
    bindings: BTreeMap<String, Vec<KeyCode>>,
    #[serde(default = "default_look_button")]
    look_button: String,
    #[serde(default)]
    pointer_bindings: BTreeMap<String, Vec<PointerBinding>>,
}

fn bundled_input_bindings() -> InputBindingsFile {
    serde_json::from_str(KEYBINDINGS_JSON)
        .expect("assets/config/keybindings.json must be valid input settings")
}

/// Resolved user input settings shared by avatar control, UI help, tutorials,
/// input injection, and authored pointer tools.
///
/// Deserialization is an override layer over the bundled defaults: omitted
/// entries inherit the current bundled values while an explicit empty array
/// remains unbound.
#[derive(Resource, Reflect, Serialize, Clone, PartialEq, Debug)]
#[reflect(Resource)]
pub struct InputBindingsSettings {
    /// Semantic intent name to Bevy key names.
    #[serde(flatten)]
    pub bindings: BTreeMap<String, Vec<KeyCode>>,
    /// Pointer button activating the semantic look intent.
    #[serde(default = "default_look_button")]
    pub look_button: String,
    /// Physical pointer chords mapped to authored semantic intent names.
    #[serde(default)]
    pub pointer_bindings: BTreeMap<String, Vec<PointerBinding>>,
}

impl Default for InputBindingsSettings {
    fn default() -> Self {
        let bundled = bundled_input_bindings();
        Self {
            bindings: bundled.bindings,
            look_button: bundled.look_button,
            pointer_bindings: bundled.pointer_bindings,
        }
    }
}

impl<'de> Deserialize<'de> for InputBindingsSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct StoredInputBindings {
            #[serde(flatten)]
            bindings: BTreeMap<String, Vec<KeyCode>>,
            #[serde(default = "default_look_button")]
            look_button: String,
            #[serde(default)]
            pointer_bindings: BTreeMap<String, Vec<PointerBinding>>,
        }

        let stored = StoredInputBindings::deserialize(deserializer)?;
        let bundled = bundled_input_bindings();
        let mut bindings = bundled.bindings;
        let mut pointer_bindings = bundled.pointer_bindings;
        bindings.extend(stored.bindings);
        pointer_bindings.extend(stored.pointer_bindings);
        Ok(Self {
            bindings,
            look_button: stored.look_button,
            pointer_bindings,
        })
    }
}

impl SettingsSection for InputBindingsSettings {
    const KEY: &'static str = "input_bindings";

    fn validate_section(&self) -> Result<(), String> {
        if self.look_button_value().is_none() {
            return Err(format!("invalid look_button '{}'", self.look_button));
        }
        for intent in self.bindings.keys() {
            if lunco_control_core::parse_user_intent(intent).is_none() {
                return Err(format!("unknown input intent '{intent}'"));
            }
        }
        for (intent, bindings) in &self.pointer_bindings {
            if intent.trim().is_empty() {
                return Err("pointer input intent must not be empty".to_string());
            }
            for binding in bindings {
                if parse_look_button(&binding.button).is_none() {
                    return Err(format!(
                        "invalid pointer button '{}' for intent '{intent}'",
                        binding.button
                    ));
                }
            }
        }
        Ok(())
    }
}

impl InputBindingsSettings {
    /// Build the live leafwing map from the resolved settings section.
    pub fn input_map(&self) -> Result<InputMap<UserIntent>, String> {
        let bindings = self.key_bindings()?;
        let Some(button) = self.look_button_value() else {
            return Err(format!("invalid look_button '{}'", self.look_button));
        };
        Ok(build_input_map(bindings, button))
    }

    /// Return resolved key bindings for help, tutorials, and input injection.
    pub fn key_bindings(&self) -> Result<Vec<(UserIntent, Vec<KeyCode>)>, String> {
        self.bindings
            .iter()
            .map(|(name, keys)| {
                lunco_control_core::parse_user_intent(name)
                    .map(|intent| (intent, keys.clone()))
                    .ok_or_else(|| format!("unknown input intent '{name}'"))
            })
            .collect()
    }

    /// Resolve a compact or Bevy key spelling against the current settings.
    pub fn key_code(&self, label: &str) -> Result<Option<KeyCode>, String> {
        let needle = label.trim();
        Ok(self
            .key_bindings()?
            .into_iter()
            .flat_map(|(_, keys)| keys.into_iter())
            .find(|key| {
                let debug = format!("{key:?}");
                debug.eq_ignore_ascii_case(needle)
                    || key_label(std::slice::from_ref(key)).eq_ignore_ascii_case(needle)
            }))
    }

    /// Resolve an exact physical pointer chord into semantic intent names.
    pub fn pointer_intents(&self, button: &str, alt: bool, shift: bool, ctrl: bool) -> Vec<String> {
        self.pointer_bindings
            .iter()
            .filter_map(|(intent, bindings)| {
                bindings
                    .iter()
                    .any(|binding| binding.matches(button, alt, shift, ctrl))
                    .then(|| intent.clone())
            })
            .collect()
    }

    /// Return a human-readable key or pointer label for authored help copy.
    pub fn label(&self, binding: &str) -> Option<String> {
        if binding == "look_button" {
            self.look_button_value()?;
            return Some(format!("{} mouse button", self.look_button.to_lowercase()));
        }
        let keys = self.bindings.get(binding)?;
        (!keys.is_empty()).then(|| key_label(keys))
    }

    /// Resolve the display label for a semantic intent used by a vessel.
    pub fn label_for_intent(&self, intent: UserIntent) -> Result<String, String> {
        let name = self
            .key_bindings()?
            .into_iter()
            .find_map(|(candidate, keys)| (candidate == intent).then_some(keys))
            .filter(|keys| !keys.is_empty())
            .map_or_else(|| "unbound".to_owned(), |keys| key_label(&keys));
        Ok(name)
    }

    fn look_button_value(&self) -> Option<MouseButton> {
        parse_look_button(&self.look_button)
    }
}

/// Resolve a user-facing label from the one shared input-bindings resource.
pub fn resolved_input_label(settings: &InputBindingsSettings, intent: UserIntent) -> String {
    settings
        .label_for_intent(intent)
        .unwrap_or_else(|_| intent.to_string())
}

/// Read the pointer button that activates semantic `Look`.
pub fn parse_look_button(name: &str) -> Option<MouseButton> {
    match name.trim().to_ascii_lowercase().as_str() {
        "left" => Some(MouseButton::Left),
        "middle" => Some(MouseButton::Middle),
        "back" => Some(MouseButton::Back),
        "forward" => Some(MouseButton::Forward),
        "right" => Some(MouseButton::Right),
        _ => None,
    }
}

/// Compact user-facing spelling for a list of Bevy keys.
pub fn key_label(keys: &[KeyCode]) -> String {
    keys.iter()
        .map(|key| {
            let name = format!("{key:?}");
            name.strip_prefix("Key").unwrap_or(&name).to_string()
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Parse input settings JSON and build its semantic input map.
pub fn input_map_from_json(json: &str) -> Result<InputMap<UserIntent>, String> {
    let settings: InputBindingsSettings =
        serde_json::from_str(json).map_err(|error| format!("invalid input bindings: {error}"))?;
    settings.input_map()
}

fn build_input_map(
    bindings: Vec<(UserIntent, Vec<KeyCode>)>,
    button: MouseButton,
) -> InputMap<UserIntent> {
    use leafwing_input_manager::prelude::*;
    use lunco_control_core::UserIntent::{Look, Zoom};

    let mut input_map = InputMap::default();
    for (intent, keys) in bindings {
        for key in keys {
            input_map.insert(intent, key);
        }
    }
    input_map.insert_dual_axis(Look, DualAxislikeChord::new(button, MouseMove::default()));
    input_map.insert_axis(Zoom, MouseScrollAxis::Y);
    input_map
}

/// Install the settings resource and project changed settings to all live
/// semantic input surfaces.
pub struct InputBindingsPlugin;

impl Plugin for InputBindingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InputBindingsSettings>()
            .register_type::<InputBindingsSettings>()
            .register_settings_section::<InputBindingsSettings>()
            .add_systems(Update, refresh_live_input_maps);
    }
}

fn refresh_live_input_maps(
    settings: Res<InputBindingsSettings>,
    mut maps: Query<&mut InputMap<UserIntent>>,
) {
    if !settings.is_changed() {
        return;
    }
    let Ok(map) = settings.input_map() else {
        error!("[input] refusing to project invalid input bindings settings");
        return;
    };
    for mut live in &mut maps {
        *live = map.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::InputPlugin;
    use leafwing_input_manager::prelude::{
        ActionState, Buttonlike, DualAxislike, InputManagerPlugin, MouseMove,
    };

    #[test]
    fn configured_pointer_button_reaches_the_semantic_axis() {
        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            InputPlugin,
            InputManagerPlugin::<UserIntent>::default(),
        ));
        let entity = app
            .world_mut()
            .spawn((
                ActionState::<UserIntent>::default(),
                input_map_from_json(r#"{"look_button":"Middle"}"#).expect("valid pointer binding"),
            ))
            .id();

        MouseButton::Right.press(app.world_mut());
        MouseMove::default().set_axis_pair(app.world_mut(), Vec2::new(4.0, -2.0));
        app.update();
        assert_eq!(
            app.world()
                .entity(entity)
                .get::<ActionState<UserIntent>>()
                .expect("action state")
                .axis_pair(&UserIntent::Look),
            Vec2::ZERO
        );

        MouseButton::Middle.press(app.world_mut());
        MouseMove::default().set_axis_pair(app.world_mut(), Vec2::new(4.0, -2.0));
        app.update();
        assert_eq!(
            app.world()
                .entity(entity)
                .get::<ActionState<UserIntent>>()
                .expect("action state")
                .axis_pair(&UserIntent::Look),
            Vec2::new(4.0, -2.0)
        );
    }

    #[test]
    fn invalid_pointer_binding_is_rejected() {
        assert!(parse_look_button("sideways").is_none());
        assert!(input_map_from_json(r#"{"look_button":"sideways"}"#).is_err());
    }

    #[test]
    fn pointer_intents_are_exact_and_configurable() {
        let settings = InputBindingsSettings::default();
        assert_eq!(
            settings.pointer_intents("primary", true, false, false),
            vec!["route.add_point".to_string()]
        );
        assert!(
            settings
                .pointer_intents("primary", true, true, false)
                .is_empty()
        );
        assert_eq!(
            settings.pointer_intents("secondary", false, false, false),
            vec!["route.context".to_string()]
        );

        let rebound: InputBindingsSettings = serde_json::from_str(
            r#"{
                "pointer_bindings": {
                    "route.add_point": [{"button":"Middle", "ctrl":true}]
                }
            }"#,
        )
        .expect("valid pointer input override");
        assert!(
            rebound
                .pointer_intents("primary", true, false, false)
                .is_empty()
        );
        assert_eq!(
            rebound.pointer_intents("middle", false, false, true),
            vec!["route.add_point".to_string()]
        );
        rebound
            .validate_section()
            .expect("pointer input override is valid");
    }

    #[test]
    fn labels_and_persisted_overrides_follow_the_shared_keymap() {
        let settings = InputBindingsSettings::default();
        assert_eq!(settings.key_code("W").unwrap(), Some(KeyCode::KeyW));
        assert_eq!(settings.key_code("KeyG").unwrap(), Some(KeyCode::KeyG));
        assert_eq!(settings.key_code("not-bound").unwrap(), None);
        assert_eq!(
            resolved_input_label(&settings, UserIntent::MoveForward),
            "W"
        );
        assert_eq!(resolved_input_label(&settings, UserIntent::MoveDown), "Q");

        let rebound: InputBindingsSettings =
            serde_json::from_str(r#"{"forward":["KeyI"],"yaw_left":[]}"#)
                .expect("valid input override");
        assert_eq!(rebound.key_code("KeyI").unwrap(), Some(KeyCode::KeyI));
        assert_eq!(rebound.key_code("KeyW").unwrap(), None);
        assert_eq!(resolved_input_label(&rebound, UserIntent::MoveForward), "I");
        assert_eq!(
            resolved_input_label(&rebound, UserIntent::MoveDown),
            "unbound"
        );
    }

    #[test]
    fn bundled_keybindings_parse_and_build() {
        let settings = InputBindingsSettings::default();
        settings
            .validate_section()
            .expect("bundled keybindings must be valid");
        assert_eq!(settings.look_button, "Right");
        assert!(settings.key_bindings().unwrap().len() >= 8);
        settings.input_map().expect("bundled map must build");
    }
}
