//! Command bus for Modelica documents.

use bevy::prelude::*;
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
    OpenExample, OpenInNewView, Open, ClassAction, CloseDialogState, PendingCloseAfterSave,
    GetFile,
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
            .init_resource::<lifecycle::AppCloseFlow>()
            .add_observer(doc::on_undo_document)
            .add_observer(doc::on_redo_document)
            .add_observer(doc::on_save_document)
            .add_observer(doc::on_save_as_document)
            .add_observer(lifecycle::finish_close_after_save)
            .add_observer(lifecycle::on_close_document)
            .add_observer(lifecycle::on_document_closed_cleanup)
            .add_observer(crate::ui::uri_handler::on_modelica_uri_clicked)
            .register_type::<AutoArrangeDiagram>()
            .add_observer(crate::ui::panels::canvas_diagram::on_auto_arrange_diagram)
            .add_observer(intent::resolve_editor_intent)
            .add_observer(intent::resolve_new_document_intent)
            .add_systems(
                Startup,
                (register_modelica_uri_handler, lifecycle::prewarm_msl_library),
            )
            .add_systems(
                Update,
                (
                    lifecycle::drain_pending_tab_closes,
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

        // Manually register all commands for now to avoid macro issues with split modules
        diagram::__register_on_add_canvas_plot(app);
        plot::__register_on_add_signal_to_plot(app);
        lifecycle::__register_on_create_new_scratch_model(app);
        lifecycle::__register_on_duplicate_active_doc(app);
        lifecycle::__register_on_duplicate_model_from_read_only(app);
        util::__register_on_exit(app);
        nav::__register_on_fit_canvas(app);
        nav::__register_on_focus_component(app);
        nav::__register_on_focus_document_by_name(app);
        doc::__register_on_format_document(app);
        lifecycle::__register_on_get_file(app);
        inspect::__register_on_inspect_active_doc(app);
        diagram::__register_on_move_component(app);
        lifecycle::__register_on_new_modelica_document(app);
        plot::__register_on_new_plot_panel(app);
        lifecycle::__register_on_open(app);
        lifecycle::__register_on_open_class(app);
        lifecycle::__register_on_open_example(app);
        lifecycle::__register_on_open_file(app);
        nav::__register_on_pan_canvas(app);
        util::__register_on_ping(app);
        doc::__register_on_redo(app);
        doc::__register_on_save_active_document(app);
        doc::__register_on_save_active_document_as(app);
        sim::__register_on_set_model_input(app);
        nav::__register_on_set_view_mode(app);
        nav::__register_on_set_zoom(app);
        doc::__register_on_undo(app);
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

pub(super) fn resolve_active_doc(world: &World) -> Option<DocumentId> {
    world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document)
}

pub(super) fn entity_for_doc(world: &World, doc: DocumentId) -> Option<Entity> {
    world
        .get_resource::<crate::ui::ModelicaDocumentRegistry>()
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
