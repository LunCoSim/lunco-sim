//! Which assets the browsers list — the persisted "show test content" pref.
//!
//! Test scenes and scenarios live under `tests/` directories
//! (`assets/scenes/tests/`, `assets/scenarios/tests/`) and outnumber the scenes a
//! person actually opens. Asset browsers and the Scenarios library hide them
//! by default. **Settings → Sim Core** controls library tests. Scenarios always
//! exposes Twin tests in each Twin's **Test Scenes** submenu.
//!
//! The setting governs LISTING only. Loading is never filtered — `scene_test`
//! takes a path, and a scene referencing `scenarios/tests/…` resolves it whether
//! or not any menu shows it.

use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};
use lunco_settings::SettingsSection;
use serde::{Deserialize, Serialize};

/// Persisted browser-visibility prefs.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub struct AssetVisibilitySettings {
    /// List scenes and scenarios under `tests/` in the asset browsers.
    pub show_test_assets: bool,
}

impl SettingsSection for AssetVisibilitySettings {
    const KEY: &'static str = "asset_visibility";
}

/// Set the persisted test-asset visibility preference selected by Rhai policy.
#[Command(default)]
pub struct SetShowTestAssets {
    /// Whether asset consumers include test content in their listings.
    pub show: bool,
}

#[on_command(SetShowTestAssets)]
fn on_set_show_test_assets(
    trigger: On<SetShowTestAssets>,
    mut settings: ResMut<AssetVisibilitySettings>,
) {
    settings.show_test_assets = trigger.event().show;
}

register_commands!(on_set_show_test_assets);
