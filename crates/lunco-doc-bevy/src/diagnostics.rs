//! Unified per-document diagnostics store.
//!
//! Every domain that compiles documents — Modelica, rhai scripting, and any
//! future language — reports through this ONE resource instead of rolling its
//! own. A producer pushes [`lunco_doc::Diagnostic`]s under a
//! [`lunco_doc::DocumentId`]; consumers (an egui panel, an API status query,
//! the MCP) read the same store and the same [`status_json`] shape.
//!
//! Diagnostics carry their location as a byte range (LSP-precise,
//! source-independent); [`status_json`] converts to 1-based line/col for
//! display when given the document source.

use std::collections::HashMap;
use std::time::Duration;

use bevy::prelude::*;
use lunco_doc::{CompileState, Diagnostic, DiagnosticSeverity, DocumentId};

/// One document's compile state plus its diagnostics.
#[derive(Default, Clone)]
pub struct DocDiagnostics {
    /// Current compile lifecycle state.
    pub state: CompileState,
    /// All diagnostics from the last compile (errors, warnings, …).
    pub diagnostics: Vec<Diagnostic>,
    /// When the in-flight compile started (set by `mark_started`, consumed by
    /// `mark_finished` to report elapsed). `None` outside a compile.
    started_at: Option<web_time::Instant>,
}

impl DocDiagnostics {
    /// Whether the document currently has any error-severity diagnostic.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == DiagnosticSeverity::Error)
    }

    /// The first error-severity diagnostic's message, if any.
    pub fn error_message(&self) -> Option<&str> {
        self.diagnostics
            .iter()
            .find(|d| d.severity == DiagnosticSeverity::Error)
            .map(|d| d.message.as_str())
    }
}

/// Process-wide store of document diagnostics, keyed by [`DocumentId`].
///
/// This is the shared substrate: `init_resource` it once (idempotent — whichever
/// plugin runs first wins) and every domain writes here.
#[derive(Resource, Default)]
pub struct DocumentDiagnostics {
    by_doc: HashMap<DocumentId, DocDiagnostics>,
}

impl DocumentDiagnostics {
    /// Mark a document as compiling (clears stale diagnostics) WITHOUT stamping
    /// a start time. Prefer [`mark_started`](Self::mark_started) when you want
    /// elapsed-time reporting.
    pub fn mark_compiling(&mut self, id: DocumentId) {
        let e = self.by_doc.entry(id).or_default();
        e.state = CompileState::Compiling;
        e.diagnostics.clear();
    }

    /// Transition to `Compiling` and stamp the start time (clears stale
    /// diagnostics). Pair with [`mark_finished`](Self::mark_finished).
    pub fn mark_started(&mut self, id: DocumentId) {
        let e = self.by_doc.entry(id).or_default();
        e.state = CompileState::Compiling;
        e.diagnostics.clear();
        e.started_at = Some(web_time::Instant::now());
    }

    /// Transition to a terminal `state`, returning elapsed since the matching
    /// [`mark_started`](Self::mark_started) (if any). Does not touch
    /// diagnostics — set those via [`set_error`](Self::set_error) /
    /// [`set_ok`](Self::set_ok).
    pub fn mark_finished(&mut self, id: DocumentId, state: CompileState) -> Option<Duration> {
        let e = self.by_doc.entry(id).or_default();
        e.state = state;
        e.started_at.take().map(|t| t.elapsed())
    }

    /// True when a compile is currently in flight for `id`.
    pub fn is_compiling(&self, id: DocumentId) -> bool {
        self.state_of(id) == CompileState::Compiling
    }

    /// Record a successful compile — state `Ready`, diagnostics cleared.
    pub fn set_ok(&mut self, id: DocumentId) {
        let e = self.by_doc.entry(id).or_default();
        e.state = CompileState::Ready;
        e.diagnostics.clear();
        e.started_at = None;
    }

    /// Record a failed compile/run — state `Error`, with the given diagnostics.
    pub fn set_error(&mut self, id: DocumentId, diagnostics: Vec<Diagnostic>) {
        let e = self.by_doc.entry(id).or_default();
        e.state = CompileState::Error;
        e.diagnostics = diagnostics;
        e.started_at = None;
    }

    /// Convenience: record an error from a single flat message (no location).
    pub fn set_error_message(&mut self, id: DocumentId, message: impl Into<String>) {
        self.set_error(id, vec![Diagnostic::message_only(message)]);
    }

    /// Clear diagnostics for `id` but leave its compile state untouched (e.g.
    /// the user dismissed the error banner).
    pub fn clear_error(&mut self, id: DocumentId) {
        if let Some(e) = self.by_doc.get_mut(&id) {
            e.diagnostics.clear();
        }
    }

    /// Drop everything tracked for a document (e.g. on close).
    pub fn clear(&mut self, id: DocumentId) {
        self.by_doc.remove(&id);
    }

    /// Alias of [`clear`](Self::clear) — drop all state for a removed document.
    pub fn remove(&mut self, id: DocumentId) {
        self.by_doc.remove(&id);
    }

    /// The tracked entry for a document, if any.
    pub fn get(&self, id: DocumentId) -> Option<&DocDiagnostics> {
        self.by_doc.get(&id)
    }

    /// The document's compile state (`Idle` if untracked).
    pub fn state_of(&self, id: DocumentId) -> CompileState {
        self.by_doc
            .get(&id)
            .map(|e| e.state)
            .unwrap_or(CompileState::Idle)
    }

    /// The first error-severity diagnostic's message for `id`, if any (the flat
    /// summary form — the analogue of the old `error_for`).
    pub fn error_message(&self, id: DocumentId) -> Option<&str> {
        self.by_doc.get(&id).and_then(DocDiagnostics::error_message)
    }

    /// The document's diagnostics (empty if untracked).
    pub fn diagnostics(&self, id: DocumentId) -> &[Diagnostic] {
        self.by_doc
            .get(&id)
            .map(|e| e.diagnostics.as_slice())
            .unwrap_or(&[])
    }
}

/// The canonical status JSON every domain's status query returns:
/// `{ state, ok, diagnostics: [{ severity, message, line, col }] }`
/// (`line`/`col` 1-based, null when the diagnostic is unlocated).
pub fn status_json(entry: Option<&DocDiagnostics>) -> serde_json::Value {
    let state = entry.map(|e| e.state).unwrap_or(CompileState::Idle);
    let diags: Vec<serde_json::Value> = entry
        .map(|e| e.diagnostics.iter().map(diagnostic_json).collect())
        .unwrap_or_default();
    serde_json::json!({
        "state": state.as_str(),
        "ok": state == CompileState::Ready,
        "diagnostics": diags,
    })
}

/// Serialise one diagnostic.
fn diagnostic_json(d: &Diagnostic) -> serde_json::Value {
    let severity = match d.severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Info => "info",
        DiagnosticSeverity::Hint => "hint",
    };
    serde_json::json!({
        "severity": severity,
        "message": d.message,
        "line": d.line,
        "col": d.col,
    })
}
