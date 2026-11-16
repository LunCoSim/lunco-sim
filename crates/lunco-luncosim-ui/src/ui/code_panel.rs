//! Code panel — shows the source for the currently-selected entity.
//!
//! Reads `SelectedEntities`, then queries:
//!   - the generic Modelica document registry for `.mo` source
//!     attached to a `ModelicaModel` entity (e.g. the Red Balloon).
//!   - `lunco_scripting::ScriptRegistry` + `ScriptedModel` component for
//!     scripted source (e.g. the Green Balloon).
//!
//! Read-only source presentation; authored edits use the document or behaviour editor.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_doc_bevy::DocumentRegistry;
use lunco_modelica_document::ModelicaDocument;
use lunco_scene_selection::SelectedEntities;
use lunco_scripting::ScriptRegistry;
use lunco_scripting::doc::{ScriptLanguage, ScriptedModel};
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};

pub(crate) struct CodePanel;

impl Panel for CodePanel {
    fn id(&self) -> PanelId {
        PanelId("rover_code")
    }
    fn title(&self) -> String {
        "Source Viewer".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }
    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Builder
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ctx.panel_content_frame()
            .show(ui, |ui| code_panel_content(ui, ctx));
    }
}

fn code_panel_content(ui: &mut egui::Ui, ctx: &mut PanelCtx) {
    ui.heading("Source Viewer");
    ui.weak(
        "Read-only · select and copy source here. Edit through the document or Behaviour Editor.",
    );

    let Some(entity) = ctx.resource::<SelectedEntities>().and_then(|s| s.primary()) else {
        ui.label("No entity selected.");
        ui.label(egui::RichText::new("Left-click an object to select.").weak());
        return;
    };

    // Try Modelica first — resolve entity → DocumentId → source.
    let modelica = ctx
        .resource::<DocumentRegistry<ModelicaDocument>>()
        .and_then(|r| {
            let doc = r.document_of(entity)?;
            r.host(doc).map(|h| h.document().source().to_string())
        });

    if let Some(source) = modelica {
        ui.label(egui::RichText::new("Modelica").small().weak());
        ui.separator();
        render_source(ui, &source);
        return;
    }

    // Otherwise look for a ScriptedModel component → ScriptRegistry.
    let script_doc_id = ctx.get::<ScriptedModel>(entity).and_then(|m| m.document_id);

    if let Some(doc_id) = script_doc_id {
        let script_source = ctx.resource::<ScriptRegistry>().and_then(|r| {
            r.documents
                .get(&DocumentId::new(doc_id))
                .map(|h| (h.document().language, h.document().source.clone()))
        });
        if let Some((language, source)) = script_source {
            let label = match language {
                ScriptLanguage::Python => "Python",
                ScriptLanguage::Rhai => "Rhai",
            };
            ui.label(egui::RichText::new(label).small().weak());
            ui.separator();
            render_source(ui, &source);
            return;
        }
    }

    ui.label("This entity has no attached model.");
    ui.label(
        egui::RichText::new(
            "This panel displays models already attached by the authored USD component. "
                .to_owned()
                + "Arbitrary .mo/.py attachment is not available from this panel yet.",
        )
        .weak(),
    );
}

fn render_source(ui: &mut egui::Ui, source: &str) {
    egui::ScrollArea::both()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            let mut text = source;
            ui.add(
                lunco_workbench_widgets::text_editor::code(&mut text)
                    .desired_width(f32::INFINITY)
                    .desired_rows(20),
            );
        });
}
