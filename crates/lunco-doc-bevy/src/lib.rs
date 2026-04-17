//! # lunco-doc-bevy
//!
//! Bevy integration for the LunCoSim Document System.
//!
//! `lunco-doc` is pure data (zero deps, headless-testable). This crate
//! adds the ECS-facing half: generic document-lifecycle events that any
//! domain's registry (Modelica, USD, scripting, SysML, …) fires through,
//! plus the [`TwinJournal`] that subscribes to those events to give the
//! whole project one canonical change log.
//!
//! ## The architectural rule
//!
//! **Documents are mutated only through `#[Command]` observers.** Every
//! user intent — clicking Compile, typing in the editor, dropping a
//! component on the diagram, tweaking a parameter, invoking a remote
//! script — is a command whose observer applies ops to one or more
//! [`lunco_doc::Document`]s. The documents fire
//! [`DocumentChanged`] and its siblings, and every subscriber
//! (re-render, re-parse, telemetry refresh, plot-variable update,
//! replay, audit) reacts from those events.
//!
//! This rule is the nucleus: it means
//!
//! - **Undo/redo works everywhere for free** — per-document
//!   [`lunco_doc::DocumentHost`] stacks handle it.
//! - **Scripting / API / keyboard shortcuts share one path** — they
//!   all fire the same commands.
//! - **The Twin journal is a complete record** — nothing mutates a
//!   document without going through an event this crate already logs.
//! - **Views don't poll** — they `On<DocumentChanged>` (or similar)
//!   and react.
//!
//! ## Events
//!
//! All four events carry a [`lunco_doc::DocumentId`] so subscribers can
//! resolve the backing document from whichever domain registry owns it.
//! They are plain Bevy `Event`s (no `Reflect`), because `DocumentId`
//! intentionally stays bevy-reflect-free in `lunco-doc`. Commands that
//! need to be reflected (for scripting dispatch) wrap their `DocumentId`
//! with `#[reflect(ignore)]` or similar.
//!
//! ## TwinJournal
//!
//! [`TwinJournal`] is a session-scoped, append-only log of document
//! events. It is *not* undo history — per-document undo still lives on
//! `DocumentHost`. The journal answers different questions: "what
//! happened this session, in what order, across all documents?" —
//! useful for replay, audit, debugging, and future cross-doc
//! transactions.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::time::Instant;

use bevy::prelude::*;
use lunco_doc::DocumentId;

// ─────────────────────────────────────────────────────────────────────────────
// Lifecycle events
// ─────────────────────────────────────────────────────────────────────────────

/// A [`Document`](lunco_doc::Document) in some domain registry just had
/// an op applied (source changed, generation bumped).
///
/// Subscribers observe via `On<DocumentChanged>` to re-render, re-parse,
/// invalidate caches, or refresh derived UI. Firing happens from the
/// domain registry's mutation path — typically via a `drain_pending_changes`
/// system that fans queued ids out as observer triggers.
#[derive(Event, Clone, Debug)]
pub struct DocumentChanged {
    /// The document whose generation just advanced.
    pub doc: DocumentId,
}

/// A new [`Document`](lunco_doc::Document) was added to a domain
/// registry (file opened, New Model created, script allocated, …).
///
/// Fires once per document, immediately after the registry inserts it.
/// Precedes any [`DocumentChanged`] for the same id that would otherwise
/// represent "filled-with-initial-source".
#[derive(Event, Clone, Debug)]
pub struct DocumentOpened {
    /// The newly-registered document.
    pub doc: DocumentId,
}

/// A [`Document`](lunco_doc::Document) was removed from its registry
/// (entity despawned + cleanup, explicit Close, registry reset).
///
/// Subscribers can drop per-document caches (plots, telemetry history,
/// layout state). The id may be reused later for a different document;
/// consumers should not assume ids are stable across Close→Open pairs.
#[derive(Event, Clone, Debug)]
pub struct DocumentClosed {
    /// The document that no longer exists in the registry.
    pub doc: DocumentId,
}

/// A [`Document`](lunco_doc::Document)'s contents were persisted to
/// disk (Save, Save As).
///
/// Fires after a successful write. Dirty trackers use this together
/// with [`DocumentChanged`] to maintain the save indicator.
#[derive(Event, Clone, Debug)]
pub struct DocumentSaved {
    /// The document that was just persisted.
    pub doc: DocumentId,
}

// ─────────────────────────────────────────────────────────────────────────────
// TwinJournal
// ─────────────────────────────────────────────────────────────────────────────

/// One entry in the [`TwinJournal`].
///
/// Journal entries intentionally do *not* carry op payloads — ops live
/// on their `DocumentHost`'s undo stack. The journal is a *summary*
/// timeline: "at time `t`, event `kind` happened on document `doc`."
/// Consumers that need the full op walk the host.
#[derive(Clone, Debug)]
pub struct TwinEvent {
    /// Wall-clock time the event was appended (session-local).
    pub at: Instant,
    /// What happened.
    pub kind: TwinEventKind,
}

/// Discriminant for [`TwinEvent`].
///
/// Kept flat (not a wrapped Bevy event) so consumers can serialize,
/// diff, and persist the journal without pulling in Bevy's event types.
/// When a future transactional / multi-document op lands, add a
/// `Transaction { name, events: Vec<TwinEventKind> }` variant here.
#[derive(Clone, Debug)]
pub enum TwinEventKind {
    /// A document was added to its registry.
    Opened {
        /// The id of the newly-opened document.
        doc: DocumentId,
    },
    /// A document had an op applied (source advanced).
    Changed {
        /// The id of the changed document.
        doc: DocumentId,
    },
    /// A document was saved to disk.
    Saved {
        /// The id of the saved document.
        doc: DocumentId,
    },
    /// A document was removed from its registry.
    Closed {
        /// The id of the closed document.
        doc: DocumentId,
    },
}

/// Twin-level, append-only, session-scoped change log.
///
/// **Not user-facing undo.** Per-document undo stays on
/// [`lunco_doc::DocumentHost`]. This resource records every document
/// lifecycle event for replay, audit, diagnostics, and cross-document
/// introspection.
///
/// The journal is unbounded today. When a session's journal outgrows
/// comfortable memory, rotation (to disk, to a ring buffer) will land
/// here; the public shape of reads won't change.
#[derive(Resource, Default)]
pub struct TwinJournal {
    entries: Vec<TwinEvent>,
}

impl TwinJournal {
    /// Append an event. Internal helper — prefer firing one of the
    /// lifecycle events and letting the journal's own subscribers pick
    /// it up. Exposed for tests and manual seeding.
    pub fn push(&mut self, kind: TwinEventKind) {
        self.entries.push(TwinEvent {
            at: Instant::now(),
            kind,
        });
    }

    /// Read all entries in chronological order.
    pub fn entries(&self) -> &[TwinEvent] {
        &self.entries
    }

    /// Total entry count.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the journal is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Last `n` entries, newest last. Convenience for UI / debug.
    pub fn tail(&self, n: usize) -> &[TwinEvent] {
        let start = self.entries.len().saturating_sub(n);
        &self.entries[start..]
    }

    /// All entries for a given document, newest last.
    pub fn filter_by_doc(&self, doc: DocumentId) -> impl Iterator<Item = &TwinEvent> {
        self.entries.iter().filter(move |e| event_doc(&e.kind) == doc)
    }
}

fn event_doc(kind: &TwinEventKind) -> DocumentId {
    match kind {
        TwinEventKind::Opened { doc }
        | TwinEventKind::Changed { doc }
        | TwinEventKind::Saved { doc }
        | TwinEventKind::Closed { doc } => *doc,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────────

/// Registers the [`TwinJournal`] resource and the four lifecycle-event
/// subscribers that append to it.
///
/// Domain crates (lunco-modelica, lunco-usd, …) add this plugin once
/// per app. Events from any domain's registry flow into one journal.
pub struct TwinJournalPlugin;

impl Plugin for TwinJournalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TwinJournal>()
            .add_observer(on_document_opened)
            .add_observer(on_document_changed)
            .add_observer(on_document_saved)
            .add_observer(on_document_closed);
    }
}

fn on_document_opened(trigger: On<DocumentOpened>, mut journal: ResMut<TwinJournal>) {
    journal.push(TwinEventKind::Opened { doc: trigger.event().doc });
}

fn on_document_changed(trigger: On<DocumentChanged>, mut journal: ResMut<TwinJournal>) {
    journal.push(TwinEventKind::Changed { doc: trigger.event().doc });
}

fn on_document_saved(trigger: On<DocumentSaved>, mut journal: ResMut<TwinJournal>) {
    journal.push(TwinEventKind::Saved { doc: trigger.event().doc });
}

fn on_document_closed(trigger: On<DocumentClosed>, mut journal: ResMut<TwinJournal>) {
    journal.push(TwinEventKind::Closed { doc: trigger.event().doc });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_push_and_tail() {
        let mut j = TwinJournal::default();
        let d1 = DocumentId::new(1);
        let d2 = DocumentId::new(2);
        j.push(TwinEventKind::Opened { doc: d1 });
        j.push(TwinEventKind::Changed { doc: d1 });
        j.push(TwinEventKind::Opened { doc: d2 });
        assert_eq!(j.len(), 3);
        assert_eq!(j.tail(2).len(), 2);
        assert_eq!(j.filter_by_doc(d1).count(), 2);
        assert_eq!(j.filter_by_doc(d2).count(), 1);
    }
}
