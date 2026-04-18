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

pub mod commands;
pub use commands::{CompileModel, CreateNewScratchModel, ModelicaCommandsPlugin};

pub mod panels;
pub mod viz;

use crate::ModelicaModel;

/// Fan queued document lifecycle notifications out as observer triggers.
///
/// The registry accumulates ids on every mutation (allocate → Opened +
/// Changed, `checkpoint_source` with new text → Changed, explicit
/// `mark_changed` after `host_mut` undo/redo → Changed, `remove_document`
/// → Closed). This system drains all three queues once per frame and
/// emits the matching generic events from [`lunco_doc_bevy`] so any
/// observer (panel re-render, diagram re-parse, plot variable-list
/// refresh, Twin journal, …) reacts without polling generation
/// counters.
///
/// Fire order per frame: Opened, Changed, Closed. Opened-before-Changed
/// means subscribers that key on "track docs I've seen Opened for" can
/// safely skip Changed events for unknown ids.
fn drain_document_changes(
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut commands: Commands,
) {
    for doc in registry.drain_pending_opened() {
        commands.trigger(lunco_doc_bevy::DocumentOpened::local(doc));
    }
    for doc in registry.drain_pending_changes() {
        commands.trigger(lunco_doc_bevy::DocumentChanged::local(doc));
    }
    for doc in registry.drain_pending_closed() {
        commands.trigger(lunco_doc_bevy::DocumentClosed::local(doc));
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
    signals: Option<ResMut<lunco_viz::SignalRegistry>>,
    viz_registry: Option<ResMut<lunco_viz::VisualizationRegistry>>,
) {
    let Some(mut registry) = registry else { return };
    let mut compile_states = compile_states;
    let mut signals = signals;
    let mut viz_registry = viz_registry;
    for entity in removed.read() {
        if let Some(doc) = registry.unlink_entity(entity) {
            registry.remove_document(doc);
            if let Some(states) = compile_states.as_mut() {
                states.remove(doc);
            }
        }
        // Drop every registered signal + plot binding for this entity
        // so stale plots don't keep reading the last values forever.
        if let Some(sigs) = signals.as_mut() {
            sigs.drop_entity(entity);
        }
        if let Some(reg) = viz_registry.as_mut() {
            crate::ui::viz::drop_entity_bindings(reg, entity);
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
        // Center is seeded with no singleton tab — model views are
        // multi-instance tabs opened dynamically by the Package Browser
        // (one tab per open document). An app that boots with a
        // default model can pre-open a tab after setup via
        // `WorkbenchLayout::open_instance(MODEL_VIEW_KIND, doc.raw())`.
        //
        // Keep a placeholder center tab so the dock's cross layout
        // still builds on apps with nothing open yet. When the first
        // real model tab opens, the placeholder stays docked next
        // to it — users can close it.
        layout.set_center(vec![PanelId("modelica_welcome")]);
        layout.set_active_center_tab(0);
        // Right dock gets two tabs: Inspector (params/variables) and
        // the Component Palette (MSL instantiation). Figma / Unreal
        // pattern — asset browser on the right, always visible while
        // the user is working in the center canvas.
        layout.set_right_inspector_tabs(vec![
            PanelId("modelica_inspector"),
            PanelId("modelica_component_palette"),
        ]);
        // Bottom dock: Graphs first so it's the default active tab —
        // the simulation plot is what a user running a model wants
        // to see on landing, not the log stream. Console stays one
        // click away for compile / save / error output (VS Code's
        // Terminal/Output/Problems pattern, just with a different
        // default active tab).
        layout.set_bottom_tabs(vec![
            PanelId("modelica_graphs"),
            PanelId("modelica_diagnostics"),
            PanelId("modelica_console"),
        ]);
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
        // Twin-level change journal subscribes to the generic document
        // lifecycle events this plugin fires. One journal per App —
        // adding the plugin multiple times is a no-op on `init_resource`.
        app.add_plugins(lunco_doc_bevy::TwinJournalPlugin);

        // Intent layer: key chords → EditorIntent. Domain resolvers
        // (installed by ModelicaCommandsPlugin below) translate intents
        // into concrete commands for the docs they own.
        app.add_plugins(lunco_doc_bevy::EditorIntentPlugin);

        // Command bus for Modelica documents — Undo / Redo / Save /
        // Close (generic) + Compile (domain-specific) — plus the
        // EditorIntent resolver. UI buttons, keyboard shortcuts,
        // scripts, and the remote API all funnel through these.
        app.add_plugins(ModelicaCommandsPlugin);

        app.init_resource::<WorkbenchState>()
            .init_resource::<ModelicaDocumentRegistry>()
            .init_resource::<CompileStates>()
            .init_resource::<panels::model_view::ModelTabs>()
            .init_resource::<panels::diagram::DiagramState>()
            .init_resource::<panels::diagram::DiagramTheme>()
            .init_resource::<panels::code_editor::EditorBufferState>()
            .init_resource::<panels::palette::PaletteState>()
            .init_resource::<panels::diagram::ModelSignatureCache>()
            .init_resource::<panels::console::ConsoleLog>()
            .init_resource::<panels::diagnostics::DiagnosticsLog>()
            .insert_resource(panels::package_browser::PackageTreeCache::new())
            .add_systems(Update, panels::package_browser::handle_package_loading_tasks)
            .add_systems(Update, cleanup_removed_documents)
            .add_systems(Update, drain_document_changes)
            .add_systems(Update, panels::diagnostics::refresh_diagnostics)
            .add_systems(Startup, register_settings_menu)
            .register_panel(panels::package_browser::PackageBrowserPanel)
            .register_panel(panels::welcome::WelcomePanel)
            .register_panel(panels::telemetry::TelemetryPanel)
            .register_panel(panels::graphs::GraphsPanel)
            .register_panel(panels::console::ConsolePanel)
            .register_panel(panels::diagnostics::DiagnosticsPanel)
            .register_panel(panels::inspector::InspectorPanel)
            .register_panel(panels::canvas_diagram::CanvasDiagramPanel)
            .init_resource::<panels::canvas_diagram::CanvasDiagramState>()
            .init_resource::<panels::canvas_diagram::DrillInLoads>()
            .add_systems(Update, panels::canvas_diagram::drive_drill_in_loads)
            .register_panel(panels::palette::ComponentPalettePanel)
            // Multi-instance: one tab per open document. Instances are
            // opened at runtime by the Package Browser.
            .register_instance_panel(panels::model_view::ModelViewPanel::default())
            .register_workspace(AnalyzeWorkspace);
    }
}

/// Push Modelica editor preferences onto the application-wide
/// Settings menu. Lives in the workbench Settings dropdown rather
/// than a per-panel gear button — keeps editor toolbar tidy and
/// all prefs discoverable in one place.
fn register_settings_menu(world: &mut World) {
    use bevy_egui::egui;
    let Some(mut layout) = world
        .get_resource_mut::<lunco_workbench::WorkbenchLayout>()
    else {
        return;
    };
    layout.register_settings(|ui, world| {
        ui.label(egui::RichText::new("Code Editor").weak().small());
        let mut buf = world.resource_mut::<panels::code_editor::EditorBufferState>();
        ui.checkbox(&mut buf.word_wrap, "Word wrap")
            .on_hover_text("Wrap long lines at editor width");
        ui.checkbox(&mut buf.auto_indent, "Auto indent")
            .on_hover_text("Copy previous line's indent on Enter");
    });
}
