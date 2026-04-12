//! Logs panel — system log output for Modelica compilation and simulation.

use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;

use crate::ui::WorkbenchState;

/// Logs panel — system log output.
pub struct LogsPanel;

impl WorkbenchPanel for LogsPanel {
    fn id(&self) -> &str { "modelica_timeline" }
    fn title(&self) -> String { "📋 Logs".into() }
    fn closable(&self) -> bool { true }
    fn default_visible(&self) -> bool { true }
    fn needs_world(&self) -> bool { true }

    fn bg_color(&self) -> Option<egui::Color32> {
        Some(egui::Color32::from_rgb(35, 35, 40))
    }

    fn ui(&mut self, _ui: &mut egui::Ui) {}

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let Some(state) = world.get_resource::<WorkbenchState>() else { return };

        if state.logs.is_empty() {
            ui.label("No logs yet.");
            return;
        }

        egui::ScrollArea::vertical()
            .id_salt("modelica_logs_scroll")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for log in &state.logs {
                    ui.label(log);
                }
            });
    }
}
