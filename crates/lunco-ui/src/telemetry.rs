//! Telemetry panel — WorkbenchPanel implementation.
//!
//! Shows avatar status, surface mode info, lat/lon/alt, camera mode,
//! and navigation buttons (Return to Orbit).

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

/// Telemetry panel — shows avatar status and surface coordinates.
pub struct TelemetryPanel;

impl WorkbenchPanel for TelemetryPanel {
    fn id(&self) -> &str { "telemetry" }
    fn title(&self) -> String { "Telemetry".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Telemetry requires world access.");
    }

    fn ui_world(&mut self, ui: &mut egui::Ui, _world: &mut World) {
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);
        ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);

        ui.label("Telemetry moved to Avatar Status panel.");
    }
}
