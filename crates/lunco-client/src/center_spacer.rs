//! Center spacer panel — reserves space in the center of the dock for 3D scene.
//!
//! This invisible panel has no background (transparent) and absorbs no input,
//! so the 3D scene shows through.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

/// Invisible center panel that reserves space for the 3D scene.
/// Named "preview" so bevy_workbench auto-places it in the Center slot.
pub struct CenterSpacer;

impl WorkbenchPanel for CenterSpacer {
    fn id(&self) -> &str { "preview_3d" }
    fn title(&self) -> String { "3D View".into() }
    fn closable(&self) -> bool { false }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { false }

    /// Transparent — lets 3D scene show through.
    fn bg_color(&self) -> Option<egui::Color32> { None }

    /// Hide the tab bar — this panel acts as a viewport for the 3D scene.
    fn hide_tab(&self) -> bool { true }

    fn ui(&mut self, _ui: &mut egui::Ui) {
        // No UI — just reserves space
    }
}

/// Plugin that registers the center spacer panel.
pub struct CenterSpacerPlugin;

impl Plugin for CenterSpacerPlugin {
    fn build(&self, app: &mut App) {
        use bevy_workbench::WorkbenchApp;
        app.register_panel(CenterSpacer);
    }
}
