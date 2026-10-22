//! Shared presentation preferences for engineering diagram hosts and widgets.
use bevy::prelude::*;

/// Hiding controls leaves authored topology, diagrams and live values intact.
/// The central settings owner persists this preference, never model source.
#[derive(Resource, Reflect, serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
#[reflect(Resource)]
#[serde(default)]
pub struct DiagramPresentationSettings {
    pub show_controls: bool,
    /// Preserve deliberate authored backgrounds/text instead of the source
    /// library's automatic dark-theme color inversion.
    pub use_authored_colors: bool,
}

impl Default for DiagramPresentationSettings {
    fn default() -> Self {
        Self { show_controls: true, use_authored_colors: false }
    }
}

impl lunco_settings::SettingsSection for DiagramPresentationSettings {
    const KEY: &'static str = "diagram_presentation";
}
