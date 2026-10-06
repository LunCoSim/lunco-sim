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
//! This crate is **UI-free and headless-capable**. It depends only on the
//! bevy ECS/app substrate (`Resource`/`Event`/`Plugin`/observers — no
//! render/winit/egui), so the same Workspace + its [`session`] binding
//! ([`WorkspaceResource`], [`WorkspacePlugin`], the add/close events) run
//! identically in the windowed editor and a `--no-ui` API-only server.
//! Recents *persistence* (config-dir I/O) is left to the consumer.
//!
//! # Folder views and runtime source lifetime
//!
//! Documents live in the Workspace — *all* of them, Twin-attached or not.
//! Twin folders provide the authoring and display lens through [`Workspace::twin_for`].
//! Each document also records its exact runtime owner at source admission.
//! Saving or moving a file changes its authored origin without transferring
//! resident source to another session. Active scope and retirement use the
//! stored runtime owner; a clean explicit reopen can admit newly read source.
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

pub mod journal_persistence;
pub mod open;
pub mod recents;
pub mod rename;
pub mod session;

pub use open::{ResetTwinSetting, SetTwinSetting, TwinSettingInput};
pub use recents::Recents;
pub use session::{
    DocumentClosed, DocumentOpened, FileRenamed, RegisterDocument, TwinAdded, TwinClosed,
    UnregisterDocument, WorkspacePlugin, WorkspaceResource,
};

pub use lunco_doc::{DocumentId, DocumentOrigin};
pub use lunco_storage::StorageHandle;
pub use lunco_twin::{DocumentKindId, FileKind, Twin, TwinMode, TwinSettingValue};

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
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
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

/// Scope admitted by the authenticated scene transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReplicationScope {
    Application,
    Twin(TwinId),
}

/// Exact remote scene mount and transport lifetime; host IDs are never local IDs.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReplicatedSceneOwner {
    pub connection: bevy::prelude::Entity,
    pub host_twin: TwinId,
    pub authority: String,
    pub root: std::path::PathBuf,
    pub owns_mount: bool,
}

/// Authenticated owner captured at replicated input admission.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReplicationOwner {
    Application { connection: bevy::prelude::Entity },
    Twin { scene: ReplicatedSceneOwner },
}
impl ReplicationOwner {
    pub fn connection(&self) -> bevy::prelude::Entity {
        match self {
            Self::Application { connection } => *connection,
            Self::Twin { scene } => scene.connection,
        }
    }
    pub fn scope(&self) -> ReplicationScope {
        match self {
            Self::Application { .. } => ReplicationScope::Application,
            Self::Twin { scene } => ReplicationScope::Twin(scene.host_twin),
        }
    }
    pub fn is_in_active_scope(
        &self,
        workspace: Option<&Workspace>,
        current: Option<&Self>,
    ) -> bool {
        current == Some(self)
            && match self {
                Self::Application { .. } => {
                    workspace.is_none_or(|workspace| workspace.active_twin.is_none())
                }
                Self::Twin { .. } => true,
            }
    }
}

/// Authoritative lifetime of document work, independent of its display grouping.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum DocumentRuntimeOwner {
    #[default]
    Application,
    LocalTwin(TwinId),
    Replicated(ReplicationOwner),
}
impl DocumentRuntimeOwner {
    pub fn local_twin(&self) -> Option<TwinId> {
        match self {
            Self::LocalTwin(twin) => Some(*twin),
            _ => None,
        }
    }

    /// Validate an admitted lifetime without reading source bytes or paths.
    pub fn is_current(
        &self,
        workspace: Option<&Workspace>,
        replication: Option<&ReplicationOwner>,
    ) -> bool {
        match self {
            Self::Application => true,
            Self::LocalTwin(id) => workspace.is_some_and(|workspace| workspace.twin(*id).is_some()),
            Self::Replicated(owner) => replication == Some(owner),
        }
    }
}

/// Immutable ownership candidates captured before a file load leaves its caller.
/// The loader resolves native identity through storage away from the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDocumentAdmission {
    local_roots: Vec<(TwinId, std::path::PathBuf)>,
    replication: Option<ReplicationOwner>,
}

/// Canonical source identity and exact runtime lifetime resolved by its loader.
#[derive(Debug)]
pub struct ResolvedFileDocument {
    pub path: std::path::PathBuf,
    pub runtime: DocumentRuntimeOwner,
}

impl FileDocumentAdmission {
    pub fn capture(workspace: Option<&Workspace>, replication: Option<&ReplicationOwner>) -> Self {
        fn collect_roots(twin: &Twin, id: TwinId, roots: &mut Vec<(TwinId, std::path::PathBuf)>) {
            roots.push((id, twin.root.clone()));
            for child in twin.children() {
                collect_roots(child, id, roots);
            }
        }
        let mut local_roots = Vec::new();
        if let Some(workspace) = workspace {
            for (id, twin) in workspace.twins() {
                collect_roots(twin, id, &mut local_roots);
            }
        }
        Self {
            local_roots,
            replication: replication.cloned(),
        }
    }

    /// Run on the existing storage/loader worker, before reading the source.
    /// Unresolved identity fails admission instead of becoming application work.
    pub fn resolve(self, path: &std::path::Path) -> Result<ResolvedFileDocument, String> {
        let path = lunco_storage::canonicalize_file_path(path)
            .map_err(|error| format!("file identity could not be resolved: {error}"))?;
        for (id, root) in self.local_roots {
            #[cfg(target_arch = "wasm32")]
            let root = lunco_storage::canonicalize_file_path(&root).map_err(|error| {
                format!("local Twin {id:?} root identity could not be resolved: {error}")
            })?;
            if path.starts_with(root) {
                return Ok(ResolvedFileDocument {
                    path,
                    runtime: DocumentRuntimeOwner::LocalTwin(id),
                });
            }
        }
        if let Some(ReplicationOwner::Twin { scene }) = self.replication {
            #[cfg(not(target_arch = "wasm32"))]
            let root = &scene.root;
            #[cfg(target_arch = "wasm32")]
            let root = lunco_storage::canonicalize_file_path(&scene.root).map_err(|error| {
                format!("replicated scene root identity could not be resolved: {error}")
            })?;
            if path.starts_with(root) {
                return Ok(ResolvedFileDocument {
                    path,
                    runtime: DocumentRuntimeOwner::Replicated(ReplicationOwner::Twin { scene }),
                });
            }
        }
        Ok(ResolvedFileDocument {
            path,
            runtime: DocumentRuntimeOwner::Application,
        })
    }
    /// Read through the backend that owns the captured file identity. Mounted
    /// browser sources use OPFS; private unmounted editor keys use WebStorage.
    pub async fn read(
        self,
        path: &std::path::Path,
    ) -> Result<(ResolvedFileDocument, Vec<u8>), String> {
        self.read_contents(path, None).await
    }

    /// Read admitted bytes under the caller's source budget. The captured
    /// identity and backend selection are the same as an ordinary file read.
    pub async fn read_bounded(
        self,
        path: &std::path::Path,
        max_bytes: usize,
    ) -> Result<(ResolvedFileDocument, Vec<u8>), String> {
        self.read_contents(path, Some(max_bytes)).await
    }

    async fn read_contents(
        self,
        path: &std::path::Path,
        max_bytes: Option<usize>,
    ) -> Result<(ResolvedFileDocument, Vec<u8>), String> {
        let resolved = self.resolve(path)?;
        #[cfg(not(target_arch = "wasm32"))]
        let bytes = {
            use lunco_storage::Storage;
            let storage = lunco_storage::FileStorage::new();
            let handle = StorageHandle::File(resolved.path.clone());
            match max_bytes {
                Some(limit) => storage.read_bounded(&handle, limit).await,
                None => storage.read(&handle).await,
            }
            .map_err(|error| format!("read failed `{}`: {error}", resolved.path.display()))?
        };
        #[cfg(target_arch = "wasm32")]
        let mut resolved = resolved;
        #[cfg(target_arch = "wasm32")]
        let bytes = if resolved.runtime == DocumentRuntimeOwner::Application {
            resolved.path = path.to_path_buf();
            use lunco_storage::Storage;
            let storage = lunco_storage::WebStorage::new();
            let handle = StorageHandle::File(path.to_path_buf());
            match max_bytes {
                Some(limit) => storage.read_bounded(&handle, limit).await,
                None => storage.read(&handle).await,
            }
            .map_err(|error| format!("read failed `{}`: {error}", path.display()))?
        } else {
            let storage = lunco_storage::OpfsStorage::new();
            let handle = StorageHandle::File(resolved.path.clone());
            match max_bytes {
                Some(limit) => storage.read_bounded(&handle, limit).await,
                None => storage.read(&handle).await,
            }
            .map_err(|error| format!("read failed `{}`: {error}", resolved.path.display()))?
        };
        Ok((resolved, bytes))
    }
}

/// Document and exact runtime owner captured when work is admitted.
#[derive(bevy::prelude::Component, Debug, Clone, PartialEq, Eq)]
pub struct PinnedDocumentRuntimeOwner {
    pub document: DocumentId,
    pub runtime: DocumentRuntimeOwner,
}
impl PinnedDocumentRuntimeOwner {
    pub fn for_document(
        document: DocumentId,
        workspace: Option<&Workspace>,
    ) -> Result<Self, String> {
        let runtime = match workspace {
            Some(workspace) => {
                let entry = workspace.document(document).ok_or_else(|| {
                    format!("document {document} is not registered in the workspace")
                })?;
                let runtime = workspace.runtime_owner_for(entry);
                if runtime
                    .local_twin()
                    .is_some_and(|id| workspace.twin(id).is_none())
                {
                    return Err(format!("document {document} has a closed Twin context"));
                }
                runtime
            }
            None => DocumentRuntimeOwner::Application,
        };
        Ok(Self { document, runtime })
    }
    pub fn is_current(
        &self,
        workspace: Option<&Workspace>,
        replication: Option<&ReplicationOwner>,
    ) -> bool {
        let registered = workspace.map_or(
            self.runtime == DocumentRuntimeOwner::Application,
            |workspace| {
                workspace
                    .document(self.document)
                    .is_some_and(|entry| workspace.runtime_owner_for(entry) == self.runtime)
            },
        );
        registered && self.runtime.is_current(workspace, replication)
    }
    pub fn is_in_active_scope(
        &self,
        workspace: Option<&Workspace>,
        replication: Option<&ReplicationOwner>,
    ) -> bool {
        self.is_current(workspace, replication)
            && match &self.runtime {
                DocumentRuntimeOwner::Application => {
                    workspace.is_none_or(|workspace| workspace.active_twin.is_none())
                        && !matches!(replication, Some(ReplicationOwner::Twin { .. }))
                }
                DocumentRuntimeOwner::LocalTwin(twin) => {
                    workspace.is_some_and(|workspace| workspace.active_twin == Some(*twin))
                        && !matches!(replication, Some(ReplicationOwner::Twin { .. }))
                }
                DocumentRuntimeOwner::Replicated(owner) => {
                    owner.is_in_active_scope(workspace, replication)
                }
            }
    }
    /// Explicit library sources retain their application owner; authored sibling
    /// overlays must share this exact admitted document runtime.
    pub fn shares_runtime_with(
        &self,
        document: DocumentId,
        workspace: Option<&Workspace>,
        replication: Option<&ReplicationOwner>,
    ) -> bool {
        self.is_current(workspace, replication)
            && Self::for_document(document, workspace)
                .is_ok_and(|other| other.runtime == self.runtime)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DocumentEntry
// ─────────────────────────────────────────────────────────────────────────────

/// Workspace-level metadata for one open Document.
///
/// The actual Document (AST, source, undo stack) lives in a domain
/// registry (e.g. `DocumentRegistry<ModelicaDocument>`); the Workspace only
/// tracks what's open and how it relates to Twins. That separation
/// keeps the Workspace type free of per-format generics.
///
/// # Twin association
///
/// Folder association is resolved by [`Workspace::twin_for`] for display and
/// authoring. Runtime source ownership is captured independently at admission;
/// saving or reopening a folder does not transfer resident source into a new
/// session. Only a clean explicit file reopen can admit newly read source.
#[derive(Debug, Clone)]
pub struct DocumentEntry {
    /// Identity allocated by whichever registry owns the Document.
    pub id: DocumentId,
    /// Coarse classification (Modelica model, USD stage, …). Lets the
    /// Workspace route tab-opening to the right panel renderer without
    /// inspecting the document itself.
    pub kind: DocumentKindId,
    /// Persistence state of the Document (Untitled vs File, writable).
    pub origin: DocumentOrigin,
    /// Exact runtime context captured at source admission, independent of the
    /// current folder lens. A clean explicit file reopen can admit new source.
    pub runtime_context: DocumentRuntimeOwner,
    /// Display title for the tab ("Rover.mo", "● Untitled-1", …).
    /// The Workspace doesn't enforce a format — consumers set and
    /// update this as they see fit.
    pub title: String,
    /// Whether the owning document has unsaved changes. Domain registries
    /// update this mirror on `DocumentChanged` and `DocumentSaved`.
    pub dirty: bool,
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
    /// Reopening restores the folder lens; retired runtime source pins remain retired.
    pub fn close_twin(&mut self, id: TwinId) {
        let active_document_belongs_to_closed_twin = self
            .active_document
            .and_then(|doc| self.document(doc))
            .is_some_and(|entry| {
                self.runtime_owner_for(entry) == DocumentRuntimeOwner::LocalTwin(id)
            });
        self.twins.retain(|(tid, _)| *tid != id);
        if self.active_twin == Some(id) {
            self.active_twin = self.twins.first().map(|(tid, _)| *tid);
        }
        if active_document_belongs_to_closed_twin {
            self.active_document = self
                .documents
                .iter()
                .find(|entry| self.document_is_in_active_scope(entry))
                .map(|entry| entry.id);
        }
    }

    /// Whether a document belongs to the active Twin, or to the loose-file
    /// scope when no Twin is active. Browser and session readers use this
    /// predicate so a closed/replaced Twin cannot remain visible through the
    /// Workspace's intentionally global document registry.
    pub fn document_is_in_active_scope(&self, entry: &DocumentEntry) -> bool {
        match self.active_twin {
            Some(active) => {
                self.runtime_owner_for(entry) == DocumentRuntimeOwner::LocalTwin(active)
            }
            None => self.runtime_owner_for(entry) == DocumentRuntimeOwner::Application,
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

    /// Mutable look up. Required by callers that need to call
    /// [`Twin::reload`] after on-disk changes (rename, save-as-new-file)
    /// without forcing a `close_twin` + `add_twin` round-trip (which
    /// would change the `TwinId` and break any state keyed off it —
    /// experiments, cosim entities, document context bindings).
    pub fn twin_mut(&mut self, id: TwinId) -> Option<&mut Twin> {
        self.twins
            .iter_mut()
            .find(|(tid, _)| *tid == id)
            .map(|(_, t)| t)
    }

    // ── Documents ───────────────────────────────────────────────────

    /// Register or update explicitly admitted document metadata by exact id.
    /// The producer validates any source replacement before passing its owner.
    pub fn add_document(&mut self, entry: DocumentEntry) {
        let has_path = matches!(&entry.origin, DocumentOrigin::File { .. });
        if has_path {
            if let Some(p) = entry.origin.canonical_path() {
                self.recents.push_loose(p.to_path_buf());
            }
        }
        if let Some(resident) = self
            .documents
            .iter_mut()
            .find(|resident| resident.id == entry.id)
        {
            *resident = entry;
        } else {
            self.documents.push(entry);
        }
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

    /// Mutable access — callers update `title`, `dirty`, and `origin`
    /// (after Save-As).
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
    /// Display and authoring use a path-based folder lens for File documents
    /// and the creation pin for pathless documents. Runtime lifetime remains
    /// independent and is read through `runtime_owner_for`.
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
            entry.runtime_context.local_twin()
        }
    }

    /// Snapshot the visible creation context at synchronous document admission.
    /// An admitted remote scene overrides the retained local browser lens.
    pub fn new_document_runtime_owner(
        &self,
        replication: Option<&ReplicationOwner>,
    ) -> DocumentRuntimeOwner {
        match replication {
            Some(owner @ ReplicationOwner::Twin { .. }) => {
                DocumentRuntimeOwner::Replicated(owner.clone())
            }
            _ => self.active_twin.map_or(
                DocumentRuntimeOwner::Application,
                DocumentRuntimeOwner::LocalTwin,
            ),
        }
    }

    /// Read the admitted source lifetime. Folder changes affect `twin_for`, not
    /// already admitted work; a file loader explicitly rebinds clean disk source.
    pub fn runtime_owner_for(&self, entry: &DocumentEntry) -> DocumentRuntimeOwner {
        entry.runtime_context.clone()
    }

    /// Documents this Twin claims, per [`twin_for`](Self::twin_for).
    /// Iteration order follows the documents list.
    pub fn documents_in_twin(&self, id: TwinId) -> impl Iterator<Item = &DocumentEntry> {
        self.documents
            .iter()
            .filter(move |d| self.twin_for(d) == Some(id))
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
            lunco_storage::ensure_directory_sync(parent).unwrap();
        }
        lunco_storage::write_file_sync(path, contents.as_bytes()).unwrap();
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
    fn bounded_file_admission_preserves_runtime_identity_and_read_errors() {
        let root = tempfile::tempdir().expect("owned source root");
        let source = root.path().join("source # % Мир.mo");
        write(&source, "abc");
        let mut workspace = Workspace::new();
        let twin = workspace.add_twin(load_twin(root.path()));
        let admission = FileDocumentAdmission::capture(Some(&workspace), None);
        let (resolved, bytes) =
            bevy::tasks::futures_lite::future::block_on(admission.clone().read_bounded(&source, 3))
                .expect("exact budget");
        assert_eq!(resolved.runtime, DocumentRuntimeOwner::LocalTwin(twin));
        assert_eq!(
            resolved.path,
            lunco_storage::canonicalize_file_path(&source).expect("source identity")
        );
        assert_eq!(bytes, b"abc");
        let error =
            bevy::tasks::futures_lite::future::block_on(admission.clone().read_bounded(&source, 2))
                .expect_err("oversized source");
        assert!(
            error.contains("read failed") && error.contains("2-byte read limit"),
            "{error}"
        );
        assert!(
            bevy::tasks::futures_lite::future::block_on(
                admission.read_bounded(&root.path().join("missing.mo"), 3)
            )
            .is_err()
        );
    }

    #[test]
    fn file_admission_pins_canonical_owner_and_rejects_retired_lifetimes() {
        let temp = tempfile::tempdir().expect("temporary roots");
        let local_root = temp.path().join("local");
        let remote_root = temp.path().join("remote # % Мир");
        let local_path = local_root.join("local.mo");
        let remote_path = remote_root.join("source # %.mo");
        let loose_path = temp.path().join("loose.mo");
        write(&local_path, "model Local end Local;");
        write(&remote_path, "model Remote end Remote;");
        write(&loose_path, "model Loose end Loose;");
        let mut workspace = Workspace::new();
        let local = workspace.add_twin(load_twin(&local_root));
        let remote = ReplicationOwner::Twin {
            scene: ReplicatedSceneOwner {
                connection: bevy::prelude::Entity::from_bits(11),
                host_twin: TwinId::new(7),
                authority: "remote-owner".into(),
                root: lunco_storage::canonicalize_file_path(&remote_root)
                    .expect("admitted remote root"),
                owns_mount: true,
            },
        };
        let unrelated_root = temp.path().join("unrelated");
        write(
            &unrelated_root.join("twin.toml"),
            "name=\"unrelated\"\nversion=\"0.1.0\"\n",
        );
        workspace.add_twin(load_twin(&unrelated_root));
        let snapshot = FileDocumentAdmission::capture(Some(&workspace), Some(&remote));
        std::fs::remove_dir_all(&unrelated_root).expect("retire unrelated storage fixture");
        assert_eq!(
            snapshot
                .clone()
                .resolve(&loose_path)
                .expect("unrelated deleted root does not block loose source")
                .runtime,
            DocumentRuntimeOwner::Application
        );
        let admitted = snapshot
            .clone()
            .resolve(&remote_path)
            .expect("canonical remote file");
        assert_eq!(
            admitted.path,
            lunco_storage::canonicalize_file_path(&remote_path).expect("canonical source")
        );
        assert_eq!(
            admitted.runtime,
            DocumentRuntimeOwner::Replicated(remote.clone())
        );
        assert!(admitted.runtime.is_current(Some(&workspace), Some(&remote)));
        assert!(!admitted.runtime.is_current(Some(&workspace), None));
        let mut replacement = remote.clone();
        let ReplicationOwner::Twin { scene } = &mut replacement else {
            panic!("fixture owner");
        };
        scene.connection = bevy::prelude::Entity::from_bits(12);
        assert!(
            !admitted
                .runtime
                .is_current(Some(&workspace), Some(&replacement))
        );
        let doc = DocumentId::new(100);
        workspace.add_document(DocumentEntry {
            id: doc,
            kind: DocumentKindId::new("modelica"),
            origin: DocumentOrigin::File {
                path: admitted.path,
                writable: true,
            },
            runtime_context: admitted.runtime,
            title: "Remote".into(),
            dirty: false,
        });
        let source =
            PinnedDocumentRuntimeOwner::for_document(doc, Some(&workspace)).expect("remote source");
        assert_eq!(
            source.runtime,
            DocumentRuntimeOwner::Replicated(remote.clone())
        );
        assert!(!source.is_current(Some(&workspace), Some(&replacement)));
        assert_eq!(
            snapshot
                .clone()
                .resolve(&loose_path)
                .expect("loose source")
                .runtime,
            DocumentRuntimeOwner::Application
        );
        assert!(
            snapshot
                .clone()
                .resolve(&remote_root.join("missing.mo"))
                .expect_err("missing identity must reject")
                .contains("identity could not be resolved")
        );

        workspace.close_twin(local);
        let replacement_local = workspace.add_twin(load_twin(&local_root));
        let admitted_local = snapshot.resolve(&local_path).expect("delayed local source");
        assert_eq!(
            admitted_local.runtime,
            DocumentRuntimeOwner::LocalTwin(local)
        );
        assert_ne!(local, replacement_local);
        assert!(
            !admitted_local
                .runtime
                .is_current(Some(&workspace), Some(&remote))
        );

        workspace.add_document(DocumentEntry {
            id: DocumentId::new(101),
            kind: DocumentKindId::new("modelica"),
            origin: DocumentOrigin::writable_file(&local_path),
            runtime_context: admitted_local.runtime,
            title: "Retired".into(),
            dirty: true,
        });
        workspace.active_twin = Some(replacement_local);
        assert!(
            !workspace.document_is_in_active_scope(
                workspace
                    .document(DocumentId::new(101))
                    .expect("retained source")
            )
        );
        let remote_as_local = workspace.add_twin(load_twin(&remote_root));
        let preferred = FileDocumentAdmission::capture(Some(&workspace), Some(&remote))
            .resolve(&remote_path)
            .expect("local path precedence");
        assert_eq!(
            preferred.runtime,
            DocumentRuntimeOwner::LocalTwin(remote_as_local)
        );
        assert_eq!(
            workspace.runtime_owner_for(workspace.document(doc).expect("remote entry")),
            DocumentRuntimeOwner::Replicated(remote)
        );
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
        write(
            &root.join("twin.toml"),
            r#"name = "t"
version = "0.1.0"
"#,
        );
        let model_path = root.join("Rover.mo");
        write(&model_path, "model Rover end Rover;");

        let twin = load_twin(root);
        let mut ws = Workspace::new();
        let tid = ws.add_twin(twin);

        ws.add_document(DocumentEntry {
            id: DocumentId::new(1),
            kind: DocumentKindId::new("modelica"),
            origin: DocumentOrigin::writable_file(&model_path),
            runtime_context: DocumentRuntimeOwner::Application,
            title: "Rover.mo".into(),
            dirty: false,
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
            kind: DocumentKindId::new("modelica"),
            origin: DocumentOrigin::untitled("Untitled-1"),
            runtime_context: DocumentRuntimeOwner::LocalTwin(tid),
            title: "● Untitled-1".into(),
            dirty: true,
        });

        // Untitled with context pin is claimed by that Twin even though
        // it has no filesystem path to match against.
        assert_eq!(ws.twin_for(&ws.documents()[0]), Some(tid));
    }

    #[test]
    fn active_scope_excludes_documents_from_another_twin() {
        let tmp = tempfile::tempdir().unwrap();
        let a_root = tmp.path().join("a");
        let b_root = tmp.path().join("b");
        write(&a_root.join("twin.toml"), "name=\"a\"\nversion=\"0.1.0\"\n");
        write(&b_root.join("twin.toml"), "name=\"b\"\nversion=\"0.1.0\"\n");
        let a_file = a_root.join("a.mo");
        let b_file = b_root.join("b.mo");
        write(&a_file, "");
        write(&b_file, "");

        let mut ws = Workspace::new();
        let a = ws.add_twin(load_twin(&a_root));
        let b = ws.add_twin(load_twin(&b_root));
        ws.add_document(DocumentEntry {
            id: DocumentId::new(1),
            kind: DocumentKindId::new("modelica"),
            origin: DocumentOrigin::writable_file(&a_file),
            runtime_context: DocumentRuntimeOwner::LocalTwin(a),
            title: "a.mo".into(),
            dirty: false,
        });
        ws.add_document(DocumentEntry {
            id: DocumentId::new(2),
            kind: DocumentKindId::new("modelica"),
            origin: DocumentOrigin::writable_file(&b_file),
            runtime_context: DocumentRuntimeOwner::LocalTwin(b),
            title: "b.mo".into(),
            dirty: false,
        });

        assert_eq!(ws.active_twin, Some(a));
        assert!(ws.document_is_in_active_scope(&ws.documents()[0]));
        assert!(!ws.document_is_in_active_scope(&ws.documents()[1]));

        ws.active_twin = Some(b);
        assert!(!ws.document_is_in_active_scope(&ws.documents()[0]));
        assert!(ws.document_is_in_active_scope(&ws.documents()[1]));
    }

    #[test]
    fn closing_twin_drops_its_selected_document_in_any_folder_lens() {
        let tmp = tempfile::tempdir().unwrap();
        let a_root = tmp.path().join("a");
        let b_root = tmp.path().join("b");
        write(&a_root.join("twin.toml"), "name=\"a\"\nversion=\"0.1.0\"\n");
        write(&b_root.join("twin.toml"), "name=\"b\"\nversion=\"0.1.0\"\n");
        let a_file = a_root.join("a.mo");
        let b_file = b_root.join("b.mo");
        write(&a_file, "");
        write(&b_file, "");

        for closing_active in [true, false] {
            let mut ws = Workspace::new();
            let a = ws.add_twin(load_twin(&a_root));
            let b = ws.add_twin(load_twin(&b_root));
            ws.add_document(DocumentEntry {
                id: DocumentId::new(1),
                kind: DocumentKindId::new("modelica"),
                origin: DocumentOrigin::writable_file(&a_file),
                runtime_context: DocumentRuntimeOwner::LocalTwin(a),
                title: "a.mo".into(),
                dirty: false,
            });
            ws.add_document(DocumentEntry {
                id: DocumentId::new(2),
                kind: DocumentKindId::new("modelica"),
                origin: DocumentOrigin::writable_file(&b_file),
                runtime_context: DocumentRuntimeOwner::LocalTwin(b),
                title: "b.mo".into(),
                dirty: false,
            });
            if !closing_active {
                ws.active_twin = Some(b);
            }
            ws.active_document = Some(DocumentId::new(1));

            ws.close_twin(a);

            assert_eq!(ws.active_twin, Some(b));
            assert_eq!(ws.active_document, Some(DocumentId::new(2)));
        }
    }

    #[test]
    fn twin_for_path_match_trumps_pin() {
        // Folder display uses A while source runtime remains pinned to B.
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
            kind: DocumentKindId::new("modelica"),
            origin: DocumentOrigin::writable_file(&model),
            runtime_context: DocumentRuntimeOwner::LocalTwin(b),
            title: "shared.mo".into(),
            dirty: false,
        });
        assert_eq!(ws.twin_for(&ws.documents()[0]), Some(a));
        assert_eq!(
            ws.runtime_owner_for(&ws.documents()[0]),
            DocumentRuntimeOwner::LocalTwin(b)
        );
    }

    #[test]
    fn close_twin_orphans_docs_but_keeps_them_open() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("twin.toml"),
            "name=\"t\"\nversion=\"0.1.0\"\n",
        );
        let model = tmp.path().join("m.mo");
        write(&model, "");

        let twin = load_twin(tmp.path());
        let mut ws = Workspace::new();
        let tid = ws.add_twin(twin);
        ws.add_document(DocumentEntry {
            id: DocumentId::new(1),
            kind: DocumentKindId::new("modelica"),
            origin: DocumentOrigin::writable_file(&model),
            runtime_context: DocumentRuntimeOwner::Application,
            title: "m.mo".into(),
            dirty: false,
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
            kind: DocumentKindId::new("modelica"),
            origin: DocumentOrigin::untitled("U"),
            runtime_context: DocumentRuntimeOwner::Application,
            title: "U".into(),
            dirty: true,
        });
        ws.active_document = Some(id);
        let closed = ws.close_document(id);
        assert!(closed.is_some());
        assert_eq!(ws.active_document, None);
    }

    #[test]
    fn pinned_remote_documents_require_exact_mount_and_transport_and_filter_overlays() {
        let mut workspace = Workspace::new();
        let temporary = tempfile::tempdir().expect("temporary local root");
        let local = workspace.add_twin(load_twin(temporary.path()));
        workspace.active_twin = None;
        let remote = ReplicationOwner::Twin {
            scene: ReplicatedSceneOwner {
                connection: bevy::prelude::Entity::from_bits(1),
                host_twin: local,
                authority: "remote".into(),
                root: std::path::PathBuf::from("remote-cache"),
                owns_mount: true,
            },
        };
        for (raw, runtime_context) in [
            (1, DocumentRuntimeOwner::Application),
            (2, DocumentRuntimeOwner::Replicated(remote.clone())),
            (3, DocumentRuntimeOwner::LocalTwin(local)),
        ] {
            workspace.add_document(DocumentEntry {
                id: DocumentId::new(raw),
                kind: DocumentKindId::new("modelica"),
                origin: DocumentOrigin::untitled("Probe"),
                runtime_context,
                title: "Probe".into(),
                dirty: false,
            });
        }
        let source = PinnedDocumentRuntimeOwner::for_document(DocumentId::new(2), Some(&workspace))
            .expect("registered remote document");
        assert!(source.is_current(Some(&workspace), Some(&remote)));
        assert!(source.is_in_active_scope(Some(&workspace), Some(&remote)));
        assert!(!source.is_current(Some(&workspace), None));
        let mut replacement = remote.clone();
        if let ReplicationOwner::Twin { scene } = &mut replacement {
            scene.connection = bevy::prelude::Entity::from_bits(2);
        }
        assert!(!source.is_current(Some(&workspace), Some(&replacement)));
        assert!(!source.shares_runtime_with(DocumentId::new(1), Some(&workspace), Some(&remote)));
        assert!(!source.shares_runtime_with(DocumentId::new(3), Some(&workspace), Some(&remote)));
        let loose = PinnedDocumentRuntimeOwner::for_document(DocumentId::new(1), Some(&workspace))
            .expect("application document");
        assert!(loose.is_current(Some(&workspace), Some(&remote)));
        assert!(!loose.is_in_active_scope(Some(&workspace), Some(&remote)));
        let application_connection = ReplicationOwner::Application {
            connection: remote.connection(),
        };
        assert!(loose.is_in_active_scope(Some(&workspace), Some(&application_connection)));
        workspace.active_twin = Some(local);
        let local_source =
            PinnedDocumentRuntimeOwner::for_document(DocumentId::new(3), Some(&workspace))
                .expect("local document");
        assert!(local_source.is_in_active_scope(Some(&workspace), Some(&application_connection)));
        assert!(!local_source.is_in_active_scope(Some(&workspace), Some(&remote)));
        assert!(source.is_in_active_scope(Some(&workspace), Some(&remote)));
        assert_eq!(
            workspace.new_document_runtime_owner(Some(&remote)),
            DocumentRuntimeOwner::Replicated(remote)
        );
    }
}
