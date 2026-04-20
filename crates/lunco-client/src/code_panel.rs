//! Code panel — shows the source for the currently-selected entity.
//!
//! Reads `SelectedEntity`, then queries:
//!   - `lunco_modelica::ui::ModelicaDocumentRegistry` for `.mo` source
//!     attached to a `ModelicaModel` entity (e.g. the Red Balloon).
//!   - `lunco_scripting::ScriptRegistry` + `ScriptedModel` component for
//!     Python source (e.g. the Green Balloon).
//!
//! Read-only for now — Phase 1 of the interactive-modeling story.
//! Edit-and-recompile lands when the Document System exposes
//! checkpointing on edit.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_modelica::ui::ModelicaDocumentRegistry;
use lunco_sandbox_edit::SelectedEntity;
use lunco_scripting::ScriptRegistry;
use lunco_scripting::doc::ScriptedModel;
use lunco_workbench::{Panel, PanelId, PanelSlot};

pub struct CodePanel;

impl Panel for CodePanel {
    fn id(&self) -> PanelId { PanelId("rover_code") }
    fn title(&self) -> String { "📄 Code".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let mantle = world.resource::<lunco_theme::Theme>().colors.mantle;
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| code_panel_content(ui, world));
    }
}

fn code_panel_content(ui: &mut egui::Ui, world: &mut World) {
    ui.heading("Code");

    let Some(entity) = world
        .get_resource::<SelectedEntity>()
        .and_then(|s| s.entity)
    else {
        ui.label("No entity selected.");
        ui.label(egui::RichText::new("Shift+click an object to select.").weak());
        return;
    };

    // Try Modelica first — resolve entity → DocumentId → source.
    let modelica = world
        .get_resource::<ModelicaDocumentRegistry>()
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
    let script_doc_id = world
        .query::<&ScriptedModel>()
        .get(world, entity)
        .ok()
        .and_then(|m| m.document_id);

    if let Some(doc_id) = script_doc_id {
        let script_source = world
            .get_resource::<ScriptRegistry>()
            .and_then(|r| {
                r.documents
                    .get(&DocumentId::new(doc_id))
                    .map(|h| h.document().source.clone())
            });
        if let Some(source) = script_source {
            ui.label(egui::RichText::new("Python").small().weak());
            ui.separator();
            render_source(ui, &source);
            return;
        }
    }

    ui.label("This entity has no attached model.");
    ui.label(
        egui::RichText::new("Drag a .mo or .py from the Models palette onto it. (TODO)")
            .weak()
            .small(),
    );
}

fn render_source(ui: &mut egui::Ui, source: &str) {
    egui::ScrollArea::both()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            // Use a TextEdit in read-only mode for monospace + selectable
            // text without writing back. Source is cloned per frame so
            // mutating the buffer locally has no effect on the doc.
            let mut buf = source.to_string();
            ui.add(
                egui::TextEdit::multiline(&mut buf)
                    .font(egui::TextStyle::Monospace)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(20)
                    .interactive(true), // selectable but not editable due to clone
            );
        });
}
