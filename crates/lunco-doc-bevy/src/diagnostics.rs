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

use bevy::prelude::*;
use lunco_doc::{CompileState, Diagnostic, DiagnosticSeverity, DocumentId};

/// One document's compile state plus its diagnostics.
#[derive(Default, Clone)]
pub struct DocDiagnostics {
    /// Current compile lifecycle state.
    pub state: CompileState,
    /// All diagnostics from the last compile (errors, warnings, …).
    pub diagnostics: Vec<Diagnostic>,
}

impl DocDiagnostics {
    /// Whether the document currently has any error-severity diagnostic.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == DiagnosticSeverity::Error)
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
    /// Mark a document as compiling (clears stale diagnostics).
    pub fn mark_compiling(&mut self, id: DocumentId) {
        let e = self.by_doc.entry(id).or_default();
        e.state = CompileState::Compiling;
        e.diagnostics.clear();
    }

    /// Record a successful compile — state `Ready`, diagnostics cleared.
    pub fn set_ok(&mut self, id: DocumentId) {
        let e = self.by_doc.entry(id).or_default();
        e.state = CompileState::Ready;
        e.diagnostics.clear();
    }

    /// Record a failed compile/run — state `Error`, with the given diagnostics.
    pub fn set_error(&mut self, id: DocumentId, diagnostics: Vec<Diagnostic>) {
        let e = self.by_doc.entry(id).or_default();
        e.state = CompileState::Error;
        e.diagnostics = diagnostics;
    }

    /// Drop everything tracked for a document (e.g. on close).
    pub fn clear(&mut self, id: DocumentId) {
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

    /// The document's diagnostics (empty if untracked).
    pub fn diagnostics(&self, id: DocumentId) -> &[Diagnostic] {
        self.by_doc
            .get(&id)
            .map(|e| e.diagnostics.as_slice())
            .unwrap_or(&[])
    }
}

/// The canonical status JSON every domain's status query returns:
/// `{ state, ok, diagnostics: [{ severity, message, line, col }] }`.
/// `line`/`col` are 1-based and present only when both the diagnostic has a
/// range and `source` is provided.
pub fn status_json(entry: Option<&DocDiagnostics>, source: Option<&str>) -> serde_json::Value {
    let state = entry.map(|e| e.state).unwrap_or(CompileState::Idle);
    let diags: Vec<serde_json::Value> = entry
        .map(|e| {
            e.diagnostics
                .iter()
                .map(|d| diagnostic_json(d, source))
                .collect()
        })
        .unwrap_or_default();
    serde_json::json!({
        "state": state.as_str(),
        "ok": state == CompileState::Ready,
        "diagnostics": diags,
    })
}

/// Serialise one diagnostic, computing 1-based line/col from `source` if both a
/// range and the source are available.
fn diagnostic_json(d: &Diagnostic, source: Option<&str>) -> serde_json::Value {
    let severity = match d.severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Info => "info",
        DiagnosticSeverity::Hint => "hint",
    };
    let (line, col) = match (source, d.range) {
        (Some(src), Some(_)) => {
            let (l, c) = d.line_col(src).unwrap();
            (Some(l), Some(c))
        }
        _ => (None, None),
    };
    serde_json::json!({
        "severity": severity,
        "message": d.message,
        "line": line,
        "col": col,
    })
}
