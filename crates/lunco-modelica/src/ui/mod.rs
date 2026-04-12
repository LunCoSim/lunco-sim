//! Modelica workbench UI.
//!
//! Panels:
//! - Library Browser (left dock)
//! - Code Editor (center viewport, no tab)
//! - Telemetry (right dock)
//! - Graphs (bottom dock)
//! - Logs (bottom dock)

use bevy::prelude::*;
use bevy_workbench::WorkbenchApp;

pub mod state;
pub use state::*;

mod panels;

/// Plugin that registers all Modelica workbench UI panels.
pub struct ModelicaUiPlugin;

impl Plugin for ModelicaUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkbenchState>()
            .register_panel(panels::library_browser::LibraryBrowserPanel)
            .register_panel(panels::code_editor::CodeEditorPanel)
            .register_panel(panels::telemetry::TelemetryPanel)
            .register_panel(panels::graphs::GraphsPanel)
            .register_panel(panels::logs::LogsPanel);
    }
}
