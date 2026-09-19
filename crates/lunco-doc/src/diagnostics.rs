//! Per-document diagnostics — the pure-data half of the unified diagnostics
//! substrate.
//!
//! [`DocDiagnostics`] is one document's compile state plus its diagnostics; it
//! has no Bevy dependency, so it lives here in `lunco-doc`. The ECS `Resource`
//! that stores these per [`crate::DocumentId`] lives in `lunco-doc-bevy`
//! (`DocumentDiagnostics`). [`document_status`] is the shared typed projection
//! used by domain status queries.

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

/// One diagnostic projected for a document-status query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticStatus {
    /// Stable lowercase severity tag.
    pub severity: &'static str,
    /// Human-readable diagnostic message.
    pub message: String,
    /// 1-based source line, if located.
    pub line: Option<u32>,
    /// 1-based source column, if located.
    pub col: Option<u32>,
}

/// Shared typed status projection for document compile queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocStatus {
    /// Stable lowercase compile-state tag.
    pub state: &'static str,
    /// Whether compilation is ready without errors.
    pub ok: bool,
    /// Diagnostics from the latest compile result.
    pub diagnostics: Vec<DiagnosticStatus>,
}

/// Project optional diagnostics into the shared typed status contract.
pub fn document_status(entry: Option<&DocDiagnostics>) -> DocStatus {
    let state = entry.map(|e| e.state).unwrap_or(CompileState::Idle);
    let diagnostics = entry
        .map(|e| {
            e.diagnostics
                .iter()
                .map(|diagnostic| DiagnosticStatus {
                    severity: diagnostic.severity.as_str(),
                    message: diagnostic.message.clone(),
                    line: diagnostic.line,
                    col: diagnostic.col,
                })
                .collect()
        })
        .unwrap_or_default();
    DocStatus {
        state: state.as_str(),
        ok: state == CompileState::Ready,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_status_projects_state_and_diagnostics_without_transport_values() {
        let diagnostics = DocDiagnostics {
            state: CompileState::Error,
            diagnostics: vec![Diagnostic::error("parse failed", Some(4), None)],
        };

        assert_eq!(
            document_status(Some(&diagnostics)),
            DocStatus {
                state: "error",
                ok: false,
                diagnostics: vec![DiagnosticStatus {
                    severity: "error",
                    message: "parse failed".to_owned(),
                    line: Some(4),
                    col: None,
                }],
            }
        );
        assert_eq!(
            document_status(None),
            DocStatus {
                state: "idle",
                ok: false,
                diagnostics: Vec::new(),
            }
        );
    }
}
