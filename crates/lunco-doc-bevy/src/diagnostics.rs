//! The ECS half of the unified diagnostics substrate.
//!
//! The diagnostic types and the per-document [`DocDiagnostics`] snapshot are
//! pure data in `lunco-doc`; this module adds only the Bevy `Resource` that
//! stores them per [`DocumentId`], plus compile-timing bookkeeping. Every domain
//! that compiles documents — Modelica, rhai scripting, future languages —
//! reports through this ONE resource and reads back via [`lunco_doc::status_json`].

use std::collections::HashMap;
use std::time::Duration;

use bevy::platform::time::Instant;
use bevy::prelude::*;
use lunco_doc::{CompileState, Diagnostic, DocDiagnostics, DocumentId};

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
        e.state = CompileState::Compiling;
        e.diagnostics.clear();
        self.started.insert(id, Instant::now());
    }

    /// Transition to a terminal `state`, returning elapsed since the matching
    /// [`mark_started`](Self::mark_started) (if any). Does not touch
    /// diagnostics — set those via [`set_error`](Self::set_error) /
    /// [`set_ok`](Self::set_ok).
    pub fn mark_finished(&mut self, id: DocumentId, state: CompileState) -> Option<Duration> {
        self.by_doc.entry(id).or_default().state = state;
        self.started.remove(&id).map(|t| t.elapsed())
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
        self.started.remove(&id);
    }

    /// Record a failed compile/run — state `Error`, with the given diagnostics.
    pub fn set_error(&mut self, id: DocumentId, diagnostics: Vec<Diagnostic>) {
        let e = self.by_doc.entry(id).or_default();
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
        e.state = if has_error { CompileState::Error } else { CompileState::Ready };
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
            e.diagnostics.clear();
        }
    }

    /// Drop everything tracked for a document (e.g. on close).
    pub fn clear(&mut self, id: DocumentId) {
        self.by_doc.remove(&id);
        self.started.remove(&id);
    }

    /// Alias of [`clear`](Self::clear) — drop all state for a removed document.
    pub fn remove(&mut self, id: DocumentId) {
        self.clear(id);
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
