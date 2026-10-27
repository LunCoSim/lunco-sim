//! Which assets the browsers list — the persisted "show test content" pref.
//!
//! Test scenes and scenarios live under `tests/` directories
//! (`assets/scenes/tests/`, `assets/scenarios/tests/`) and outnumber the scenes a
//! person actually opens. Asset browsers and the Scenarios library hide them
//! by default. Scenarios always exposes Twin tests through Test Scenarios.
//!
//! The setting governs LISTING only. Loading is never filtered — `scene_test`
//! takes a path, and a scene referencing `scenarios/tests/…` resolves it whether
//! or not any menu shows it.

use bevy::prelude::*;
use bevy_egui::egui;
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

/// Push the filter into the workbench **Settings** menu, alongside every other
/// persisted view pref.
pub(crate) fn register_settings_submenu(world: &mut World) {
    let Some(mut menus) = world.get_resource_mut::<lunco_workbench_core::WorkbenchMenuRegistry>()
    else {
        return;
    };
    menus.register_settings_submenu("Asset browser", |ui, ctx| {
        ui.label(
            egui::RichText::new("Assets")
                .text_style(lunco_theme::TypographyRole::Label.text_style())
                .weak(),
        );
        let Some(mut settings) = ctx.resource::<AssetVisibilitySettings>().copied() else {
            return;
        };
        let original = settings;
        ui.checkbox(&mut settings.show_test_assets, "Show test scenes")
            .on_hover_text(
                "Show assets under `tests/` in file browsers and add Library tests to Scenarios. \
                 Test Scenarios from open Twins are always available.",
            );
        if settings != original {
            ctx.set_resource(settings);
        }
    });
}
