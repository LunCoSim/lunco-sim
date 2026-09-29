use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use bevy_egui::egui;
use egui_extras::{Column, TableBuilder};
use lunco_doc::{Document, FileBacked};
use lunco_doc_bevy::{DocumentRegistry, OpenFile};
use lunco_sysml::{ApplySysmlOps, SaveSysmlDocument, SysmlApiOp, SysmlDocument};
use lunco_sysml_ast::SysmlElementHandle;
use lunco_sysml_ir::VerificationVerdict;
use lunco_workbench_core::source::OpenTwinSource;
use lunco_workbench_core::{
    Panel, PanelCtx, PanelId, PanelMenuGroup, PanelScrollPolicy, PanelSlot, PerspectiveId,
};

use crate::verification::{
    CancelSysmlVerification, CancelSysmlVerificationSuite, RunSysmlVerification,
    RunSysmlVerificationSuite, SysmlVerificationRuns, VerificationRunOutcome,
};
use crate::view_model::{
    AnalysisState, ModelElementView, ModelStructureNodeView, ParserDiagnosticView, RequirementRole,
    RequirementView, RuntimeRequirementEvidence, SourceFileView, SysmlRequirementsViewModel,
};

/// Canonical source view read from the SysML document owner.
pub(crate) struct DocumentView {
    pub id: lunco_doc::DocumentId,
    pub generation: u64,
    pub source: String,
    pub dirty: bool,
    pub writable: bool,
}

struct SourceEditor {
    twin_id: Option<lunco_workspace::TwinId>,
    logical_uri: String,
    doc_id: Option<lunco_doc::DocumentId>,
    buffer: String,
    base_source: String,
    base_generation: Option<u64>,
    conflict: bool,
    focus_line: Option<usize>,
}

impl SourceEditor {
    fn pending(
        twin_id: Option<lunco_workspace::TwinId>,
        source: &SourceFileView,
        focus_line: Option<usize>,
    ) -> Self {
        Self {
            twin_id,
            logical_uri: source.logical_uri.clone(),
            doc_id: source.document_id,
            buffer: String::new(),
            base_source: String::new(),
            base_generation: None,
            conflict: false,
            focus_line,
        }
    }

    fn attach_document(&mut self, document: &DocumentView) {
        self.doc_id = Some(document.id);
        self.buffer.clone_from(&document.source);
        self.base_source.clone_from(&document.source);
        self.base_generation = Some(document.generation);
        self.conflict = false;
    }

    fn sync_document(&mut self, document: &DocumentView) {
        let generation_changed = self.base_generation != Some(document.generation);
        if !generation_changed {
            return;
        }
        let has_local_draft = self.buffer != self.base_source;
        if !has_local_draft || self.buffer == document.source {
            self.buffer.clone_from(&document.source);
            self.base_source.clone_from(&document.source);
            self.base_generation = Some(document.generation);
            self.conflict = false;
        } else if document.source != self.base_source {
            self.conflict = true;
        }
    }

    fn has_local_draft(&self) -> bool {
        self.buffer != self.base_source
    }
}

enum PanelAction {
    OpenFile(String),
    OpenTwinSource {
        twin_root: String,
        relative_path: String,
    },
    ApplySource {
        doc_id: lunco_doc::DocumentId,
        parent_generation: u64,
        source: String,
    },
    Save(lunco_doc::DocumentId),
    RunVerification {
        twin_id: lunco_workspace::TwinId,
        source_revision: u64,
        name: String,
    },
    CancelVerification {
        twin_id: lunco_workspace::TwinId,
        name: String,
    },
    RunVerificationSuite {
        twin_id: lunco_workspace::TwinId,
        source_revision: u64,
        names: Vec<String>,
    },
    CancelVerificationSuite {
        twin_id: lunco_workspace::TwinId,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Filter for current or stale structured requirement-check evidence.
enum EvidenceFilter {
    #[default]
    All,
    Failed,
    Unverified,
    Stale,
    Missing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Filter for Twin-mapped scene-test execution state.
enum TestFilter {
    #[default]
    All,
    Failed,
    SuiteFailures,
    NeedsResult,
    NotRun,
    Stale,
    Unmapped,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Filter for formal requirement criteria and resolved verification links.
enum CoverageFilter {
    #[default]
    All,
    MissingRequire,
    MissingVerify,
    UnmappedVerify,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Independent status filters applied together to the requirement list.
struct RequirementFilters {
    evidence: EvidenceFilter,
    tests: TestFilter,
    coverage: CoverageFilter,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RequirementsWorkspaceView {
    #[default]
    Requirements,
    Traceability,
    Structure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RequirementDraftSource {
    twin_id: lunco_workspace::TwinId,
    logical_uri: String,
}

#[derive(bevy::prelude::Resource, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SysmlRequirementPanelState {
    twin_id: Option<lunco_workspace::TwinId>,
    selected_requirement: Option<String>,
    draft_source: Option<RequirementDraftSource>,
    unsaved_source: bool,
    pending_source: Option<(lunco_workspace::TwinId, String, usize)>,
    open_traceability: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RequirementSortColumn {
    #[default]
    Id,
    Kind,
    Source,
    Evidence,
    TwinTest,
    Coverage,
    Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequirementQuickFilter {
    NeedsCriterion,
    NeedsVerify,
    NeedsTwinMapping,
    NotRun,
}

/// Compact requirements browser for the active Twin, with source navigation
/// and an inline editor backed by the canonical SysML document registry.
#[derive(Default)]
pub struct SysmlRequirementsPanel {
    search: String,
    filters: RequirementFilters,
    selected_source: Option<String>,
    selected_requirement: Option<String>,
    selected_structure_element: Option<SysmlElementHandle>,
    view_twin_id: Option<lunco_workspace::TwinId>,
    active_view: RequirementsWorkspaceView,
    trace_search: String,
    structure_search: String,
    sort_column: RequirementSortColumn,
    sort_descending: bool,
    editor: Option<SourceEditor>,
}

impl Panel for SysmlRequirementsPanel {
    fn id(&self) -> PanelId {
        PanelId("sysml_requirements")
    }

    fn title(&self) -> String {
        "SysML Requirements".into()
    }

    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Editor
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::Center
    }

    fn preferred_perspective(&self) -> Option<PerspectiveId> {
        Some(PerspectiveId("editor"))
    }

    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let mut panel_state = ctx
            .resource::<SysmlRequirementPanelState>()
            .cloned()
            .unwrap_or_default();
        let actions = {
            let view_model = ctx.resource::<SysmlRequirementsViewModel>();
            let documents = ctx.resource::<DocumentRegistry<SysmlDocument>>();
            let runs = ctx.resource::<SysmlVerificationRuns>();
            self.render_panel(ui, view_model, documents, runs, &theme, &mut panel_state)
        };
        panel_state.twin_id = self.view_twin_id;
        panel_state.selected_requirement = self.view_twin_id.and(self.selected_requirement.clone());
        panel_state.draft_source = self.editor.as_ref().and_then(|editor| {
            editor.has_local_draft().then(|| {
                editor.twin_id.map(|twin_id| RequirementDraftSource {
                    twin_id,
                    logical_uri: editor.logical_uri.clone(),
                })
            })?
        });
        if ctx.resource::<SysmlRequirementPanelState>() != Some(&panel_state) {
            ctx.set_resource(panel_state);
        }
        dispatch_panel_actions(ctx, actions);
    }
}

/// Stable dock id for the selected requirement's detail pane.
const SYSML_REQUIREMENT_DETAILS_PANEL_ID: PanelId = PanelId("sysml_requirement_details");

#[derive(Default)]
pub(super) struct SysmlRequirementDetailsPanel;

impl Panel for SysmlRequirementDetailsPanel {
    fn id(&self) -> PanelId {
        SYSML_REQUIREMENT_DETAILS_PANEL_ID
    }

    fn title(&self) -> String {
        "Requirement details".into()
    }

    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Editor
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspectorBottom
    }

    fn visible_in_perspective(&self, perspective: PerspectiveId) -> Option<PanelSlot> {
        (perspective == PerspectiveId("editor")).then_some(PanelSlot::RightInspectorBottom)
    }

    fn preferred_perspective(&self) -> Option<PerspectiveId> {
        Some(PerspectiveId("editor"))
    }

    fn scroll_policy(&self) -> PanelScrollPolicy {
        PanelScrollPolicy::SelfManaged
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let Some(view_model) = ctx.resource::<SysmlRequirementsViewModel>() else {
            ui.label("SysML requirements view is not available.");
            return;
        };
        match &view_model.state {
            AnalysisState::NoActiveTwin => {
                ui.label("Open a Twin to inspect its requirements.");
                return;
            }
            AnalysisState::Waiting => {
                muted(ui, &theme, "Waiting for SysML source analysis…");
                return;
            }
            AnalysisState::NoSources => {
                ui.label("This Twin has no indexed .sysml or .kerml sources.");
                return;
            }
            AnalysisState::Failed(errors) => {
                for error in errors {
                    ui.colored_label(theme.tokens.error, error);
                }
                return;
            }
            AnalysisState::Ready => {}
        }

        let mut panel_state = ctx
            .resource::<SysmlRequirementPanelState>()
            .cloned()
            .unwrap_or_default();
        let selected = (panel_state.twin_id == view_model.twin_id)
            .then_some(panel_state.selected_requirement.as_deref())
            .flatten()
            .and_then(|name| {
                view_model
                    .requirements
                    .iter()
                    .find(|requirement| requirement.qualified_name == name)
            });
        let has_draft_for_another_source = panel_state
            .draft_source
            .as_ref()
            .zip(selected)
            .is_some_and(|(draft, requirement)| {
                Some(draft.twin_id) != view_model.twin_id
                    || draft.logical_uri != requirement.logical_uri
            });
        let mut actions = Vec::new();
        let mut open_traceability = false;
        let source = SysmlRequirementsPanel::render_requirement_detail(
            ui,
            view_model,
            selected,
            ctx.resource::<SysmlVerificationRuns>(),
            panel_state.unsaved_source && panel_state.twin_id == view_model.twin_id,
            has_draft_for_another_source,
            &theme,
            &mut actions,
            &mut open_traceability,
        );
        if open_traceability {
            panel_state.open_traceability = true;
        }
        if let (Some((source, line)), Some(twin_id)) = (source, view_model.twin_id) {
            panel_state.pending_source = Some((twin_id, source.logical_uri, line));
        }
        if ctx.resource::<SysmlRequirementPanelState>() != Some(&panel_state) {
            ctx.set_resource(panel_state);
        }
        dispatch_panel_actions(ctx, actions);
    }
}

fn dispatch_panel_actions(ctx: &mut PanelCtx, actions: Vec<PanelAction>) {
    for action in actions {
        match action {
            PanelAction::OpenFile(path) => ctx.trigger(OpenFile { path }),
            PanelAction::OpenTwinSource {
                twin_root,
                relative_path,
            } => ctx.trigger(OpenTwinSource {
                twin_root,
                relative_path,
                pinned: true,
                focus: Some(true),
            }),
            PanelAction::ApplySource {
                doc_id,
                parent_generation,
                source,
            } => ctx.trigger(ApplySysmlOps {
                doc_id,
                ops: vec![SysmlApiOp::ReplaceSource { source }],
                parent_generation: Some(parent_generation),
            }),
            PanelAction::Save(doc_id) => ctx.trigger(SaveSysmlDocument { doc_id }),
            PanelAction::RunVerification {
                twin_id,
                source_revision,
                name,
            } => ctx.trigger(RunSysmlVerification {
                twin_id,
                source_revision,
                name,
            }),
            PanelAction::CancelVerification { twin_id, name } => {
                ctx.trigger(CancelSysmlVerification { twin_id, name })
            }
            PanelAction::RunVerificationSuite {
                twin_id,
                source_revision,
                names,
            } => ctx.trigger(RunSysmlVerificationSuite {
                twin_id,
                source_revision,
                names,
            }),
            PanelAction::CancelVerificationSuite { twin_id } => {
                ctx.trigger(CancelSysmlVerificationSuite { twin_id })
            }
        }
    }
}

impl SysmlRequirementsPanel {
    fn render_panel(
        &mut self,
        ui: &mut egui::Ui,
        view_model: Option<&SysmlRequirementsViewModel>,
        documents: Option<&DocumentRegistry<SysmlDocument>>,
        runs: Option<&SysmlVerificationRuns>,
        theme: &lunco_theme::Theme,
        panel_state: &mut SysmlRequirementPanelState,
    ) -> Vec<PanelAction> {
        let Some(view_model) = view_model else {
            ui.label("SysML requirements view is not available.");
            panel_state.twin_id = None;
            panel_state.selected_requirement = None;
            panel_state.draft_source = None;
            panel_state.pending_source = None;
            panel_state.open_traceability = false;
            return Vec::new();
        };
        let mut actions = Vec::new();
        if self.view_twin_id != view_model.twin_id {
            self.view_twin_id = view_model.twin_id;
            self.active_view = RequirementsWorkspaceView::Requirements;
            self.selected_structure_element = None;
            panel_state.pending_source = None;
            panel_state.open_traceability = false;
        }
        if panel_state.twin_id == view_model.twin_id {
            if panel_state.open_traceability {
                self.active_view = RequirementsWorkspaceView::Traceability;
                panel_state.open_traceability = false;
            }
        }
        self.render_header(ui, view_model, theme);

        match &view_model.state {
            AnalysisState::NoActiveTwin => {
                ui.label("Open a Twin to browse its SysML requirements.");
                return actions;
            }
            AnalysisState::Waiting => {
                muted(
                    ui,
                    theme,
                    "Waiting for the active Twin's SysML source analysis…",
                );
                return actions;
            }
            AnalysisState::NoSources => {
                ui.label("This Twin has no indexed .sysml or .kerml sources.");
                return actions;
            }
            AnalysisState::Failed(errors) => {
                for error in errors {
                    ui.colored_label(theme.tokens.error, error);
                }
                return actions;
            }
            AnalysisState::Ready => {}
        }

        if view_model.analysis_has_errors {
            ui.colored_label(
                theme.tokens.error,
                format!(
                    "INVALID MODEL · {} syntax, name-resolution, or package-collision diagnostic(s)",
                    view_model.parser_diagnostics.len()
                ),
            );
            ui.label("Fix the source diagnostics before trusting results or running verification.");
        }

        if self.selected_source.as_ref().is_some_and(|selected| {
            !view_model
                .source_files
                .iter()
                .any(|file| &file.logical_uri == selected)
        }) {
            self.selected_source = None;
        }
        if self.selected_requirement.as_ref().is_none_or(|selected| {
            !view_model
                .requirements
                .iter()
                .any(|requirement| &requirement.qualified_name == selected)
        }) {
            self.selected_requirement = view_model
                .requirements
                .iter()
                .find(|requirement| has_mapped_verification(requirement, view_model))
                .or_else(|| view_model.requirements.first())
                .map(|requirement| requirement.qualified_name.clone());
        }

        let unsaved_source = self.has_unsaved_source_edits(view_model, documents);
        panel_state.unsaved_source = unsaved_source;
        self.render_workspace_tabs(ui, theme);
        let mut edit_source = match self.active_view {
            RequirementsWorkspaceView::Requirements if view_model.requirements.is_empty() => {
                ui.label("No requirement declarations were found in the analyzed source set.");
                ui.separator();
                ui.heading("SysML source files");
                let mut open_source = None;
                for source in &view_model.source_files {
                    ui.horizontal(|ui| {
                        ui.label(source.relative_path.display().to_string());
                        if ui.button("Open").clicked() {
                            open_source = Some(source.clone());
                        }
                    });
                }
                open_source.map(|source| (source, 1))
            }
            RequirementsWorkspaceView::Requirements => self.render_requirement_workspace(
                ui,
                view_model,
                runs,
                unsaved_source,
                theme,
                &mut actions,
            ),
            RequirementsWorkspaceView::Traceability => self.render_traceability_workspace(
                ui,
                view_model,
                runs,
                unsaved_source,
                theme,
                &mut actions,
            ),
            RequirementsWorkspaceView::Structure => {
                self.render_structure_workspace(ui, view_model, theme)
            }
        };
        if panel_state.twin_id == view_model.twin_id
            && let Some((twin_id, logical_uri, line)) = panel_state.pending_source.take()
            && Some(twin_id) == view_model.twin_id
            && let Some(source) = view_model
                .source_files
                .iter()
                .find(|source| source.logical_uri == logical_uri)
        {
            edit_source = Some((source.clone(), line));
        }
        if let Some(source) = edit_source {
            let (source, line) = source;
            let same_source = self.editor.as_ref().is_some_and(|editor| {
                editor.logical_uri == source.logical_uri && editor.twin_id == view_model.twin_id
            });
            let blocked_by_draft = self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.has_local_draft() && !same_source);
            if blocked_by_draft {
                muted(
                    ui,
                    theme,
                    "Save or discard the current source draft before opening another file.",
                );
            } else if same_source {
                if let Some(editor) = self.editor.as_mut() {
                    editor.focus_line = Some(line);
                }
            } else {
                self.editor = Some(self.editor_for_source(
                    view_model.twin_id,
                    &source,
                    Some(line),
                    documents,
                ));
            }
        }

        let close_editor = if let Some(editor) = &mut self.editor {
            ui.separator();
            Self::render_source_editor(ui, view_model, editor, documents, theme, &mut actions)
        } else {
            false
        };
        if close_editor {
            self.editor = None;
        }
        self.render_source_diagnostics(ui, view_model, documents, theme);
        actions
    }

    fn render_workspace_tabs(&mut self, ui: &mut egui::Ui, theme: &lunco_theme::Theme) {
        ui.horizontal(|ui| {
            for (view, label) in [
                (RequirementsWorkspaceView::Requirements, "Requirements"),
                (RequirementsWorkspaceView::Traceability, "Traceability"),
                (RequirementsWorkspaceView::Structure, "Structure"),
            ] {
                ui.selectable_value(&mut self.active_view, view, label);
            }
        });
        ui.separator();
        ui.add_space(theme.spacing.item_spacing);
    }

    fn render_requirement_workspace(
        &mut self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        runs: Option<&SysmlVerificationRuns>,
        unsaved_source: bool,
        theme: &lunco_theme::Theme,
        actions: &mut Vec<PanelAction>,
    ) -> Option<(SourceFileView, usize)> {
        self.render_summary(ui, view_model, runs, unsaved_source, theme, actions);
        ui.add_space(theme.spacing.item_spacing);
        self.render_filters(ui, view_model, runs, theme);
        ui.add_space(theme.spacing.item_spacing);

        let search = self.search.trim().to_lowercase();
        let filtered: Vec<usize> = view_model
            .requirements
            .iter()
            .enumerate()
            .filter(|(_, requirement)| {
                self.selected_source
                    .as_deref()
                    .is_none_or(|source| requirement.logical_uri == source)
                    && requirement_matches_search(requirement, &search)
                    && requirement_matches_filters(requirement, view_model, runs, self.filters)
            })
            .map(|(index, _)| index)
            .collect();

        if self.selected_requirement.as_ref().is_none_or(|selected| {
            !filtered
                .iter()
                .any(|index| view_model.requirements[*index].qualified_name == *selected)
        }) {
            self.selected_requirement = filtered
                .iter()
                .map(|index| &view_model.requirements[*index])
                .find(|requirement| has_mapped_verification(requirement, view_model))
                .or_else(|| {
                    filtered
                        .first()
                        .map(|index| &view_model.requirements[*index])
                })
                .map(|requirement| requirement.qualified_name.clone());
        }
        ui.heading("Requirements");
        self.render_requirement_list(ui, view_model, &filtered, runs, theme);
        None
    }

    fn render_traceability_workspace(
        &mut self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        runs: Option<&SysmlVerificationRuns>,
        unsaved_source: bool,
        theme: &lunco_theme::Theme,
        actions: &mut Vec<PanelAction>,
    ) -> Option<(SourceFileView, usize)> {
        ui.heading("Requirement traceability");
        if view_model.requirements.is_empty() {
            muted(
                ui,
                theme,
                "This source set has no requirement declarations to map.",
            );
            return None;
        }

        ui.horizontal(|ui| {
            ui.label("Requirement");
            ui.add(
                egui::TextEdit::singleline(&mut self.trace_search)
                    .hint_text("Filter requirements")
                    .desired_width(190.0),
            );
            let selected_label = self
                .selected_requirement
                .as_deref()
                .and_then(|name| {
                    view_model
                        .requirements
                        .iter()
                        .find(|requirement| requirement.qualified_name == name)
                })
                .map(|requirement| requirement.display_name.as_str())
                .unwrap_or("Select requirement");
            egui::ComboBox::from_id_salt("sysml_traceability_requirement")
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    let query = self.trace_search.trim().to_lowercase();
                    for requirement in &view_model.requirements {
                        if !query.is_empty()
                            && !requirement.display_name.to_lowercase().contains(&query)
                            && !requirement.qualified_name.to_lowercase().contains(&query)
                        {
                            continue;
                        }
                        ui.selectable_value(
                            &mut self.selected_requirement,
                            Some(requirement.qualified_name.clone()),
                            format!(
                                "{} · {}",
                                requirement.display_name, requirement.qualified_name
                            ),
                        );
                    }
                });
        });

        let selected = self.selected_requirement.as_deref().and_then(|name| {
            view_model
                .requirements
                .iter()
                .find(|requirement| requirement.qualified_name == name)
        });
        let Some(requirement) = selected else {
            muted(
                ui,
                theme,
                "Choose a requirement to inspect its source-to-test path.",
            );
            return None;
        };

        let mut open_source = None;
        egui::ScrollArea::horizontal()
            .id_salt("sysml_traceability_map")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    trace_card(ui, theme, "REQUIREMENT", |ui| {
                        ui.strong(&requirement.display_name);
                        muted(ui, theme, &requirement.qualified_name);
                        let (rollup, color, explanation) = requirement_rollup_label(
                            requirement_rollup_state(requirement, view_model, runs),
                            theme,
                        );
                        ui.colored_label(color, rollup).on_hover_text(explanation);
                        if let Some(path) = &requirement.relative_path {
                            ui.label(format!(
                                "{}:{}",
                                path.display(),
                                requirement.line.unwrap_or(1)
                            ));
                            if ui.small_button("Open requirement").clicked() {
                                open_source = view_model
                                    .source_files
                                    .iter()
                                    .find(|source| source.logical_uri == requirement.logical_uri)
                                    .cloned()
                                    .map(|source| (source, requirement.line.unwrap_or(1)));
                            }
                        }
                    });
                    trace_arrow(ui, theme);
                    trace_card(ui, theme, "SUBJECT TYPES", |ui| {
                        if requirement.subjects.is_empty() {
                            muted(ui, theme, "No subject type is declared.");
                        }
                        for subject in &requirement.subjects {
                            let type_name = subject.type_name.as_deref().unwrap_or("untyped");
                            ui.label(format!("{} : {type_name}", subject.name));
                            if subject.ambiguous_target {
                                ui.colored_label(theme.tokens.warning, "Ambiguous model type");
                            }
                            if let Some(target) = &subject.target
                                && ui
                                    .small_button(format!("Open {}", target.display_name))
                                    .clicked()
                            {
                                open_source = source_location(view_model, target);
                            }
                        }
                    });
                    trace_arrow(ui, theme);
                    trace_card(ui, theme, "SATISFIED BY", |ui| {
                        if requirement.satisfied_by.is_empty() {
                            muted(ui, theme, "No explicit SysML satisfy relationship.");
                        }
                        for element in &requirement.satisfied_by {
                            ui.label(format!(
                                "{} · {}",
                                structure_kind_label(&element.kind),
                                element.display_name
                            ));
                            muted(ui, theme, &element.qualified_name);
                            if ui
                                .small_button(format!("Open {}", element.display_name))
                                .clicked()
                            {
                                open_source = source_location(view_model, element);
                            }
                        }
                    });
                    trace_arrow(ui, theme);
                    trace_card(ui, theme, "VERIFY & RUN", |ui| {
                        if requirement.verification_cases.is_empty() {
                            ui.colored_label(theme.tokens.warning, "No resolved verify link.");
                        }
                        for name in &requirement.verification_cases {
                            ui.separator();
                            ui.strong(name);
                            let test = view_model
                                .verification_cases
                                .iter()
                                .find(|case| case.name == *name);
                            let verification_element = test
                                .and_then(|test| test.source_element.as_ref())
                                .or_else(|| {
                                    view_model
                                        .model_elements
                                        .iter()
                                        .find(|element| element.qualified_name == *name)
                                });
                            if let Some(element) = verification_element
                                && ui.small_button("Open verification").clicked()
                            {
                                open_source = source_location(view_model, element);
                            }
                            if let Some(test) = test {
                                ui.colored_label(theme.tokens.success, "TWIN TEST MAPPED");
                                muted(ui, theme, &format!("Scene: {}", test.scene.display()));
                                let (label, color) = verification_case_status(
                                    view_model.twin_id,
                                    name,
                                    view_model.source_revision,
                                    runs,
                                    theme,
                                );
                                ui.colored_label(color, label);
                                let can_run = cfg!(not(target_arch = "wasm32"))
                                    && !runs.is_some_and(SysmlVerificationRuns::has_active_run)
                                    && !unsaved_source
                                    && !view_model.analysis_has_errors
                                    && view_model.verification_setup_errors.is_empty();
                                if ui
                                    .add_enabled(can_run, egui::Button::new("Run test"))
                                    .on_hover_text(if unsaved_source {
                                        "Save or discard SysML edits before running saved Twin files."
                                    } else {
                                        "Run this mapped scene through the production scene-test runner."
                                    })
                                    .clicked()
                                    && let (Some(twin_id), Some(source_revision)) =
                                        (view_model.twin_id, view_model.source_revision)
                                {
                                    actions.push(PanelAction::RunVerification {
                                        twin_id,
                                        source_revision,
                                        name: name.clone(),
                                    });
                                }
                            } else {
                                ui.colored_label(theme.tokens.warning, "NO TWIN TEST MAPPING");
                            }
                        }
                    });
                    trace_arrow(ui, theme);
                    trace_card(ui, theme, "EVIDENCE", |ui| {
                        let (label, color, explanation) =
                            evidence_status(requirement, view_model, runs, theme);
                        ui.colored_label(color, label).on_hover_text(explanation);
                        let (coverage, coverage_color) =
                            model_coverage_label(requirement, view_model, theme);
                        ui.colored_label(coverage_color, coverage);
                        let records = requirement_evidence_records(requirement, view_model, runs);
                        if records.is_empty() {
                            muted(ui, theme, "No structured check evidence is available.");
                        }
                        for evidence in &records {
                            ui.separator();
                            ui.label(&evidence.channel);
                            ui.label(format!(
                                "{} checks · {} fail · {} inconclusive · {} unverified · {} errors",
                                evidence.checks,
                                evidence.failures,
                                evidence.inconclusive,
                                evidence.unverified,
                                evidence.errors,
                            ));
                            let revision_state = if Some(evidence.source_revision)
                                == view_model.source_revision
                            {
                                "CURRENT"
                            } else {
                                "STALE"
                            };
                            muted(
                                ui,
                                theme,
                                &format!(
                                    "{revision_state} · revision {} · tick {}",
                                    evidence.source_revision, evidence.sim_tick
                                ),
                            );
                        }
                        muted(
                            ui,
                            theme,
                            &format!(
                                "Analyzed revision {}",
                                view_model.source_revision.unwrap_or_default()
                            ),
                        );
                    });
                });
            });
        muted(
            ui,
            theme,
            "Subject types are shown separately from explicit satisfy links. Test and evidence status carry their source revision.",
        );
        open_source
    }

    fn render_structure_workspace(
        &mut self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        theme: &lunco_theme::Theme,
    ) -> Option<(SourceFileView, usize)> {
        ui.heading("Model structure");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.structure_search)
                    .hint_text("Filter packages, parts, ports, connections…")
                    .desired_width(280.0),
            );
            muted(
                ui,
                theme,
                &format!(
                    "{} structural elements",
                    count_structure_nodes(&view_model.model_structure)
                ),
            );
        });
        if view_model.model_structure.is_empty() {
            muted(
                ui,
                theme,
                "No package, part, item, interface, port, or connection elements were found in the supported projection.",
            );
            return None;
        }

        let query = self.structure_search.trim().to_lowercase();
        let matching = matching_structure_nodes(&view_model.model_structure, &query);
        if self.selected_structure_element.is_some_and(|selected| {
            find_structure_element(&view_model.model_structure, selected).is_none()
        }) {
            self.selected_structure_element = None;
        }

        let mut open_source = None;
        if ui.available_width() >= 760.0 {
            ui.columns(2, |columns| {
                columns[0].heading("Packages and structure");
                egui::ScrollArea::vertical()
                    .id_salt("sysml_model_structure_tree")
                    .max_height(440.0)
                    .show(&mut columns[0], |ui| {
                        for root in &view_model.model_structure {
                            self.render_structure_node(
                                ui,
                                view_model,
                                root,
                                0,
                                &query,
                                &matching,
                                theme,
                                &mut open_source,
                            );
                        }
                    });
                columns[1].heading("Selected element");
                if let Some(selected) = self
                    .selected_structure_element
                    .and_then(|handle| find_structure_element(&view_model.model_structure, handle))
                {
                    render_structure_details(
                        &mut columns[1],
                        view_model,
                        selected,
                        theme,
                        &mut open_source,
                    );
                } else {
                    muted(
                        &mut columns[1],
                        theme,
                        "Select a package, part, port, interface, item, or connection.",
                    );
                }
            });
        } else {
            ui.heading("Packages and structure");
            egui::ScrollArea::vertical()
                .id_salt("sysml_model_structure_tree_narrow")
                .max_height(360.0)
                .show(ui, |ui| {
                    for root in &view_model.model_structure {
                        self.render_structure_node(
                            ui,
                            view_model,
                            root,
                            0,
                            &query,
                            &matching,
                            theme,
                            &mut open_source,
                        );
                    }
                });
            ui.separator();
            ui.heading("Selected element");
            if let Some(selected) = self
                .selected_structure_element
                .and_then(|handle| find_structure_element(&view_model.model_structure, handle))
            {
                render_structure_details(ui, view_model, selected, theme, &mut open_source);
            } else {
                muted(
                    ui,
                    theme,
                    "Select a structural element to inspect and open its source.",
                );
            }
        }
        open_source
    }

    fn render_structure_node(
        &mut self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        node: &ModelStructureNodeView,
        depth: usize,
        query: &str,
        matching: &std::collections::HashSet<SysmlElementHandle>,
        theme: &lunco_theme::Theme,
        open_source: &mut Option<(SourceFileView, usize)>,
    ) {
        if !matching.contains(&node.element.handle) {
            return;
        }
        let handle = node.element.handle;
        let selected = self.selected_structure_element == Some(handle);
        let id = ui.make_persistent_id(("sysml_structure_node", handle));
        let default_open = !query.is_empty() || depth == 0;
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            default_open,
        )
        .show_header(ui, |ui| {
            let title = format!(
                "{} · {}",
                structure_kind_label(&node.element.kind),
                node.element.display_name
            );
            if ui
                .selectable_label(
                    selected,
                    egui::RichText::new(title)
                        .color(structure_kind_color(&node.element.kind, theme)),
                )
                .clicked()
            {
                self.selected_structure_element = Some(handle);
            }
            if ui.small_button("Open").clicked() {
                self.selected_structure_element = Some(handle);
                *open_source = source_location(view_model, &node.element);
            }
        })
        .body(|ui| {
            for child in &node.children {
                self.render_structure_node(
                    ui,
                    view_model,
                    child,
                    depth + 1,
                    query,
                    matching,
                    theme,
                    open_source,
                );
            }
        });
    }

    fn has_unsaved_source_edits(
        &self,
        view_model: &SysmlRequirementsViewModel,
        documents: Option<&DocumentRegistry<SysmlDocument>>,
    ) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.twin_id == view_model.twin_id && editor.has_local_draft())
            || view_model
                .source_files
                .iter()
                .filter_map(|source| source.document_view(documents))
                .any(|document| document.dirty)
    }

    fn render_header(
        &self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        theme: &lunco_theme::Theme,
    ) {
        ui.horizontal(|ui| {
            ui.heading("SysML Requirements");
            if let Some(name) = &view_model.twin_name {
                muted(ui, theme, name);
            }
        });
        if let Some(revision) = view_model.source_revision {
            muted(ui, theme, &format!("Analyzed source revision {revision}"));
        }
    }

    fn render_summary(
        &mut self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        runs: Option<&SysmlVerificationRuns>,
        unsaved_source: bool,
        theme: &lunco_theme::Theme,
        actions: &mut Vec<PanelAction>,
    ) {
        let evidence_counts = [
            (EvidenceState::Pass, "pass", theme.tokens.success),
            (EvidenceState::Fail, "fail", theme.tokens.error),
            (
                EvidenceState::Inconclusive,
                "inconclusive",
                theme.tokens.warning,
            ),
            (
                EvidenceState::Unverified,
                "unverified",
                theme.tokens.warning,
            ),
            (EvidenceState::Error, "error", theme.tokens.error),
            (EvidenceState::Stale, "stale", theme.tokens.warning),
            (
                EvidenceState::NoEvidence,
                "no evidence",
                theme.tokens.text_subdued,
            ),
        ]
        .map(|(state, label, color)| {
            let count = view_model
                .requirements
                .iter()
                .filter(|requirement| evidence_state(requirement, view_model, runs) == state)
                .count();
            (label, count, color)
        });
        let execution_counts = [
            (ExecutionState::Pass, "pass", theme.tokens.success),
            (ExecutionState::Fail, "fail", theme.tokens.error),
            (
                ExecutionState::Inconclusive,
                "inconclusive",
                theme.tokens.warning,
            ),
            (
                ExecutionState::Unverified,
                "unverified",
                theme.tokens.warning,
            ),
            (ExecutionState::Error, "error", theme.tokens.error),
            (ExecutionState::RunError, "run error", theme.tokens.error),
            (ExecutionState::Partial, "partial", theme.tokens.warning),
            (
                ExecutionState::Running,
                "running",
                theme.tokens.text_subdued,
            ),
            (ExecutionState::NotRun, "not run", theme.tokens.text_subdued),
            (ExecutionState::Stale, "stale", theme.tokens.warning),
            (
                ExecutionState::NoVerify,
                "no verify link",
                theme.tokens.warning,
            ),
            (
                ExecutionState::NoRunner,
                "no Twin mapping",
                theme.tokens.warning,
            ),
            (ExecutionState::NoVerdict, "no verdict", theme.tokens.error),
            (
                ExecutionState::Cancelled,
                "cancelled",
                theme.tokens.text_subdued,
            ),
        ]
        .map(|(state, label, color)| {
            let count = view_model
                .requirements
                .iter()
                .filter(|requirement| {
                    execution_summary(requirement, view_model, runs).state == state
                })
                .count();
            (label, count, color)
        });
        let missing_criteria = view_model
            .requirements
            .iter()
            .filter(|requirement| !requirement.has_required_constraint)
            .count();
        let no_verify = view_model
            .requirements
            .iter()
            .filter(|requirement| requirement.verification_cases.is_empty())
            .count();
        let unmapped_verify = view_model
            .requirements
            .iter()
            .filter(|requirement| {
                !requirement.verification_cases.is_empty()
                    && requirement
                        .verification_cases
                        .iter()
                        .any(|name| !verification_case_is_mapped(name, view_model))
            })
            .count();
        let definitions = view_model
            .requirements
            .iter()
            .filter(|requirement| requirement.role == RequirementRole::Definition)
            .count();
        let usages = view_model.requirements.len().saturating_sub(definitions);
        let not_run = view_model
            .requirements
            .iter()
            .filter(|requirement| {
                execution_summary(requirement, view_model, runs).state == ExecutionState::NotRun
            })
            .count();
        let mut quick_filter = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("Inventory");
            ui.label(format!("{usages} requirement usages"));
            ui.separator();
            ui.label(format!("{definitions} definitions"));
            ui.separator();
            ui.label(format!("{} source files", view_model.source_files.len()));
        });
        ui.horizontal_wrapped(|ui| {
            if quick_filter_button(
                ui,
                theme,
                missing_criteria,
                "no formal require",
                theme.tokens.text_subdued,
            ) {
                quick_filter = Some(RequirementQuickFilter::NeedsCriterion);
            }
            if quick_filter_button(ui, theme, no_verify, "no verify link", theme.tokens.warning) {
                quick_filter = Some(RequirementQuickFilter::NeedsVerify);
            }
            if quick_filter_button(
                ui,
                theme,
                unmapped_verify,
                "unmapped Twin tests",
                theme.tokens.warning,
            ) {
                quick_filter = Some(RequirementQuickFilter::NeedsTwinMapping);
            }
            if quick_filter_button(
                ui,
                theme,
                not_run,
                "tests not run",
                theme.tokens.text_subdued,
            ) {
                quick_filter = Some(RequirementQuickFilter::NotRun);
            }
        });
        if let Some(quick_filter) = quick_filter {
            self.apply_quick_filter(quick_filter);
        }
        egui::CollapsingHeader::new("Status breakdown")
            .default_open(false)
            .show(ui, |ui| {
                self.render_status_breakdown(
                    ui,
                    view_model,
                    runs,
                    &evidence_counts,
                    &execution_counts,
                    missing_criteria,
                    no_verify,
                    unmapped_verify,
                    theme,
                );
            });
        ui.horizontal_wrapped(|ui| {
            let names = mapped_verification_names(view_model);
            if let Some(twin_id) = view_model.twin_id
                && let Some((started, total, pending, stopping)) =
                    runs.and_then(|runs| runs.suite_progress(twin_id))
            {
                ui.spinner();
                let active_name = runs
                    .and_then(SysmlVerificationRuns::active_case)
                    .filter(|(active_twin, _)| *active_twin == twin_id)
                    .map(|(_, name)| name);
                let progress = if stopping {
                    format!("Stopping suite after current test · {started}/{total} started")
                } else {
                    active_name.map_or_else(
                        || format!("Preparing next test · {started}/{total} started · {pending} queued"),
                        |name| format!("Running {name} · {started}/{total} started · {pending} queued"),
                    )
                };
                muted(ui, theme, &progress);
                if !stopping && ui.button("Stop suite").clicked() {
                    actions.push(PanelAction::CancelVerificationSuite { twin_id });
                }
            } else {
                let can_run = cfg!(not(target_arch = "wasm32"))
                    && !runs.is_some_and(SysmlVerificationRuns::has_active_run)
                    && !unsaved_source
                    && !view_model.analysis_has_errors
                    && view_model.verification_setup_errors.is_empty();
                let selected_names = self
                    .selected_requirement
                    .as_deref()
                    .and_then(|selected| {
                        view_model
                            .requirements
                            .iter()
                            .find(|requirement| requirement.qualified_name == selected)
                    })
                    .map(|requirement| mapped_names_for_requirement(requirement, view_model))
                    .unwrap_or_default();
                ui.horizontal_wrapped(|ui| {
                    let selected_label = format!(
                        "Run selected requirement tests ({})",
                        selected_names.len()
                    );
                    if ui
                        .add_enabled(
                            can_run && !selected_names.is_empty(),
                            egui::Button::new(selected_label),
                        )
                        .on_hover_text(
                            "Runs the distinct Twin-mapped cases linked from the selected requirement.",
                        )
                        .clicked()
                        && let (Some(twin_id), Some(source_revision)) =
                            (view_model.twin_id, view_model.source_revision)
                    {
                        actions.push(PanelAction::RunVerificationSuite {
                            twin_id,
                            source_revision,
                            names: selected_names,
                        });
                    }

                    let label = format!("Run all mapped tests ({})", names.len());
                    if ui
                        .add_enabled(can_run && !names.is_empty(), egui::Button::new(label))
                        .on_hover_text(if unsaved_source {
                            "Save or discard SysML edits before running the saved Twin source."
                        } else if cfg!(target_arch = "wasm32") {
                            "Run tests from the desktop application."
                        } else if names.is_empty() {
                            "No Twin-mapped verification cases are linked from a requirement."
                        } else if runs.is_some_and(SysmlVerificationRuns::has_active_run) {
                            "Wait for the active Twin test or suite to finish."
                        } else if !view_model.verification_setup_errors.is_empty() {
                            "Fix the Twin test setup issues above first."
                        } else {
                            "Runs each distinct mapped case once."
                        })
                        .clicked()
                        && let (Some(twin_id), Some(source_revision)) =
                            (view_model.twin_id, view_model.source_revision)
                    {
                        actions.push(PanelAction::RunVerificationSuite {
                            twin_id,
                            source_revision,
                            names,
                        });
                    }
                });
                muted(
                    ui,
                    theme,
                    "Each unique mapped case runs once; missing verify links and Twin mappings remain visible as gaps.",
                );
            }
        });
        if let Some(twin_id) = view_model.twin_id
            && let Some(suite) = runs.and_then(|runs| runs.last_suite(twin_id))
            && runs.and_then(|runs| runs.suite_progress(twin_id)).is_none()
        {
            let passed = suite
                .cases
                .iter()
                .filter(|case| case.outcome == VerificationRunOutcome::Passed)
                .count();
            let failed = suite
                .cases
                .iter()
                .filter(|case| case.outcome == VerificationRunOutcome::Failed)
                .count();
            let inconclusive = suite
                .cases
                .iter()
                .filter(|case| case.outcome == VerificationRunOutcome::Inconclusive)
                .count();
            let unverified = suite
                .cases
                .iter()
                .filter(|case| case.outcome == VerificationRunOutcome::Unverified)
                .count();
            let errors = suite
                .cases
                .iter()
                .filter(|case| case.outcome == VerificationRunOutcome::Error)
                .count();
            let run_errors = suite
                .cases
                .iter()
                .filter(|case| matches!(&case.outcome, VerificationRunOutcome::RunError(_)))
                .count();
            let no_verdict = suite
                .cases
                .iter()
                .filter(|case| case.outcome == VerificationRunOutcome::NoVerdict)
                .count();
            let cancelled = suite
                .cases
                .iter()
                .filter(|case| case.outcome == VerificationRunOutcome::Cancelled)
                .count();
            let stale = Some(suite.source_revision) != view_model.source_revision;
            let failed_names: Vec<_> = suite
                .cases
                .iter()
                .filter(|case| case.outcome == VerificationRunOutcome::Failed)
                .map(|case| case.name.clone())
                .filter(|name| verification_case_is_mapped(name, view_model))
                .collect();
            let can_run = cfg!(not(target_arch = "wasm32"))
                && !runs.is_some_and(SysmlVerificationRuns::has_active_run)
                && !unsaved_source
                && !view_model.analysis_has_errors
                && view_model.verification_setup_errors.is_empty();
            ui.horizontal_wrapped(|ui| {
                ui.strong(if stale {
                    "Previous suite report"
                } else {
                    "Last suite report"
                });
                if stale {
                    ui.colored_label(theme.tokens.warning, "STALE");
                }
                muted(
                    ui,
                    theme,
                    &format!(
                        "revision {} · {passed} passed · {failed} failed · {inconclusive} inconclusive · {unverified} unverified · {errors} errors · {run_errors} run errors · {no_verdict} no verdict · {cancelled} cancelled{}",
                        suite.source_revision,
                        if suite.stopped { " · stopped" } else { "" }
                    ),
                );
                if failed > 0 && ui.button("Show failures").clicked() {
                    self.filters = RequirementFilters::default();
                    self.filters.tests = TestFilter::SuiteFailures;
                    self.selected_requirement = view_model
                        .requirements
                        .iter()
                        .find(|requirement| {
                            suite.cases.iter().any(|case| {
                                case.outcome == VerificationRunOutcome::Failed
                                    && requirement
                                        .verification_cases
                                        .iter()
                                        .any(|name| name == &case.name)
                            })
                        })
                        .map(|requirement| requirement.qualified_name.clone());
                    self.selected_source = None;
                    self.search.clear();
                }
                if failed > 0
                    && ui
                        .add_enabled(
                            can_run && !failed_names.is_empty(),
                            egui::Button::new(format!("Rerun failed ({})", failed_names.len())),
                        )
                        .clicked()
                    && let (Some(twin_id), Some(source_revision)) =
                        (view_model.twin_id, view_model.source_revision)
                {
                    actions.push(PanelAction::RunVerificationSuite {
                        twin_id,
                        source_revision,
                        names: failed_names,
                    });
                }
            });
        }
        if let Some((twin_id, name)) = runs.and_then(SysmlVerificationRuns::active_case)
            && !runs.is_some_and(|runs| runs.suite_progress(twin_id).is_some())
            && Some(twin_id) == view_model.twin_id
        {
            ui.horizontal(|ui| {
                ui.spinner();
                let elapsed = runs
                    .and_then(|runs| runs.active_output(twin_id, name))
                    .map(|(_, elapsed)| format!(" · {:.1} s", elapsed.as_secs_f32()))
                    .unwrap_or_default();
                muted(ui, theme, &format!("Running {name}{elapsed}…"));
            });
        }
        egui::CollapsingHeader::new("Status guide")
            .default_open(false)
            .show(ui, |ui| {
                ui.label("Evidence is the Twin's structured requirement-check result. STALE belongs to an older source revision; NO EVIDENCE means no result is available.");
                ui.label("Test status describes linked scene-test execution. NO VERIFY means no resolved link; NO RUNNER means a link has no Twin scene-test mapping.");
                ui.label("NO VERDICT and RUN ERROR describe runner outcomes; PARTIAL means linked cases still need a passing result. NOT RUN, RUNNING, QUEUED, and CANCELLED describe execution state.");
                ui.label("Model coverage counts formal `require` criteria and resolved `verify` links separately from evidence and test runs. No formal `require` is not a failed check; qualitative definitions may rely on linked verification.");
                ui.label("VERIFIED requires complete coverage, current passing evidence, and passing mapped tests. FAILED means a current check or test failed; STALE and INCOMPLETE identify outdated or missing proof. This is not full KerML constraint execution.");
            });
        if view_model.stale_verification {
            let evidence_revision = view_model
                .verification_revision
                .map_or_else(|| "unknown".to_owned(), |revision| revision.to_string());
            let analyzed_revision = view_model
                .source_revision
                .map_or_else(|| "unknown".to_owned(), |revision| revision.to_string());
            ui.colored_label(
                theme.tokens.warning,
                format!(
                    "Evidence revision {evidence_revision} differs from analyzed revision {analyzed_revision}; rerun verification."
                ),
            );
        } else if let (Some(channel), Some(tick)) = (
            view_model.verification_channel.as_deref(),
            view_model.verification_sim_tick,
        ) {
            muted(
                ui,
                theme,
                &format!("Latest evidence event: {channel} · simulation tick {tick}"),
            );
        }
        if !view_model.verification_setup_errors.is_empty() {
            ui.collapsing(
                format!(
                    "Twin test setup issues ({})",
                    view_model.verification_setup_errors.len()
                ),
                |ui| {
                    for error in &view_model.verification_setup_errors {
                        ui.colored_label(theme.tokens.error, error);
                    }
                },
            );
        }
    }

    fn render_status_breakdown(
        &self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        runs: Option<&SysmlVerificationRuns>,
        evidence_counts: &[(&str, usize, egui::Color32)],
        execution_counts: &[(&str, usize, egui::Color32)],
        missing_criteria: usize,
        no_verify: usize,
        unmapped_verify: usize,
        theme: &lunco_theme::Theme,
    ) {
        ui.horizontal_wrapped(|ui| {
            status_counts(ui, "Requirement evidence", evidence_counts);
            ui.separator();
            status_counts(ui, "Test status by requirement", execution_counts);
            ui.separator();
            ui.strong("Model coverage");
            if missing_criteria > 0 {
                ui.colored_label(
                    theme.tokens.text_subdued,
                    format!("{missing_criteria} without formal require"),
                );
            }
            if no_verify > 0 {
                ui.colored_label(theme.tokens.warning, format!("{no_verify} no verify link"));
            }
            if unmapped_verify > 0 {
                ui.colored_label(
                    theme.tokens.warning,
                    format!("{unmapped_verify} verify link unmapped"),
                );
            }
        });

        let rollup_counts = [
            (
                RequirementRollupState::Verified,
                "verified",
                theme.tokens.success,
            ),
            (RequirementRollupState::Failed, "failed", theme.tokens.error),
            (RequirementRollupState::Error, "error", theme.tokens.error),
            (
                RequirementRollupState::Inconclusive,
                "inconclusive",
                theme.tokens.warning,
            ),
            (
                RequirementRollupState::RunError,
                "run error",
                theme.tokens.error,
            ),
            (
                RequirementRollupState::InvalidModel,
                "invalid model",
                theme.tokens.error,
            ),
            (RequirementRollupState::Stale, "stale", theme.tokens.warning),
            (
                RequirementRollupState::Running,
                "running",
                theme.tokens.text_subdued,
            ),
            (
                RequirementRollupState::Incomplete,
                "incomplete",
                theme.tokens.warning,
            ),
        ]
        .map(|(state, label, color)| {
            let count = view_model
                .requirements
                .iter()
                .filter(|requirement| {
                    requirement_rollup_state(requirement, view_model, runs) == state
                })
                .count();
            (label, count, color)
        });
        ui.horizontal_wrapped(|ui| {
            status_counts(ui, "Overall requirement status", &rollup_counts);
            muted(
                ui,
                theme,
                "VERIFIED requires complete coverage, current passing evidence, and passing mapped tests.",
            );
        });
    }

    fn apply_quick_filter(&mut self, quick_filter: RequirementQuickFilter) {
        self.search.clear();
        self.selected_source = None;
        self.filters = match quick_filter {
            RequirementQuickFilter::NeedsCriterion => RequirementFilters {
                coverage: CoverageFilter::MissingRequire,
                ..Default::default()
            },
            RequirementQuickFilter::NeedsVerify => RequirementFilters {
                coverage: CoverageFilter::MissingVerify,
                ..Default::default()
            },
            RequirementQuickFilter::NeedsTwinMapping => RequirementFilters {
                coverage: CoverageFilter::UnmappedVerify,
                ..Default::default()
            },
            RequirementQuickFilter::NotRun => RequirementFilters {
                tests: TestFilter::NotRun,
                ..Default::default()
            },
        };
    }

    fn render_filters(
        &mut self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        runs: Option<&SysmlVerificationRuns>,
        theme: &lunco_theme::Theme,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search name, ID, docs or test")
                    .desired_width((ui.available_width() * 0.42).max(180.0)),
            );
            let selected_name = self
                .selected_source
                .as_deref()
                .and_then(|uri| {
                    view_model
                        .source_files
                        .iter()
                        .find(|file| file.logical_uri == uri)
                })
                .map(|file| file.relative_path.display().to_string())
                .unwrap_or_else(|| "All source files".to_owned());
            egui::ComboBox::from_id_salt("sysml_requirement_source_filter")
                .selected_text(selected_name)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.selected_source.is_none(), "All source files")
                        .clicked()
                    {
                        self.selected_source = None;
                    }
                    for file in &view_model.source_files {
                        if ui
                            .selectable_label(
                                self.selected_source.as_deref() == Some(file.logical_uri.as_str()),
                                file.relative_path.display().to_string(),
                            )
                            .clicked()
                        {
                            self.selected_source = Some(file.logical_uri.clone());
                        }
                    }
                });
            if ui
                .add_enabled(
                    !self.search.is_empty()
                        || self.selected_source.is_some()
                        || self.filters != RequirementFilters::default(),
                    egui::Button::new("Clear filters"),
                )
                .clicked()
            {
                self.search.clear();
                self.selected_source = None;
                self.filters = RequirementFilters::default();
            }
        });
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("sysml_requirement_evidence_filter")
                .selected_text(match self.filters.evidence {
                    EvidenceFilter::All => "Any evidence",
                    EvidenceFilter::Failed => "Evidence: failed",
                    EvidenceFilter::Unverified => "Evidence: unverified",
                    EvidenceFilter::Stale => "Evidence: stale",
                    EvidenceFilter::Missing => "Evidence: missing",
                })
                .show_ui(ui, |ui| {
                    for (filter, label) in [
                        (EvidenceFilter::All, "Any evidence"),
                        (EvidenceFilter::Failed, "Failed evidence"),
                        (EvidenceFilter::Unverified, "Unverified evidence"),
                        (EvidenceFilter::Stale, "Stale evidence"),
                        (EvidenceFilter::Missing, "No evidence"),
                    ] {
                        if ui
                            .selectable_label(self.filters.evidence == filter, label)
                            .clicked()
                        {
                            self.filters.evidence = filter;
                        }
                    }
                });
            egui::ComboBox::from_id_salt("sysml_requirement_test_filter")
                .selected_text(match self.filters.tests {
                    TestFilter::All => "Any test result",
                    TestFilter::Failed => "Tests: failed",
                    TestFilter::SuiteFailures => "Tests: last suite failures",
                    TestFilter::NeedsResult => "Tests: needs a pass",
                    TestFilter::NotRun => "Tests: not run",
                    TestFilter::Stale => "Tests: stale",
                    TestFilter::Unmapped => "Tests: no runnable test",
                })
                .show_ui(ui, |ui| {
                    for (filter, label) in [
                        (TestFilter::All, "Any test result"),
                        (TestFilter::Failed, "Failed tests"),
                        (TestFilter::SuiteFailures, "Failures from last suite"),
                        (TestFilter::NeedsResult, "Needs a passing result"),
                        (TestFilter::NotRun, "Not run"),
                        (TestFilter::Stale, "Stale test results"),
                        (TestFilter::Unmapped, "No runnable test"),
                    ] {
                        if ui
                            .selectable_label(self.filters.tests == filter, label)
                            .clicked()
                        {
                            self.filters.tests = filter;
                        }
                    }
                });
            egui::ComboBox::from_id_salt("sysml_requirement_coverage_filter")
                .selected_text(match self.filters.coverage {
                    CoverageFilter::All => "Any model coverage",
                    CoverageFilter::MissingRequire => "Coverage: no formal require",
                    CoverageFilter::MissingVerify => "Coverage: verify missing",
                    CoverageFilter::UnmappedVerify => "Coverage: verify unmapped",
                })
                .show_ui(ui, |ui| {
                    for (filter, label) in [
                        (CoverageFilter::All, "Any model coverage"),
                        (
                            CoverageFilter::MissingRequire,
                            "No formal `require` criterion",
                        ),
                        (CoverageFilter::MissingVerify, "No `verify` link"),
                        (CoverageFilter::UnmappedVerify, "Unmapped `verify` link"),
                    ] {
                        if ui
                            .selectable_label(self.filters.coverage == filter, label)
                            .clicked()
                        {
                            self.filters.coverage = filter;
                        }
                    }
                });
        });
        let search = self.search.trim().to_lowercase();
        let visible_count = view_model
            .requirements
            .iter()
            .filter(|requirement| {
                self.selected_source
                    .as_deref()
                    .is_none_or(|source| requirement.logical_uri == source)
                    && requirement_matches_search(requirement, &search)
                    && requirement_matches_filters(requirement, view_model, runs, self.filters)
            })
            .count();
        muted(
            ui,
            theme,
            &format!(
                "{visible_count} of {} requirement elements · {} source files",
                view_model.requirements.len(),
                view_model.source_files.len(),
            ),
        );
    }

    fn render_requirement_list(
        &mut self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        filtered: &[usize],
        runs: Option<&SysmlVerificationRuns>,
        theme: &lunco_theme::Theme,
    ) {
        let mut ordered = filtered.to_vec();
        ordered.sort_by(|left, right| {
            let left = &view_model.requirements[*left];
            let right = &view_model.requirements[*right];
            let ordering = compare_requirements(left, right, view_model, runs, self.sort_column);
            let ordering = if self.sort_descending {
                ordering.reverse()
            } else {
                ordering
            };
            ordering.then_with(|| left.qualified_name.cmp(&right.qualified_name))
        });

        let mut next_sort = None;
        let mut selected_requirement = self.selected_requirement.clone();
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 8.0;
        egui::ScrollArea::horizontal()
            .id_salt("sysml_requirements_table_horizontal")
            .show(ui, |ui| {
                let table_width = (ui.available_width()
                    - ui.spacing().scroll.allocated_width()
                    - ui.spacing().item_spacing.x * 6.0)
                    .max(0.0);
                let minimum_width_scale = (table_width / 500.0).min(1.0) * 0.95;
                let table = TableBuilder::new(ui)
            .id_salt("sysml_requirements_table")
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .column(
                Column::initial(table_width * 0.20)
                    .at_least(96.0 * minimum_width_scale)
                    .clip(true),
            )
            .column(
                Column::initial(table_width * 0.09)
                    .at_least(42.0 * minimum_width_scale)
                    .clip(true),
            )
            .column(
                Column::initial(table_width * 0.11)
                    .at_least(56.0 * minimum_width_scale)
                    .clip(true),
            )
            .column(
                Column::initial(table_width * 0.14)
                    .at_least(72.0 * minimum_width_scale)
                    .clip(true),
            )
            .column(
                Column::initial(table_width * 0.14)
                    .at_least(72.0 * minimum_width_scale)
                    .clip(true),
            )
            .column(
                Column::initial(table_width * 0.16)
                    .at_least(80.0 * minimum_width_scale)
                    .clip(true),
            )
            .column(
                Column::initial(table_width * 0.16)
                    .at_least(82.0 * minimum_width_scale)
                    .clip(true),
            )
            .header(26.0, |mut header| {
                for (column, label) in [
                    (RequirementSortColumn::Id, "ID"),
                    (RequirementSortColumn::Kind, "Kind"),
                    (RequirementSortColumn::Source, "Source"),
                    (RequirementSortColumn::Evidence, "Evidence"),
                    (RequirementSortColumn::TwinTest, "Twin test"),
                    (RequirementSortColumn::Coverage, "Coverage"),
                    (RequirementSortColumn::Status, "Status"),
                ] {
                    header.col(|ui| {
                        let active = self.sort_column == column;
                        let marker = if active {
                            if self.sort_descending { " ▼" } else { " ▲" }
                        } else {
                            ""
                        };
                        if ui
                            .small_button(format!("{label}{marker}"))
                            .on_hover_text(
                                "Click to sort. Drag a column divider to resize; double-click it to fit.",
                            )
                            .clicked()
                        {
                            next_sort = Some(column);
                        }
                    });
                }
            });
                table.body(|body| {
            body.rows(row_height, ordered.len(), |mut row| {
                let requirement = &view_model.requirements[ordered[row.index()]];
                let selected = selected_requirement.as_deref()
                    == Some(requirement.qualified_name.as_str());
                row.set_selected(selected);
                row.col(|ui| {
                    ui.add(egui::Label::new(&requirement.display_name).truncate())
                        .on_hover_text(&requirement.qualified_name);
                });
                row.col(|ui| {
                    let (label, color) = requirement_role_label(requirement.role, theme);
                    let compact_label = match requirement.role {
                        RequirementRole::Definition => "DEF",
                        RequirementRole::Usage => "USE",
                    };
                    ui.colored_label(color, compact_label).on_hover_text(label);
                });
                row.col(|ui| {
                    let source = requirement_source_location(requirement);
                    let source_label = requirement_source_cell_label(requirement);
                    ui.add(egui::Label::new(source_label).truncate())
                        .on_hover_text(source);
                });
                row.col(|ui| {
                    let (label, color, explanation) =
                        evidence_status(requirement, view_model, runs, theme);
                    ui.colored_label(color, label).on_hover_text(explanation);
                });
                row.col(|ui| {
                    let execution = execution_summary(requirement, view_model, runs);
                    ui.colored_label(
                        execution_color(execution.state, theme),
                        execution_label(&execution),
                    )
                    .on_hover_text(execution_explanation(execution.state));
                });
                row.col(|ui| {
                    let (full_label, color) = model_coverage_label(requirement, view_model, theme);
                    let label = model_coverage_table_label(requirement, view_model);
                    ui.add(egui::Label::new(egui::RichText::new(label).color(color)).truncate())
                        .on_hover_text(format!(
                            "{full_label}. Formal `require` criteria and resolved `verify` links describe model coverage, not test PASS/FAIL."
                        ));
                });
                row.col(|ui| {
                    let state = requirement_rollup_state(requirement, view_model, runs);
                    let (label, color, explanation) = requirement_rollup_label(state, theme);
                    ui.colored_label(color, label).on_hover_text(explanation);
                });
                if row.response().clicked() {
                    selected_requirement = Some(requirement.qualified_name.clone());
                }
            });
        });
            });
        self.selected_requirement = selected_requirement;
        if let Some(column) = next_sort {
            if self.sort_column == column {
                self.sort_descending = !self.sort_descending;
            } else {
                self.sort_column = column;
                self.sort_descending = false;
            }
        }
    }

    fn render_requirement_detail(
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        requirement: Option<&RequirementView>,
        runs: Option<&SysmlVerificationRuns>,
        unsaved_source: bool,
        has_draft_for_another_source: bool,
        theme: &lunco_theme::Theme,
        actions: &mut Vec<PanelAction>,
        open_traceability: &mut bool,
    ) -> Option<(SourceFileView, usize)> {
        let Some(requirement) = requirement else {
            ui.label("Select a requirement to inspect its source and verification links.");
            return None;
        };
        let selected_source = egui::ScrollArea::vertical()
            .id_salt("sysml_requirement_details")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let mut selected_source = None;
                let mut open_requirement_source = false;
                let source = view_model
                    .source_files
                    .iter()
                    .find(|source| source.logical_uri == requirement.logical_uri)
                    .cloned();
                let source_line = requirement.line.unwrap_or(1);
                ui.heading(&requirement.display_name);
                let (role, role_color) = requirement_role_label(requirement.role, theme);
                ui.colored_label(role_color, role);
                muted(ui, theme, &requirement.qualified_name);
                if let Some(path) = &requirement.relative_path {
                    ui.horizontal(|ui| {
                        muted(ui, theme, &format!("{}:{source_line}", path.display()));
                        if let Some(source_file) = source.clone() {
                            let button = ui.add_enabled(
                                !has_draft_for_another_source,
                                egui::Button::new(format!("Open source at line {source_line}")),
                            );
                            if button.clicked() {
                                selected_source = Some((source_file, source_line));
                            }
                            if has_draft_for_another_source {
                                let _ = button
                                    .on_hover_text("Save or discard the open source draft first");
                            }
                        } else {
                            muted(ui, theme, "Source file is not in the current Twin index.");
                        }
                    });
                } else {
                    muted(ui, theme, &requirement.logical_uri);
                }
                for text in &requirement.documentation {
                    ui.add_space(theme.spacing.item_spacing);
                    ui.label(text);
                }
                egui::CollapsingHeader::new("Engineering trace")
                    .default_open(false)
                    .show(ui, |ui| {
                        let mapped = mapped_verification_count(requirement, view_model);
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(format!("Subjects · {}", requirement.subjects.len()));
                            if requirement.subjects.is_empty() {
                                muted(ui, theme, "none linked");
                            } else {
                                for subject in &requirement.subjects {
                                    let target = subject
                                        .target
                                        .as_ref()
                                        .map(|target| target.display_name.as_str())
                                        .unwrap_or(if subject.ambiguous_target {
                                            "ambiguous target"
                                        } else {
                                            "unresolved target"
                                        });
                                    let subject_type = subject
                                        .type_name
                                        .as_ref()
                                        .map(|type_name| format!("{}: {type_name}", subject.name))
                                        .unwrap_or_else(|| subject.name.clone());
                                    ui.label(format!("{subject_type} → {target}"));
                                }
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.strong(format!("Satisfied by · {}", requirement.satisfied_by.len()));
                            if requirement.satisfied_by.is_empty() {
                                muted(ui, theme, "no model elements linked");
                            } else {
                                for element in &requirement.satisfied_by {
                                    ui.label(&element.display_name)
                                        .on_hover_text(&element.qualified_name);
                                }
                            }
                        });
                        for verification in &requirement.verification_cases {
                            let mapped_case =
                                verification_case_is_mapped(verification, view_model);
                            ui.horizontal_wrapped(|ui| {
                                ui.label(verification);
                                ui.colored_label(
                                    if mapped_case {
                                        theme.tokens.success
                                    } else {
                                        theme.tokens.warning
                                    },
                                    if mapped_case { "Twin test mapped" } else { "No Twin test mapping" },
                                );
                            });
                        }
                        ui.horizontal_wrapped(|ui| {
                            if ui.small_button("Open full traceability").clicked() {
                                *open_traceability = true;
                            }
                            if mapped < requirement.verification_cases.len() {
                                if let Some(twin_root) = &view_model.twin_root {
                                    if ui
                                        .small_button("Open Twin test mapping")
                                        .on_hover_text("Open twin.toml to inspect registered scene tests.")
                                        .clicked()
                                    {
                                        actions.push(PanelAction::OpenTwinSource {
                                            twin_root: twin_root.to_string_lossy().into_owned(),
                                            relative_path: lunco_twin::MANIFEST_FILENAME.to_owned(),
                                        });
                                    }
                                } else {
                                    muted(ui, theme, "Twin source root is unavailable.");
                                }
                            }
                        });
                    });
                ui.separator();
                let rollup = requirement_rollup_state(requirement, view_model, runs);
                let (rollup_label, rollup_color, rollup_explanation) =
                    requirement_rollup_label(rollup, theme);
                ui.horizontal(|ui| {
                    ui.strong("Overall requirement status");
                    ui.colored_label(rollup_color, rollup_label)
                        .on_hover_text(rollup_explanation);
                });
                ui.separator();
                let (evidence_label, evidence_color, evidence_explanation) =
                    evidence_status(requirement, view_model, runs, theme);
                ui.horizontal(|ui| {
                    ui.strong("Requirement evidence");
                    ui.colored_label(evidence_color, evidence_label)
                        .on_hover_text(evidence_explanation);
                });
                render_requirement_evidence(
                    ui,
                    requirement,
                    view_model,
                    runs,
                    theme,
                    &mut open_requirement_source,
                );
                ui.separator();
                let execution = execution_summary(requirement, view_model, runs);
                ui.horizontal(|ui| {
                    ui.strong("Test execution");
                    ui.colored_label(
                        execution_color(execution.state, theme),
                        execution_label(&execution),
                    )
                    .on_hover_text(execution_explanation(execution.state));
                });
                muted(
                    ui,
                    theme,
                    &format!(
                        "{} of {} linked cases have a runnable Twin test",
                        execution.mapped, execution.linked
                    ),
                );
                ui.separator();
                ui.label("Model coverage");
                let (coverage_label, coverage_color) =
                    model_coverage_label(requirement, view_model, theme);
                ui.colored_label(coverage_color, coverage_label);
                muted(
                    ui,
                    theme,
                    "Formal `require` criteria and resolved `verify` links describe model completeness; they do not determine test PASS/FAIL.",
                );
                ui.separator();
                ui.label("Verification tests");
                if requirement.verification_cases.is_empty() {
                    ui.colored_label(
                        theme.tokens.warning,
                        "No resolved SysML `verify` link covers this requirement",
                    );
                } else {
                    for verification in &requirement.verification_cases {
                        if let Some(case) = view_model
                            .verification_cases
                            .iter()
                            .find(|case| case.name == *verification)
                        {
                            let case_result = runs.and_then(|runs| {
                                view_model
                                    .twin_id
                                    .and_then(|twin_id| runs.result(twin_id, verification))
                            });
                                let case_is_running = runs.is_some_and(|runs| {
                                    view_model.twin_id.is_some_and(|twin_id| {
                                        runs.is_running(twin_id, verification)
                                    })
                                });
                                let case_is_queued = runs.is_some_and(|runs| {
                                    view_model.twin_id.is_some_and(|twin_id| {
                                        runs.is_queued(twin_id, verification)
                                    })
                                });
                            let case_stale = case_result.is_some_and(|result| {
                                Some(result.source_revision) != view_model.source_revision
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label(verification)
                                    .on_hover_text(format!("Scene: {}", case.scene.display()));
                                let (label, color) = if case_is_running {
                                    ("RUNNING", theme.tokens.text_subdued)
                                } else if case_is_queued {
                                    ("QUEUED", theme.tokens.text_subdued)
                                } else if case_stale {
                                    ("STALE", theme.tokens.warning)
                                } else if let Some(result) = case_result {
                                    match &result.outcome {
                                        VerificationRunOutcome::Passed => {
                                            ("PASS", theme.tokens.success)
                                        }
                                        VerificationRunOutcome::Failed => {
                                            ("FAIL", theme.tokens.error)
                                        }
                                        VerificationRunOutcome::Inconclusive => {
                                            ("INCONCLUSIVE", theme.tokens.warning)
                                        }
                                        VerificationRunOutcome::Unverified => {
                                            ("UNVERIFIED", theme.tokens.warning)
                                        }
                                        VerificationRunOutcome::Error => {
                                            ("ERROR", theme.tokens.error)
                                        }
                                        VerificationRunOutcome::Cancelled => {
                                            ("CANCELLED", theme.tokens.text_subdued)
                                        }
                                        VerificationRunOutcome::NoVerdict => {
                                            ("NO VERDICT", theme.tokens.error)
                                        }
                                        VerificationRunOutcome::RunError(_) => {
                                            ("RUN ERROR", theme.tokens.error)
                                        }
                                    }
                                } else {
                                    ("NOT RUN", theme.tokens.text_subdued)
                                };
                                let status = ui.colored_label(color, label);
                                if let Some(result) = case_result.filter(|_| !case_is_queued) {
                                    status.on_hover_text(&result.summary);
                                }
                                if case_is_running {
                                    if let Some(twin_id) = view_model.twin_id
                                        && let Some((_, elapsed)) = runs.and_then(|runs| {
                                            runs.active_output(twin_id, verification)
                                        })
                                    {
                                        muted(ui, theme, &format!("Running for {:.1} s", elapsed.as_secs_f32()));
                                    }
                                    if ui.button("Cancel").clicked()
                                        && let Some(twin_id) = view_model.twin_id
                                    {
                                        actions.push(PanelAction::CancelVerification {
                                            twin_id,
                                            name: verification.clone(),
                                        });
                                    }
                                } else if case_is_queued {
                                    muted(ui, theme, "Waiting for the earlier linked tests to finish.");
                                } else {
                                    let can_run = cfg!(not(target_arch = "wasm32"))
                                        && !runs.is_some_and(SysmlVerificationRuns::has_active_run)
                                        && !unsaved_source
                                        && !view_model.analysis_has_errors
                                        && view_model.verification_setup_errors.is_empty();
                                    if ui
                                        .add_enabled(can_run, egui::Button::new("Run test"))
                                        .on_hover_text(if view_model.analysis_has_errors {
                                            "Fix SysML syntax, name-resolution, and package-collision diagnostics before running."
                                        } else if unsaved_source {
                                            "Save or discard SysML edits before running the saved Twin source."
                                        } else if cfg!(target_arch = "wasm32") {
                                            "Run tests from the desktop application."
                                        } else if runs.is_some_and(SysmlVerificationRuns::has_active_run) {
                                            "Wait for the active Twin test to finish."
                                        } else if !view_model.verification_setup_errors.is_empty() {
                                            "Fix the Twin test setup issues above first."
                                        } else {
                                            "Run this Twin-mapped scene and verification script headlessly."
                                        })
                                        .clicked()
                                    {
                                        if let (Some(twin_id), Some(source_revision)) =
                                            (view_model.twin_id, view_model.source_revision)
                                        {
                                            actions.push(PanelAction::RunVerification {
                                                twin_id,
                                                source_revision,
                                                name: verification.clone(),
                                            });
                                        }
                                    }
                                }
                            });
                            if let Some(result) = case_result.filter(|_| !case_is_queued) {
                                muted(
                                    ui,
                                    theme,
                                    &format!("{} · {:.1} s", result.summary, result.elapsed.as_secs_f32()),
                                );
                                if result.observed_source_revision.is_some()
                                    && !result.source_revision_matches
                                {
                                    muted(
                                        ui,
                                        theme,
                                        &format!(
                                            "Source revision mismatch · requested {} · child observed {}",
                                            result.source_revision,
                                            result.observed_source_revision.map_or_else(
                                                || "unavailable".to_owned(),
                                                |revision| revision.to_string(),
                                            ),
                                        ),
                                    );
                                }
                                if !result.diagnostics.is_empty() {
                                    ui.colored_label(
                                        if matches!(
                                            &result.outcome,
                                            VerificationRunOutcome::Failed
                                                | VerificationRunOutcome::NoVerdict
                                                | VerificationRunOutcome::Error
                                                | VerificationRunOutcome::RunError(_)
                                        ) {
                                            theme.tokens.error
                                        } else if matches!(
                                            &result.outcome,
                                            VerificationRunOutcome::Inconclusive
                                        ) {
                                            theme.tokens.warning
                                        } else {
                                            theme.tokens.text_subdued
                                        },
                                        if matches!(&result.outcome, VerificationRunOutcome::Failed) {
                                            "Why it failed"
                                        } else if matches!(&result.outcome, VerificationRunOutcome::Error) {
                                            "Verification error"
                                        } else if matches!(&result.outcome, VerificationRunOutcome::Inconclusive) {
                                            "Why it is inconclusive"
                                        } else {
                                            "Run details"
                                        },
                                    );
                                    for diagnostic in &result.diagnostics {
                                        ui.label(diagnostic);
                                    }
                                }
                                if !result.output.is_empty() {
                                    ui.collapsing("Run output", |ui| {
                                        egui::ScrollArea::vertical()
                                            .id_salt(("sysml_run_output", verification))
                                            .max_height(180.0)
                                            .show(ui, |ui| {
                                                ui.monospace(&result.output);
                                            });
                                    });
                                }
                            } else if case_is_running {
                                if let Some(twin_id) = view_model.twin_id
                                    && let Some((output, _)) = runs.and_then(|runs| {
                                        runs.active_output(twin_id, verification)
                                    })
                                    && !output.is_empty()
                                {
                                    ui.collapsing("Live output", |ui| {
                                        egui::ScrollArea::vertical()
                                            .id_salt(("sysml_live_run_output", verification))
                                            .max_height(180.0)
                                            .show(ui, |ui| {
                                                ui.monospace(output);
                                            });
                                    });
                                }
                            } else {
                                muted(ui, theme, &format!("Scene: {}", case.scene.display()));
                            }
                            if let Some(channel) = &case.verdict_channel {
                                muted(ui, theme, &format!("Verdict channel: {channel}"));
                            }
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(verification);
                                ui.colored_label(theme.tokens.warning, "NO TWIN TEST MAPPING");
                            });
                            muted(
                                ui,
                                theme,
                                "This verify link has no Twin scene-test registration, so bulk execution skips it.",
                            );
                        }
                    }
                    if unsaved_source {
                        muted(
                            ui,
                            theme,
                            "Save or discard open SysML edits before running; tests read the saved Twin files.",
                        );
                    }
                }
                ui.separator();
                if open_requirement_source && let Some(source) = source {
                    selected_source = Some((source, source_line));
                }
                selected_source
            })
            .inner;
        selected_source
    }

    fn editor_for_source(
        &self,
        twin_id: Option<lunco_workspace::TwinId>,
        source: &SourceFileView,
        focus_line: Option<usize>,
        documents: Option<&DocumentRegistry<SysmlDocument>>,
    ) -> SourceEditor {
        let mut editor = SourceEditor::pending(twin_id, source, focus_line);
        if let Some(document) = source.document_view(documents) {
            editor.attach_document(&document);
        }
        editor
    }

    fn render_source_editor(
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        editor: &mut SourceEditor,
        documents: Option<&DocumentRegistry<SysmlDocument>>,
        theme: &lunco_theme::Theme,
        actions: &mut Vec<PanelAction>,
    ) -> bool {
        if editor.doc_id.is_none() {
            if let Some(source) = view_model
                .source_files
                .iter()
                .find(|source| source.logical_uri == editor.logical_uri)
            {
                if let Some(document) = source.document_view(documents) {
                    editor.attach_document(&document);
                }
            }
        }

        let mut close = false;
        ui.horizontal(|ui| {
            ui.label("Source editor");
            if let Some(line) = editor.focus_line {
                muted(ui, theme, &format!("Line {line}"));
            }
            muted(ui, theme, &editor.logical_uri);
            if editor.twin_id != view_model.twin_id {
                ui.colored_label(theme.tokens.warning, "from another open Twin");
            }
            if ui.button("Close editor").clicked() && !editor.has_local_draft() {
                close = true;
            }
        });
        if close {
            return true;
        }

        let document = editor
            .doc_id
            .and_then(|doc_id| documents.and_then(|registry| registry.host(doc_id)))
            .map(|host| {
                let document = host.document();
                DocumentView {
                    id: document.id(),
                    generation: document.generation(),
                    source: document.source().to_owned(),
                    dirty: document.is_dirty(),
                    writable: document.origin().is_writable(),
                }
            });
        let Some(document) = document else {
            if let Some(source) = view_model
                .source_files
                .iter()
                .find(|source| source.logical_uri == editor.logical_uri)
            {
                if ui.button("Open source file").clicked() {
                    actions.push(PanelAction::OpenFile(
                        source.absolute_path.to_string_lossy().into_owned(),
                    ));
                }
            }
            muted(
                ui,
                theme,
                "Waiting for the canonical SysML document to open.",
            );
            return false;
        };
        editor.sync_document(&document);
        if editor.conflict {
            ui.colored_label(
                theme.tokens.warning,
                "The source changed elsewhere while this draft was open. Reload it before saving.",
            );
            if ui.button("Reload current source").clicked() {
                editor.attach_document(&document);
            }
        }
        if !document.writable {
            ui.colored_label(
                theme.tokens.warning,
                "Read-only source; editing is disabled.",
            );
        }
        if editor.conflict {
            muted(
                ui,
                theme,
                "Reload the current source before editing or saving this draft.",
            );
        }

        let file_diagnostics: Vec<&ParserDiagnosticView> = view_model
            .parser_diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.logical_uri == editor.logical_uri)
            .collect();
        if !file_diagnostics.is_empty() {
            ui.collapsing(format!("Diagnostics ({})", file_diagnostics.len()), |ui| {
                for diagnostic in &file_diagnostics {
                    if ui
                        .button(format!(
                            "{} · line {} · {}",
                            diagnostic_kind_label(diagnostic.kind),
                            diagnostic.line,
                            diagnostic.message
                        ))
                        .clicked()
                    {
                        editor.focus_line = Some(diagnostic.line);
                    }
                }
            });
        }

        let text_edit_id = egui::Id::new(("sysml_source_editor_text", &editor.logical_uri));
        if let Some(line) = editor.focus_line.take() {
            let offset = line_to_char_offset(&editor.buffer, line);
            let mut state = egui::TextEdit::load_state(ui.ctx(), text_edit_id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(offset),
                )));
            egui::TextEdit::store_state(ui.ctx(), text_edit_id, state);
            ui.ctx()
                .memory_mut(|memory| memory.request_focus(text_edit_id));
        }

        egui::ScrollArea::vertical()
            .id_salt("sysml_source_editor")
            .auto_shrink([false; 2])
            .max_height(ui.available_height().max(1.0))
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut editor.buffer)
                        .id(text_edit_id)
                        .code_editor()
                        .interactive(document.writable && !editor.conflict)
                        .desired_width(f32::INFINITY)
                        .desired_rows(18),
                );
            });

        let local_draft = editor.has_local_draft();
        if local_draft {
            ui.colored_label(theme.tokens.warning, "Unsaved source edits");
        } else if document.dirty {
            ui.colored_label(
                theme.tokens.warning,
                "Document changes are not saved to disk",
            );
        }
        ui.horizontal(|ui| {
            let can_save = document.writable && !editor.conflict && (local_draft || document.dirty);
            if ui
                .add_enabled(can_save, egui::Button::new("Save source"))
                .clicked()
            {
                if local_draft {
                    if let Some(parent_generation) = editor.base_generation {
                        actions.push(PanelAction::ApplySource {
                            doc_id: document.id,
                            parent_generation,
                            source: editor.buffer.clone(),
                        });
                    }
                }
                actions.push(PanelAction::Save(document.id));
            }
            if local_draft && ui.button("Discard draft").clicked() {
                editor.attach_document(&document);
            }
            if !document.dirty && !local_draft {
                muted(ui, theme, "Saved");
            }
        });
        false
    }

    fn render_source_diagnostics(
        &mut self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        documents: Option<&DocumentRegistry<SysmlDocument>>,
        theme: &lunco_theme::Theme,
    ) {
        if view_model.parser_diagnostics.is_empty() {
            return;
        }
        let mut jump_to = None;
        ui.collapsing(
            format!(
                "SysML diagnostics ({})",
                view_model.parser_diagnostics.len()
            ),
            |ui| {
                for diagnostic in &view_model.parser_diagnostics {
                    let location = view_model
                        .source_files
                        .iter()
                        .find(|source| source.logical_uri == diagnostic.logical_uri)
                        .map(|source| {
                            format!("{}:{}", source.relative_path.display(), diagnostic.line)
                        })
                        .unwrap_or_else(|| {
                            format!("{}:{}", diagnostic.logical_uri, diagnostic.line)
                        });
                    if ui
                        .selectable_label(
                            false,
                            egui::RichText::new(format!(
                                "{} · {location} · {}",
                                diagnostic_kind_label(diagnostic.kind),
                                diagnostic.message
                            ))
                            .color(theme.tokens.error),
                        )
                        .clicked()
                        && let Some(source) = view_model
                            .source_files
                            .iter()
                            .find(|source| source.logical_uri == diagnostic.logical_uri)
                    {
                        jump_to = Some((source.clone(), diagnostic.line));
                    }
                }
            },
        );
        let Some((source, line)) = jump_to else {
            return;
        };
        if self.editor.as_ref().is_some_and(|editor| {
            editor.has_local_draft()
                && (editor.logical_uri != source.logical_uri
                    || editor.twin_id != view_model.twin_id)
        }) {
            ui.colored_label(
                theme.tokens.warning,
                "Save or discard the open draft before jumping to another source file.",
            );
        } else if let Some(editor) = self.editor.as_mut().filter(|editor| {
            editor.logical_uri == source.logical_uri && editor.twin_id == view_model.twin_id
        }) {
            editor.focus_line = Some(line);
        } else {
            self.editor =
                Some(self.editor_for_source(view_model.twin_id, &source, Some(line), documents));
        }
    }
}

fn has_mapped_verification(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
) -> bool {
    mapped_verification_count(requirement, view_model) > 0
}

fn mapped_verification_count(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
) -> usize {
    requirement
        .verification_cases
        .iter()
        .filter(|name| verification_case_is_mapped(name, view_model))
        .count()
}

fn verification_case_is_mapped(name: &str, view_model: &SysmlRequirementsViewModel) -> bool {
    view_model
        .verification_cases
        .iter()
        .any(|case| case.name == name)
}

fn mapped_verification_names(view_model: &SysmlRequirementsViewModel) -> Vec<String> {
    view_model
        .requirements
        .iter()
        .flat_map(|requirement| requirement.verification_cases.iter())
        .filter(|name| verification_case_is_mapped(name, view_model))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn mapped_names_for_requirement(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
) -> Vec<String> {
    requirement
        .verification_cases
        .iter()
        .filter(|name| verification_case_is_mapped(name, view_model))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExecutionState {
    Pass,
    Fail,
    Inconclusive,
    Unverified,
    Error,
    RunError,
    Partial,
    Running,
    NotRun,
    Stale,
    NoVerify,
    NoRunner,
    NoVerdict,
    Cancelled,
}

struct ExecutionSummary {
    state: ExecutionState,
    linked: usize,
    mapped: usize,
    passed: usize,
    running: usize,
    failed: usize,
    inconclusive: usize,
    unverified: usize,
    error: usize,
    stale: usize,
    no_verdict: usize,
    run_error: usize,
    cancelled: usize,
}

fn execution_summary(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
) -> ExecutionSummary {
    let mut summary = ExecutionSummary {
        state: ExecutionState::NotRun,
        linked: requirement.verification_cases.len(),
        mapped: 0,
        passed: 0,
        running: 0,
        failed: 0,
        inconclusive: 0,
        unverified: 0,
        error: 0,
        stale: 0,
        no_verdict: 0,
        run_error: 0,
        cancelled: 0,
    };
    for name in &requirement.verification_cases {
        if !verification_case_is_mapped(name, view_model) {
            continue;
        }
        summary.mapped += 1;
        if runs.is_some_and(|runs| {
            view_model
                .twin_id
                .is_some_and(|twin_id| runs.is_running(twin_id, name))
        }) {
            summary.running += 1;
            continue;
        }
        if runs.is_some_and(|runs| {
            view_model
                .twin_id
                .is_some_and(|twin_id| runs.is_queued(twin_id, name))
        }) {
            summary.running += 1;
            continue;
        }
        let Some(result) = runs.and_then(|runs| {
            view_model
                .twin_id
                .and_then(|twin_id| runs.result(twin_id, name))
        }) else {
            continue;
        };
        if Some(result.source_revision) != view_model.source_revision {
            summary.stale += 1;
            continue;
        }
        match &result.outcome {
            VerificationRunOutcome::Passed => summary.passed += 1,
            VerificationRunOutcome::Failed => summary.failed += 1,
            VerificationRunOutcome::Inconclusive => summary.inconclusive += 1,
            VerificationRunOutcome::Unverified => summary.unverified += 1,
            VerificationRunOutcome::Error => summary.error += 1,
            VerificationRunOutcome::Cancelled => summary.cancelled += 1,
            VerificationRunOutcome::NoVerdict => summary.no_verdict += 1,
            VerificationRunOutcome::RunError(_) => summary.run_error += 1,
        }
    }
    summary.state = if summary.failed > 0 {
        ExecutionState::Fail
    } else if summary.error > 0 {
        ExecutionState::Error
    } else if summary.inconclusive > 0 {
        ExecutionState::Inconclusive
    } else if summary.unverified > 0 {
        ExecutionState::Unverified
    } else if summary.run_error > 0 {
        ExecutionState::RunError
    } else if summary.no_verdict > 0 {
        ExecutionState::NoVerdict
    } else if summary.running > 0 {
        ExecutionState::Running
    } else if summary.stale > 0 {
        ExecutionState::Stale
    } else if summary.linked == 0 {
        ExecutionState::NoVerify
    } else if summary.mapped < summary.linked || summary.mapped == 0 {
        ExecutionState::NoRunner
    } else if summary.passed == summary.mapped {
        ExecutionState::Pass
    } else if summary.cancelled > 0 && summary.passed == 0 {
        ExecutionState::Cancelled
    } else if summary.passed > 0 || summary.cancelled > 0 {
        ExecutionState::Partial
    } else {
        ExecutionState::NotRun
    };
    summary
}

fn execution_label(summary: &ExecutionSummary) -> String {
    match summary.state {
        ExecutionState::Pass => format!("PASS · {}/{}", summary.passed, summary.mapped),
        ExecutionState::Fail => format!("FAIL · {}/{} failed", summary.failed, summary.mapped),
        ExecutionState::Inconclusive => format!("INCONCLUSIVE · {}", summary.inconclusive),
        ExecutionState::Unverified => format!("UNVERIFIED · {}", summary.unverified),
        ExecutionState::Error => format!("ERROR · {}", summary.error),
        ExecutionState::RunError => format!("RUN ERROR · {}", summary.run_error),
        ExecutionState::Partial => format!("PARTIAL · {}/{}", summary.passed, summary.mapped),
        ExecutionState::Running => format!("RUNNING · {}/{}", summary.passed, summary.mapped),
        ExecutionState::NotRun => "NOT RUN".to_owned(),
        ExecutionState::Stale => "STALE".to_owned(),
        ExecutionState::NoVerify => "NO VERIFY".to_owned(),
        ExecutionState::NoRunner => "NO RUNNER".to_owned(),
        ExecutionState::NoVerdict => "NO VERDICT".to_owned(),
        ExecutionState::Cancelled => "CANCELLED".to_owned(),
    }
}

fn execution_color(state: ExecutionState, theme: &lunco_theme::Theme) -> egui::Color32 {
    match state {
        ExecutionState::Pass => theme.tokens.success,
        ExecutionState::Fail
        | ExecutionState::Error
        | ExecutionState::NoVerdict
        | ExecutionState::RunError => theme.tokens.error,
        ExecutionState::Inconclusive
        | ExecutionState::Unverified
        | ExecutionState::Partial
        | ExecutionState::Stale
        | ExecutionState::NoVerify
        | ExecutionState::NoRunner => theme.tokens.warning,
        ExecutionState::Running | ExecutionState::NotRun | ExecutionState::Cancelled => {
            theme.tokens.text_subdued
        }
    }
}

fn execution_explanation(state: ExecutionState) -> &'static str {
    match state {
        ExecutionState::Pass => "All runnable linked cases passed on the current source revision.",
        ExecutionState::Fail => "At least one linked scene test failed on the current revision.",
        ExecutionState::Inconclusive => {
            "At least one linked verification lacks enough evidence for a pass or fail."
        }
        ExecutionState::Unverified => {
            "At least one linked verification did not establish whether its requirements are met."
        }
        ExecutionState::Error => {
            "At least one linked verification encountered an evaluation error."
        }
        ExecutionState::RunError => {
            "The runner could not establish a trustworthy result for at least one linked test."
        }
        ExecutionState::Partial => {
            "Some linked cases passed; remaining cases need a current result."
        }
        ExecutionState::Running => "At least one linked scene test is running or queued.",
        ExecutionState::NotRun => "Runnable tests are linked, but none has a current result.",
        ExecutionState::Stale => "The available test result belongs to an older source revision.",
        ExecutionState::NoVerify => "The requirement has no resolved SysML verify link.",
        ExecutionState::NoRunner => "At least one verify link has no runnable Twin test mapping.",
        ExecutionState::NoVerdict => "A test ended without a standard verification verdict.",
        ExecutionState::Cancelled => "The test run was cancelled before it produced a verdict.",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Revision-aware aggregate state for structured requirement evidence.
enum EvidenceState {
    Pass,
    Fail,
    Inconclusive,
    Unverified,
    Error,
    Stale,
    NoEvidence,
}

/// Computes evidence state for this requirement against the analyzed revision.
fn evidence_state(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
) -> EvidenceState {
    let mut current_checks = 0_u64;
    let mut current_failures = 0_u64;
    let mut current_inconclusive = 0_u64;
    let mut current_errors = 0_u64;
    let mut current_unverified = 0_u64;
    let mut has_stale_evidence = false;
    for evidence in requirement_evidence_records(requirement, view_model, runs) {
        if Some(evidence.source_revision) == view_model.source_revision {
            current_checks = current_checks.saturating_add(evidence.checks);
            current_failures = current_failures.saturating_add(evidence.failures);
            current_inconclusive = current_inconclusive.saturating_add(evidence.inconclusive);
            current_errors = current_errors.saturating_add(evidence.errors);
            current_unverified = current_unverified.saturating_add(evidence.unverified);
        } else if evidence.checks > 0 {
            has_stale_evidence = true;
        }
    }
    if current_errors > 0 {
        EvidenceState::Error
    } else if current_failures > 0 {
        EvidenceState::Fail
    } else if current_inconclusive > 0 {
        EvidenceState::Inconclusive
    } else if current_unverified > 0 {
        EvidenceState::Unverified
    } else if current_checks > 0 {
        EvidenceState::Pass
    } else if has_stale_evidence {
        EvidenceState::Stale
    } else {
        EvidenceState::NoEvidence
    }
}

fn evidence_status(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
    theme: &lunco_theme::Theme,
) -> (&'static str, egui::Color32, &'static str) {
    match evidence_state(requirement, view_model, runs) {
        EvidenceState::Pass => (
            "PASS",
            theme.tokens.success,
            "Current-revision requirement checks have no failures.",
        ),
        EvidenceState::Fail => (
            "FAIL",
            theme.tokens.error,
            "Current-revision requirement checks include failures.",
        ),
        EvidenceState::Inconclusive => (
            "INCONCLUSIVE",
            theme.tokens.warning,
            "Current-revision requirement evidence did not support a pass or fail.",
        ),
        EvidenceState::Unverified => (
            "UNVERIFIED",
            theme.tokens.warning,
            "Current-revision requirement evidence does not establish verification.",
        ),
        EvidenceState::Error => (
            "ERROR",
            theme.tokens.error,
            "Current-revision requirement evidence encountered an evaluation error.",
        ),
        EvidenceState::Stale => (
            "STALE",
            theme.tokens.warning,
            "Only evidence from an older SysML source revision is available.",
        ),
        EvidenceState::NoEvidence => (
            "NO EVIDENCE",
            theme.tokens.text_subdued,
            "No structured requirement checks are available for this requirement.",
        ),
    }
}

/// Renders provenance and bounded check details for the selected requirement.
fn render_requirement_evidence(
    ui: &mut egui::Ui,
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
    theme: &lunco_theme::Theme,
    open_requirement_source: &mut bool,
) {
    for evidence in requirement_evidence_records(requirement, view_model, runs) {
        let stale = Some(evidence.source_revision) != view_model.source_revision;
        let (state, color) = if stale {
            ("STALE", theme.tokens.warning)
        } else if evidence.errors > 0 {
            ("ERROR", theme.tokens.error)
        } else if evidence.failures > 0 {
            ("FAIL", theme.tokens.error)
        } else if evidence.inconclusive > 0 {
            ("INCONCLUSIVE", theme.tokens.warning)
        } else if evidence.unverified > 0 {
            ("UNVERIFIED", theme.tokens.warning)
        } else {
            ("PASS", theme.tokens.success)
        };
        let label = format!(
            "{} · {} checks · {} fail · {} inconclusive · {} unverified · {} error · {}",
            evidence.channel,
            evidence.checks,
            evidence.failures,
            evidence.inconclusive,
            evidence.unverified,
            evidence.errors,
            state
        );
        ui.collapsing(egui::RichText::new(label).color(color), |ui| {
            let verification = evidence
                .verification
                .as_deref()
                .map_or_else(|| "verification not supplied".to_owned(), str::to_owned);
            muted(
                ui,
                theme,
                &format!(
                    "{verification} · source revision {} · simulation tick {}",
                    evidence.source_revision, evidence.sim_tick
                ),
            );
            if evidence.details.is_empty() {
                muted(
                    ui,
                    theme,
                    "This channel emitted aggregate counts without check details.",
                );
                return;
            }
            for check in &evidence.details {
                ui.horizontal_wrapped(|ui| {
                    let (label, color) = match check.verdict {
                        VerificationVerdict::Pass => ("PASS", theme.tokens.success),
                        VerificationVerdict::Fail => ("FAIL", theme.tokens.error),
                        VerificationVerdict::Inconclusive => ("INCONCLUSIVE", theme.tokens.warning),
                        VerificationVerdict::Unverified => ("UNVERIFIED", theme.tokens.warning),
                        VerificationVerdict::Error => ("ERROR", theme.tokens.error),
                    };
                    ui.colored_label(color, label);
                    if let Some(id) = &check.id {
                        ui.strong(id);
                    }
                    if let Some(kind) = &check.kind {
                        ui.label(kind);
                    }
                    if let Some(component) = &check.component {
                        muted(ui, theme, component);
                    }
                    if let Some(path) = &check.path {
                        muted(ui, theme, path);
                    }
                    if check.verdict != VerificationVerdict::Pass
                        && ui.small_button("Open source").clicked()
                    {
                        *open_requirement_source = true;
                    }
                });
                if let Some(error) = &check.error {
                    ui.colored_label(theme.tokens.error, error);
                }
                if let Some(actual) = &check.actual {
                    muted(ui, theme, &format!("Actual: {actual}"));
                }
                if let Some(expected) = &check.expected {
                    muted(ui, theme, &format!("Expected: {expected}"));
                }
            }
            if u64::try_from(evidence.details.len()).is_ok_and(|details| details < evidence.checks)
            {
                muted(
                    ui,
                    theme,
                    &format!(
                        "Showing {} check details of {}; failed checks are retained first.",
                        evidence.details.len(),
                        evidence.checks
                    ),
                );
            }
        });
    }
}

fn requirement_evidence_records(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
) -> Vec<RuntimeRequirementEvidence> {
    let mut records = BTreeMap::<(String, Option<String>, u64), RuntimeRequirementEvidence>::new();
    let mut add = |incoming: &RuntimeRequirementEvidence| {
        let key = (
            incoming.channel.clone(),
            incoming.verification.clone(),
            incoming.source_revision,
        );
        let Some(existing) = records.get_mut(&key) else {
            records.insert(key, incoming.clone());
            return;
        };
        existing.checks = existing.checks.max(incoming.checks);
        existing.failures = existing.failures.max(incoming.failures);
        existing.inconclusive = existing.inconclusive.max(incoming.inconclusive);
        existing.unverified = existing.unverified.max(incoming.unverified);
        existing.errors = existing.errors.max(incoming.errors);
        existing.sim_tick = existing.sim_tick.max(incoming.sim_tick);
        for detail in &incoming.details {
            if !existing.details.contains(detail)
                && (existing.details.len() < 64
                    || (detail.verdict != VerificationVerdict::Pass
                        && existing
                            .details
                            .iter()
                            .rposition(|item| item.verdict == VerificationVerdict::Pass)
                            .is_some()))
            {
                if existing.details.len() >= 64
                    && let Some(index) = existing
                        .details
                        .iter()
                        .rposition(|item| item.verdict == VerificationVerdict::Pass)
                {
                    existing.details.remove(index);
                }
                existing.details.push(detail.clone());
            }
        }
    };
    for evidence in &requirement.runtime_evidence {
        add(evidence);
    }
    if let (Some(runs), Some(twin_id)) = (runs, view_model.twin_id) {
        for name in &requirement.verification_cases {
            if let Some(result) = runs.result(twin_id, name) {
                for evidence in result
                    .evidence
                    .iter()
                    .filter(|evidence| evidence.requirement == requirement.qualified_name)
                {
                    add(evidence);
                }
            }
        }
    }
    records.into_values().collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequirementRollupState {
    Verified,
    Failed,
    Inconclusive,
    Error,
    RunError,
    InvalidModel,
    Stale,
    Running,
    Incomplete,
}

fn requirement_rollup_label(
    state: RequirementRollupState,
    theme: &lunco_theme::Theme,
) -> (&'static str, egui::Color32, &'static str) {
    match state {
        RequirementRollupState::Verified => (
            "VERIFIED",
            theme.tokens.success,
            "Model coverage is complete; current requirement evidence and all linked mapped tests passed.",
        ),
        RequirementRollupState::Failed => (
            "FAILED",
            theme.tokens.error,
            "A current structured requirement check or linked scene test failed.",
        ),
        RequirementRollupState::Inconclusive => (
            "INCONCLUSIVE",
            theme.tokens.warning,
            "Current evidence does not establish whether this requirement passed or failed.",
        ),
        RequirementRollupState::Error => (
            "ERROR",
            theme.tokens.error,
            "A current requirement check or verification case encountered an evaluation error.",
        ),
        RequirementRollupState::RunError => (
            "RUN ERROR",
            theme.tokens.error,
            "A linked run could not establish a trustworthy result.",
        ),
        RequirementRollupState::InvalidModel => (
            "INVALID MODEL",
            theme.tokens.error,
            "Parser or resolver diagnostics prevent verification of this source snapshot.",
        ),
        RequirementRollupState::Stale => (
            "STALE",
            theme.tokens.warning,
            "The available evidence or test result belongs to an older SysML source revision.",
        ),
        RequirementRollupState::Running => (
            "RUNNING",
            theme.tokens.text_subdued,
            "At least one linked scene test is running or queued.",
        ),
        RequirementRollupState::Incomplete => (
            "INCOMPLETE",
            theme.tokens.warning,
            "Coverage, current requirement evidence, or a current passing linked test is missing.",
        ),
    }
}

fn requirement_rollup_state(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
) -> RequirementRollupState {
    if view_model.analysis_has_errors {
        return RequirementRollupState::InvalidModel;
    }
    let evidence = evidence_state(requirement, view_model, runs);
    let execution = execution_summary(requirement, view_model, runs).state;
    if evidence == EvidenceState::Fail || execution == ExecutionState::Fail {
        RequirementRollupState::Failed
    } else if evidence == EvidenceState::Error || execution == ExecutionState::Error {
        RequirementRollupState::Error
    } else if evidence == EvidenceState::Inconclusive || execution == ExecutionState::Inconclusive {
        RequirementRollupState::Inconclusive
    } else if execution == ExecutionState::RunError {
        RequirementRollupState::RunError
    } else if execution == ExecutionState::Running {
        RequirementRollupState::Running
    } else if evidence == EvidenceState::Stale || execution == ExecutionState::Stale {
        RequirementRollupState::Stale
    } else {
        let mapped = mapped_verification_count(requirement, view_model);
        let complete_coverage = requirement.has_required_constraint
            && !requirement.verification_cases.is_empty()
            && mapped == requirement.verification_cases.len();
        if complete_coverage && evidence == EvidenceState::Pass && execution == ExecutionState::Pass
        {
            RequirementRollupState::Verified
        } else {
            RequirementRollupState::Incomplete
        }
    }
}

/// Summarizes formal criteria and mapped verify links without using run status.
fn model_coverage_label(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    theme: &lunco_theme::Theme,
) -> (String, egui::Color32) {
    let linked = requirement.verification_cases.len();
    let mapped = mapped_verification_count(requirement, view_model);
    let criterion = if requirement.has_required_constraint {
        "require ✓"
    } else {
        "no formal require"
    };
    let links = if linked == 0 {
        "verify missing".to_owned()
    } else {
        format!("verify {mapped}/{linked} mapped")
    };
    let complete = requirement.has_required_constraint && linked > 0 && mapped == linked;
    let informational_definition = requirement.role == RequirementRole::Definition
        && !requirement.has_required_constraint
        && linked > 0
        && mapped == linked;
    (
        format!("{criterion} · {links}"),
        if complete {
            theme.tokens.success
        } else if informational_definition {
            theme.tokens.text_subdued
        } else {
            theme.tokens.warning
        },
    )
}

fn model_coverage_table_label(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
) -> String {
    let linked = requirement.verification_cases.len();
    let mapped = mapped_verification_count(requirement, view_model);
    let criterion = if requirement.has_required_constraint {
        "req ✓"
    } else {
        "req —"
    };
    let links = if linked == 0 {
        "ver —".to_owned()
    } else {
        format!("ver {mapped}/{linked}")
    };
    format!("{criterion} · {links}")
}

/// Applies the three independent status dimensions as a combined list filter.
fn requirement_matches_search(requirement: &RequirementView, query: &str) -> bool {
    query.is_empty()
        || requirement.qualified_name.to_lowercase().contains(query)
        || requirement.display_name.to_lowercase().contains(query)
        || requirement
            .documentation
            .iter()
            .any(|text| text.to_lowercase().contains(query))
        || requirement
            .relative_path
            .as_ref()
            .is_some_and(|path| path.to_string_lossy().to_lowercase().contains(query))
        || requirement
            .verification_cases
            .iter()
            .any(|name| name.to_lowercase().contains(query))
}

fn requirement_matches_filters(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
    filters: RequirementFilters,
) -> bool {
    let evidence = evidence_state(requirement, view_model, runs);
    let test = execution_summary(requirement, view_model, runs).state;
    let mapped_count = mapped_verification_count(requirement, view_model);
    let coverage_matches = match filters.coverage {
        CoverageFilter::All => true,
        CoverageFilter::MissingRequire => !requirement.has_required_constraint,
        CoverageFilter::MissingVerify => requirement.verification_cases.is_empty(),
        CoverageFilter::UnmappedVerify => {
            !requirement.verification_cases.is_empty()
                && mapped_count < requirement.verification_cases.len()
        }
    };
    let evidence_matches = match filters.evidence {
        EvidenceFilter::All => true,
        EvidenceFilter::Failed => evidence == EvidenceState::Fail,
        EvidenceFilter::Unverified => evidence == EvidenceState::Unverified,
        EvidenceFilter::Stale => evidence == EvidenceState::Stale,
        EvidenceFilter::Missing => evidence == EvidenceState::NoEvidence,
    };
    let test_matches = match filters.tests {
        TestFilter::All => true,
        TestFilter::Failed => test == ExecutionState::Fail,
        TestFilter::SuiteFailures => {
            requirement_has_last_suite_failure(requirement, view_model, runs)
        }
        TestFilter::NeedsResult => matches!(
            test,
            ExecutionState::Partial
                | ExecutionState::NotRun
                | ExecutionState::Cancelled
                | ExecutionState::Inconclusive
                | ExecutionState::Unverified
                | ExecutionState::Error
                | ExecutionState::NoVerdict
                | ExecutionState::RunError
        ),
        TestFilter::NotRun => test == ExecutionState::NotRun,
        TestFilter::Stale => test == ExecutionState::Stale,
        TestFilter::Unmapped => {
            matches!(test, ExecutionState::NoRunner | ExecutionState::NoVerify)
        }
    };
    evidence_matches && test_matches && coverage_matches
}

fn requirement_has_last_suite_failure(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
) -> bool {
    view_model
        .twin_id
        .and_then(|twin_id| runs.and_then(|runs| runs.last_suite(twin_id)))
        .is_some_and(|suite| {
            suite.cases.iter().any(|case| {
                case.outcome == VerificationRunOutcome::Failed
                    && requirement
                        .verification_cases
                        .iter()
                        .any(|name| name == &case.name)
            })
        })
}

/// Shows only non-zero states so the overview stays compact.
fn status_counts(ui: &mut egui::Ui, category: &str, counts: &[(&str, usize, egui::Color32)]) {
    ui.strong(category);
    for (label, count, color) in counts {
        if *count > 0 {
            ui.colored_label(*color, format!("{count} {label}"));
        }
    }
}

fn requirement_source_location(requirement: &RequirementView) -> String {
    requirement
        .relative_path
        .as_ref()
        .map(|path| format!("{}:{}", path.display(), requirement.line.unwrap_or(1)))
        .unwrap_or_else(|| requirement.logical_uri.clone())
}

fn requirement_source_cell_label(requirement: &RequirementView) -> String {
    requirement
        .relative_path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| {
            format!(
                "{}:{}",
                name.to_string_lossy(),
                requirement.line.unwrap_or(1)
            )
        })
        .unwrap_or_else(|| requirement_source_location(requirement))
}

fn compare_requirements(
    left: &RequirementView,
    right: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
    column: RequirementSortColumn,
) -> Ordering {
    match column {
        RequirementSortColumn::Id => left.qualified_name.cmp(&right.qualified_name),
        RequirementSortColumn::Kind => {
            requirement_role_rank(left.role).cmp(&requirement_role_rank(right.role))
        }
        RequirementSortColumn::Source => {
            requirement_source_location(left).cmp(&requirement_source_location(right))
        }
        RequirementSortColumn::Evidence => evidence_sort_rank(left, view_model, runs)
            .cmp(&evidence_sort_rank(right, view_model, runs)),
        RequirementSortColumn::TwinTest => execution_sort_rank(left, view_model, runs)
            .cmp(&execution_sort_rank(right, view_model, runs)),
        RequirementSortColumn::Coverage => {
            coverage_sort_rank(left, view_model).cmp(&coverage_sort_rank(right, view_model))
        }
        RequirementSortColumn::Status => {
            rollup_sort_rank(left, view_model, runs).cmp(&rollup_sort_rank(right, view_model, runs))
        }
    }
}

fn requirement_role_rank(role: RequirementRole) -> u8 {
    match role {
        RequirementRole::Definition => 0,
        RequirementRole::Usage => 1,
    }
}

fn evidence_sort_rank(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
) -> u8 {
    match evidence_state(requirement, view_model, runs) {
        EvidenceState::Fail => 0,
        EvidenceState::Error => 1,
        EvidenceState::Inconclusive | EvidenceState::Unverified => 2,
        EvidenceState::Stale => 3,
        EvidenceState::NoEvidence => 4,
        EvidenceState::Pass => 5,
    }
}

fn execution_sort_rank(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
) -> u8 {
    match execution_summary(requirement, view_model, runs).state {
        ExecutionState::Fail => 0,
        ExecutionState::Error => 1,
        ExecutionState::RunError => 2,
        ExecutionState::NoVerdict => 3,
        ExecutionState::NoRunner => 4,
        ExecutionState::NoVerify => 5,
        ExecutionState::NotRun => 6,
        ExecutionState::Stale => 7,
        ExecutionState::Cancelled => 8,
        ExecutionState::Partial => 9,
        ExecutionState::Inconclusive | ExecutionState::Unverified => 10,
        ExecutionState::Running => 11,
        ExecutionState::Pass => 12,
    }
}

fn coverage_sort_rank(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
) -> u8 {
    if !requirement.has_required_constraint {
        return 0;
    }
    if requirement.verification_cases.is_empty() {
        return 1;
    }
    let mapped = mapped_verification_count(requirement, view_model);
    if mapped < requirement.verification_cases.len() {
        2
    } else {
        3
    }
}

fn rollup_sort_rank(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
) -> u8 {
    match requirement_rollup_state(requirement, view_model, runs) {
        RequirementRollupState::Failed => 0,
        RequirementRollupState::Error => 1,
        RequirementRollupState::RunError => 2,
        RequirementRollupState::InvalidModel => 3,
        RequirementRollupState::Inconclusive => 4,
        RequirementRollupState::Stale => 5,
        RequirementRollupState::Running => 6,
        RequirementRollupState::Incomplete => 7,
        RequirementRollupState::Verified => 8,
    }
}

fn quick_filter_button(
    ui: &mut egui::Ui,
    theme: &lunco_theme::Theme,
    count: usize,
    label: &str,
    color: egui::Color32,
) -> bool {
    ui.add_enabled(
        count > 0,
        egui::Button::new(egui::RichText::new(format!("{count} {label}")).color(color))
            .min_size(egui::vec2(155.0, 34.0))
            .fill(theme.tokens.node_card)
            .stroke(egui::Stroke::new(1.0, theme.tokens.node_border)),
    )
    .on_hover_text(format!("Show requirement elements with {label}."))
    .clicked()
}

fn trace_card<R>(
    ui: &mut egui::Ui,
    theme: &lunco_theme::Theme,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(theme.tokens.node_card)
        .stroke(egui::Stroke::new(1.0, theme.tokens.node_border))
        .corner_radius(egui::CornerRadius::same(theme.rounding.button.round() as u8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_width(190.0);
            ui.set_max_width(220.0);
            ui.strong(title);
            ui.add_space(theme.spacing.item_spacing);
            add_contents(ui)
        })
}

fn trace_arrow(ui: &mut egui::Ui, theme: &lunco_theme::Theme) {
    ui.add_sized(
        [22.0, 36.0],
        egui::Label::new(
            egui::RichText::new("→")
                .size(20.0)
                .color(theme.tokens.text_subdued),
        ),
    );
}

fn source_location(
    view_model: &SysmlRequirementsViewModel,
    element: &ModelElementView,
) -> Option<(SourceFileView, usize)> {
    view_model
        .source_files
        .iter()
        .find(|source| source.logical_uri == element.logical_uri)
        .cloned()
        .map(|source| (source, element.line))
}

fn verification_case_status(
    twin_id: Option<lunco_workspace::TwinId>,
    name: &str,
    source_revision: Option<u64>,
    runs: Option<&SysmlVerificationRuns>,
    theme: &lunco_theme::Theme,
) -> (&'static str, egui::Color32) {
    let Some(twin_id) = twin_id else {
        return ("NO ACTIVE TWIN", theme.tokens.text_subdued);
    };
    if runs.is_some_and(|runs| runs.is_running(twin_id, name)) {
        return ("RUNNING", theme.tokens.accent);
    }
    if runs.is_some_and(|runs| runs.is_queued(twin_id, name)) {
        return ("QUEUED", theme.tokens.text_subdued);
    }
    let Some(result) = runs.and_then(|runs| runs.result(twin_id, name)) else {
        return ("NOT RUN", theme.tokens.text_subdued);
    };
    if Some(result.source_revision) != source_revision {
        return ("STALE", theme.tokens.warning);
    }
    match &result.outcome {
        VerificationRunOutcome::Passed => ("PASS", theme.tokens.success),
        VerificationRunOutcome::Failed => ("FAIL", theme.tokens.error),
        VerificationRunOutcome::Inconclusive => ("INCONCLUSIVE", theme.tokens.warning),
        VerificationRunOutcome::Unverified => ("UNVERIFIED", theme.tokens.warning),
        VerificationRunOutcome::Error => ("ERROR", theme.tokens.error),
        VerificationRunOutcome::Cancelled => ("CANCELLED", theme.tokens.text_subdued),
        VerificationRunOutcome::NoVerdict => ("NO VERDICT", theme.tokens.error),
        VerificationRunOutcome::RunError(_) => ("RUN ERROR", theme.tokens.error),
    }
}

fn structure_kind_label(kind: &str) -> &'static str {
    match kind {
        "Package" => "Package",
        "PartDefinition" => "Part definition",
        "PartUsage" => "Part usage",
        "PortDefinition" => "Port definition",
        "PortUsage" => "Port usage",
        "ItemDefinition" => "Item definition",
        "ItemUsage" => "Item usage",
        "InterfaceDefinition" => "Interface definition",
        "InterfaceUsage" => "Interface usage",
        "ConnectionDefinition" => "Connection definition",
        "ConnectionUsage" => "Connection usage",
        _ => "SysML element",
    }
}

fn structure_kind_color(kind: &str, theme: &lunco_theme::Theme) -> egui::Color32 {
    match kind {
        "Package" => theme.schematic.class_package_badge,
        "PartDefinition" | "PartUsage" => theme.schematic.class_block_badge,
        "PortDefinition" | "PortUsage" | "ConnectionDefinition" | "ConnectionUsage" => {
            theme.schematic.class_connector_badge
        }
        "ItemDefinition" | "ItemUsage" => theme.schematic.class_record_badge,
        "InterfaceDefinition" | "InterfaceUsage" => theme.schematic.class_model_badge,
        _ => theme.tokens.text_subdued,
    }
}

fn count_structure_nodes(nodes: &[ModelStructureNodeView]) -> usize {
    nodes
        .iter()
        .map(|node| 1 + count_structure_nodes(&node.children))
        .sum()
}

fn matching_structure_nodes(
    roots: &[ModelStructureNodeView],
    query: &str,
) -> std::collections::HashSet<SysmlElementHandle> {
    fn include_matches(
        node: &ModelStructureNodeView,
        query: &str,
        included: &mut std::collections::HashSet<SysmlElementHandle>,
    ) -> bool {
        let matches_self = query.is_empty()
            || node.element.display_name.to_lowercase().contains(query)
            || node.element.qualified_name.to_lowercase().contains(query)
            || structure_kind_label(&node.element.kind)
                .to_lowercase()
                .contains(query);
        let matches_child = node.children.iter().fold(false, |found, child| {
            include_matches(child, query, included) || found
        });
        if matches_self || matches_child {
            included.insert(node.element.handle);
            true
        } else {
            false
        }
    }

    let mut included = std::collections::HashSet::new();
    for root in roots {
        include_matches(root, query, &mut included);
    }
    included
}

fn find_structure_element(
    nodes: &[ModelStructureNodeView],
    handle: SysmlElementHandle,
) -> Option<&ModelElementView> {
    for node in nodes {
        if node.element.handle == handle {
            return Some(&node.element);
        }
        if let Some(found) = find_structure_element(&node.children, handle) {
            return Some(found);
        }
    }
    None
}

fn render_structure_details(
    ui: &mut egui::Ui,
    view_model: &SysmlRequirementsViewModel,
    element: &ModelElementView,
    theme: &lunco_theme::Theme,
    open_source: &mut Option<(SourceFileView, usize)>,
) {
    ui.colored_label(
        structure_kind_color(&element.kind, theme),
        structure_kind_label(&element.kind),
    );
    ui.heading(&element.display_name);
    muted(ui, theme, &element.qualified_name);
    if let Some(path) = &element.relative_path {
        ui.label(format!("{}:{}", path.display(), element.line));
    } else {
        muted(
            ui,
            theme,
            &format!("{}:{}", element.logical_uri, element.line),
        );
    }
    if let Some(source) = source_location(view_model, element)
        && ui
            .button(format!("Open at line {}", element.line))
            .clicked()
    {
        *open_source = Some(source);
    }
}

fn muted(ui: &mut egui::Ui, theme: &lunco_theme::Theme, text: &str) {
    ui.label(egui::RichText::new(text).color(theme.tokens.text_subdued));
}

fn requirement_role_label(
    role: RequirementRole,
    theme: &lunco_theme::Theme,
) -> (&'static str, egui::Color32) {
    match role {
        RequirementRole::Definition => ("DEFINITION", theme.tokens.accent),
        RequirementRole::Usage => ("USAGE", theme.tokens.text_subdued),
    }
}

fn diagnostic_kind_label(kind: lunco_sysml_ast::SysmlDiagnosticKind) -> &'static str {
    match kind {
        lunco_sysml_ast::SysmlDiagnosticKind::Syntax => "Syntax error",
        lunco_sysml_ast::SysmlDiagnosticKind::Name => "Unresolved name",
        lunco_sysml_ast::SysmlDiagnosticKind::Collision => "Package collision",
    }
}

fn line_to_char_offset(source: &str, line: usize) -> usize {
    let bytes_before_line: usize = source
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum();
    source[..bytes_before_line.min(source.len())]
        .chars()
        .count()
}
