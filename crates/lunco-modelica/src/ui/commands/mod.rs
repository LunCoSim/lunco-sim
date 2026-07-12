//! Command bus for Modelica documents.

use bevy::prelude::*;
use lunco_core::register_commands;
use lunco_doc::DocumentId;

pub mod compile;
pub mod doc;
pub mod lifecycle;
pub mod nav;
pub mod diagram;
pub mod sim;
pub mod plot;
pub mod intent;
pub mod status;
pub mod util;
pub mod inspect;

// Re-export Command structs for easy access
pub use compile::{
    CompileActiveModel, CompileClassPickerEntry, CompileClassPickerState, CompileModel,
    FastRunActiveModel, FastRunInput, FastRunSetupEntry, FastRunSetupState, PauseActiveModel,
    PickerPurpose, ResetActiveModel, RestartActiveModel, ResumeActiveModel, RunActiveModel,
};
pub use doc::{Undo, Redo, FormatDocument, SaveActiveDocument, SaveActiveDocumentAs};
pub use lifecycle::{
    CreateNewScratchModel, DuplicateModelFromReadOnly, DuplicateActiveDoc, OpenClass,
    OpenInNewView, Open, ClassAction, CloseDialogState, PendingCloseAfterSave,
    PendingTabCloseScopes, TabCloseScope, GetFile,
};
pub use nav::{
    AutoArrangeDiagram, FocusDocumentByName, SetViewMode, SetZoom, FitCanvas,
    FocusComponent, PanCanvas,
};
pub use diagram::{MoveComponent, AddCanvasPlot};
pub use sim::{SetModelInput, SetModelInputError, apply_set_model_input};
pub use plot::{NewPlotPanel, AddSignalToPlot};
pub use util::{Ping, Exit};
pub use inspect::InspectActiveDoc;
pub use lifecycle::drain_open_file_results;

pub struct ModelicaCommandsPlugin;

impl Plugin for ModelicaCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(compile::CompilePlugin)
            .init_resource::<CloseDialogState>()
            .init_resource::<PendingCloseAfterSave>()
            .init_resource::<PendingTabCloseScopes>()
            .init_resource::<lifecycle::AppCloseFlow>()
            .add_observer(lifecycle::finish_close_after_save)
            .add_observer(lifecycle::on_document_closed_cleanup)
            .add_observer(crate::ui::uri_handler::on_modelica_uri_clicked)
            .add_observer(intent::resolve_editor_intent)
            .add_observer(intent::resolve_new_document_intent)
            .add_systems(
                Startup,
                (register_modelica_uri_handler, lifecycle::prewarm_msl_library),
            )
            .add_systems(
                Update,
                (
                    (
                        lifecycle::resolve_tab_close_scopes,
                        lifecycle::drain_pending_tab_closes,
                    )
                        .chain(),
                    status::update_status_bar,
                    status::publish_unsaved_modelica_docs,
                    lifecycle::on_window_close_requested,
                    lifecycle::finalize_app_close,
                ),
            )
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                lifecycle::render_close_dialogs,
            );

        // All typed commands, collected by the `register_commands!`
        // invocation below (path form supports the split submodules).
        register_all_commands(app);
    }
}

// Generates `register_all_commands(app)` at module scope. Observers live
// in split submodules, so each entry uses the `module::fn` path form.
register_commands!(
    diagram::on_add_canvas_plot,
    plot::on_add_signal_to_plot,
    crate::ui::panels::canvas_diagram::on_auto_arrange_diagram,
    lifecycle::on_close_document,
    lifecycle::on_create_new_scratch_model,
    lifecycle::on_duplicate_active_doc,
    lifecycle::on_duplicate_model_from_read_only,
    util::on_exit,
    nav::on_fit_canvas,
    nav::on_focus_component,
    nav::on_focus_document_by_name,
    doc::on_format_document,
    lifecycle::on_get_file,
    inspect::on_inspect_active_doc,
    diagram::on_move_component,
    lifecycle::on_new_modelica_document,
    plot::on_new_plot_panel,
    lifecycle::on_open,
    lifecycle::on_open_class,
    lifecycle::on_open_file,
    lifecycle::on_open_in_new_view,
    nav::on_pan_canvas,
    util::on_ping,
    doc::on_redo,
    doc::on_redo_document,
    doc::on_save_active_document,
    doc::on_save_active_document_as,
    doc::on_save_as_document,
    doc::on_save_document,
    sim::on_set_model_input,
    nav::on_set_view_mode,
    nav::on_set_zoom,
    doc::on_undo,
    doc::on_undo_document,
);

// ─── Helpers ─────────────────────────────────────────────────────────────────

pub(super) fn resolve_active_doc(world: &World) -> Option<DocumentId> {
    world
        .get_resource::<lunco_workspace::WorkspaceResource>()
        .and_then(|ws| ws.active_document)
}

/// CQ-111: resolve a command's target document. An unassigned id means
/// "act on the active document"; an explicit id is honored as-is. Returns
/// `None` when unassigned and there is no active document.
pub(super) fn resolve_doc_or_active(world: &World, raw: DocumentId) -> Option<DocumentId> {
    if raw.is_unassigned() {
        resolve_active_doc(world)
    } else {
        Some(raw)
    }
}

pub(super) fn entity_for_doc(world: &World, doc: DocumentId) -> Option<Entity> {
    world
        .get_resource::<crate::state::ModelicaDocumentRegistry>()
        .and_then(|r| r.entities_linked_to(doc).into_iter().next())
}

pub(super) fn approx_screen_rect() -> lunco_canvas::Rect {
    lunco_canvas::Rect::from_min_max(
        lunco_canvas::Pos::new(0.0, 0.0),
        lunco_canvas::Pos::new(800.0, 600.0),
    )
}

fn register_modelica_uri_handler(
    mut registry: ResMut<lunco_workbench::UriRegistry>,
) {
    registry.register(std::sync::Arc::new(
        crate::ui::uri_handler::ModelicaUriHandler,
    ));
    bevy::log::info!("[Modelica] registered modelica:// URI handler");
}
