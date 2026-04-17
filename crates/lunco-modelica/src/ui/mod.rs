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
use lunco_workbench::{Workspace, WorkspaceId, WorkbenchAppExt, WorkbenchLayout, PanelId};

pub mod state;
pub use state::*;

mod panels;

use crate::ModelicaModel;

/// Fan queued document-change notifications out as observer triggers.
///
/// The registry accumulates ids on every `checkpoint_source` / `allocate_*`
/// / explicit `mark_changed` call. This system drains the queue once per
/// frame and emits [`DocumentChanged`] so any observer (panel re-render,
/// diagram re-parse, plot variable-list refresh, …) can react without
/// polling document generation counters.
fn drain_document_changes(
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut commands: Commands,
) {
    for doc in registry.drain_pending_changes() {
        commands.trigger(DocumentChanged { doc });
    }
}

/// Drop the document linked to a despawned `ModelicaModel` entity, and
/// any compile-state bookkeeping attached to that document.
///
/// Behavior preserved from the entity-keyed era: when an entity is
/// despawned, its backing [`ModelicaDocument`](crate::document::ModelicaDocument)
/// is also removed. The long-term design lets documents outlive entities
/// (edit-without-running, cosim re-spawn), so this will become opt-in
/// once the tab/view layer can explicitly unload a document.
fn cleanup_removed_documents(
    mut removed: RemovedComponents<ModelicaModel>,
    registry: Option<ResMut<ModelicaDocumentRegistry>>,
    compile_states: Option<ResMut<CompileStates>>,
) {
    let Some(mut registry) = registry else { return };
    let mut compile_states = compile_states;
    for entity in removed.read() {
        if let Some(doc) = registry.unlink_entity(entity) {
            registry.remove_document(doc);
            if let Some(states) = compile_states.as_mut() {
                states.remove(doc);
            }
        }
    }
}

/// The Modelica workbench's default workspace preset.
///
/// Mirrors the "Analyze — Modelica deep dive" slot map from the workbench
/// design doc ([`docs/architecture/11-workbench.md`] § 4).
pub struct AnalyzeWorkspace;

impl Workspace for AnalyzeWorkspace {
    fn id(&self) -> WorkspaceId { WorkspaceId("modelica_analyze") }
    fn title(&self) -> String { "📊 Analyze".into() }
    fn apply(&self, layout: &mut WorkbenchLayout) {
        layout.set_activity_bar(false);
        layout.set_side_browser(Some(PanelId("modelica_package_browser")));
        layout.set_center(vec![
            PanelId("modelica_code_preview"),
            PanelId("modelica_diagram_preview"),
        ]);
        layout.set_active_center_tab(0);
        layout.set_right_inspector(Some(PanelId("modelica_inspector")));
        layout.set_bottom(Some(PanelId("modelica_console")));
    }
}

/// Plugin that registers all Modelica workbench UI panels.
///
/// Panels are entity viewers — they watch `WorkbenchState.selected_entity`
/// and render data for the active `ModelicaModel`. They work in any context:
/// standalone workbench, 3D overlay, or mission dashboard.
pub struct ModelicaUiPlugin;

impl Plugin for ModelicaUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkbenchState>()
            .init_resource::<ModelicaDocumentRegistry>()
            .init_resource::<CompileStates>()
            .init_resource::<panels::diagram::DiagramState>()
            .init_resource::<panels::diagram::DiagramTheme>()
            .init_resource::<panels::code_editor::EditorBufferState>()
            .insert_resource(panels::package_browser::PackageTreeCache::new())
            .add_systems(Update, panels::package_browser::handle_package_loading_tasks)
            .add_systems(Update, cleanup_removed_documents)
            .add_systems(Update, drain_document_changes)
            .register_panel(panels::package_browser::PackageBrowserPanel)
            .register_panel(panels::code_editor::CodeEditorPanel)
            .register_panel(panels::telemetry::TelemetryPanel)
            .register_panel(panels::graphs::GraphsPanel)
            .register_panel(panels::diagram::DiagramPanel)
            .register_panel(panels::inspector::InspectorPanel)
            .register_workspace(AnalyzeWorkspace);
    }
}
