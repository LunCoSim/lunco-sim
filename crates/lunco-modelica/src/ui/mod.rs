//! Modelica workbench UI — panels as entity viewers.
//!
//! ## Architecture: Panels Are Entity Viewers
//!
//! Each panel watches a `ModelicaModel` entity and renders its data.
//! Panels don't know if they're in a standalone workbench, a floating overlay
//! on a 3D viewport, or a mission dashboard — they just watch the selected entity.
//!
//! ```text
//!                    ModelicaModel entity
//!                    (attached to 3D objects
//!                     or standalone workbench)
//!                              │
//!           ┌──────────────────┼──────────────────┐
//!           ▼                  ▼                  ▼
//!     DiagramPanel      CodeEditorPanel    TelemetryPanel
//!     (egui-snarl)      (text editor)      (params/inputs)
//! ```
//!
//! ## Selection Bridge
//!
//! `WorkbenchState.selected_entity` is the single source of truth.
//! Any context can trigger an editor by setting it:
//! - Package Browser: click a model in the tree
//! - 3D viewport: click a rover's solar panel
//! - Colony tree: select a subsystem node
//!
//! ```rust,ignore
//! // Anywhere in the codebase:
//! fn open_modelica_editor(world: &mut World, entity: Entity) {
//!     if let Some(mut state) = world.get_resource_mut::<WorkbenchState>() {
//!         state.selected_entity = Some(entity);
//!     }
//!     // Panels auto-update because they watch WorkbenchState
//! }
//! ```
//!
//! ## Panel Layout
//!
//! bevy_workbench auto-assigns panel slots by ID convention:
//!
//! | ID Pattern         | Auto-Slot | Default Position  |
//! |--------------------|-----------|-------------------|
//! | contains "inspector" | Right   | Right dock        |
//! | contains "console"   | Bottom  | Bottom dock       |
//! | contains "preview"   | Center  | Center tab        |
//! | (no match)           | Left    | Left dock         |
//!
//! Users can drag, split, tab, and float panels freely.
//! Layout persists across sessions via bevy_workbench persistence.
//!
//! ## Panels
//!
//! - **Package Browser** (left dock) — Dymola-style library tree, click to open
//! - **Code Editor** (center tab) — source code editing, compile & run
//! - **Diagram** (center tab) — component block diagram via egui-snarl
//! - **Telemetry** (right dock) — parameters, inputs, variable toggles
//! - **Graphs** (bottom dock) — time-series plots of simulation variables

use bevy::prelude::*;
use bevy_workbench::WorkbenchApp;

pub mod state;
pub use state::*;

mod panels;

/// Plugin that registers all Modelica workbench UI panels.
///
/// Panels are entity viewers — they watch `WorkbenchState.selected_entity`
/// and render data for the active `ModelicaModel`. They work in any context:
/// standalone workbench, 3D overlay, or mission dashboard.
pub struct ModelicaUiPlugin;

impl Plugin for ModelicaUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkbenchState>()
            .init_resource::<panels::diagram::DiagramState>()
            .init_resource::<panels::diagram::DiagramTheme>()
            .init_resource::<panels::code_editor::EditorBufferState>()
            .insert_resource(panels::package_browser::PackageTreeCache::new())
            .add_systems(Update, panels::package_browser::handle_package_loading_tasks)
            .register_panel(panels::package_browser::PackageBrowserPanel)
            .register_panel(panels::code_editor::CodeEditorPanel)
            .register_panel(panels::telemetry::TelemetryPanel)
            .register_panel(panels::graphs::GraphsPanel)
            .register_panel(panels::diagram::DiagramPanel);
    }
}
