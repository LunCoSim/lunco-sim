//! # lunco-workspace
//!
//! The **Workspace** is LunCoSim's editor session — the VS Code-Workspace
//! analog. It holds what's open *right now in this window*:
//!
//! - the set of **Twins** the user has brought in (possibly from different
//!   folders on disk; no requirement that they share a parent directory);
//! - every **open Document**, including Untitled scratch buffers and loose
//!   files outside any Twin;
//! - which Twin / Document is currently active;
//! - which **Perspective** (layout preset) is active;
//! - a bounded **Recents** list so the user can re-open previous Twins /
//!   loose files quickly.
//!
//! This crate is **UI-free, ECS-free, headless-capable**. It knows nothing
//! about Bevy or egui — a Bevy `Resource` wrapper (or an observer surface)
//! belongs in a downstream crate (`lunco-workspace-bevy`, or directly in
//! `lunco-workbench` if the consumer prefers). That keeps headless CI and
//! API-only servers able to use the same Workspace type the UI uses.
//!
//! # Twin is a view, not a container
//!
//! Documents live in the Workspace — *all* of them, Twin-attached or not.
//! Twins don't own documents; they're *lenses* over the document list
//! (by path — [`lunco_twin::Twin::owns`] — or by explicit "context"
//! pinning for Untitled buffers). This keeps Untitled docs, loose files,
//! and Twin-owned files on one uniform surface and makes "Save-As moves
//! an Untitled into a Twin's folder" a zero-allocation flip of one field.
//!
//! # Minimal surface v1
//!
//! - [`Workspace`] — root type.
//! - [`DocumentEntry`] — one open Document's workspace-level metadata.
//! - [`TwinId`] — stable id the workspace assigns on Twin registration
//!   (Twin itself doesn't carry one; path alone is fragile if the Twin
//!   moves mid-session).
//! - [`Recents`] — bounded recents list.
//!
//! Deferred: session manifest on disk (`lunco-workspace.toml`), hot-exit
//! of Untitled buffers, external-change watchers. Those land in follow-up
//! milestones once this surface is wired into the UI.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod recents;

pub use recents::Recents;

pub use lunco_doc::{DocumentId, DocumentOrigin};
pub use lunco_storage::StorageHandle;
pub use lunco_twin::{DocumentKind, FileKind, Twin, TwinMode};

// ─────────────────────────────────────────────────────────────────────────────
// TwinId
// ─────────────────────────────────────────────────────────────────────────────

/// Stable identifier for a Twin registered in a Workspace.
///
/// The Workspace hands these out on [`Workspace::add_twin`]; the Twin
/// struct in `lunco-twin` does not carry an id of its own because, at
/// the crate level, it has no dependency on a session to belong to. A
/// Workspace using path-as-id would break as soon as the user renamed
/// a folder, so we assign a dense `u64` and let the path live on the
/// Twin itself.
///
/// `0` is reserved as "no Twin" (matches other id types across the
/// codebase that use `0` for the unassigned sentinel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct TwinId(u64);

impl TwinId {
    /// Construct from a raw `u64`.
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }
    /// Extract the raw `u64` for serialisation / API payloads.
    pub const fn raw(self) -> u64 {
        self.0
    }
    /// `true` for the default / unassigned sentinel (`0`).
    pub const fn is_unassigned(self) -> bool {
        self.0 == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentEntry
// ─────────────────────────────────────────────────────────────────────────────

/// Workspace-level metadata for one open Document.
///
/// The actual Document (AST, source, undo stack) lives in a domain
/// registry (e.g. `ModelicaDocumentRegistry`); the Workspace only
/// tracks what's open and how it relates to Twins. That separation
/// keeps the Workspace type free of per-format generics.
///
/// # Twin association
///
/// Two ways a DocumentEntry can be associated with a Twin:
///
/// 1. **By path** — a Persistent document whose `origin` path lies
///    under a registered Twin's folder. Resolved on demand via
///    [`Workspace::twin_for`].
/// 2. **By context pin** — an Untitled document explicitly pinned to
///    a Twin at creation ("New Model" from the Rover Twin's toolbar
///    creates an Untitled with `context_twin = Some(rover_id)`). This
///    survives until the doc is saved; on Save-As the context becomes
///    advisory and the by-path rule takes over.
///
/// Documents with neither are "loose" — shown under a Loose group in
/// the Twin Browser.
#[derive(Debug, Clone)]
pub struct DocumentEntry {
    /// Identity allocated by whichever registry owns the Document.
    pub id: DocumentId,
    /// Coarse classification (Modelica model, USD stage, …). Lets the
    /// Workspace route tab-opening to the right panel renderer without
    /// inspecting the document itself.
    pub kind: DocumentKind,
    /// Persistence state of the Document (Untitled vs File, writable).
    pub origin: DocumentOrigin,
    /// Optional pin to a Twin — used by Untitled docs to remember the
    /// context they were created in. `None` means "not pinned"; the
    /// Workspace can still associate a Persistent doc with a Twin by
    /// path lookup.
    pub context_twin: Option<TwinId>,
    /// Display title for the tab ("Rover.mo", "● Untitled-1", …).
    /// The Workspace doesn't enforce a format — consumers set and
    /// update this as they see fit.
    pub title: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace
// ─────────────────────────────────────────────────────────────────────────────

/// A LunCoSim editor session.
///
/// `Default` is an empty workspace — no Twins, no documents. Populate
/// via [`add_twin`](Self::add_twin) / [`add_document`](Self::add_document).
#[derive(Debug, Default)]
pub struct Workspace {
    /// Tracked Twins, keyed by the [`TwinId`]s this Workspace minted.
    /// Ordered by registration time so the Twin Browser renders them
    /// in the order the user added them unless the consumer sorts.
    twins: Vec<(TwinId, Twin)>,

    /// Monotonically-increasing counter for [`TwinId`] allocation.
    /// Starts at 1 so `TwinId(0)` can remain the "unassigned" sentinel.
    next_twin_id: u64,

    /// Open documents. Arbitrary order; consumers sort when rendering.
    documents: Vec<DocumentEntry>,

    /// Active Twin. Drives "Save to this Twin's folder" defaults and
    /// "New Document inherits this context". `None` when no Twin is
    /// open (the workspace has only loose documents).
    pub active_twin: Option<TwinId>,

    /// Active Document. Typically the document in the focused tab.
    pub active_document: Option<DocumentId>,

    /// Active Perspective (layout-preset identifier). Opaque string —
    /// the workbench keeps the registry; we just remember which one to
    /// activate on restore.
    pub active_perspective: Option<String>,

    /// Bounded recents list (Twin folders + loose files).
    pub recents: Recents,
}

impl Workspace {
    /// Construct an empty Workspace.
    pub fn new() -> Self {
        Self {
            next_twin_id: 1,
            ..Default::default()
        }
    }

    // ── Twins ───────────────────────────────────────────────────────

    /// Register a Twin and return its [`TwinId`]. Also bumps the
    /// recents list so subsequent launches can jump straight back in.
    pub fn add_twin(&mut self, twin: Twin) -> TwinId {
        // Ensure `next_twin_id` is safe even if the consumer constructed
        // a Workspace via `Default::default` (where `next_twin_id == 0`).
        if self.next_twin_id == 0 {
            self.next_twin_id = 1;
        }
        let id = TwinId(self.next_twin_id);
        self.next_twin_id += 1;
        self.recents.push_twin(twin.root.clone());
        self.twins.push((id, twin));
        if self.active_twin.is_none() {
            self.active_twin = Some(id);
        }
        id
    }

    /// Close a Twin. Documents rooted in that Twin's folder keep their
    /// entries — a closed Twin just drops the lens, not the docs.
    /// Re-opening the Twin re-associates by path.
    pub fn close_twin(&mut self, id: TwinId) {
        self.twins.retain(|(tid, _)| *tid != id);
        if self.active_twin == Some(id) {
            self.active_twin = self.twins.first().map(|(tid, _)| *tid);
        }
    }

    /// All registered Twins in insertion order.
    pub fn twins(&self) -> impl Iterator<Item = (TwinId, &Twin)> {
        self.twins.iter().map(|(id, t)| (*id, t))
    }

    /// Look up a Twin by id.
    pub fn twin(&self, id: TwinId) -> Option<&Twin> {
        self.twins
            .iter()
            .find(|(tid, _)| *tid == id)
            .map(|(_, t)| t)
    }

    // ── Documents ───────────────────────────────────────────────────

    /// Register an open Document. Does not attempt deduplication —
    /// callers are responsible for checking if the id is already open
    /// (typical pattern: consult [`document`](Self::document) first,
    /// focus the existing tab if found, otherwise add).
    pub fn add_document(&mut self, entry: DocumentEntry) {
        let has_path = matches!(&entry.origin, DocumentOrigin::File { .. });
        if has_path {
            if let Some(p) = entry.origin.canonical_path() {
                self.recents.push_loose(p.to_path_buf());
            }
        }
        // Document-entry assumption: caller already chose an id; no
        // conflict check here, same as how ModelicaDocumentRegistry
        // trusts its own allocator.
        self.documents.push(entry);
    }

    /// Close a Document by id. Returns the removed entry for callers
    /// that want to run cleanup (saving buffers, clearing selection,
    /// etc.) without an extra lookup.
    pub fn close_document(&mut self, id: DocumentId) -> Option<DocumentEntry> {
        let pos = self.documents.iter().position(|d| d.id == id)?;
        let entry = self.documents.remove(pos);
        if self.active_document == Some(id) {
            self.active_document = self.documents.first().map(|d| d.id);
        }
        Some(entry)
    }

    /// All open Documents.
    pub fn documents(&self) -> &[DocumentEntry] {
        &self.documents
    }

    /// Mutable access — callers update `title` (dirty marker) and
    /// `origin` (after Save-As).
    pub fn documents_mut(&mut self) -> &mut [DocumentEntry] {
        &mut self.documents
    }

    /// Look up a Document by id.
    pub fn document(&self, id: DocumentId) -> Option<&DocumentEntry> {
        self.documents.iter().find(|d| d.id == id)
    }

    /// Mutable lookup by id.
    pub fn document_mut(&mut self, id: DocumentId) -> Option<&mut DocumentEntry> {
        self.documents.iter_mut().find(|d| d.id == id)
    }

    // ── Twin-document association ──────────────────────────────────

    /// Resolve which Twin (if any) "owns" a document entry.
    ///
    /// Ordering: a path-based match on any registered Twin always wins
    /// over a context pin — once a document has been saved into a
    /// Twin's folder, the pin is stale. The pin is consulted only for
    /// Untitled documents (which have no path to match).
    pub fn twin_for(&self, entry: &DocumentEntry) -> Option<TwinId> {
        if let DocumentOrigin::File { path, .. } = &entry.origin {
            let handle = StorageHandle::File(path.clone());
            // Deepest matching Twin wins (sub-Twins are preferred over
            // the enclosing Twin), matching Twin::find_owning's rule.
            for (id, t) in &self.twins {
                if t.find_owning(&handle).is_some() {
                    return Some(*id);
                }
            }
            None
        } else {
            entry.context_twin
        }
    }

    /// Documents this Twin claims, per [`twin_for`](Self::twin_for).
    /// Iteration order follows the documents list.
    pub fn documents_in_twin(&self, id: TwinId) -> impl Iterator<Item = &DocumentEntry> {
        self.documents.iter().filter(move |d| self.twin_for(d) == Some(id))
    }

    /// Documents not claimed by any Twin. Shown under the "Loose"
    /// group in the Twin Browser.
    pub fn loose_documents(&self) -> impl Iterator<Item = &DocumentEntry> {
        self.documents.iter().filter(|d| self.twin_for(d).is_none())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_twin::TwinMode;
    use std::path::Path;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn load_twin(path: &Path) -> Twin {
        match TwinMode::open(path).unwrap() {
            TwinMode::Twin(t) | TwinMode::Folder(t) => t,
            TwinMode::Orphan(_) => panic!("expected a folder"),
        }
    }

    #[test]
    fn new_is_empty() {
        let ws = Workspace::new();
        assert_eq!(ws.twins().count(), 0);
        assert_eq!(ws.documents().len(), 0);
        assert!(ws.active_twin.is_none());
    }

    #[test]
    fn twin_id_assignment_is_sequential_and_nonzero() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a.mo"), "model A end A;");
        let twin = load_twin(tmp.path());

        let mut ws = Workspace::new();
        let a = ws.add_twin(twin);
        assert!(!a.is_unassigned());
        assert_eq!(a.raw(), 1);

        // Adding the same Twin again yields a new id — Workspace does
        // not dedupe by root path (consumers may want two views).
        let twin2 = load_twin(tmp.path());
        let b = ws.add_twin(twin2);
        assert_eq!(b.raw(), 2);
    }

    #[test]
    fn adding_twin_sets_active_when_first() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("a.mo"), "");
        let twin = load_twin(tmp.path());

        let mut ws = Workspace::new();
        assert!(ws.active_twin.is_none());
        let id = ws.add_twin(twin);
        assert_eq!(ws.active_twin, Some(id));
    }

    #[test]
    fn twin_for_persistent_doc_finds_enclosing_twin() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(&root.join("twin.toml"), r#"name = "t"
version = "0.1.0"
"#);
        let model_path = root.join("Rover.mo");
        write(&model_path, "model Rover end Rover;");

        let twin = load_twin(root);
        let mut ws = Workspace::new();
        let tid = ws.add_twin(twin);

        ws.add_document(DocumentEntry {
            id: DocumentId::new(1),
            kind: DocumentKind::Modelica,
            origin: DocumentOrigin::writable_file(&model_path),
            context_twin: None,
            title: "Rover.mo".into(),
        });

        assert_eq!(ws.twin_for(&ws.documents()[0]), Some(tid));
        assert_eq!(ws.documents_in_twin(tid).count(), 1);
        assert_eq!(ws.loose_documents().count(), 0);
    }

    #[test]
    fn twin_for_untitled_uses_context_pin() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("x.mo"), "");
        let twin = load_twin(tmp.path());

        let mut ws = Workspace::new();
        let tid = ws.add_twin(twin);

        ws.add_document(DocumentEntry {
            id: DocumentId::new(10),
            kind: DocumentKind::Modelica,
            origin: DocumentOrigin::untitled("Untitled-1"),
            context_twin: Some(tid),
            title: "● Untitled-1".into(),
        });

        // Untitled with context pin is claimed by that Twin even though
        // it has no filesystem path to match against.
        assert_eq!(ws.twin_for(&ws.documents()[0]), Some(tid));
    }

    #[test]
    fn twin_for_path_match_trumps_pin() {
        // Document is on disk inside Twin A but pinned to Twin B. The
        // path match wins — the pin only matters when a path is absent.
        let tmp = tempfile::tempdir().unwrap();
        let a_root = tmp.path().join("a");
        let b_root = tmp.path().join("b");
        write(&a_root.join("twin.toml"), "name=\"a\"\nversion=\"0.1.0\"\n");
        write(&b_root.join("twin.toml"), "name=\"b\"\nversion=\"0.1.0\"\n");
        let model = a_root.join("shared.mo");
        write(&model, "");

        let twin_a = load_twin(&a_root);
        let twin_b = load_twin(&b_root);

        let mut ws = Workspace::new();
        let a = ws.add_twin(twin_a);
        let b = ws.add_twin(twin_b);

        ws.add_document(DocumentEntry {
            id: DocumentId::new(1),
            kind: DocumentKind::Modelica,
            origin: DocumentOrigin::writable_file(&model),
            context_twin: Some(b),
            title: "shared.mo".into(),
        });
        assert_eq!(ws.twin_for(&ws.documents()[0]), Some(a));
        let _ = b; // silence unused in release
    }

    #[test]
    fn close_twin_orphans_docs_but_keeps_them_open() {
        let tmp = tempfile::tempdir().unwrap();
        write(&tmp.path().join("twin.toml"), "name=\"t\"\nversion=\"0.1.0\"\n");
        let model = tmp.path().join("m.mo");
        write(&model, "");

        let twin = load_twin(tmp.path());
        let mut ws = Workspace::new();
        let tid = ws.add_twin(twin);
        ws.add_document(DocumentEntry {
            id: DocumentId::new(1),
            kind: DocumentKind::Modelica,
            origin: DocumentOrigin::writable_file(&model),
            context_twin: None,
            title: "m.mo".into(),
        });
        assert_eq!(ws.documents_in_twin(tid).count(), 1);

        ws.close_twin(tid);
        assert_eq!(ws.twins().count(), 0);
        // Doc still open, now loose (no Twin to resolve it).
        assert_eq!(ws.documents().len(), 1);
        assert_eq!(ws.loose_documents().count(), 1);
    }

    #[test]
    fn close_document_clears_active_pointer() {
        let mut ws = Workspace::new();
        let id = DocumentId::new(42);
        ws.add_document(DocumentEntry {
            id,
            kind: DocumentKind::Modelica,
            origin: DocumentOrigin::untitled("U"),
            context_twin: None,
            title: "U".into(),
        });
        ws.active_document = Some(id);
        let closed = ws.close_document(id);
        assert!(closed.is_some());
        assert_eq!(ws.active_document, None);
    }
}

