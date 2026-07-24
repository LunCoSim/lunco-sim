//! Which assets the browsers list — the persisted "show test content" pref.
//!
//! Test scenes and scenarios live under `tests/` directories
//! (`assets/scenes/tests/`, `assets/scenarios/tests/`) and outnumber the scenes a
//! person actually opens. Off by default so the Scene menu is the list of things
//! worth opening; ONE checkbox away, because a test scene hidden from its own
//! author is how a broken one goes unnoticed.
//!
//! The setting governs LISTING only. Loading is never filtered — `scene_test`
//! takes a path, and a scene referencing `scenarios/tests/…` resolves it whether
//! or not any menu shows it.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};

/// Persisted browser-visibility prefs.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct AssetVisibilitySettings {
    /// List scenes and scenarios under `tests/` in the asset browsers.
    pub show_test_assets: bool,
}

impl Default for AssetVisibilitySettings {
    fn default() -> Self {
        Self {
            show_test_assets: false,
        }
    }
}

impl SettingsSection for AssetVisibilitySettings {
    const KEY: &'static str = "asset_visibility";
}

/// Push the filter into the workbench **Settings** menu, alongside every other
/// persisted view pref.
pub(crate) fn register_settings_menu(world: &mut World) {
    let Some(mut layout) = world.get_resource_mut::<lunco_workbench::WorkbenchLayout>() else {
        return;
    };
    layout.register_settings(|ui, world| {
        ui.label(egui::RichText::new("Assets").weak().small());
        let mut settings = world.resource_mut::<AssetVisibilitySettings>();
        ui.checkbox(&mut settings.show_test_assets, "Show test scenes")
            .on_hover_text(
                "Scenes and scenarios under `tests/` — rigs that exist to be run by \
                 `scripts/run_scene_tests.sh` and assert a verdict, not to be opened \
                 and looked at. Hidden by default so the Scene menu lists what is \
                 worth opening.",
            );
    });
}
