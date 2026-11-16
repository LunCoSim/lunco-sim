//! Scene-owned behaviour drafts, compilation feedback, and explicit document authoring.
//! Drafts survive selection changes and retire with their exact script/entity lease.
//! Running a draft and copying running source into the USD document are separate actions;
//! the document's normal Save operation owns durable storage.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_doc::{CompileState, Diagnostic, DiagnosticSeverity, DocumentId};
use lunco_doc_bevy::DocumentDiagnostics;
use lunco_scene_selection::SelectedEntities;
use lunco_scripting::doc::ScriptedModel;
use lunco_scripting::{ScenarioReloadPolicy, ScriptRegistry};
use lunco_scripting_rhai_runtime::commands::RunScenario;
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};
use lunco_workbench_widgets::UiIcon;

use crate::SaveScenario;

/// One draft belongs to an exact runtime script and entity lease.
#[derive(Default)]
struct BehaviourDraft {
    buffer: String,
    loaded_key: Option<(u64, u64)>,
    dirty: bool,
    pending: Option<(u64, String)>,
    error: Option<String>,
}

/// A paint callback carries the exact selected script lease into command admission.
#[derive(Event)]
pub(crate) struct BehaviourEditorAction {
    target: Entity,
    document: u64,
    source: Option<String>,
    diagnostic_revision: u64,
}

pub(crate) fn apply_editor_action(action: On<BehaviourEditorAction>, mut commands: Commands) {
    let target = action.target;
    let document = action.document;
    let source = action.source.clone();
    let revision = action.diagnostic_revision;
    commands.queue(move |world: &mut World| {
        if world
            .get::<ScriptedModel>(target)
            .and_then(|model| model.document_id)
            != Some(document)
        {
            return;
        }
        let id = world
            .resource_mut::<lunco_api::executor::ApiIdCounter>()
            .next_id();
        let previous = world.resource::<lunco_core::ActiveCommandId>().clone();
        world
            .resource_mut::<lunco_core::ActiveCommandId>()
            .set_with_origin(
                id,
                Some(lunco_core::CommandOrigin::LocalUser {
                    session_id: lunco_command_contracts::SessionId::LOCAL,
                }),
            );
        let submitted = source.clone();
        if let Some(source) = source {
            world.trigger(RunScenario {
                target,
                source,
                params: Default::default(),
                reload_policy: ScenarioReloadPolicy::Retain,
            });
        } else {
            world.trigger(SaveScenario { target });
        }
        world.insert_resource(previous);
        let error = match world.resource::<lunco_core::CommandResults>().get(id) {
            Some(lunco_core::CommandOutcome::Failed(message)) => Some(message.clone()),
            Some(lunco_core::CommandOutcome::Rejected(reason)) => Some(reason.to_string()),
            Some(lunco_core::CommandOutcome::Succeeded(_)) => None,
            _ => Some("Behaviour command did not report its admission result".into()),
        };
        if let Some(draft) = world
            .resource_mut::<RhaiEditorVm>()
            .drafts
            .get_mut(&(target, document))
        {
            draft.error = error;
            if let Some(source) = submitted.filter(|_| draft.error.is_none()) {
                draft.pending = Some((revision, source));
            }
        }
    });
}

/// Scene-owned editable state, including drafts for temporarily unselected scripts.
#[derive(Resource, Default)]
pub(crate) struct RhaiEditorVm {
    entity: Option<Entity>,
    doc_id: Option<u64>,
    drafts: std::collections::HashMap<(Entity, u64), BehaviourDraft>,
    state: CompileState,
    diagnostics: Vec<Diagnostic>,
    diagnostic_revision: u64,
}

impl lunco_core::SceneResource for RhaiEditorVm {}

/// Refresh from the owning script/diagnostics registries without discarding local drafts.
pub(crate) fn produce_rhai_editor_vm(
    selected: Option<Res<SelectedEntities>>,
    scripted: Query<&ScriptedModel>,
    registry: Option<Res<ScriptRegistry>>,
    diagnostics: Option<Res<DocumentDiagnostics>>,
    mut vm: ResMut<RhaiEditorVm>,
) {
    vm.drafts.retain(|(entity, doc), _| {
        scripted
            .get(*entity)
            .is_ok_and(|model| model.document_id == Some(*doc))
            && registry
                .as_deref()
                .is_some_and(|registry| registry.documents.contains_key(&DocumentId::new(*doc)))
    });
    let entity = selected
        .as_deref()
        .and_then(|selection| selection.primary());
    let doc_id = entity.and_then(|entity| scripted.get(entity).ok()?.document_id);
    let selection_changed = vm.entity != entity || vm.doc_id != doc_id;
    vm.entity = entity;
    vm.doc_id = doc_id;
    let report = doc_id.and_then(|doc_id| diagnostics.as_deref()?.get(DocumentId::new(doc_id)));
    let revision = report.map_or(0, |report| report.revision);
    if selection_changed || vm.diagnostic_revision != revision {
        vm.state = report.map_or_else(CompileState::default, |report| report.state);
        if let Some(report) = report {
            vm.diagnostics.clone_from(&report.diagnostics);
        } else {
            vm.diagnostics.clear();
        }
        vm.diagnostic_revision = revision;
    }
    let (Some(entity), Some(doc_id)) = (entity, doc_id) else {
        return;
    };
    let did = DocumentId::new(doc_id);
    let state = vm.state;
    let revision = vm.diagnostic_revision;
    if let Some(host) = registry
        .as_deref()
        .and_then(|registry| registry.documents.get(&did))
    {
        let doc = host.document();
        let draft = vm.drafts.entry((entity, doc_id)).or_default();
        let key = (doc_id, doc.generation);
        if let Some((submitted_revision, submitted_source)) = &draft.pending {
            if revision > *submitted_revision {
                if state == CompileState::Ready {
                    if doc.source == *submitted_source {
                        draft.dirty = draft.buffer != *submitted_source;
                        draft.loaded_key = Some(key);
                    } else {
                        draft.error = Some("Running source changed before this draft completed. Your draft is retained.".into());
                    }
                    draft.pending = None;
                } else if state == CompileState::Error {
                    draft.pending = None;
                }
            }
        } else if draft.loaded_key != Some(key) && !draft.dirty {
            draft.buffer = doc.source.clone();
            draft.loaded_key = Some(key);
        }
    }
}

pub(crate) struct RhaiEditorPanel;

impl Panel for RhaiEditorPanel {
    fn id(&self) -> PanelId {
        PanelId("rhai_editor")
    }
    fn title(&self) -> String {
        "Behaviour Editor".into()
    }
    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Editor
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

            let entity = vm.entity.expect("selected script checked above");
            let doc_id = vm.doc_id.expect("selected script checked above");
            let state = vm.state;
            let diagnostics = vm.diagnostics.clone();
            let diagnostic_revision = vm.diagnostic_revision;
            let draft = vm.drafts.entry((entity, doc_id)).or_default();
            let text_id = egui::Id::new(("rhai_editor_textedit", Some(entity)));
            let mut do_save = false;
            let mut do_revert = false;
            let mut jump_to: Option<usize> = None;

            // ── Toolbar: run / revert + compile status ──────────────────────
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(draft.dirty && draft.pending.is_none(), |ui| {
                    if ui
                        .button("Run draft")
                        .on_hover_text("Compile and run this draft. The scene document and saved file are unchanged.")
                        .clicked()
                    {
                        do_save = true;
                    }
                    if ui.button("↩ Revert").on_hover_text("Discard this draft and reload the script document source").clicked() {
                        do_revert = true;
                    }
                });
                let (icon, txt, col) = status_label(&state, &diagnostics, &theme);
                ui.horizontal_wrapped(|ui| {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(18.0, 18.0),
                        egui::Sense::hover(),
                    );
                    lunco_workbench_widgets::paint_icon(ui.painter(), icon, rect, col);
                    ui.label(egui::RichText::new(txt).color(col));
                });
                if draft.dirty {
                    ui.label(egui::RichText::new("Draft changes").color(theme.tokens.warning));
                }
            });
            if let Some(error) = &draft.error { ui.colored_label(theme.tokens.error, error); }
            if draft.pending.is_some() { ui.spinner(); ui.label("Compiling draft…"); }
            if ui.add_enabled(!draft.dirty && draft.pending.is_none() && state == CompileState::Ready,
                egui::Button::new("Apply running source to document"))
                .on_hover_text("Copy the compiled source into the scene document. Use File → Save to save the document to disk.")
                .clicked() {
                ctx.trigger(BehaviourEditorAction { target: entity, document: doc_id, source: None, diagnostic_revision });
            }
            ui.weak("Run previews a draft. Apply updates the scene document; File → Save writes it to disk.");
            ui.separator();

            // ── Editor ───────────────────────────────────────────────────────
            let editor_height = (ui.available_height() - diagnostics_height(&diagnostics)).max(80.0);
            let resp = egui::ScrollArea::vertical()
                .id_salt("rhai_editor_scroll")
                .max_height(editor_height)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add(
                        lunco_workbench_widgets::text_editor::code(&mut draft.buffer)
                            .id(text_id)
                            .desired_width(f32::INFINITY)
                            .desired_rows(20),
                    )
                })
                .inner;
            if resp.changed() {
                draft.dirty = true;
            }

            // ── Diagnostics list (the line/col gutter, as a jump list) ────────
            if !diagnostics.is_empty() {
                ui.separator();
                ui.label(
                    egui::RichText::new("Diagnostics")
                        .text_style(lunco_theme::TypographyRole::Label.text_style())
                        .weak(),
                );
                egui::ScrollArea::vertical()
                    .id_salt("rhai_editor_diags")
                    .max_height(110.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for d in &diagnostics {
                            let col = severity_color(d.severity, &theme);
                            let loc = match (d.line, d.col) {
                                (Some(l), Some(c)) => format!("{l}:{c}"),
                                (Some(l), None) => format!("{l}"),
                                _ => "—".to_string(),
                            };
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(18.0, 18.0),
                                egui::Sense::hover(),
                            );
                            lunco_workbench_widgets::paint_icon(
                                ui.painter(),
                                severity_icon(d.severity),
                                rect,
                                col,
                            );
                            let text = format!("{loc}  {}", d.message);
                            let hit = ui.add(
                                egui::Label::new(egui::RichText::new(text).color(col).monospace())
                                    .sense(egui::Sense::click()),
                            );
                            if hit.clicked() {
                                if let Some(line) = d.line {
                                    jump_to = Some(line_col_to_char_offset(
                                        &draft.buffer,
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
                draft.pending = None;
                // Drop the sync marker + dirty flag; the producer reloads the
                // saved source next frame.
                draft.loaded_key = None;
                draft.dirty = false;
            }
            if do_save {
                ctx.trigger(BehaviourEditorAction { target: entity, document: doc_id,
                    source: Some(draft.buffer.clone()), diagnostic_revision });
            }

        });
    }
}

fn empty_hint(ui: &mut egui::Ui, msg: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.label(egui::RichText::new(msg).weak());
        ui.label(
            egui::RichText::new("Left-click an object, then edit its Rhai behaviour here.").weak(),
        );
    });
}

/// Height to reserve for the diagnostics strip so the editor sizes to fill the
/// rest.
fn diagnostics_height(diags: &[Diagnostic]) -> f32 {
    if diags.is_empty() { 0.0 } else { 130.0 }
}

fn status_label(
    state: &CompileState,
    diags: &[Diagnostic],
    theme: &lunco_theme::Theme,
) -> (UiIcon, String, egui::Color32) {
    let errors = diags
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Error)
        .count();
    let warnings = diags
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Warning)
        .count();
    match state {
        CompileState::Ready => (UiIcon::Check, "compiled".to_string(), theme.tokens.success),
        CompileState::Error if errors > 0 => (
            UiIcon::Error,
            format!("{errors} error{}", if errors == 1 { "" } else { "s" }),
            theme.tokens.error,
        ),
        CompileState::Error => (UiIcon::Error, "error".to_string(), theme.tokens.error),
        _ if warnings > 0 => (
            UiIcon::Warning,
            format!("{warnings} warning{}", if warnings == 1 { "" } else { "s" }),
            theme.tokens.warning,
        ),
        _ => (UiIcon::Info, "idle".to_string(), theme.tokens.text_subdued),
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

fn severity_icon(s: DiagnosticSeverity) -> UiIcon {
    match s {
        DiagnosticSeverity::Error => UiIcon::Error,
        DiagnosticSeverity::Warning => UiIcon::Warning,
        DiagnosticSeverity::Info => UiIcon::Info,
        DiagnosticSeverity::Hint => UiIcon::Check,
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

/// Read-only projection for automation and accessibility diagnostics of editor state.
struct InspectBehaviourEditor;
impl lunco_api::ApiQueryProvider for InspectBehaviourEditor {
    fn name(&self) -> &'static str {
        "InspectBehaviourEditor"
    }
    fn execute(&self, world: &World, _: &lunco_hooks::HookValue) -> lunco_api::ApiQueryResult {
        let vm = world.resource::<RhaiEditorVm>();
        let draft = vm.entity.zip(vm.doc_id).and_then(|key| vm.drafts.get(&key));
        Ok(Some(lunco_api_core::api_value!({
            "selected_document": vm.doc_id,
            "draft_count": vm.drafts.len(),
            "source": draft.map(|draft| draft.buffer.clone()),
            "dirty": draft.is_some_and(|draft| draft.dirty),
            "pending": draft.is_some_and(|draft| draft.pending.is_some()),
            "error": draft.and_then(|draft| draft.error.clone()),
        })))
    }
}

pub(crate) fn register_inspection(app: &mut App) {
    app.init_resource::<lunco_api::ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<lunco_api::ApiQueryRegistry>()
        .register(InspectBehaviourEditor);
}
