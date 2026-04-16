//! Telemetry panel — WorkbenchPanel implementation.
//!
//! Shows avatar status, surface mode info, lat/lon/alt, camera mode,
//! and navigation buttons (Return to Orbit).

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

/// Telemetry panel — shows avatar status and surface coordinates.
pub struct TelemetryPanel;

impl Panel for TelemetryPanel {
    fn id(&self) -> PanelId { PanelId("telemetry") }
    fn title(&self) -> String { "Telemetry".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }

    fn render(&mut self, ui: &mut egui::Ui, _world: &mut World) {
        ui.style_mut().visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);
        ui.style_mut().visuals.widgets.inactive.bg_fill = egui::Color32::from_rgba_unmultiplied(30, 30, 35, 230);

        ui.label("Telemetry moved to Avatar Status panel.");
    }
}
