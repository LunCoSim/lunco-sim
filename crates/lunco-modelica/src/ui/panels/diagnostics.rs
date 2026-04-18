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

/// Bevy system: refresh [`DiagnosticsLog`] from the bound document's
/// `AstCache.errors` on every frame.
///
/// Cheap — just reads the already-populated error list, doesn't
/// reparse. Runs every frame unconditionally for simplicity; the
/// log's `replace` is O(n) in the (tiny) number of current errors.
/// If this ever shows up on a profile, gate on document-generation
/// change — until then, the simplicity wins.
pub fn refresh_diagnostics(
    workbench: Res<WorkbenchState>,
    registry: Res<ModelicaDocumentRegistry>,
    mut diagnostics: ResMut<DiagnosticsLog>,
) {
    let doc_id = workbench.open_model.as_ref().and_then(|m| m.doc);
    let Some(doc_id) = doc_id else {
        if !diagnostics.entries.is_empty() {
            diagnostics.clear();
        }
        return;
    };
    let Some(host) = registry.host(doc_id) else {
        if !diagnostics.entries.is_empty() {
            diagnostics.clear();
        }
        return;
    };

    let cache = host.document().ast();
    let entries: Vec<LogEntry> = match &cache.result {
        Ok(_) => Vec::new(),
        Err(msg) => vec![LogEntry {
            at: std::time::Instant::now(),
            level: LogLevel::Error,
            text: msg.clone(),
        }],
    };
    diagnostics.replace(entries);
}
