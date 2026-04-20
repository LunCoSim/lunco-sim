//! Shared simulation state for the Modelica workbench UI.
//!
//! ## Entity Viewer Pattern
//!
//! This resource is the **selection bridge** between any context (library browser,
//! 3D viewport click, colony tree) and the Modelica editor panels.
//!
//! `selected_entity` is the single source of truth — panels watch it and
//! render data for the active `ModelicaModel`. Any context can set it:
//!
//! ```rust,ignore
//! // Library Browser: double-click a .mo file
//! // 3D viewport: click a rover's solar panel
//! // Colony tree: select a subsystem node
//! state.selected_entity = Some(entity);
//! ```
//!
//! Panels don't know where the entity came from. They just render it.

use bevy::prelude::*;
use std::collections::HashMap;
use lunco_doc::{DocumentHost, DocumentId, DocumentOrigin};
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicPtr, Ordering};

use std::sync::Arc;

use crate::document::{ModelicaDocument, ModelicaOp};

// ---------------------------------------------------------------------------
// Model File Tracking
// ---------------------------------------------------------------------------

/// Which model is currently open in the editor.
#[derive(Debug, Clone, Default)]
pub struct OpenModel {
    /// Modelica package path (e.g., "Modelica.Electrical.Analog.Basic.Resistor")
    /// or file path for user models (e.g., "Battery.mo").
    pub model_path: String,
    /// Display name shown in breadcrumb (e.g., "Resistor" or "Battery").
    pub display_name: String,
    /// Source code text.
    pub source: Arc<str>,
    /// Byte offsets of the start of each line (prevents O(N) string allocations).
    pub line_starts: Arc<[usize]>,
    /// Memoized model name from AST.
    pub detected_name: Option<String>,
    /// Pre-computed text layout for high-performance rendering.
    pub cached_galley: Option<Arc<bevy_egui::egui::Galley>>,
    /// Whether this model is read-only.
    pub read_only: bool,
    /// Which library this model came from.
    pub library: ModelLibrary,
}

/// Which library a model belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelLibrary {
    /// Modelica Standard Library (read-only).
    MSL,
    /// Bundled models shipped with LunCoSim (read-only for now).
    #[default]
    Bundled,
    /// User-created models (writable, from opened folder).
    User,
    /// In-memory model created by user (writable until saved).
    InMemory,
}

/// Static cell bridging JS file picker → Bevy system on wasm32.
/// Set by `set_file_load_result` when user selects a .mo file.
/// Read and cleared by `update_file_load_result` each frame.
#[cfg(target_arch = "wasm32")]
static FILE_LOAD_CELL: AtomicPtr<String> = AtomicPtr::new(std::ptr::null_mut());

/// Called from JS when a .mo file is loaded via browser file picker.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn set_file_load_result(content: &str) {
    let prev = FILE_LOAD_CELL.swap(Box::into_raw(Box::new(content.to_string())), Ordering::SeqCst);
    if !prev.is_null() {
        unsafe { drop(Box::from_raw(prev)); }
    }
}

/// Consumes pending file load from browser file picker and updates editor buffer.
/// Runs each frame on wasm32.
#[cfg(target_arch = "wasm32")]
pub fn update_file_load_result(mut state: ResMut<WorkbenchState>) {
    let prev = FILE_LOAD_CELL.swap(std::ptr::null_mut(), Ordering::SeqCst);
    if !prev.is_null() {
        let content = unsafe { Box::from_raw(prev) };
        state.editor_buffer = *content;
    }
}

/// Shared state for the Modelica workbench UI.
///
/// # What lives here (post–Workspace migration)
///
/// This resource is now a **UI cache** on top of the authoritative
/// session state in [`lunco_workbench::WorkspaceResource`]:
///
/// - **Identity of the active document** lives on the Workspace
///   (`active_document: Option<DocumentId>`). This struct only carries
///   the derived render-side snapshot (`open_model`) so per-frame
///   painting doesn't chase it through the registry.
/// - **Selection bridge** — `selected_entity` still funnels library /
///   3D-viewport / colony-tree clicks into the editor panels.
/// - **Transient UI flags** — compile error, "is loading" spinner,
///   diagram-rebuild marker.
///
/// Plot-related fields (`history`, `plotted_variables`, `max_history`,
/// `plot_auto_fit`) used to live here and moved to `lunco-viz`
/// (`SignalRegistry`, `VisualizationConfig.inputs`, `VizFitRequests`)
/// when the Graphs panel migrated.
#[derive(Resource)]
pub struct WorkbenchState {
    /// Current Modelica source code in the editor. Mirror of the
    /// active document's source, kept here because egui's `TextEdit`
    /// owns a `&mut String` and the registry hands out `Arc<str>`.
    pub editor_buffer: String,
    /// **Selection bridge**: which `ModelicaModel` entity panels are viewing.
    /// Set by any context (library, 3D viewport, colony tree).
    pub selected_entity: Option<Entity>,
    /// Last compilation error message, if any.
    pub compilation_error: Option<String>,
    /// Render-side snapshot of the active document — display name,
    /// source, line starts, read-only flag, cached galley. The
    /// *identity* of the active doc is
    /// [`lunco_workbench::WorkspaceResource::active_document`]; this is
    /// just what the renderer needs without a per-frame registry hit.
    pub open_model: Option<OpenModel>,
    /// Flag to signal the diagram panel should rebuild from open_model source.
    pub diagram_dirty: bool,
    /// Whether a model is currently being loaded in the background.
    pub is_loading: bool,
}

// ---------------------------------------------------------------------------
// Document lifecycle events
// ---------------------------------------------------------------------------
//
// `DocumentChanged` / `DocumentOpened` / `DocumentClosed` / `DocumentSaved`
// are now defined as *generic* events in `lunco_doc_bevy`. The registry
// queues pending events in per-kind `Vec<DocumentId>` buffers, and
// `drain_document_changes` (in `ui::mod`) drains them and fires the
// typed triggers. This decouples registry mutations from the Bevy
// trigger machinery and lets every domain funnel through the same
// events — the `TwinJournal` picks them all up.

// ---------------------------------------------------------------------------
// CompileState — per-document compile lifecycle
// ---------------------------------------------------------------------------

/// Current compile-lifecycle state for a single [`ModelicaDocument`].
///
/// Separate from [`ModelicaModel::is_stepping`] (a per-entity simulation
/// tick guard) and from error *content* (which lives in
/// [`WorkbenchState::compilation_error`] today). This enum is the
/// answer to "is a compile in flight for this document?" — UI uses it
/// to disable the Compile button while the worker is busy and to show
/// an at-a-glance status chip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CompileState {
    /// Nothing has been compiled yet (or the last result has been
    /// invalidated by an edit — we don't track that today, but the
    /// default "fresh" state maps here).
    #[default]
    Idle,
    /// A `ModelicaCommand::Compile` has been sent to the worker and we
    /// are waiting for the matching result.
    Compiling,
    /// The last compile succeeded. The worker's cached DAE is current.
    Ready,
    /// The last compile failed. See `WorkbenchState::compilation_error`
    /// for details (will migrate to per-document error storage later).
    Error,
}

/// Per-document compile-state map.
///
/// Populated when a compile is dispatched and updated when the worker
/// responds. UI reads it through
/// [`state_of`](Self::state_of). Missing entries are treated as
/// [`CompileState::Idle`].
#[derive(Resource, Default)]
pub struct CompileStates {
    by_doc: HashMap<DocumentId, CompileState>,
    /// When each doc's currently-in-flight compile started. Cleared
    /// on terminal transition (Ready / Error) via
    /// [`set_and_stamp`](Self::set_and_stamp). Used to log
    /// "compile X finished in Y ms" to Console / Diagnostics when
    /// the worker responds.
    compile_started: HashMap<DocumentId, std::time::Instant>,
}

impl CompileStates {
    /// Current state for `doc`. Missing → [`CompileState::Idle`].
    pub fn state_of(&self, doc: DocumentId) -> CompileState {
        self.by_doc.get(&doc).copied().unwrap_or_default()
    }

    /// True when `doc` has a compile currently in flight.
    pub fn is_compiling(&self, doc: DocumentId) -> bool {
        matches!(self.state_of(doc), CompileState::Compiling)
    }

    /// Overwrite the state for `doc`.
    pub fn set(&mut self, doc: DocumentId, state: CompileState) {
        self.by_doc.insert(doc, state);
    }

    /// Stamp the compile start time AND transition to `Compiling`.
    /// Use instead of `set(doc, Compiling)` when dispatching a
    /// compile — the stamp lets us measure elapsed on terminal
    /// transition.
    pub fn mark_started(&mut self, doc: DocumentId) {
        self.by_doc.insert(doc, CompileState::Compiling);
        self.compile_started.insert(doc, std::time::Instant::now());
    }

    /// Transition to a terminal state and return elapsed time since
    /// the last `mark_started` (if any). Clears the stamp so a
    /// future compile starts clean.
    pub fn mark_finished(
        &mut self,
        doc: DocumentId,
        state: CompileState,
    ) -> Option<std::time::Duration> {
        self.by_doc.insert(doc, state);
        self.compile_started.remove(&doc).map(|t| t.elapsed())
    }

    /// Drop any recorded state for `doc` (e.g. when a document is removed).
    pub fn remove(&mut self, doc: DocumentId) {
        self.by_doc.remove(&doc);
        self.compile_started.remove(&doc);
    }
}

// ---------------------------------------------------------------------------
// ModelicaDocumentRegistry — DocumentId-keyed DocumentHost storage
// ---------------------------------------------------------------------------

/// Registry of [`DocumentHost<ModelicaDocument>`] instances, keyed by
/// [`DocumentId`].
///
/// **The single source of truth for Modelica source text.** Every spawn
/// path (CodeEditor Compile, Diagram auto-compile, `balloon_setup`, the
/// workbench binaries) allocates a document here and stores its id in
/// [`crate::ModelicaModel::document`]. The entity becomes a runtime
/// *reference* to the document, not its owner — a document can exist
/// before any entity is spawned and can outlive an entity (e.g. a user
/// stops a sim but keeps editing).
///
/// A secondary `entity → DocumentId` index is maintained so despawn
/// cleanup (`ui::cleanup_removed_documents`) and "which doc does this
/// entity view?" lookups stay cheap. The index is an optimization; the
/// authoritative storage is `hosts`.
///
/// Consumers read source/parameters through
/// [`host`](Self::host)`(doc).document().source()` — resolving
/// `entity → DocumentId` via [`document_of`](Self::document_of) (or
/// reading `ModelicaModel.document` directly) first.
///
/// Still outside this registry:
///
/// - `EditorBufferState.text` — the egui TextEdit working buffer
///   (keystroke-responsive; committed into the Document on focus-loss
///   or Compile).
/// - `WorkbenchState.open_model.source` — the library browser's
///   "current file" slot, used before any compile/spawn. Will fold into
///   the registry once file-open creates a Document directly.
#[derive(Resource, Default)]
pub struct ModelicaDocumentRegistry {
    hosts: HashMap<DocumentId, DocumentHost<ModelicaDocument>>,
    by_entity: HashMap<Entity, DocumentId>,
    next_doc_id: u64,
    /// Docs that were just added via `allocate*`. Drained into
    /// [`lunco_doc_bevy::DocumentOpened`] triggers each frame.
    pending_opened: Vec<DocumentId>,
    /// Docs whose source just advanced (allocate initial source,
    /// `checkpoint_source` with different text, explicit
    /// [`mark_changed`](Self::mark_changed) after `host_mut` undo/redo).
    /// Drained into [`lunco_doc_bevy::DocumentChanged`] triggers.
    ///
    /// Direct mutations through [`host_mut`](Self::host_mut) must call
    /// [`mark_changed`](Self::mark_changed) explicitly — the registry
    /// cannot intercept arbitrary uses of a `&mut DocumentHost`.
    pending_changes: Vec<DocumentId>,
    /// Docs that were just dropped via [`remove_document`](Self::remove_document).
    /// Drained into [`lunco_doc_bevy::DocumentClosed`] triggers.
    pending_closed: Vec<DocumentId>,
}

impl ModelicaDocumentRegistry {
    /// Allocate a fresh Untitled [`DocumentId`] + [`DocumentHost`]
    /// holding `source`. Display name is `Untitled-<id>`. Not linked
    /// to any entity.
    pub fn allocate(&mut self, source: String) -> DocumentId {
        self.next_doc_id = self.next_doc_id.saturating_add(1);
        let id = DocumentId::new(self.next_doc_id);
        let origin = DocumentOrigin::untitled(format!("Untitled-{}", id.raw()));
        let doc = ModelicaDocument::with_origin(id, source, origin);
        self.hosts.insert(id, DocumentHost::new(doc));
        self.pending_opened.push(id);
        self.pending_changes.push(id);
        id
    }

    /// Allocate a fresh [`DocumentId`] + [`DocumentHost`] with an
    /// explicit origin. Use this when opening from disk or bundled
    /// assets so `SaveDocument` + read-only badges work.
    pub fn allocate_with_origin(
        &mut self,
        source: String,
        origin: DocumentOrigin,
    ) -> DocumentId {
        self.next_doc_id = self.next_doc_id.saturating_add(1);
        let id = DocumentId::new(self.next_doc_id);
        let doc = ModelicaDocument::with_origin(id, source, origin);
        self.hosts.insert(id, DocumentHost::new(doc));
        // One `Opened` for the lifecycle, then one `Changed` so any
        // subscriber that only listens to changes (diagram re-parse,
        // plot variable-list refresh, …) still sees the initial source.
        self.pending_opened.push(id);
        self.pending_changes.push(id);
        id
    }

    /// Allocate a fresh [`DocumentId`] WITHOUT building the host.
    /// Pairs with [`Self::install_prebuilt`]: caller uses the id
    /// to build a `ModelicaDocument` off-thread (the parse can take
    /// seconds on large MSL package files), then installs the
    /// fully-parsed host back in the registry on the main thread.
    ///
    /// Emits no `Opened` / `Changed` yet — those fire on
    /// `install_prebuilt`. UI panels that query the registry with
    /// an unallocated id just see a miss.
    pub fn reserve_id(&mut self) -> DocumentId {
        self.next_doc_id = self.next_doc_id.saturating_add(1);
        DocumentId::new(self.next_doc_id)
    }

    /// Install a pre-built document under a previously-reserved id.
    /// Intended for the async-load path: the heavy parse runs on a
    /// background task, and only the cheap HashMap insert happens
    /// on the UI thread.
    pub fn install_prebuilt(&mut self, id: DocumentId, doc: ModelicaDocument) {
        self.hosts.insert(id, DocumentHost::new(doc));
        self.pending_opened.push(id);
        self.pending_changes.push(id);
    }

    /// Link `entity` to `doc`. Replaces any prior link for `entity`.
    /// The document must already exist (call [`allocate`](Self::allocate)
    /// first); linking to an unknown id is a no-op in release builds and
    /// a debug assertion failure otherwise.
    pub fn link(&mut self, entity: Entity, doc: DocumentId) {
        debug_assert!(self.hosts.contains_key(&doc), "link to unknown DocumentId {doc}");
        self.by_entity.insert(entity, doc);
    }

    /// Convenience: [`allocate`](Self::allocate) + [`link`](Self::link).
    /// Returns the new [`DocumentId`] — the caller must also write it into
    /// `ModelicaModel::document` so downstream systems can resolve it
    /// without touching the registry.
    pub fn open_for(&mut self, entity: Entity, source: String) -> DocumentId {
        let id = self.allocate(source);
        self.by_entity.insert(entity, id);
        id
    }

    /// Convenience: [`allocate_with_origin`](Self::allocate_with_origin) + [`link`](Self::link).
    pub fn open_for_with_origin(
        &mut self,
        entity: Entity,
        source: String,
        origin: DocumentOrigin,
    ) -> DocumentId {
        let id = self.allocate_with_origin(source, origin);
        self.by_entity.insert(entity, id);
        id
    }

    /// Look up a document by its canonical path. Returns `None` for
    /// untitled docs or if no document was opened from that path.
    /// Used by API / scripting to resolve `path` → `DocumentId`.
    pub fn find_by_path(&self, path: &std::path::Path) -> Option<DocumentId> {
        self.hosts.iter().find_map(|(id, host)| {
            (host.document().canonical_path() == Some(path)).then_some(*id)
        })
    }

    /// Entities currently linked to this document. Typically 0 (editing
    /// without a running sim) or 1; >1 in cosim scenarios.
    pub fn entities_linked_to(&self, doc: DocumentId) -> Vec<Entity> {
        self.by_entity
            .iter()
            .filter_map(|(e, d)| (*d == doc).then_some(*e))
            .collect()
    }

    /// Replace the source on an existing document. Returns `true` if the
    /// document changed (different source), `false` on no-op (identical
    /// source) or unknown id.
    ///
    /// Queues a [`DocumentChanged`] notification when the source actually
    /// changes; [`drain_pending_changes`](Self::drain_pending_changes)
    /// emits the observer trigger on the next system run.
    pub fn checkpoint_source(&mut self, doc: DocumentId, source: String) -> bool {
        let Some(host) = self.hosts.get_mut(&doc) else { return false };
        if host.document().source() == source {
            return false;
        }
        // Best-effort: ReplaceSource cannot fail today, but the trait
        // signature is fallible so we swallow the Result rather than
        // propagate it. Callers don't care about the details.
        let _ = host.apply(ModelicaOp::ReplaceSource { new: source });
        self.pending_changes.push(doc);
        true
    }

    /// Explicitly mark a document as changed. Required after direct
    /// mutations through [`host_mut`](Self::host_mut) (undo / redo),
    /// since the registry cannot observe those through a bare `&mut`.
    pub fn mark_changed(&mut self, doc: DocumentId) {
        self.pending_changes.push(doc);
    }

    /// Drain queued change notifications. The `drain_document_changes`
    /// system calls this each frame and fans the ids out as
    /// [`lunco_doc_bevy::DocumentChanged`] triggers.
    pub fn drain_pending_changes(&mut self) -> Vec<DocumentId> {
        std::mem::take(&mut self.pending_changes)
    }

    /// Drain queued Opened notifications. The same drain-and-fire
    /// system in `ui::mod` turns these into
    /// [`lunco_doc_bevy::DocumentOpened`] triggers.
    pub fn drain_pending_opened(&mut self) -> Vec<DocumentId> {
        std::mem::take(&mut self.pending_opened)
    }

    /// Drain queued Closed notifications. Fanned out as
    /// [`lunco_doc_bevy::DocumentClosed`] triggers.
    pub fn drain_pending_closed(&mut self) -> Vec<DocumentId> {
        std::mem::take(&mut self.pending_closed)
    }

    /// Immutable access to a document host by id.
    pub fn host(&self, doc: DocumentId) -> Option<&DocumentHost<ModelicaDocument>> {
        self.hosts.get(&doc)
    }

    /// Mutable access to a document host by id.
    pub fn host_mut(&mut self, doc: DocumentId) -> Option<&mut DocumentHost<ModelicaDocument>> {
        self.hosts.get_mut(&doc)
    }

    /// Which [`DocumentId`] does `entity` reference, if any.
    pub fn document_of(&self, entity: Entity) -> Option<DocumentId> {
        self.by_entity.get(&entity).copied()
    }

    /// Drop a document. Any entity linked to it is unlinked too.
    /// Queues a `Closed` notification for observers / the Twin journal.
    pub fn remove_document(&mut self, doc: DocumentId) {
        if self.hosts.remove(&doc).is_some() {
            self.pending_closed.push(doc);
        }
        self.by_entity.retain(|_, d| *d != doc);
    }

    /// Mark the given document as persisted at its current generation.
    /// Called by the `SaveDocument` observer after a successful disk
    /// write. Bookkeeping-only — not an op, not undoable.
    pub fn mark_document_saved(&mut self, doc: DocumentId) {
        if let Some(host) = self.hosts.get_mut(&doc) {
            host.document_mut().mark_saved();
        }
    }

    /// Drop the entity→document link without removing the document itself.
    /// Returns the id that was unlinked (if any) so callers can decide
    /// whether to also [`remove_document`](Self::remove_document) it.
    pub fn unlink_entity(&mut self, entity: Entity) -> Option<DocumentId> {
        self.by_entity.remove(&entity)
    }

    /// Number of registered documents.
    pub fn len(&self) -> usize {
        self.hosts.len()
    }

    /// Whether the registry currently tracks any documents.
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// Iterate `(DocumentId, &DocumentHost)` for every loaded document.
    /// Iteration order is not stable — callers that need a stable
    /// presentation order (e.g. the Twin Browser's Modelica section)
    /// must sort the results themselves.
    pub fn iter(&self) -> impl Iterator<Item = (DocumentId, &DocumentHost<ModelicaDocument>)> {
        self.hosts.iter().map(|(id, host)| (*id, host))
    }
}

impl Default for WorkbenchState {
    fn default() -> Self {
        Self {
            editor_buffer: String::new(),
            selected_entity: None,
            compilation_error: None,
            open_model: None,
            diagram_dirty: false,
            is_loading: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::Document;

    fn fake_entity(bits: u64) -> Entity {
        Entity::from_bits(bits)
    }

    #[test]
    fn allocate_creates_host() {
        let mut reg = ModelicaDocumentRegistry::default();

        assert!(reg.is_empty());
        let doc = reg.allocate("model A end A;".into());
        assert_eq!(reg.len(), 1);

        let host = reg.host(doc).expect("host registered");
        assert_eq!(host.document().source(), "model A end A;");
        assert_eq!(host.generation(), 0, "allocate doesn't apply an op");
    }

    #[test]
    fn checkpoint_applies_op_on_change() {
        let mut reg = ModelicaDocumentRegistry::default();
        let doc = reg.allocate("model A end A;".into());

        let changed = reg.checkpoint_source(doc, "model B end B;".into());
        assert!(changed);

        let host = reg.host(doc).unwrap();
        assert_eq!(host.document().source(), "model B end B;");
        assert_eq!(host.generation(), 1);
        assert!(host.can_undo());
    }

    #[test]
    fn checkpoint_no_op_when_source_unchanged() {
        let mut reg = ModelicaDocumentRegistry::default();
        let doc = reg.allocate("same".into());

        let changed = reg.checkpoint_source(doc, "same".into());
        assert!(!changed, "re-checkpointing identical source must not bump generation");
        assert_eq!(reg.host(doc).unwrap().generation(), 0);
    }

    #[test]
    fn checkpoint_unknown_doc_is_noop() {
        let mut reg = ModelicaDocumentRegistry::default();
        let changed = reg.checkpoint_source(DocumentId::new(999), "x".into());
        assert!(!changed);
        assert!(reg.is_empty());
    }

    #[test]
    fn undo_restores_previous_source() {
        let mut reg = ModelicaDocumentRegistry::default();
        let doc = reg.allocate("v1".into());
        reg.checkpoint_source(doc, "v2".into());
        reg.checkpoint_source(doc, "v3".into());

        let host = reg.host_mut(doc).unwrap();
        host.undo().unwrap();
        assert_eq!(host.document().source(), "v2");
        host.undo().unwrap();
        assert_eq!(host.document().source(), "v1");
    }

    #[test]
    fn open_for_links_entity_and_document() {
        let mut reg = ModelicaDocumentRegistry::default();
        let e = fake_entity(0x0000_0001_0000_0001);

        let doc = reg.open_for(e, "model A end A;".into());
        assert_eq!(reg.document_of(e), Some(doc));
        assert_eq!(reg.host(doc).unwrap().document().source(), "model A end A;");
    }

    #[test]
    fn remove_document_drops_host_and_unlinks_entity() {
        let mut reg = ModelicaDocumentRegistry::default();
        let e = fake_entity(0x0000_0002_0000_0002);
        let doc = reg.open_for(e, "x".into());
        assert_eq!(reg.len(), 1);

        reg.remove_document(doc);
        assert!(reg.is_empty());
        assert!(reg.host(doc).is_none());
        assert_eq!(reg.document_of(e), None);
    }

    #[test]
    fn unlink_entity_returns_doc_but_keeps_document() {
        let mut reg = ModelicaDocumentRegistry::default();
        let e = fake_entity(0x0000_0003_0000_0003);
        let doc = reg.open_for(e, "x".into());

        let unlinked = reg.unlink_entity(e);
        assert_eq!(unlinked, Some(doc));
        assert_eq!(reg.document_of(e), None);
        assert!(reg.host(doc).is_some(), "document outlives the link");
    }

    #[test]
    fn multiple_documents_tracked_independently() {
        let mut reg = ModelicaDocumentRegistry::default();
        let a = fake_entity(0x0000_0004_0000_0004);
        let b = fake_entity(0x0000_0005_0000_0005);

        let doc_a = reg.open_for(a, "source_a".into());
        let doc_b = reg.open_for(b, "source_b".into());
        reg.checkpoint_source(doc_a, "source_a_v2".into());

        assert_eq!(reg.host(doc_a).unwrap().document().source(), "source_a_v2");
        assert_eq!(reg.host(doc_b).unwrap().document().source(), "source_b");
        assert_ne!(doc_a, doc_b, "each allocation gets a distinct DocumentId");
    }
}
