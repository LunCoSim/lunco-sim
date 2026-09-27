//! The ECS half of the unified diagnostics substrate.
//!
//! The diagnostic types and the per-document [`DocDiagnostics`] snapshot are
//! pure data in `lunco-doc`; this module adds only the Bevy `Resource` that
//! stores them per [`DocumentId`], plus compile-timing bookkeeping. Every domain
//! that compiles documents — Modelica, rhai scripting, future languages —
//! reports through this ONE resource and projects status via
//! [`lunco_doc::document_status`].

use std::collections::HashMap;
use std::time::Duration;

use bevy::platform::time::Instant;
use bevy::prelude::*;
use lunco_doc::{CompileState, Diagnostic, DiagnosticSourceReport, DocDiagnostics, DocumentId};

/// Process-wide store of document diagnostics, keyed by [`DocumentId`].
///
/// This is the shared substrate: `init_resource` it once (idempotent — whichever
/// plugin runs first wins) and every domain writes here. The diagnostic data
/// itself ([`DocDiagnostics`]) lives in `lunco-doc`; this resource adds the ECS
/// store plus per-document compile-timing (the runtime concern that doesn't
/// belong in the pure-data layer).
#[derive(Resource, Default)]
pub struct DocumentDiagnostics {
    by_doc: HashMap<DocumentId, DocDiagnostics>,
    /// When each in-flight compile started — set by `mark_started`, consumed by
    /// `mark_finished` to report elapsed. Kept out of `DocDiagnostics` so that
    /// type stays pure data. Uses Bevy's portable `Instant` (std on native,
    /// web-time on wasm) — no extra clock dependency.
    started: HashMap<DocumentId, Instant>,
}

impl DocumentDiagnostics {
    /// Mark a document as compiling (clears stale diagnostics) WITHOUT stamping
    /// a start time. Prefer [`mark_started`](Self::mark_started) for elapsed.
    pub fn mark_compiling(&mut self, id: DocumentId) {
        let e = self.by_doc.entry(id).or_default();
        if e.state != CompileState::Compiling || !e.diagnostics.is_empty() {
            e.revision = e.revision.saturating_add(1);
        }
        e.state = CompileState::Compiling;
        e.diagnostics.clear();
    }

    /// Transition to `Compiling` and stamp the start time (clears stale
    /// diagnostics). Pair with [`mark_finished`](Self::mark_finished).
    // `Instant` here is `bevy::platform::time::Instant` — the portable clock
    // (std on native, web-time on wasm). On the *native* target bevy's re-export
    // resolves to the same `DefId` as `std::time::Instant`, so clippy's
    // `disallowed_methods` ban (which exists to catch the wasm-panicking std
    // clock) fires on correct code. Documented false positive; see clippy.toml.
    #[allow(clippy::disallowed_methods)]
    pub fn mark_started(&mut self, id: DocumentId) {
        let e = self.by_doc.entry(id).or_default();
        if e.state != CompileState::Compiling || !e.diagnostics.is_empty() {
            e.revision = e.revision.saturating_add(1);
        }
        e.state = CompileState::Compiling;
        e.diagnostics.clear();
        self.started.insert(id, Instant::now());
    }

    /// Transition to a terminal `state`, returning elapsed since the matching
    /// [`mark_started`](Self::mark_started) (if any). Does not touch
    /// diagnostics — set those via [`set_error`](Self::set_error) /
    /// [`set_ok`](Self::set_ok).
    pub fn mark_finished(&mut self, id: DocumentId, state: CompileState) -> Option<Duration> {
        let entry = self.by_doc.entry(id).or_default();
        if entry.state != state {
            entry.revision = entry.revision.saturating_add(1);
        }
        entry.state = state;
        self.started.remove(&id).map(|t| t.elapsed())
    }

    /// True when a compile is currently in flight for `id`.
    pub fn is_compiling(&self, id: DocumentId) -> bool {
        self.state_of(id) == CompileState::Compiling
    }

    /// Record a successful compile — state `Ready`, diagnostics cleared.
    pub fn set_ok(&mut self, id: DocumentId) {
        let e = self.by_doc.entry(id).or_default();
        if e.state != CompileState::Ready || !e.diagnostics.is_empty() {
            e.revision = e.revision.saturating_add(1);
        }
        e.state = CompileState::Ready;
        e.diagnostics.clear();
        self.started.remove(&id);
    }

    /// Record a failed compile/run — state `Error`, with the given diagnostics.
    pub fn set_error(&mut self, id: DocumentId, diagnostics: Vec<Diagnostic>) {
        let e = self.by_doc.entry(id).or_default();
        if e.state != CompileState::Error || e.diagnostics != diagnostics {
            e.revision = e.revision.saturating_add(1);
        }
        e.state = CompileState::Error;
        e.diagnostics = diagnostics;
        self.started.remove(&id);
    }

    /// Record diagnostics whose *severity* decides the compile state: any
    /// error-severity diagnostic ⇒ [`CompileState::Error`]; a warning/info-only
    /// set ⇒ [`CompileState::Ready`] (it compiled and ran — the diagnostics are
    /// advisory and still surface); an empty set clears to `Ready`, like
    /// [`set_ok`](Self::set_ok). Use this where a document can carry non-fatal
    /// notices (e.g. a scenario warning) that must not masquerade as a red
    /// compile error.
    pub fn set_diagnostics(&mut self, id: DocumentId, diagnostics: Vec<Diagnostic>) {
        let has_error = diagnostics
            .iter()
            .any(|d| d.severity == lunco_doc::DiagnosticSeverity::Error);
        let e = self.by_doc.entry(id).or_default();
        let state = if has_error {
            CompileState::Error
        } else {
            CompileState::Ready
        };
        if e.state != state || e.diagnostics != diagnostics {
            e.revision = e.revision.saturating_add(1);
        }
        e.state = state;
        e.diagnostics = diagnostics;
        self.started.remove(&id);
    }

    /// Convenience: record an error from a single flat message (no location).
    pub fn set_error_message(&mut self, id: DocumentId, message: impl Into<String>) {
        self.set_error(id, vec![Diagnostic::message_only(message)]);
    }

    /// Clear diagnostics for `id` but leave its compile state untouched (e.g.
    /// the user dismissed the error banner).
    pub fn clear_error(&mut self, id: DocumentId) {
        if let Some(e) = self.by_doc.get_mut(&id) {
            if !e.diagnostics.is_empty() {
                e.diagnostics.clear();
                e.revision = e.revision.saturating_add(1);
            }
        }
    }

    /// Drop everything tracked for a document (e.g. on close).
    pub fn clear(&mut self, id: DocumentId) {
        self.by_doc.remove(&id);
        self.started.remove(&id);
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

    /// Publish a complete report for one producer and exact document
    /// generation. Older asynchronous results are rejected so a late worker
    /// cannot replace findings from a newer edit.
    pub fn set_source_report(&mut self, id: DocumentId, report: DiagnosticSourceReport) -> bool {
        if self
            .by_doc
            .get(&id)
            .and_then(|entry| entry.sources.get(&report.id))
            .is_some_and(|current| current == &report)
        {
            return false;
        }
        let entry = self.by_doc.entry(id).or_default();
        if entry.sources.get(&report.id).is_some_and(|current| {
            current.generation > report.generation
                || (current.generation == report.generation
                    && match (current.revision, report.revision) {
                        (Some(current), Some(next)) => current > next,
                        (Some(_), None) => true,
                        (None, _) => false,
                    })
        }) {
            return false;
        }
        entry.sources.insert(report.id.clone(), report);
        entry.revision = entry.revision.saturating_add(1);
        true
    }

    /// Remove one producer's report, for example when its source document
    /// closes or the producer is no longer installed.
    pub fn clear_source(&mut self, id: DocumentId, source_id: &str) {
        if let Some(entry) = self.by_doc.get_mut(&id) {
            if entry.sources.remove(source_id).is_some() {
                entry.revision = entry.revision.saturating_add(1);
            }
        }
    }
}

/// Drop a closing document's entry from the shared substrate. Registered by
/// [`TwinJournalPlugin`](crate::TwinJournalPlugin): every domain reports into
/// the one resource, so the close-time cleanup lives with the resource — not
/// inside any single domain's plugin.
pub fn drop_diagnostics_on_close(
    trigger: On<crate::CloseDocument>,
    diagnostics: Option<ResMut<DocumentDiagnostics>>,
) {
    if let Some(mut d) = diagnostics {
        d.clear(trigger.event().doc_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::DiagnosticSourceState;

    fn source_report(
        generation: u64,
        revision: Option<u64>,
        state: DiagnosticSourceState,
    ) -> DiagnosticSourceReport {
        DiagnosticSourceReport {
            id: "modelica.rumoca-lint".to_owned(),
            domain: "modelica".to_owned(),
            generation,
            revision,
            state,
            message: None,
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn producer_reports_reject_older_generation_and_revision() {
        let mut diagnostics = DocumentDiagnostics::default();
        let doc_id = DocumentId::new(7);
        assert!(diagnostics.set_source_report(
            doc_id,
            source_report(4, Some(8), DiagnosticSourceState::Ready),
        ));
        assert!(!diagnostics.set_source_report(
            doc_id,
            source_report(4, Some(7), DiagnosticSourceState::Pending),
        ));
        assert!(!diagnostics.set_source_report(
            doc_id,
            source_report(3, Some(99), DiagnosticSourceState::Ready),
        ));
        assert_eq!(
            diagnostics.get(doc_id).unwrap().sources["modelica.rumoca-lint"].revision,
            Some(8),
        );
    }
}
