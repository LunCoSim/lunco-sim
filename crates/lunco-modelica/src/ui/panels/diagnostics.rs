//! Diagnostics panel — Modelica-specific parse and semantic errors.
//!
//! Bottom-dock tab next to Console, sharing the same visual shape
//! ([`crate::ui::panels::log::render_log_view`]) but scoped to
//! Modelica document diagnostics. Console accumulates every
//! workbench event; Diagnostics only shows the *current* set of
//! problems with the open model.
//!
//! # Source of truth
//!
//! Refreshed by [`refresh_diagnostics`] each frame. It reads the
//! bound document's `AstCache.errors` list (populated by rumoca's
//! `parse_to_syntax` recovery) and mirrors them into
//! [`DiagnosticsLog`]. Empty on a clean parse; one entry per
//! diagnostic when the parser has something to say.
//!
//! Modelled as a *replaced-every-frame* log (not append-only like
//! Console) so the panel reflects the current state — fix the error
//! in the code editor and the entry disappears automatically.

use std::collections::VecDeque;

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::ui::panels::log::{render_log_view, LogEntry, LogLevel};
use crate::ui::state::{ModelicaDocumentRegistry, WorkbenchState};

/// Panel id.
pub const DIAGNOSTICS_PANEL_ID: PanelId = PanelId("modelica_diagnostics");

/// Current diagnostics for the open model. Rebuilt from AST state
/// each frame rather than accumulated — a fixed parse becomes a
/// cleared log.
#[derive(Resource, Default)]
pub struct DiagnosticsLog {
    entries: VecDeque<LogEntry>,
}

impl DiagnosticsLog {
    /// Replace the current entries in-place.
    pub fn replace(&mut self, entries: Vec<LogEntry>) {
        self.entries.clear();
        self.entries.extend(entries);
    }

    /// Clear all entries.
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
        "⚠ Diagnostics".into()
    }

    fn default_slot(&self) -> PanelSlot {
        // Sit next to Console, which also docks at the Bottom.
        PanelSlot::Bottom
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        if world.get_resource::<DiagnosticsLog>().is_none() {
            world.insert_resource(DiagnosticsLog::default());
        }
        // Snapshot so the scroll area doesn't hold a long world borrow.
        let snapshot: VecDeque<LogEntry> =
            world.resource::<DiagnosticsLog>().entries.clone();

        let mut clear_requested = false;
        render_log_view(
            ui,
            &snapshot,
            "(no diagnostics — model parses cleanly)",
            &mut clear_requested,
        );
        if clear_requested {
            world.resource_mut::<DiagnosticsLog>().clear();
        }
    }
}

/// What changed between refreshes. Stored as `Local<DiagnosticsCursor>`
/// so we skip work on frames where neither the bound document, its
/// AST generation, nor the compile-error string moved.
#[derive(Default)]
pub struct DiagnosticsCursor {
    bound_doc: Option<lunco_doc::DocumentId>,
    last_ast_gen: u64,
    /// Hash of `compilation_error` — cheaper to compare than the
    /// string itself and avoids keeping a clone around.
    last_error_hash: u64,
}

fn hash_str(s: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
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
    workbench: Res<WorkbenchState>,
    workspace: Res<lunco_workbench::WorkspaceResource>,
    registry: Res<ModelicaDocumentRegistry>,
    mut diagnostics: ResMut<DiagnosticsLog>,
    mut cursor: bevy::prelude::Local<DiagnosticsCursor>,
) {
    let doc_id = workspace.active_document;

    // No doc bound → clear once and stop.
    let Some(doc_id) = doc_id else {
        if cursor.bound_doc.is_some() {
            cursor.bound_doc = None;
            cursor.last_ast_gen = 0;
            cursor.last_error_hash = hash_str(None);
            diagnostics.clear();
        }
        return;
    };

    let Some(host) = registry.host(doc_id) else {
        if cursor.bound_doc.is_some() {
            cursor.bound_doc = None;
            cursor.last_ast_gen = 0;
            cursor.last_error_hash = hash_str(None);
            diagnostics.clear();
        }
        return;
    };

    let ast_gen = host.document().ast().generation;
    let err_hash = hash_str(workbench.compilation_error.as_deref());
    // Lint depends on source content. AST gen ticks on every source
    // mutation, so combining (ast_gen, err_hash) is enough — no extra
    // source hash needed.

    // Fast-path: nothing that could affect diagnostics changed.
    if cursor.bound_doc == Some(doc_id)
        && cursor.last_ast_gen == ast_gen
        && cursor.last_error_hash == err_hash
    {
        return;
    }

    // Something moved — rebuild the entry list.
    cursor.bound_doc = Some(doc_id);
    cursor.last_ast_gen = ast_gen;
    cursor.last_error_hash = err_hash;

    let mut entries: Vec<LogEntry> = Vec::new();

    // 1. AST parse errors — caught by rumoca's recovering parser.
    if let Err(msg) = &host.document().ast().result {
        entries.push(LogEntry {
            at: std::time::Instant::now(),
            level: LogLevel::Error,
            text: msg.clone(),
        });
    }

    // 2. Compile / run errors — the simulator worker writes these
    // into `WorkbenchState.compilation_error` whenever a compile
    // or simulation step fails. Without mirroring them here the
    // Diagnostics panel stayed empty even when a red "Error" chip
    // was visible in the toolbar.
    if let Some(msg) = workbench.compilation_error.as_ref() {
        entries.push(LogEntry {
            at: std::time::Instant::now(),
            level: LogLevel::Error,
            text: msg.clone(),
        });
    }

    // 3. Lint findings — `rumoca-tool-lint` runs on the source and
    // returns warnings/style issues with line+column. Cheap (rumoca
    // re-uses its parse cache), so running on every change-tick is
    // fine. Surfaces as Warning-level rows; if a future linter rule
    // is upgraded to Error, mirror its level here.
    let source = host.document().source();
    if !source.is_empty() {
        let opts = rumoca_tool_lint::LintOptions::default();
        let display_name = host.document().origin().display_name();
        for msg in rumoca_tool_lint::lint(source, &display_name, &opts) {
            let level = match msg.level {
                rumoca_tool_lint::LintLevel::Error => LogLevel::Error,
                rumoca_tool_lint::LintLevel::Warning => LogLevel::Warn,
                _ => LogLevel::Info,
            };
            entries.push(LogEntry {
                at: std::time::Instant::now(),
                level,
                text: format!(
                    "{}:{}:{}  [{}] {}",
                    msg.file, msg.line, msg.column, msg.rule, msg.message
                ),
            });
        }
    }

    diagnostics.replace(entries);
}
