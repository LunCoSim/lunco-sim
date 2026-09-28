use bevy_egui::egui;
use lunco_doc::{Document, FileBacked};
use lunco_doc_bevy::{DocumentRegistry, OpenFile};
use lunco_sysml::{ApplySysmlOps, SaveSysmlDocument, SysmlApiOp, SysmlDocument};
use lunco_workbench_core::{
    Panel, PanelCtx, PanelId, PanelMenuGroup, PanelScrollPolicy, PanelSlot, PerspectiveId,
};

use crate::verification::{
    CancelSysmlVerification, RunSysmlVerification, SysmlVerificationRuns, VerificationRunOutcome,
};
use crate::view_model::{
    AnalysisState, ParserDiagnosticView, RequirementView, SourceFileView,
    SysmlRequirementsViewModel,
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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Filter for current or stale structured requirement-check evidence.
enum EvidenceFilter {
    #[default]
    All,
    Failed,
    Stale,
    Missing,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Filter for Twin-mapped scene-test execution state.
enum TestFilter {
    #[default]
    All,
    Failed,
    NeedsResult,
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

/// Compact requirements browser for the active Twin, with source navigation
/// and an inline editor backed by the canonical SysML document registry.
#[derive(Default)]
pub struct SysmlRequirementsPanel {
    search: String,
    filters: RequirementFilters,
    selected_source: Option<String>,
    selected_requirement: Option<String>,
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
        let actions = {
            let view_model = ctx.resource::<SysmlRequirementsViewModel>();
            let documents = ctx.resource::<DocumentRegistry<SysmlDocument>>();
            let runs = ctx.resource::<SysmlVerificationRuns>();
            self.render_panel(ui, view_model, documents, runs, &theme)
        };
        for action in actions {
            match action {
                PanelAction::OpenFile(path) => ctx.trigger(OpenFile { path }),
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
    ) -> Vec<PanelAction> {
        let Some(view_model) = view_model else {
            ui.label("SysML requirements view is not available.");
            return Vec::new();
        };
        let mut actions = Vec::new();
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

        if view_model.requirements.is_empty() {
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
            if let Some(source) = open_source {
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
                        editor.focus_line = Some(1);
                    }
                } else {
                    self.editor = Some(self.editor_for_source(
                        view_model.twin_id,
                        &source,
                        Some(1),
                        documents,
                    ));
                }
            }
            self.render_source_diagnostics(ui, view_model, documents, theme);
            let close_editor = if let Some(editor) = &mut self.editor {
                ui.separator();
                Self::render_source_editor(ui, view_model, editor, documents, theme, &mut actions)
            } else {
                false
            };
            if close_editor {
                self.editor = None;
            }
            return actions;
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

        self.render_summary(ui, view_model, runs, theme);
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
                    && (search.is_empty()
                        || requirement.qualified_name.to_lowercase().contains(&search)
                        || requirement.display_name.to_lowercase().contains(&search)
                        || requirement
                            .documentation
                            .iter()
                            .any(|text| text.to_lowercase().contains(&search))
                        || requirement.relative_path.as_ref().is_some_and(|path| {
                            path.to_string_lossy().to_lowercase().contains(&search)
                        })
                        || requirement
                            .verification_cases
                            .iter()
                            .any(|name| name.to_lowercase().contains(&search)))
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
        let selected = self.selected_requirement.as_deref().and_then(|name| {
            view_model
                .requirements
                .iter()
                .find(|requirement| requirement.qualified_name == name)
        });
        let unsaved_source = self.has_unsaved_source_edits(view_model, documents);
        let mut edit_source = None;
        if ui.available_width() >= 760.0 {
            ui.columns(2, |columns| {
                columns[0].heading("Requirements");
                self.render_requirement_list(&mut columns[0], view_model, &filtered, runs, theme);
                columns[1].heading("Requirement details");
                edit_source = self.render_requirement_detail(
                    &mut columns[1],
                    view_model,
                    selected,
                    runs,
                    unsaved_source,
                    theme,
                    &mut actions,
                );
            });
        } else {
            ui.heading("Requirements");
            self.render_requirement_list(ui, view_model, &filtered, runs, theme);
            ui.separator();
            ui.heading("Requirement details");
            edit_source = self.render_requirement_detail(
                ui,
                view_model,
                selected,
                runs,
                unsaved_source,
                theme,
                &mut actions,
            );
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
        &self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        runs: Option<&SysmlVerificationRuns>,
        theme: &lunco_theme::Theme,
    ) {
        let evidence_counts = [
            (EvidenceState::Pass, "pass", theme.tokens.success),
            (EvidenceState::Fail, "fail", theme.tokens.error),
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
                .filter(|requirement| evidence_state(requirement, view_model) == state)
                .count();
            (label, count, color)
        });
        let execution_counts = [
            (ExecutionState::Pass, "pass", theme.tokens.success),
            (ExecutionState::Fail, "fail", theme.tokens.error),
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
            (ExecutionState::RunError, "run error", theme.tokens.error),
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
                    && requirement.verification_cases.iter().any(|name| {
                        !view_model
                            .verification_cases
                            .iter()
                            .any(|case| case.name == *name)
                    })
            })
            .count();
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{} requirements", view_model.requirements.len()));
            ui.separator();
            status_counts(ui, "Requirement evidence", &evidence_counts);
            ui.separator();
            status_counts(ui, "Test status by requirement", &execution_counts);
            ui.separator();
            ui.strong("Model coverage");
            if missing_criteria > 0 {
                ui.colored_label(
                    theme.tokens.warning,
                    format!("{missing_criteria} require missing"),
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
        if let Some((twin_id, name)) = runs.and_then(SysmlVerificationRuns::active_case)
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
                ui.label("Evidence is the structured result emitted by the Twin's requirement checks. PASS/FAIL summarizes checks; STALE means the evidence source revision differs from the analyzed revision; NO EVIDENCE means no check result is available.");
                ui.label("Tests are the mapped scene-test runs. PASS means all linked runnable cases passed; FAIL means at least one failed; PARTIAL means results are mixed.");
                ui.label("NOT RUN has no current result; RUNNING is active; CANCELLED ended without a verdict; STALE belongs to an older source revision.");
                ui.label("Model coverage counts formal `require` criteria and resolved `verify` links separately from evidence and test runs.");
                ui.label("NO VERIFY means no resolved link; NO RUNNER means a link has no Twin scene-test mapping. NO VERDICT and RUN ERROR describe the run itself.");
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
        });
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("sysml_requirement_evidence_filter")
                .selected_text(match self.filters.evidence {
                    EvidenceFilter::All => "Any evidence",
                    EvidenceFilter::Failed => "Evidence: failed",
                    EvidenceFilter::Stale => "Evidence: stale",
                    EvidenceFilter::Missing => "Evidence: missing",
                })
                .show_ui(ui, |ui| {
                    for (filter, label) in [
                        (EvidenceFilter::All, "Any evidence"),
                        (EvidenceFilter::Failed, "Failed evidence"),
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
                    TestFilter::NeedsResult => "Tests: needs result",
                    TestFilter::Stale => "Tests: stale",
                    TestFilter::Unmapped => "Tests: no runnable test",
                })
                .show_ui(ui, |ui| {
                    for (filter, label) in [
                        (TestFilter::All, "Any test result"),
                        (TestFilter::Failed, "Failed tests"),
                        (TestFilter::NeedsResult, "Needs a current result"),
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
                    CoverageFilter::MissingRequire => "Coverage: require missing",
                    CoverageFilter::MissingVerify => "Coverage: verify missing",
                    CoverageFilter::UnmappedVerify => "Coverage: verify unmapped",
                })
                .show_ui(ui, |ui| {
                    for (filter, label) in [
                        (CoverageFilter::All, "Any model coverage"),
                        (CoverageFilter::MissingRequire, "Missing `require` criteria"),
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
        muted(
            ui,
            theme,
            &format!(
                "{} of {} requirements · {} source files",
                view_model
                    .requirements
                    .iter()
                    .filter(|requirement| requirement_matches_filters(
                        requirement,
                        view_model,
                        runs,
                        self.filters,
                    ))
                    .count(),
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
        let row_height =
            ui.text_style_height(&egui::TextStyle::Body) * 3.0 + theme.spacing.item_spacing;
        egui::ScrollArea::vertical()
            .id_salt("sysml_requirements_rows")
            .auto_shrink([false; 2])
            .show_rows(ui, row_height, filtered.len(), |ui, range| {
                for index in range {
                    let requirement = &view_model.requirements[filtered[index]];
                    let selected = self.selected_requirement.as_deref()
                        == Some(requirement.qualified_name.as_str());
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(selected, requirement.display_name.clone())
                                .on_hover_text(&requirement.qualified_name)
                                .clicked()
                            {
                                self.selected_requirement =
                                    Some(requirement.qualified_name.clone());
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let execution =
                                        execution_summary(requirement, view_model, runs);
                                    ui.colored_label(
                                        execution_color(execution.state, theme),
                                        execution_label(&execution),
                                    )
                                    .on_hover_text(execution_explanation(execution.state));
                                },
                            );
                        });
                        let source_location = requirement
                            .relative_path
                            .as_ref()
                            .map(|path| {
                                format!("{}:{}", path.display(), requirement.line.unwrap_or(1))
                            })
                            .unwrap_or_else(|| requirement.logical_uri.clone());
                        let execution = execution_summary(requirement, view_model, runs);
                        let (evidence_label, evidence_color, evidence_explanation) =
                            evidence_status(requirement, view_model, theme);
                        let (quality_label, quality_color) =
                            model_coverage_label(requirement, view_model, theme);
                        ui.horizontal_wrapped(|ui| {
                            muted(ui, theme, &source_location);
                            ui.colored_label(
                                execution_color(execution.state, theme),
                                format!("Tests {}/{}", execution.passed, execution.mapped),
                            );
                            ui.colored_label(evidence_color, evidence_label)
                                .on_hover_text(evidence_explanation);
                            ui.colored_label(quality_color, quality_label);
                        });
                    });
                }
            });
    }

    fn render_requirement_detail(
        &mut self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        requirement: Option<&RequirementView>,
        runs: Option<&SysmlVerificationRuns>,
        unsaved_source: bool,
        theme: &lunco_theme::Theme,
        actions: &mut Vec<PanelAction>,
    ) -> Option<(SourceFileView, usize)> {
        let Some(requirement) = requirement else {
            ui.label("Select a requirement to inspect its source and verification links.");
            return None;
        };
        egui::ScrollArea::vertical()
            .id_salt("sysml_requirement_details")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.heading(&requirement.display_name);
                muted(ui, theme, &requirement.qualified_name);
                for text in &requirement.documentation {
                    ui.add_space(theme.spacing.item_spacing);
                    ui.label(text);
                }
                ui.separator();
                let (evidence_label, evidence_color, evidence_explanation) =
                    evidence_status(requirement, view_model, theme);
                ui.horizontal(|ui| {
                    ui.strong("Requirement evidence");
                    ui.colored_label(evidence_color, evidence_label)
                        .on_hover_text(evidence_explanation);
                });
                render_requirement_evidence(ui, requirement, view_model, theme);
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
                            let case_stale = case_result.is_some_and(|result| {
                                Some(result.source_revision) != view_model.source_revision
                            });
                            ui.horizontal_wrapped(|ui| {
                                ui.label(verification)
                                    .on_hover_text(format!("Scene: {}", case.scene.display()));
                                let (label, color) = if case_is_running {
                                    ("RUNNING", theme.tokens.text_subdued)
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
                                        VerificationRunOutcome::Cancelled => {
                                            ("CANCELLED", theme.tokens.text_subdued)
                                        }
                                        VerificationRunOutcome::NoVerdict => {
                                            ("NO VERDICT", theme.tokens.error)
                                        }
                                        VerificationRunOutcome::Error(_) => {
                                            ("RUN ERROR", theme.tokens.error)
                                        }
                                    }
                                } else {
                                    ("NOT RUN", theme.tokens.text_subdued)
                                };
                                let status = ui.colored_label(color, label);
                                if let Some(result) = case_result {
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
                                } else {
                                    let can_run = cfg!(not(target_arch = "wasm32"))
                                        && !runs.is_some_and(SysmlVerificationRuns::has_active_run)
                                        && !unsaved_source
                                        && view_model.verification_setup_errors.is_empty();
                                    if ui
                                        .add_enabled(can_run, egui::Button::new("Run test"))
                                        .on_hover_text(if unsaved_source {
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
                            if let Some(result) = case_result {
                                muted(
                                    ui,
                                    theme,
                                    &format!("{} · {:.1} s", result.summary, result.elapsed.as_secs_f32()),
                                );
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
                let source = view_model
                    .source_files
                    .iter()
                    .find(|source| source.logical_uri == requirement.logical_uri)
                    .cloned();
                let mut selected_source = None;
                if let Some(path) = &requirement.relative_path {
                    let line = requirement.line.unwrap_or(1);
                    ui.horizontal(|ui| {
                        ui.label(format!("{}:{line}", path.display()));
                        if let Some(source) = source.clone() {
                            let current_draft_for_other_file =
                                self.editor.as_ref().is_some_and(|editor| {
                                    editor.has_local_draft()
                                        && (editor.logical_uri != source.logical_uri
                                            || editor.twin_id != view_model.twin_id)
                                });
                            let button = ui.add_enabled(
                                !current_draft_for_other_file,
                                egui::Button::new(format!("Open at line {line}")),
                            );
                            if button.clicked() {
                                selected_source = Some((source, line));
                            }
                            if current_draft_for_other_file {
                                let _ = button
                                    .on_hover_text("Save or discard the open source draft first");
                            }
                        }
                    });
                    if source.is_none() {
                        muted(
                            ui,
                            theme,
                            "The analyzed source file is not in the current Twin index.",
                        );
                    }
                } else {
                    muted(ui, theme, &requirement.logical_uri);
                    muted(
                        ui,
                        theme,
                        "This source location cannot be mapped to an editable Twin file.",
                    );
                }
                selected_source
            })
            .inner
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
                        .button(format!("Line {} · {}", diagnostic.line, diagnostic.message))
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
                "Parser diagnostics ({})",
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
                            egui::RichText::new(format!("{location} · {}", diagnostic.message))
                                .color(theme.tokens.warning),
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
    requirement.verification_cases.iter().any(|name| {
        view_model
            .verification_cases
            .iter()
            .any(|case| case.name == *name)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExecutionState {
    Pass,
    Fail,
    Partial,
    Running,
    NotRun,
    Stale,
    NoVerify,
    NoRunner,
    NoVerdict,
    RunError,
    Cancelled,
}

struct ExecutionSummary {
    state: ExecutionState,
    linked: usize,
    mapped: usize,
    passed: usize,
    running: usize,
    failed: usize,
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
        stale: 0,
        no_verdict: 0,
        run_error: 0,
        cancelled: 0,
    };
    for name in &requirement.verification_cases {
        if !view_model
            .verification_cases
            .iter()
            .any(|case| case.name == *name)
        {
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
            VerificationRunOutcome::Cancelled => summary.cancelled += 1,
            VerificationRunOutcome::NoVerdict => summary.no_verdict += 1,
            VerificationRunOutcome::Error(_) => summary.run_error += 1,
        }
    }
    summary.state = if summary.failed > 0 {
        ExecutionState::Fail
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
        ExecutionState::Fail => format!("FAIL · {}/{}", summary.passed, summary.mapped),
        ExecutionState::Partial => format!("PARTIAL · {}/{}", summary.passed, summary.mapped),
        ExecutionState::Running => format!("RUNNING · {}/{}", summary.passed, summary.mapped),
        ExecutionState::NotRun => "NOT RUN".to_owned(),
        ExecutionState::Stale => "STALE".to_owned(),
        ExecutionState::NoVerify => "NO VERIFY".to_owned(),
        ExecutionState::NoRunner => "NO RUNNER".to_owned(),
        ExecutionState::NoVerdict => "NO VERDICT".to_owned(),
        ExecutionState::RunError => "RUN ERROR".to_owned(),
        ExecutionState::Cancelled => "CANCELLED".to_owned(),
    }
}

fn execution_color(state: ExecutionState, theme: &lunco_theme::Theme) -> egui::Color32 {
    match state {
        ExecutionState::Pass => theme.tokens.success,
        ExecutionState::Fail | ExecutionState::NoVerdict | ExecutionState::RunError => {
            theme.tokens.error
        }
        ExecutionState::Partial
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
        ExecutionState::Partial => {
            "Some linked cases passed; remaining cases need a current result."
        }
        ExecutionState::Running => "A linked scene test is running.",
        ExecutionState::NotRun => "Runnable tests are linked, but none has a current result.",
        ExecutionState::Stale => "The available test result belongs to an older source revision.",
        ExecutionState::NoVerify => "The requirement has no resolved SysML verify link.",
        ExecutionState::NoRunner => "At least one verify link has no runnable Twin test mapping.",
        ExecutionState::NoVerdict => "A test ended without a PASS or FAIL verdict.",
        ExecutionState::RunError => "The test runner could not start or be monitored.",
        ExecutionState::Cancelled => "The test run was cancelled before it produced a verdict.",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Revision-aware aggregate state for structured requirement evidence.
enum EvidenceState {
    Pass,
    Fail,
    Stale,
    NoEvidence,
}

/// Computes evidence state for this requirement against the analyzed revision.
fn evidence_state(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
) -> EvidenceState {
    let mut current_checks = 0_u64;
    let mut current_failures = 0_u64;
    let mut has_stale_evidence = false;
    for evidence in &requirement.runtime_evidence {
        if Some(evidence.source_revision) == view_model.source_revision {
            current_checks = current_checks.saturating_add(evidence.checks);
            current_failures = current_failures.saturating_add(evidence.failures);
        } else if evidence.checks > 0 {
            has_stale_evidence = true;
        }
    }
    if current_failures > 0 {
        EvidenceState::Fail
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
    theme: &lunco_theme::Theme,
) -> (&'static str, egui::Color32, &'static str) {
    match evidence_state(requirement, view_model) {
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
    theme: &lunco_theme::Theme,
) {
    for evidence in &requirement.runtime_evidence {
        let stale = Some(evidence.source_revision) != view_model.source_revision;
        let state = if stale {
            "STALE"
        } else if evidence.failures > 0 {
            "FAIL"
        } else {
            "PASS"
        };
        let color = if stale {
            theme.tokens.warning
        } else if evidence.failures > 0 {
            theme.tokens.error
        } else {
            theme.tokens.success
        };
        let label = format!(
            "{} · {} checks · {} failures · {}",
            evidence.channel, evidence.checks, evidence.failures, state
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
                    let (label, color) = if check.passed {
                        ("PASS", theme.tokens.success)
                    } else {
                        ("FAIL", theme.tokens.error)
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

/// Summarizes formal criteria and mapped verify links without using run status.
fn model_coverage_label(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    theme: &lunco_theme::Theme,
) -> (String, egui::Color32) {
    let linked = requirement.verification_cases.len();
    let mapped = requirement
        .verification_cases
        .iter()
        .filter(|name| {
            view_model
                .verification_cases
                .iter()
                .any(|case| case.name.as_str() == name.as_str())
        })
        .count();
    let criterion = if requirement.has_required_constraint {
        "require ✓"
    } else {
        "require missing"
    };
    let links = if linked == 0 {
        "verify missing".to_owned()
    } else {
        format!("verify {mapped}/{linked} mapped")
    };
    let complete = requirement.has_required_constraint && linked > 0 && mapped == linked;
    (
        format!("{criterion} · {links}"),
        if complete {
            theme.tokens.success
        } else {
            theme.tokens.warning
        },
    )
}

/// Applies the three independent status dimensions as a combined list filter.
fn requirement_matches_filters(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
    filters: RequirementFilters,
) -> bool {
    let evidence = evidence_state(requirement, view_model);
    let test = execution_summary(requirement, view_model, runs).state;
    let mapped_count = requirement
        .verification_cases
        .iter()
        .filter(|name| {
            view_model
                .verification_cases
                .iter()
                .any(|case| case.name.as_str() == name.as_str())
        })
        .count();
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
        EvidenceFilter::Stale => evidence == EvidenceState::Stale,
        EvidenceFilter::Missing => evidence == EvidenceState::NoEvidence,
    };
    let test_matches = match filters.tests {
        TestFilter::All => true,
        TestFilter::Failed => test == ExecutionState::Fail,
        TestFilter::NeedsResult => matches!(
            test,
            ExecutionState::Partial
                | ExecutionState::NotRun
                | ExecutionState::Cancelled
                | ExecutionState::NoVerdict
                | ExecutionState::RunError
        ),
        TestFilter::Stale => test == ExecutionState::Stale,
        TestFilter::Unmapped => {
            matches!(test, ExecutionState::NoRunner | ExecutionState::NoVerify)
        }
    };
    evidence_matches && test_matches && coverage_matches
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

fn muted(ui: &mut egui::Ui, theme: &lunco_theme::Theme, text: &str) {
    ui.label(egui::RichText::new(text).color(theme.tokens.text_subdued));
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
