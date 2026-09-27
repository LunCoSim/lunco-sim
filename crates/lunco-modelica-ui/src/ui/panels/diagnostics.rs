//! Diagnostics panel — Modelica-specific parse and semantic errors.
//!
//! Bottom-dock tab next to Console, sharing the same visual shape
//! ([`lunco_ui::log::render_log_view`]) but scoped to
//! Modelica document diagnostics. Console accumulates every
//! workbench event; Diagnostics only shows the *current* set of
//! problems with the open model.
//!
//! # Source of truth
//!
//! Refreshed by [`refresh_diagnostics`] when the active Modelica source,
//! compile result, or shared Rumoca lint snapshot changes. Located parse,
//! compiler, and lint findings are collected into [`DiagnosticsLog`], whose
//! bounded history keeps recent authoring feedback visible while the source is
//! edited.

use std::collections::VecDeque;

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_ui::log::{LogEntry, LogLevel, SourceLoc, render_log_view};
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};

use crate::ui::document_context::ModelicaDocuments;

/// Panel id.
pub const DIAGNOSTICS_PANEL_ID: PanelId = PanelId("modelica_diagnostics");

/// Clear the diagnostics view after paint.
#[derive(Event, Clone, Copy, Default)]
pub(crate) struct ClearDiagnosticsRequested;

/// Move the editor caret to a selected diagnostic location.
#[derive(Event)]
pub(crate) struct DiagnosticJumpRequested {
    pub(crate) doc: Option<lunco_doc::DocumentId>,
    pub(crate) loc: SourceLoc,
}

pub(crate) fn on_clear_diagnostics_requested(
    _trigger: On<ClearDiagnosticsRequested>,
    mut log: ResMut<DiagnosticsLog>,
) {
    log.clear();
}

pub(crate) fn on_diagnostic_jump_requested(
    trigger: On<DiagnosticJumpRequested>,
    mut request: ResMut<crate::ui::panels::code_editor::EditorJumpRequest>,
) {
    let event = trigger.event();
    request.request(event.doc, event.loc);
}

/// Bounded history of diagnostics for the open Modelica model.
#[derive(Resource, Default)]
pub struct DiagnosticsLog {
    entries: VecDeque<LogEntry>,
}

impl DiagnosticsLog {
    /// Maximum history retained. Older entries fall off the front
    /// when new ones arrive. 200 is generous for a compile/lint
    /// channel (errors come in bursts, not streams) while keeping
    /// memory bounded.
    const MAX_ENTRIES: usize = 200;

    /// Append-with-dedup: push new entries onto the end, skipping
    /// any whose `text` is identical to the *previous* entry. This
    /// keeps the history (compile failed, then succeeded, then
    /// failed again → all three events visible) while collapsing
    /// "same error fired twice in one refresh".
    ///
    /// Replaces the earlier `replace` semantics which cleared the
    /// log every refresh and lost the exact error message the
    /// moment the user navigated away.
    pub fn append(&mut self, entries: Vec<LogEntry>) {
        for e in entries {
            let dup = self
                .entries
                .back()
                .map(|last| last.text == e.text && last.level == e.level)
                .unwrap_or(false);
            if dup {
                continue;
            }
            self.entries.push_back(e);
        }
        while self.entries.len() > Self::MAX_ENTRIES {
            self.entries.pop_front();
        }
    }

    /// Clear all entries. Kept for the panel's "Clear" button and
    /// for tests — we no longer call this from the refresh system.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Read-only access to the current entries.
    pub fn entries(&self) -> &VecDeque<LogEntry> {
        &self.entries
    }
}

pub struct DiagnosticsPanel;

impl Panel for DiagnosticsPanel {
    fn id(&self) -> PanelId {
        DIAGNOSTICS_PANEL_ID
    }

    fn title(&self) -> String {
        "Diagnostics".into()
    }

    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Lunica
    }

    fn default_slot(&self) -> PanelSlot {
        // Sit next to Console, which also docks at the Bottom.
        PanelSlot::Bottom
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // Snapshot so the scroll area doesn't hold a long world borrow.
        let snapshot: VecDeque<LogEntry> = ctx
            .resource::<DiagnosticsLog>()
            .map(|d| d.entries.clone())
            .unwrap_or_default();

        let theme = ctx
            .resource::<lunco_theme::Theme>()
            .cloned()
            .unwrap_or_else(lunco_theme::Theme::dark);
        let muted = theme.tokens.text_subdued;
        let mut clear_requested = false;
        let lint_status = ctx
            .resource::<lunco_workspace::WorkspaceResource>()
            .and_then(|workspace| workspace.active_document)
            .and_then(|doc| {
                let generation = ctx
                    .resource::<ModelicaDocuments>()?
                    .host(doc)?
                    .document()
                    .ast()
                    .generation;
                ctx.resource::<lunco_doc_bevy::DocumentDiagnostics>()?
                    .get(doc)?
                    .sources
                    .get("modelica.rumoca-lint")
                    .filter(|snapshot| snapshot.generation == generation)
                    .map(|snapshot| snapshot.state.clone())
            });
        match lint_status {
            Some(lunco_doc::DiagnosticSourceState::Pending) => {
                ui.label(
                    egui::RichText::new("Rumoca lint is running in the background…").color(muted),
                );
            }
            Some(
                lunco_doc::DiagnosticSourceState::Unavailable(message)
                | lunco_doc::DiagnosticSourceState::Failed(message),
            ) => {
                ui.label(egui::RichText::new(format!("Rumoca lint: {message}")).color(muted));
            }
            Some(lunco_doc::DiagnosticSourceState::Ready) | None => {}
        }
        let jump = render_log_view(
            ui,
            &snapshot,
            "(no diagnostics — model parses cleanly)",
            &mut clear_requested,
            muted,
            &theme,
        );
        if clear_requested {
            ctx.trigger(ClearDiagnosticsRequested);
        }
        // A located diagnostic was clicked — ask the code editor to
        // jump to it. The target is the currently active document
        // (lint runs on the bound doc, so it's already the open tab).
        if let Some(loc) = jump {
            let doc = ctx
                .resource::<lunco_workspace::WorkspaceResource>()
                .and_then(|ws| ws.active_document);
            ctx.trigger(DiagnosticJumpRequested { doc, loc });
        }
    }
}

/// What changed between refreshes. Stored as `Local<DiagnosticsCursor>` so
/// background lint completion also refreshes the visible diagnostics.
#[derive(Default)]
pub struct DiagnosticsCursor {
    bound_doc: Option<lunco_doc::DocumentId>,
    last_revision: Option<u64>,
}

/// Bevy system: refresh [`DiagnosticsLog`] only when the set of
/// diagnostics *could* have changed.
///
/// Change detection: compare (bound doc id, AST generation, hash of
/// compile-error string) to the previous tick's values. If all three
/// match, return immediately — no allocations, no `replace` call.
/// This avoids the "recompute + replace per frame" pattern that was
/// the initial implementation and kept the log's internal VecDeque
/// churning even when nothing was changing.
pub fn refresh_diagnostics(
    workspace: Res<lunco_workspace::WorkspaceResource>,
    registry: Res<ModelicaDocuments>,
    document_diagnostics: Res<lunco_doc_bevy::DocumentDiagnostics>,
    mut diagnostics: ResMut<DiagnosticsLog>,
    mut cursor: bevy::prelude::Local<DiagnosticsCursor>,
) {
    let doc_id = workspace.active_document;

    // No doc bound → clear once and stop.
    let Some(doc_id) = doc_id else {
        if cursor.bound_doc.is_some() {
            cursor.bound_doc = None;
            cursor.last_revision = None;
            // Preserve history — user may want to read the last
            // compile error after closing the tab.
        }
        return;
    };

    let Some(host) = registry.host(doc_id) else {
        if cursor.bound_doc.is_some() {
            cursor.bound_doc = None;
            cursor.last_revision = None;
            // Preserve history — user may want to read the last
            // compile error after closing the tab.
        }
        return;
    };

    let revision = document_diagnostics
        .get(doc_id)
        .map(|report| report.revision);

    // Fast-path: nothing that could affect diagnostics changed.
    if cursor.bound_doc == Some(doc_id) && cursor.last_revision == revision {
        return;
    }

    // Something moved — rebuild the entry list.
    cursor.bound_doc = Some(doc_id);
    cursor.last_revision = revision;

    let mut entries: Vec<LogEntry> = Vec::new();

    // Model name used to tag every entry pushed in this refresh —
    // the user's ask: "we should show names of models there." Each
    // row carries which model the message came from so you can
    // read the Diagnostics log across multiple open tabs without
    // guessing. `display_name` falls back to the origin's filename
    // or "Untitled" when the doc has no explicit name yet.
    let model_tag = Some(host.document().origin().display_name());

    // Every domain producer publishes to this same document snapshot. The
    // panel keeps its bounded history, while its rows retain the API's code,
    // remediation, and source-location fields.
    if let Some(report) = document_diagnostics.get(doc_id) {
        let generation = host.document().ast().generation;
        let current_diagnostics = report
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic, None))
            .chain(
                report
                    .sources
                    .values()
                    .filter(|channel| channel.generation == generation)
                    .flat_map(|channel| {
                        channel
                            .diagnostics
                            .iter()
                            .map(move |diagnostic| (diagnostic, Some(channel.id.as_str())))
                    }),
            );
        for (diagnostic, source) in current_diagnostics {
            let level = match diagnostic.severity {
                lunco_doc::DiagnosticSeverity::Error => LogLevel::Error,
                lunco_doc::DiagnosticSeverity::Warning => LogLevel::Warn,
                lunco_doc::DiagnosticSeverity::Info | lunco_doc::DiagnosticSeverity::Hint => {
                    LogLevel::Info
                }
            };
            let mut text = String::new();
            if let Some(code) = diagnostic.code.as_deref() {
                text.push('[');
                text.push_str(code);
                text.push_str("] ");
            } else if let Some(source) = source {
                text.push('[');
                text.push_str(source);
                text.push_str("] ");
            }
            text.push_str(&diagnostic.message);
            if let Some(suggestion) = diagnostic.suggestion.as_deref() {
                text.push_str("\nSuggestion: ");
                text.push_str(suggestion);
            }
            entries.push(LogEntry {
                at: web_time::Instant::now(),
                level,
                text,
                model: model_tag.clone(),
                loc: diagnostic
                    .line
                    .zip(diagnostic.col)
                    .map(|(line, column)| SourceLoc { line, column }),
            });
        }
    }

    diagnostics.append(entries);
}
