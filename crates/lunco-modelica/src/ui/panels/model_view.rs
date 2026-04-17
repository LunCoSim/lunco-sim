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
use crate::ui::panels::code_editor::EditorBufferState;
use crate::ui::panels::{code_editor::CodeEditorPanel, diagram::DiagramPanel};
use crate::ui::{CompileState, CompileStates, ModelicaDocumentRegistry, WorkbenchState};

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
    // Resolve everything we need from the world up front so the
    // closure below doesn't re-borrow mid-layout.
    let display_name = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .map(|m| m.display_name.clone())
        .unwrap_or_else(|| "(no model)".to_string());

    // Prefer open_model.doc — it's set on every open/create path,
    // including in-memory models before they have a selected entity.
    // Fall back to entity → doc for pre-command-bus paths.
    let doc_id = world
        .resource::<WorkbenchState>()
        .open_model
        .as_ref()
        .and_then(|m| m.doc)
        .or_else(|| {
            world
                .resource::<WorkbenchState>()
                .selected_entity
                .and_then(|e| world.get::<ModelicaModel>(e).map(|m| m.document))
        })
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

    // Dirty + save-availability info, derived from the document.
    let (is_dirty, can_save) = doc_id
        .and_then(|d| world.resource::<ModelicaDocumentRegistry>().host(d))
        .map(|h| {
            let doc = h.document();
            let has_real_path = doc
                .canonical_path()
                .map(|p| !p.to_string_lossy().starts_with("mem://"))
                .unwrap_or(false);
            (doc.is_dirty(), !doc.is_read_only() && has_real_path)
        })
        .unwrap_or((false, false));

    // Undo/redo availability snapshot.
    let undo_redo = doc_id
        .and_then(|d| world.resource::<ModelicaDocumentRegistry>().host(d))
        .map(|h| (h.can_undo(), h.can_redo(), h.undo_depth(), h.redo_depth()));

    // Collect button presses without mutating world inside the closure.
    let mut compile_clicked = false;
    let mut save_clicked = false;
    let mut undo_clicked = false;
    let mut redo_clicked = false;
    let mut dismiss_error = false;
    let mut new_view_mode = panel.view_mode;

    ui.horizontal(|ui| {
        // ── Model identity (+ dirty dot) ──
        let title = if is_dirty {
            format!("● {}", display_name)
        } else {
            display_name.clone()
        };
        ui.label(egui::RichText::new(title).strong());
        if is_read_only {
            ui.colored_label(egui::Color32::from_rgb(200, 150, 50), "👁 read-only");
        }
        ui.separator();

        // ── View-mode toggle ──
        let text_sel = panel.view_mode == ModelViewMode::Text;
        let diag_sel = panel.view_mode == ModelViewMode::Diagram;
        if ui.selectable_label(text_sel, "📝 Text").clicked() {
            new_view_mode = ModelViewMode::Text;
        }
        if ui.selectable_label(diag_sel, "🔗 Diagram").clicked() {
            new_view_mode = ModelViewMode::Diagram;
        }
        ui.separator();

        // ── Status chip ──
        if let Some(ref err) = compilation_error {
            let chip = ui
                .colored_label(egui::Color32::LIGHT_RED, "⚠ Error")
                .on_hover_text(err);
            if chip.clicked() {
                dismiss_error = true;
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

        // ── Undo / Redo ──
        if let Some((can_undo, can_redo, undo_n, redo_n)) = undo_redo {
            ui.separator();
            undo_clicked = ui
                .add_enabled(can_undo, egui::Button::new("↶"))
                .on_hover_text(format!("Undo — {undo_n} available (Ctrl+Z)"))
                .clicked();
            redo_clicked = ui
                .add_enabled(can_redo, egui::Button::new("↷"))
                .on_hover_text(format!("Redo — {redo_n} available (Ctrl+Shift+Z)"))
                .clicked();
        }

        // ── Save ──
        ui.separator();
        let save_tip = if !can_save {
            "Save not available (read-only or Save-As required for in-memory models)"
        } else if is_dirty {
            "Save to disk (Ctrl+S)"
        } else {
            "Already saved"
        };
        save_clicked = ui
            .add_enabled(can_save && is_dirty, egui::Button::new("💾 Save"))
            .on_hover_text(save_tip)
            .clicked();

        // ── Compile ──
        ui.separator();
        let compile_enabled = !is_read_only
            && doc_id.is_some()
            && !matches!(compile_state, CompileState::Compiling);
        compile_clicked = ui
            .add_enabled(compile_enabled, egui::Button::new("🚀 Compile"))
            .on_hover_text("Compile the current model and run it (F5)")
            .clicked();
    });

    // Apply side effects *outside* the ui closure. All mutations that
    // land a Document op go through the command bus now — no direct
    // helper calls — so the Twin journal and every other subscriber
    // sees the same events regardless of whether a button, shortcut,
    // or script triggered them.
    if new_view_mode != panel.view_mode {
        panel.view_mode = new_view_mode;
    }
    if dismiss_error {
        if let Some(mut s) = world.get_resource_mut::<WorkbenchState>() {
            s.compilation_error = None;
        }
    }
    if let Some(doc) = doc_id {
        if undo_clicked {
            world.commands().trigger(lunco_doc_bevy::UndoDocument { doc });
        }
        if redo_clicked {
            world.commands().trigger(lunco_doc_bevy::RedoDocument { doc });
        }
        if save_clicked {
            world.commands().trigger(lunco_doc_bevy::SaveDocument { doc });
        }
        if compile_clicked {
            // Flush the active view's state into the Document first so
            // the compile observer reads the user's latest work. Text
            // mode uses the editor buffer; Diagram mode regenerates
            // source from the visual diagram.
            match panel.view_mode {
                ModelViewMode::Text => {
                    let buffer = world.resource::<EditorBufferState>().text.clone();
                    if !buffer.is_empty() {
                        world
                            .resource_mut::<ModelicaDocumentRegistry>()
                            .checkpoint_source(doc, buffer);
                    }
                    world.commands().trigger(crate::ui::CompileModel { doc });
                }
                ModelViewMode::Diagram => {
                    // Diagram regenerates source from DiagramState via
                    // the domain-specific do_compile path, which already
                    // handles temp-file write + entity spawn + worker
                    // dispatch. Once the visual-ops refactor lands,
                    // Diagram will generate source, checkpoint it, and
                    // fire CompileModel the same as Text.
                    crate::ui::panels::diagram::do_compile(world);
                }
            }
        }
    }
}
