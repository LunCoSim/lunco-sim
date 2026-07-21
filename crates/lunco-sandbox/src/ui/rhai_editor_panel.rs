//! Rhai **behaviour editor** — edit the Rhai script attached to the selected
//! prim, save it (which hot-reloads the scenario and persists to the prim's
//! `lunco:script`), and see compile diagnostics with click-to-jump.
//!
//! This is the writable counterpart of the read-only [`CodePanel`](super::code_panel):
//! it follows the same selection (`SelectedEntities`) and resolves the same
//! `ScriptedModel → ScriptRegistry` source, but adds an editable buffer, a
//! diagnostics list fed by `DocumentDiagnostics` (the line/col the rhai compile
//! path already produces), and a Save that reboots the behaviour.
//!
//! # Data flow
//!
//! ```text
//!   SelectedEntities.primary() ─┐
//!   ScriptedModel.document_id ──┤  produce_rhai_editor_vm  ┌─> RhaiEditorVm (buffer,
//!   ScriptRegistry.source ──────┤   (Update, view-model)   │      diagnostics, state)
//!   DocumentDiagnostics ────────┘                          └─> panel renders it
//!
//!   Save & Run ──> RunScenario { source }  (sets live source + hot-reloads)
//!             └──> SaveScenario            (persists source onto lunco:script)
//! ```
//!
//! The buffer is only re-synced from the registry when the selected doc or its
//! generation changes **and** there are no unsaved edits (`dirty`), so typing is
//! never clobbered by a background reload.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::{CompileState, Diagnostic, DiagnosticSeverity, DocumentId};
use lunco_doc_bevy::DocumentDiagnostics;
use lunco_sandbox_edit::SelectedEntities;
use lunco_scripting::commands::RunScenario;
use lunco_scripting::doc::ScriptedModel;
use lunco_scripting::ScriptRegistry;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

use crate::SaveScenario;

/// View-model for the Rhai editor, produced each frame from ECS state so the
/// panel (which can only read resources) has everything it needs.
#[derive(Resource, Default)]
pub(crate) struct RhaiEditorVm {
    /// The prim entity whose script is shown (follows `SelectedEntities`).
    entity: Option<Entity>,
    /// Its scenario document id, if it has a `ScriptedModel`.
    doc_id: Option<u64>,
    /// Editable source buffer.
    buffer: String,
    /// Scenario params (JSON string) carried through `RunScenario` on save.
    params: String,
    /// `(doc_id, generation)` the buffer was last synced from — a mismatch (and
    /// no unsaved edits) triggers a reload.
    loaded_key: Option<(u64, u64)>,
    /// The buffer has edits not yet saved.
    dirty: bool,
    /// Last compile state + diagnostics for the shown doc.
    state: CompileState,
    diagnostics: Vec<Diagnostic>,
}

/// View-model producer (WP-8): mirror ECS script state into [`RhaiEditorVm`].
pub(crate) fn produce_rhai_editor_vm(
    selected: Option<Res<SelectedEntities>>,
    scripted: Query<&ScriptedModel>,
    registry: Option<Res<ScriptRegistry>>,
    diagnostics: Option<Res<DocumentDiagnostics>>,
    mut vm: ResMut<RhaiEditorVm>,
) {
    let entity = selected.as_deref().and_then(|s| s.primary());

    // Selection changed → forget everything (including unsaved edits: they
    // belonged to the previously-selected prim).
    if entity != vm.entity {
        vm.entity = entity;
        vm.doc_id = None;
        vm.buffer.clear();
        vm.params.clear();
        vm.loaded_key = None;
        vm.dirty = false;
        vm.diagnostics.clear();
        vm.state = CompileState::default();
    }

    let Some(entity) = entity else {
        return;
    };
    let Some(doc_id) = scripted.get(entity).ok().and_then(|m| m.document_id) else {
        vm.doc_id = None;
        return;
    };
    vm.doc_id = Some(doc_id);
    let did = DocumentId::new(doc_id);

    if let Some(reg) = registry.as_deref() {
        if let Some(host) = reg.documents.get(&did) {
            let doc = host.document();
            let key = (doc_id, doc.generation);
            // Reload only on a real change and only when there are no unsaved
            // edits — otherwise a background generation bump would wipe typing.
            if vm.loaded_key != Some(key) && !vm.dirty {
                vm.buffer = doc.source.clone();
                vm.params = doc.params.clone();
                vm.loaded_key = Some(key);
            }
        }
    }

    if let Some(store) = diagnostics.as_deref() {
        if let Some(d) = store.get(did) {
            vm.state = d.state.clone();
            vm.diagnostics = d.diagnostics.clone();
        } else {
            vm.state = CompileState::default();
            vm.diagnostics.clear();
        }
    }
}

pub(crate) struct RhaiEditorPanel;

impl Panel for RhaiEditorPanel {
    fn id(&self) -> PanelId {
        PanelId("rhai_editor")
    }
    fn title(&self) -> String {
        "📜 Behaviour".into()
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        ctx.resource_scope::<RhaiEditorVm, ()>(|ctx, vm| {
            if vm.entity.is_none() {
                empty_hint(ui, "Select a prim to edit its Rhai behaviour.");
                return;
            }
            if vm.doc_id.is_none() {
                empty_hint(ui, "The selected prim has no attached Rhai script.");
                return;
            }

            let text_id = egui::Id::new(("rhai_editor_textedit", vm.entity));
            let mut do_save = false;
            let mut do_revert = false;
            let mut jump_to: Option<usize> = None;

            // ── Toolbar: save / revert + compile status ──────────────────────
            ui.horizontal(|ui| {
                ui.add_enabled_ui(vm.dirty, |ui| {
                    if ui
                        .button("💾 Save & Run")
                        .on_hover_text("Hot-reload the scenario with these edits and persist them to the prim's lunco:script")
                        .clicked()
                    {
                        do_save = true;
                    }
                    if ui.button("↩ Revert").on_hover_text("Discard edits, reload the saved source").clicked() {
                        do_revert = true;
                    }
                });
                let (txt, col) = status_label(&vm.state, &vm.diagnostics, &theme);
                ui.label(egui::RichText::new(txt).color(col));
                if vm.dirty {
                    ui.label(egui::RichText::new("● unsaved").color(theme.tokens.warning));
                }
            });
            ui.separator();

            // ── Editor ───────────────────────────────────────────────────────
            let editor_height = (ui.available_height() - diagnostics_height(&vm.diagnostics)).max(80.0);
            let resp = egui::ScrollArea::vertical()
                .id_salt("rhai_editor_scroll")
                .max_height(editor_height)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut vm.buffer)
                            .id(text_id)
                            .font(egui::TextStyle::Monospace)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(20),
                    )
                })
                .inner;
            if resp.changed() {
                vm.dirty = true;
            }

            // ── Diagnostics list (the line/col gutter, as a jump list) ────────
            if !vm.diagnostics.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Diagnostics").small().weak());
                egui::ScrollArea::vertical()
                    .id_salt("rhai_editor_diags")
                    .max_height(110.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for d in &vm.diagnostics {
                            let col = severity_color(d.severity, &theme);
                            let loc = match (d.line, d.col) {
                                (Some(l), Some(c)) => format!("{l}:{c}"),
                                (Some(l), None) => format!("{l}"),
                                _ => "—".to_string(),
                            };
                            let text = format!("{}  {}  {}", severity_glyph(d.severity), loc, d.message);
                            let hit = ui.add(
                                egui::Label::new(egui::RichText::new(text).color(col).monospace())
                                    .sense(egui::Sense::click()),
                            );
                            if hit.clicked() {
                                if let Some(line) = d.line {
                                    jump_to = Some(line_col_to_char_offset(
                                        &vm.buffer,
                                        line,
                                        d.col.unwrap_or(1),
                                    ));
                                }
                            }
                        }
                    });
            }

            // ── Apply deferred actions ───────────────────────────────────────
            if let Some(offset) = jump_to {
                move_cursor(ui.ctx(), text_id, offset);
            }
            if do_revert {
                // Drop the sync marker + dirty flag; the producer reloads the
                // saved source next frame.
                vm.loaded_key = None;
                vm.dirty = false;
            }
            if do_save {
                let entity = vm.entity.expect("checked above");
                let source = vm.buffer.clone();
                let params = vm.params.clone();
                // The producer resyncs to the new generation once RunScenario
                // bumps it; clearing dirty lets that reload land.
                vm.dirty = false;
                ctx.defer(move |world| {
                    world.trigger(RunScenario { target: entity, source, params });
                    world.trigger(SaveScenario { target: entity });
                });
            }
        });
    }
}

fn empty_hint(ui: &mut egui::Ui, msg: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.label(egui::RichText::new(msg).weak());
        ui.label(
            egui::RichText::new("Shift+click an object, then edit its Rhai behaviour here.")
                .weak()
                .small(),
        );
    });
}

/// Height to reserve for the diagnostics strip so the editor sizes to fill the
/// rest.
fn diagnostics_height(diags: &[Diagnostic]) -> f32 {
    if diags.is_empty() {
        0.0
    } else {
        130.0
    }
}

fn status_label(
    state: &CompileState,
    diags: &[Diagnostic],
    theme: &lunco_theme::Theme,
) -> (String, egui::Color32) {
    let errors = diags.iter().filter(|d| d.severity == DiagnosticSeverity::Error).count();
    let warnings = diags.iter().filter(|d| d.severity == DiagnosticSeverity::Warning).count();
    match state {
        CompileState::Ready => ("✓ compiled".to_string(), theme.tokens.success),
        CompileState::Error if errors > 0 => (
            format!("✗ {errors} error{}", if errors == 1 { "" } else { "s" }),
            theme.tokens.error,
        ),
        CompileState::Error => ("✗ error".to_string(), theme.tokens.error),
        _ if warnings > 0 => (
            format!("⚠ {warnings} warning{}", if warnings == 1 { "" } else { "s" }),
            theme.tokens.warning,
        ),
        _ => ("—".to_string(), theme.tokens.text_subdued),
    }
}

fn severity_color(s: DiagnosticSeverity, theme: &lunco_theme::Theme) -> egui::Color32 {
    match s {
        DiagnosticSeverity::Error => theme.tokens.error,
        DiagnosticSeverity::Warning => theme.tokens.warning,
        DiagnosticSeverity::Info => theme.colors.blue,
        DiagnosticSeverity::Hint => theme.tokens.text_subdued,
    }
}

fn severity_glyph(s: DiagnosticSeverity) -> &'static str {
    match s {
        DiagnosticSeverity::Error => "✗",
        DiagnosticSeverity::Warning => "⚠",
        DiagnosticSeverity::Info => "ℹ",
        DiagnosticSeverity::Hint => "·",
    }
}

/// Move the editor's caret to `offset` and focus it — used by diagnostic
/// click-to-jump. Mirrors the Modelica code editor's cursor-reposition idiom.
fn move_cursor(ctx: &egui::Context, id: egui::Id, offset: usize) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        let ccursor = egui::text::CCursor::new(offset);
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(ccursor)));
        state.store(ctx, id);
        ctx.memory_mut(|m| m.request_focus(id));
    }
}

/// Convert a 1-based (line, column) into a char offset within `text`, clamped to
/// the buffer. Columns past a line's end clamp to its end; lines past EOF clamp
/// to the last position. egui cursors index by `char`.
fn line_col_to_char_offset(text: &str, line: u32, column: u32) -> usize {
    let target_line = line.saturating_sub(1) as usize;
    let target_col = column.saturating_sub(1) as usize;
    let mut offset = 0usize;
    for (idx, l) in text.split_inclusive('\n').enumerate() {
        let line_chars = l.chars().filter(|&c| c != '\n').count();
        if idx == target_line {
            return offset + target_col.min(line_chars);
        }
        offset += l.chars().count();
    }
    text.chars().count()
}
