//! ModelViewPanel — one center tab per open model, with a view-mode switcher.
//!
//! # Goal
//!
//! The user opens a `.mo` file and gets a single tab whose title is the
//! model name. Inside the tab a toolbar lets them flip between Text and
//! Diagram views of the *same* document. This replaces the prior split
//! of `modelica_code_preview` + `modelica_diagram_preview` as independent
//! sibling tabs, which looked like two separate apps.
//!
//! # Current state (phase 3-lite)
//!
//! The panel owns a `view_mode` and delegates the *body* rendering to
//! the existing `CodeEditorPanel` / `DiagramPanel` render methods. That
//! means today the user sees **two stacked toolbars**: this panel's
//! unified row on top and the legacy per-panel row below. The next pass
//! strips the inner toolbars by factoring the panel bodies into
//! `render_code_body` / `render_diagram_body` free functions and moving
//! the action handlers (Compile, Undo/Redo) up here. It's real debt,
//! called out by `// LEGACY:` markers below so it doesn't hide.
//!
//! # Single-instance today, multi-tab later
//!
//! The `Panel` trait is one-instance-per-id, so this is a singleton for
//! now. Real multi-tab (one tab per open model, with split support)
//! requires the `TabId` enum refactor in `lunco-workbench` discussed in
//! the phase plan — deferred until the single-tab flow is proven.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::ModelicaModel;
use crate::ui::{CompileState, CompileStates, ModelicaDocumentRegistry, WorkbenchState};
use crate::ui::panels::{code_editor::CodeEditorPanel, diagram::DiagramPanel};

/// Which rendering mode the model view is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelViewMode {
    /// Raw Modelica source (egui TextEdit).
    #[default]
    Text,
    /// Block-diagram canvas (egui-snarl).
    Diagram,
}

/// Unified per-model center panel. See the module docs for the plan.
pub struct ModelViewPanel {
    /// Which view is currently shown. Persisted on the panel struct so a
    /// re-render during the same session preserves the user's choice.
    pub view_mode: ModelViewMode,
    /// Delegate — legacy body rendering for text. Will be replaced by a
    /// free `render_code_body` fn once the toolbars are fully migrated.
    code: CodeEditorPanel,
    /// Delegate — legacy body rendering for diagram. Same migration path.
    diagram: DiagramPanel,
}

impl Default for ModelViewPanel {
    fn default() -> Self {
        Self {
            view_mode: ModelViewMode::default(),
            code: CodeEditorPanel,
            diagram: DiagramPanel,
        }
    }
}

impl Panel for ModelViewPanel {
    fn id(&self) -> PanelId {
        PanelId("modelica_model_view")
    }

    fn title(&self) -> String {
        // Fallback label shown before any model is opened and when
        // `dynamic_title` can't reach the world (shouldn't happen).
        "🔧 Model".into()
    }

    fn dynamic_title(&self, world: &World) -> String {
        // Tab label follows the currently open model so the tab reads
        // like a file name (Dymola / VS Code convention) instead of a
        // generic panel label. Falls back to the static title when
        // there's no open model.
        world
            .get_resource::<WorkbenchState>()
            .and_then(|s| s.open_model.as_ref().map(|m| m.display_name.clone()))
            .unwrap_or_else(|| self.title())
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn closable(&self) -> bool {
        false
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        render_unified_toolbar(self, ui, world);
        ui.separator();

        // Body — delegate to the old panels. This is the LEGACY bit:
        // their own toolbars render below ours until they're extracted.
        match self.view_mode {
            ModelViewMode::Text => self.code.render(ui, world),
            ModelViewMode::Diagram => self.diagram.render(ui, world),
        }
    }
}

/// Render the top toolbar: model name · view toggle · Compile · undo/redo · status.
///
/// Held separate so we can unit-test the layout later without wiring a
/// full Bevy app. Mutations that happen here (view-mode swap, compile
/// button press) are the visible user-intent — action handlers live in
/// the underlying panels for now and are re-triggered via the existing
/// channels.
fn render_unified_toolbar(panel: &mut ModelViewPanel, ui: &mut egui::Ui, world: &mut World) {
    // Resolve the display name + doc + compile state once up front so
    // the closure isn't fighting borrow checker over `world`.
    let display_name = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .map(|m| m.display_name.clone())
        .unwrap_or_else(|| "(no model)".to_string());

    let doc_id = world
        .resource::<WorkbenchState>()
        .selected_entity
        .and_then(|e| world.get::<ModelicaModel>(e).map(|m| m.document))
        .filter(|d| !d.is_unassigned());

    let compile_state = doc_id
        .map(|d| world.resource::<CompileStates>().state_of(d))
        .unwrap_or_default();

    let is_read_only = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .map(|m| m.read_only)
        .unwrap_or(false);

    let compilation_error = world.resource::<WorkbenchState>().compilation_error.clone();

    ui.horizontal(|ui| {
        // ── Model identity ──
        ui.label(egui::RichText::new(&display_name).strong());
        if is_read_only {
            ui.colored_label(
                egui::Color32::from_rgb(200, 150, 50),
                "👁 read-only",
            );
        }
        ui.separator();

        // ── View-mode toggle ──
        let text_sel = panel.view_mode == ModelViewMode::Text;
        let diag_sel = panel.view_mode == ModelViewMode::Diagram;
        if ui.selectable_label(text_sel, "📝 Text").clicked() {
            panel.view_mode = ModelViewMode::Text;
        }
        if ui.selectable_label(diag_sel, "🔗 Diagram").clicked() {
            panel.view_mode = ModelViewMode::Diagram;
        }
        ui.separator();

        // ── Status chip ──
        if let Some(ref err) = compilation_error {
            let chip = ui
                .colored_label(egui::Color32::LIGHT_RED, "⚠ Error")
                .on_hover_text(err);
            if chip.clicked() {
                if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
                    s.compilation_error = None;
                }
            }
        } else {
            match compile_state {
                CompileState::Compiling => {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 200, 80),
                        "⏳ Compiling…",
                    );
                }
                CompileState::Ready => {
                    ui.colored_label(egui::Color32::GREEN, "✓ Ready");
                }
                CompileState::Error => {
                    ui.colored_label(egui::Color32::LIGHT_RED, "⚠ Error");
                }
                CompileState::Idle => {
                    ui.colored_label(egui::Color32::GRAY, "◌ Idle");
                }
            }
        }

        // ── Undo/redo (document-level) ──
        // Goes through `apply_document_undo` / `apply_document_redo` so
        // the editor buffer stays in sync with the reverted source.
        if let Some(doc) = doc_id {
            let summary = world
                .resource::<ModelicaDocumentRegistry>()
                .host(doc)
                .map(|h| (h.can_undo(), h.can_redo(), h.undo_depth(), h.redo_depth()));
            if let Some((can_undo, can_redo, undo_n, redo_n)) = summary {
                ui.separator();
                let undo_clicked = ui
                    .add_enabled(can_undo, egui::Button::new("↶"))
                    .on_hover_text(format!("Undo — {undo_n} available"))
                    .clicked();
                let redo_clicked = ui
                    .add_enabled(can_redo, egui::Button::new("↷"))
                    .on_hover_text(format!("Redo — {redo_n} available"))
                    .clicked();
                if undo_clicked {
                    crate::ui::panels::code_editor::apply_document_undo(world);
                } else if redo_clicked {
                    crate::ui::panels::code_editor::apply_document_redo(world);
                }
            }
        }

        // Compile: Text mode dispatches from the editor buffer; Diagram
        // mode regenerates source from the visual diagram and dispatches
        // that. One button, same semantics as a user would expect.
        ui.separator();
        let compile_enabled = !is_read_only
            && !matches!(compile_state, CompileState::Compiling);
        let compile_clicked = ui
            .add_enabled(compile_enabled, egui::Button::new("🚀 Compile"))
            .on_hover_text("Compile the current model and run it")
            .clicked();
        if compile_clicked {
            match panel.view_mode {
                ModelViewMode::Text => {
                    crate::ui::panels::code_editor::dispatch_compile_from_buffer(world);
                }
                ModelViewMode::Diagram => {
                    crate::ui::panels::diagram::do_compile(world);
                }
            }
        }
    });
}
