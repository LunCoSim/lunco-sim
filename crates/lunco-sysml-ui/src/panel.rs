use bevy_egui::egui;
use lunco_doc::{Document, FileBacked};
use lunco_doc_bevy::{DocumentRegistry, OpenFile};
use lunco_sysml::{ApplySysmlOps, SaveSysmlDocument, SysmlApiOp, SysmlDocument};
use lunco_workbench_core::{
    Panel, PanelCtx, PanelId, PanelMenuGroup, PanelScrollPolicy, PanelSlot,
};

use crate::verification::{RunSysmlVerification, SysmlVerificationRuns, VerificationRunOutcome};
use crate::view_model::{
    AnalysisState, RequirementView, SourceFileView, SysmlRequirementsViewModel,
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
}

impl SourceEditor {
    fn pending(twin_id: Option<lunco_workspace::TwinId>, source: &SourceFileView) -> Self {
        Self {
            twin_id,
            logical_uri: source.logical_uri.clone(),
            doc_id: source.document_id,
            buffer: String::new(),
            base_source: String::new(),
            base_generation: None,
            conflict: false,
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
}

/// Compact requirements browser for the active Twin, with source navigation
/// and an inline editor backed by the canonical SysML document registry.
#[derive(Default)]
pub struct SysmlRequirementsPanel {
    search: String,
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
        let actions = Vec::new();
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
            self.render_source_diagnostics(ui, view_model, theme);
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
        self.render_filters(ui, view_model, theme);
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
        let mut actions = actions;
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
            if let Some(editor) = &self.editor {
                if editor.has_local_draft()
                    && (editor.logical_uri != source.logical_uri
                        || editor.twin_id != view_model.twin_id)
                {
                    muted(
                        ui,
                        theme,
                        "Save or discard the current source draft before opening another file.",
                    );
                } else {
                    self.editor =
                        Some(self.editor_for_source(view_model.twin_id, &source, documents));
                }
            } else {
                self.editor = Some(self.editor_for_source(view_model.twin_id, &source, documents));
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
        self.render_source_diagnostics(ui, view_model, theme);
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
            muted(ui, theme, &format!("Source revision {revision}"));
        }
    }

    fn render_summary(
        &self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        runs: Option<&SysmlVerificationRuns>,
        theme: &lunco_theme::Theme,
    ) {
        let mut counts = [0usize; 9];
        for requirement in &view_model.requirements {
            counts[status_for(requirement, view_model, runs, theme)
                .kind
                .index()] += 1;
        }
        let missing_criteria = view_model
            .requirements
            .iter()
            .filter(|requirement| !requirement.has_required_constraint)
            .count();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} requirements", view_model.requirements.len()));
            ui.separator();
            ui.colored_label(
                theme.tokens.success,
                format!("{} pass", counts[StatusKind::Pass.index()]),
            );
            ui.separator();
            ui.colored_label(
                theme.tokens.error,
                format!("{} fail", counts[StatusKind::Fail.index()]),
            );
            ui.separator();
            ui.colored_label(
                theme.tokens.warning,
                format!("{} partial", counts[StatusKind::Partial.index()]),
            );
            ui.separator();
            muted(
                ui,
                theme,
                &format!("{} not run", counts[StatusKind::NotRun.index()]),
            );
            ui.separator();
            ui.colored_label(
                theme.tokens.warning,
                format!("{} no verify link", counts[StatusKind::NoVerify.index()]),
            );
        });
        ui.horizontal_wrapped(|ui| {
            if counts[StatusKind::NoRunner.index()] > 0 {
                ui.colored_label(
                    theme.tokens.warning,
                    format!(
                        "{} without Twin test mapping",
                        counts[StatusKind::NoRunner.index()]
                    ),
                );
            }
            if counts[StatusKind::Stale.index()] > 0 {
                ui.colored_label(
                    theme.tokens.warning,
                    format!("{} stale", counts[StatusKind::Stale.index()]),
                );
            }
            if counts[StatusKind::NoVerdict.index()] > 0 {
                ui.colored_label(
                    theme.tokens.error,
                    format!("{} no verdict", counts[StatusKind::NoVerdict.index()]),
                );
            }
            if counts[StatusKind::RunError.index()] > 0 {
                ui.colored_label(
                    theme.tokens.error,
                    format!("{} run errors", counts[StatusKind::RunError.index()]),
                );
            }
            muted(
                ui,
                theme,
                &format!("{missing_criteria} missing formal criterion"),
            );
        });
        muted(
            ui,
            theme,
            "PASS/FAIL comes from current source evidence; NOT RUN means a test is linked but has no current result.",
        );
        if let Some((twin_id, name)) = runs.and_then(SysmlVerificationRuns::active_case)
            && Some(twin_id) == view_model.twin_id
        {
            ui.horizontal(|ui| {
                ui.spinner();
                muted(ui, theme, &format!("Running {name} headlessly…"));
            });
        }
        egui::CollapsingHeader::new("Status guide")
            .default_open(true)
            .show(ui, |ui| {
                for (kind, explanation) in [
                    (StatusKind::Pass, "All registered tests linked to this requirement passed on the current source revision."),
                    (StatusKind::Fail, "Current requirement evidence or at least one linked test failed."),
                    (StatusKind::Partial, "At least one linked test passed, but other linked tests have not passed yet."),
                    (StatusKind::NotRun, "A Twin test is registered, but no current result is available."),
                    (StatusKind::Stale, "The available result was produced from an older source revision; rerun it."),
                    (StatusKind::NoVerify, "The SysML source does not link a verification case to this requirement."),
                    (StatusKind::NoRunner, "A SysML verification link exists, but the Twin has no runnable mapping for it."),
                    (StatusKind::NoVerdict, "The test ended without a PASS or FAIL verdict. Open its run summary."),
                ] {
                    let badge = status_badge(kind, theme);
                    ui.horizontal(|ui| {
                        ui.colored_label(badge.color, badge.label);
                        ui.label(explanation);
                    });
                }
                let badge = status_badge(StatusKind::RunError, theme);
                ui.horizontal(|ui| {
                    ui.colored_label(badge.color, badge.label);
                    ui.label(badge.explanation);
                });
                ui.label("Missing formal criterion is a source-quality gap; it does not mean a test failed.");
            });
        if view_model.stale_verification {
            let revision = view_model
                .verification_revision
                .map_or_else(|| "unknown".to_owned(), |revision| revision.to_string());
            ui.colored_label(
                theme.tokens.warning,
                format!(
                    "Runtime evidence is from source revision {revision}; rerun affected tests."
                ),
            );
        } else if let (Some(channel), Some(tick)) = (
            view_model.verification_channel.as_deref(),
            view_model.verification_sim_tick,
        ) {
            muted(
                ui,
                theme,
                &format!("Last evidence: {channel} · simulation tick {tick}"),
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
        theme: &lunco_theme::Theme,
    ) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Filter requirements")
                    .desired_width(ui.available_width() * 0.5),
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
        muted(
            ui,
            theme,
            &format!("{} source files", view_model.source_files.len()),
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
            ui.text_style_height(&egui::TextStyle::Body) * 2.0 + theme.spacing.item_spacing;
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
                                if let Some(path) = &requirement.relative_path {
                                    self.selected_source = view_model
                                        .source_files
                                        .iter()
                                        .find(|source| source.relative_path == *path)
                                        .map(|source| source.logical_uri.clone());
                                }
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let badge = status_for(requirement, view_model, runs, theme);
                                    ui.colored_label(badge.color, badge.label)
                                        .on_hover_text(badge.explanation);
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
                        muted(ui, theme, &source_location);
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
    ) -> Option<SourceFileView> {
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
                let badge = status_for(requirement, view_model, runs, theme);
                ui.colored_label(badge.color, badge.label)
                    .on_hover_text(badge.explanation);
                if let Some(result) = requirement.runtime_result {
                    let result_line = if view_model.stale_verification {
                        format!(
                            "{} checks · {} failures · from an older source revision",
                            result.checks, result.failures
                        )
                    } else {
                        format!("{} checks · {} failures", result.checks, result.failures)
                    };
                    muted(ui, theme, &result_line);
                }
                for text in &requirement.documentation {
                    ui.add_space(theme.spacing.item_spacing);
                    ui.label(text);
                }
                ui.separator();
                ui.label("Acceptance criterion");
                if requirement.has_required_constraint {
                    ui.colored_label(theme.tokens.success, "Formal `require` criterion present");
                } else {
                    ui.colored_label(
                        theme.tokens.warning,
                        "Missing formal `require` criterion (source-quality issue, not a test failure)",
                    );
                }
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
                                let can_run = !case_is_running
                                    && cfg!(not(target_arch = "wasm32"))
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
                            });
                            if let Some(result) = case_result {
                                muted(ui, theme, &result.summary);
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
                                egui::Button::new("Edit source"),
                            );
                            if button.clicked() {
                                selected_source = Some(source);
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
        documents: Option<&DocumentRegistry<SysmlDocument>>,
    ) -> SourceEditor {
        let mut editor = SourceEditor::pending(twin_id, source);
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
            muted(ui, theme, "This source is read-only.");
        }

        egui::ScrollArea::vertical()
            .id_salt("sysml_source_editor")
            .auto_shrink([false; 2])
            .max_height(ui.available_height().max(1.0))
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut editor.buffer)
                        .code_editor()
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
        &self,
        ui: &mut egui::Ui,
        view_model: &SysmlRequirementsViewModel,
        theme: &lunco_theme::Theme,
    ) {
        if view_model.parser_diagnostics.is_empty() {
            return;
        }
        ui.collapsing(
            format!(
                "Parser diagnostics ({})",
                view_model.parser_diagnostics.len()
            ),
            |ui| {
                for diagnostic in &view_model.parser_diagnostics {
                    ui.colored_label(theme.tokens.warning, diagnostic);
                }
            },
        );
    }
}

#[derive(Clone, Copy)]
enum StatusKind {
    Pass,
    Fail,
    Partial,
    NotRun,
    Stale,
    NoVerify,
    NoRunner,
    NoVerdict,
    RunError,
}

impl StatusKind {
    fn index(self) -> usize {
        match self {
            Self::Pass => 0,
            Self::Fail => 1,
            Self::Partial => 2,
            Self::NotRun => 3,
            Self::Stale => 4,
            Self::NoVerify => 5,
            Self::NoRunner => 6,
            Self::NoVerdict => 7,
            Self::RunError => 8,
        }
    }
}

struct StatusBadge {
    kind: StatusKind,
    label: &'static str,
    color: egui::Color32,
    explanation: &'static str,
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

fn status_for(
    requirement: &RequirementView,
    view_model: &SysmlRequirementsViewModel,
    runs: Option<&SysmlVerificationRuns>,
    theme: &lunco_theme::Theme,
) -> StatusBadge {
    let current_revision = view_model.source_revision;
    let mut current_result_count = 0;
    let mut current_pass_count = 0;
    let mut current_failed = false;
    let mut current_no_verdict = false;
    let mut current_run_error = false;
    let mut has_stale_result =
        view_model.stale_verification && requirement.runtime_result.is_some();
    if let (Some(runs), Some(twin_id)) = (runs, view_model.twin_id) {
        for name in &requirement.verification_cases {
            let Some(result) = runs.result(twin_id, name) else {
                continue;
            };
            if Some(result.source_revision) != current_revision {
                has_stale_result = true;
                continue;
            }
            current_result_count += 1;
            match &result.outcome {
                VerificationRunOutcome::Passed => current_pass_count += 1,
                VerificationRunOutcome::Failed => current_failed = true,
                VerificationRunOutcome::NoVerdict => current_no_verdict = true,
                VerificationRunOutcome::Error(_) => current_run_error = true,
            }
        }
    }
    let registered_count = requirement
        .verification_cases
        .iter()
        .filter(|name| {
            view_model
                .verification_cases
                .iter()
                .any(|case| &case.name == *name)
        })
        .count();
    let has_unmapped_case = registered_count < requirement.verification_cases.len();

    let kind = if current_failed {
        StatusKind::Fail
    } else if current_no_verdict {
        StatusKind::NoVerdict
    } else if current_run_error {
        StatusKind::RunError
    } else if current_result_count > 0 && !has_unmapped_case {
        if current_result_count == requirement.verification_cases.len()
            && current_pass_count == current_result_count
        {
            StatusKind::Pass
        } else {
            StatusKind::Partial
        }
    } else if let Some(result) = requirement
        .runtime_result
        .filter(|_| !view_model.stale_verification)
    {
        if result.failures > 0 {
            StatusKind::Fail
        } else if result.checks > 0 {
            StatusKind::Pass
        } else {
            StatusKind::NotRun
        }
    } else if has_stale_result {
        StatusKind::Stale
    } else if requirement.verification_cases.is_empty() {
        StatusKind::NoVerify
    } else if registered_count == 0 || has_unmapped_case {
        StatusKind::NoRunner
    } else {
        StatusKind::NotRun
    };
    status_badge(kind, theme)
}

fn status_badge(kind: StatusKind, theme: &lunco_theme::Theme) -> StatusBadge {
    let (label, color, explanation) = match kind {
        StatusKind::Pass => (
            "PASS",
            theme.tokens.success,
            "Current source evidence has checks and zero failures, or all linked Twin tests have passed.",
        ),
        StatusKind::Fail => (
            "FAIL",
            theme.tokens.error,
            "Current requirement evidence or at least one linked Twin test failed.",
        ),
        StatusKind::Partial => (
            "PARTIAL",
            theme.tokens.warning,
            "At least one linked Twin test passed, but other linked tests have not passed yet.",
        ),
        StatusKind::NotRun => (
            "NOT RUN",
            theme.tokens.text_subdued,
            "A Twin test is registered, but no result exists for the current source revision.",
        ),
        StatusKind::Stale => (
            "STALE",
            theme.tokens.warning,
            "The result belongs to an older source revision; rerun its verification test.",
        ),
        StatusKind::NoVerify => (
            "NO VERIFY",
            theme.tokens.warning,
            "The SysML source has no resolved verification link for this requirement.",
        ),
        StatusKind::NoRunner => (
            "NO RUNNER",
            theme.tokens.warning,
            "A SysML verification link exists, but this Twin has no runnable test mapping for it.",
        ),
        StatusKind::NoVerdict => (
            "NO VERDICT",
            theme.tokens.error,
            "The test ended without a PASS or FAIL verdict; inspect the run summary.",
        ),
        StatusKind::RunError => (
            "RUN ERROR",
            theme.tokens.error,
            "The test process could not be started or monitored; inspect the run error message.",
        ),
    };
    StatusBadge {
        kind,
        label,
        color,
        explanation,
    }
}

fn muted(ui: &mut egui::Ui, theme: &lunco_theme::Theme, text: &str) {
    ui.label(egui::RichText::new(text).color(theme.tokens.text_subdued));
}
