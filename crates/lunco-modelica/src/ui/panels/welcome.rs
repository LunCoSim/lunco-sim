//! Welcome center placeholder.
//!
//! The Modelica workspace's center slot is populated at runtime by
//! multi-instance model tabs opened from the Package Browser. Until
//! the first tab opens — and after the user closes every model tab —
//! the dock would otherwise be empty, and `WorkbenchLayout::rebuild_dock`
//! bails when center is empty (skipping the side/right/bottom splits
//! too). This singleton panel sits in center so the cross layout
//! always has something to anchor.
//!
//! Non-closable so the user can't accidentally remove the anchor.
//! Shows a short hint about opening a model.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

/// Panel id under which the welcome placeholder is registered.
pub const WELCOME_PANEL_ID: PanelId = PanelId("modelica_welcome");

/// The welcome placeholder panel. Zero-sized.
pub struct WelcomePanel;

impl Panel for WelcomePanel {
    fn id(&self) -> PanelId {
        WELCOME_PANEL_ID
    }

    fn title(&self) -> String {
        "🏠 Welcome".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn closable(&self) -> bool {
        false
    }

    fn render(&mut self, ui: &mut egui::Ui, _world: &mut World) {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.heading("LunCoSim Modelica Workbench");
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new(
                    "Click a model in the 📚 Package Browser to open it as a tab.",
                )
                .size(13.0)
                .color(egui::Color32::GRAY),
            );
            ui.label(
                egui::RichText::new("Or ➕ create a new model to start from scratch.")
                    .size(13.0)
                    .color(egui::Color32::GRAY),
            );
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new(
                    "Shortcuts: Ctrl+S Save · Ctrl+Z Undo · Ctrl+Shift+Z Redo · F5 Compile",
                )
                .size(10.0)
                .color(egui::Color32::DARK_GRAY),
            );
        });
    }
}
