//! Per-document diagnostics — the pure-data half of the unified diagnostics
//! substrate.
//!
//! [`DocDiagnostics`] is one document's compile state plus its diagnostics; it
//! has no Bevy dependency, so it lives here in `lunco-doc`. The ECS `Resource`
//! that stores these per [`crate::DocumentId`] lives in `lunco-doc-bevy`
//! (`DocumentDiagnostics`). [`status_json`] is the one JSON shape every domain's
//! status query returns.

use crate::{CompileState, Diagnostic, DiagnosticSeverity};

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

    /// The first error-severity diagnostic's message, if any.
    pub fn error_message(&self) -> Option<&str> {
        self.diagnostics
            .iter()
            .find(|d| d.severity == DiagnosticSeverity::Error)
            .map(|d| d.message.as_str())
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
