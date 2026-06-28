//! `UsdDocumentRegistry` — owns every live [`UsdDocument`] keyed by
//! [`lunco_doc::DocumentId`].
//!
//! Mirrors the small surface of `ModelicaDocumentRegistry` but stays
//! deliberately minimal in Phase 2: allocate / host / remove plus
//! pending-event rings that the commands plugin drains into the
//! `lunco-doc-bevy` notification triggers
//! ([`DocumentOpened`](lunco_doc_bevy::DocumentOpened),
//! [`lunco_doc_bevy::DocumentChanged`](lunco_doc_bevy::DocumentChanged),
//! [`DocumentClosed`](lunco_doc_bevy::DocumentClosed)).
//!
//! Entity linking, async-load reservation, and AST-staleness tracking
//! land in later phases when the UI / viewport actually needs them.

use std::collections::HashMap;

use bevy::prelude::Resource;
use lunco_doc::{DocumentHost, DocumentId, DocumentOrigin};

use crate::document::UsdDocument;

/// Registry of live USD documents.
///
/// Single source of truth for "which `.usda` files are open right
/// now." Commands, observers, and (later) browser sections read
/// through here; nobody else holds `DocumentHost<UsdDocument>`.
#[derive(Resource, Default)]
pub struct UsdDocumentRegistry {
    hosts: HashMap<DocumentId, DocumentHost<UsdDocument>>,
    /// Twin-journal handle, wired once the [`JournalResource`] appears (see
    /// `wire_usd_journal_handle`). When set, every host gets a
    /// [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder) so edits —
    /// including undo/redo — auto-record (A3). `None` in headless-without-
    /// journal builds → no recording.
    journal: Option<lunco_doc_bevy::JournalResource>,
    next_doc_id: u64,
    /// Docs that were just added — drained into
    /// [`DocumentOpened`](lunco_doc_bevy::DocumentOpened) triggers.
    pending_opened: Vec<DocumentId>,
    /// Docs whose generation just advanced — drained into
    /// [`lunco_doc_bevy::DocumentChanged`](lunco_doc_bevy::DocumentChanged).
    pending_changes: Vec<DocumentId>,
    /// Docs that were just dropped — drained into
    /// [`DocumentClosed`](lunco_doc_bevy::DocumentClosed).
    pending_closed: Vec<DocumentId>,
}

impl UsdDocumentRegistry {
    /// Allocate a new document with an explicit origin and install it.
    ///
    /// Use this from the OpenFile observer (origin = writable file)
    /// and from File→New (origin = untitled).
    pub fn allocate(&mut self, source: String, origin: DocumentOrigin) -> DocumentId {
        self.next_doc_id = self.next_doc_id.saturating_add(1);
        let id = DocumentId::new(self.next_doc_id);
        let doc = UsdDocument::with_origin(id, source, origin);
        self.hosts.insert(id, DocumentHost::new(doc));
        // A3: fit the journal recorder at creation (reactive to the open),
        // so the very first edit is journaled. No-op if the journal isn't
        // wired yet — `set_journal` retro-fits when it appears.
        self.attach_recorder(id);
        // One Opened (lifecycle) + one Changed (initial-source seed)
        // — same shape as `ModelicaDocumentRegistry::allocate_with_origin`.
        self.pending_opened.push(id);
        self.pending_changes.push(id);
        id
    }

    /// Wire the Twin-journal handle and retro-fit a recorder onto every
    /// existing host. Called once, reactively, the frame the
    /// [`JournalResource`](lunco_doc_bevy::JournalResource) first appears.
    /// Hosts opened afterwards get their recorder at [`allocate`](Self::allocate).
    pub fn set_journal(&mut self, journal: lunco_doc_bevy::JournalResource) {
        self.journal = Some(journal);
        let ids: Vec<_> = self.hosts.keys().copied().collect();
        for id in ids {
            self.attach_recorder(id);
        }
    }

    /// Attach a [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder) to
    /// `id`'s host when a journal is wired and the host lacks one. The A3
    /// auto-bridge seam — replaces all per-op recording.
    fn attach_recorder(&mut self, id: DocumentId) {
        if let Some(journal) = &self.journal {
            if let Some(host) = self.hosts.get_mut(&id) {
                if !host.has_recorder() {
                    lunco_doc_bevy::attach_journal_recorder(host, journal);
                }
            }
        }
    }

    /// Borrow the host for `doc`, or `None` if unknown.
    pub fn host(&self, doc: DocumentId) -> Option<&DocumentHost<UsdDocument>> {
        self.hosts.get(&doc)
    }

    /// Mutably borrow the host for `doc`. Direct mutations through
    /// this handle MUST be paired with [`mark_changed`](Self::mark_changed)
    /// — the registry can't see arbitrary uses of `&mut DocumentHost`.
    /// `host.apply(...)` callers should use the convenience
    /// [`apply`](Self::apply) wrapper which marks for them.
    pub fn host_mut(&mut self, doc: DocumentId) -> Option<&mut DocumentHost<UsdDocument>> {
        self.hosts.get_mut(&doc)
    }

    /// True iff `doc` is a USD document we own. Used by the
    /// [`SaveDocument`](lunco_doc_bevy::SaveDocument) observer to
    /// gate without false-positives on Modelica / SysML ids.
    pub fn contains(&self, doc: DocumentId) -> bool {
        self.hosts.contains_key(&doc)
    }

    /// Apply an op via the host and queue a Changed notification.
    /// Convenience wrapper so callers don't have to remember
    /// [`mark_changed`](Self::mark_changed).
    pub fn apply(
        &mut self,
        doc: DocumentId,
        op: <UsdDocument as lunco_doc::Document>::Op,
    ) -> Result<lunco_doc::Ack, lunco_doc::Reject> {
        let host = self
            .hosts
            .get_mut(&doc)
            .ok_or_else(|| lunco_doc::Reject::InvalidOp(format!("unknown doc {doc}")))?;
        let ack = host.apply(lunco_doc::Mutation::local(op))?;
        self.pending_changes.push(doc);
        Ok(ack)
    }

    /// Mark `doc` as changed without applying an op — for direct
    /// `host_mut` mutations (undo/redo loops, reload-from-disk).
    pub fn mark_changed(&mut self, doc: DocumentId) {
        if self.hosts.contains_key(&doc) {
            self.pending_changes.push(doc);
        }
    }

    /// Remove a document from the registry. Returns the dropped host
    /// (caller may inspect it before drop) or `None` if unknown.
    pub fn remove(&mut self, doc: DocumentId) -> Option<DocumentHost<UsdDocument>> {
        let host = self.hosts.remove(&doc)?;
        self.pending_closed.push(doc);
        Some(host)
    }

    /// Iterator over every live document id.
    pub fn ids(&self) -> impl Iterator<Item = DocumentId> + '_ {
        self.hosts.keys().copied()
    }

    /// Drain the pending-events rings. The commands plugin calls this
    /// each frame to fire `DocumentOpened` / `DocumentChanged` /
    /// `DocumentClosed` triggers.
    pub fn drain_pending(&mut self) -> PendingEvents {
        PendingEvents {
            opened: std::mem::take(&mut self.pending_opened),
            changed: std::mem::take(&mut self.pending_changes),
            closed: std::mem::take(&mut self.pending_closed),
        }
    }
}

/// Snapshot of pending lifecycle events drained from the registry.
pub struct PendingEvents {
    /// Docs newly added since the last drain.
    pub opened: Vec<DocumentId>,
    /// Docs whose generation advanced since the last drain.
    pub changed: Vec<DocumentId>,
    /// Docs removed since the last drain.
    pub closed: Vec<DocumentId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{LayerId, UsdOp};

    const TINY_USDA: &str = "#usda 1.0\ndef Xform \"World\" {}\n";

    #[test]
    fn allocate_emits_opened_and_changed() {
        let mut reg = UsdDocumentRegistry::default();
        let id = reg.allocate(
            TINY_USDA.to_string(),
            DocumentOrigin::writable_file("/tmp/scene.usda"),
        );
        let pending = reg.drain_pending();
        assert_eq!(pending.opened, vec![id]);
        assert_eq!(pending.changed, vec![id]);
        assert!(pending.closed.is_empty());
    }

    #[test]
    fn apply_marks_changed() {
        let mut reg = UsdDocumentRegistry::default();
        let id = reg.allocate(
            TINY_USDA.to_string(),
            DocumentOrigin::writable_file("/tmp/scene.usda"),
        );
        reg.drain_pending(); // clear initial events
        reg.apply(
            id,
            UsdOp::ReplaceSource {
                edit_target: LayerId::root(),
                text: "#usda 1.0\n".to_string(),
            },
        )
        .unwrap();
        let pending = reg.drain_pending();
        assert!(pending.opened.is_empty());
        assert_eq!(pending.changed, vec![id]);
    }

    #[test]
    fn remove_emits_closed() {
        let mut reg = UsdDocumentRegistry::default();
        let id = reg.allocate(
            TINY_USDA.to_string(),
            DocumentOrigin::writable_file("/tmp/scene.usda"),
        );
        reg.drain_pending();
        assert!(reg.remove(id).is_some());
        let pending = reg.drain_pending();
        assert_eq!(pending.closed, vec![id]);
        assert!(!reg.contains(id));
    }

    #[test]
    fn apply_to_unknown_id_errors() {
        let mut reg = UsdDocumentRegistry::default();
        let result = reg.apply(
            DocumentId::new(999),
            UsdOp::ReplaceSource {
                edit_target: LayerId::root(),
                text: "x".to_string(),
            },
        );
        assert!(matches!(result, Err(lunco_doc::Reject::InvalidOp(_))));
    }
}
