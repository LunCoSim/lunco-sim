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
use lunco_doc::{Document, DocumentHost, DocumentId, DocumentOrigin};
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::document::{ModelicaDocument, ModelicaOp};

// ---------------------------------------------------------------------------
// Model File Tracking
// ---------------------------------------------------------------------------

// `OpenModel` retired (2026-05-08). The cache held
// fields all derivable from the document host:
//   - `source` → `host.document().source()` + Index.
//   - `display_name` → `host.document().origin().display_name()`.
//   - `read_only` → `host.document().is_read_only()`.
//   - `detected_name` → `extract_model_name_from_ast(host.document().strict_ast()?)`.
//   - `library` → derive from `host.document().origin()`.
//   - `cached_galley` → moved to `EditorBufferState`.
//   - `model_path` → derive from origin (file path / mem://name / msl://qualified).
// Helpers in this module: `detected_name_for`, `read_only_for`,
// `display_name_for`.

/// Which library a model belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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
/// session state in [`lunco_workspace::WorkspaceResource`]:
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
    // Per-doc compile errors live on `CompileStates.errors[doc]`;
    // readers go through `compile_states.error_message(doc)`.
    // Per-doc loading state lives on the `StatusBus` —
    // `bus.is_busy(BusyScope::Document(doc.0))` /
    // `bus.lifecycle(BusyScope::Document(doc.0), has_content)`.
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
// events — the canonical `JournalResource` records them all.


// ---------------------------------------------------------------------------
// B.3 phase 6 helpers — drop-in replacements for `OpenModel` field
// reads. Derive each field from the document registry so the legacy
// `WorkbenchState.open_model` cache can be retired one reader at a
// time.
// ---------------------------------------------------------------------------

/// Detected top-level model name for `doc`. Replaces
/// `open_model.detected_name`. Returns `None` when the doc has no
/// AST yet (parse pending) or when no model declaration exists.
pub fn detected_name_for(world: &bevy::prelude::World, doc: DocumentId) -> Option<String> {
    crate::sim_default::default_simulation_class(world, doc)
}

/// `PanelCtx` sibling of [`detected_name_for`].
#[cfg(feature = "ui")]
pub fn detected_name_for_ctx(
    ctx: &lunco_workbench::PanelCtx,
    doc: DocumentId,
) -> Option<String> {
    crate::sim_default::default_simulation_class_ctx(ctx, doc)
}

/// `PanelCtx` sibling of [`read_only_for`].
#[cfg(feature = "ui")]
pub fn read_only_for_ctx(ctx: &lunco_workbench::PanelCtx, doc: DocumentId) -> bool {
    ctx.resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.host(doc))
        .map(|h| h.document().is_read_only())
        .unwrap_or(false)
}

/// `PanelCtx` sibling of [`display_name_for`].
#[cfg(feature = "ui")]
pub fn display_name_for_ctx(
    ctx: &lunco_workbench::PanelCtx,
    doc: DocumentId,
) -> Option<String> {
    ctx.resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.host(doc))
        .map(|h| h.document().origin().display_name())
}

/// Read-only flag for `doc`. Replaces `open_model.read_only`.
pub fn read_only_for(world: &bevy::prelude::World, doc: DocumentId) -> bool {
world
        .resource::<ModelicaDocumentRegistry>()
        .host(doc)
        .map(|h| h.document().is_read_only())
        .unwrap_or(false)
}

/// Display name for `doc`. Replaces `open_model.display_name`.
pub fn display_name_for(world: &bevy::prelude::World, doc: DocumentId) -> Option<String> {
world
        .resource::<ModelicaDocumentRegistry>()
        .host(doc)
        .map(|h| h.document().origin().display_name())
}

// ---------------------------------------------------------------------------
// ModelicaDocumentRegistry — DocumentId-keyed DocumentHost storage
// ---------------------------------------------------------------------------

/// Registry of [`DocumentHost<ModelicaDocument>`] instances, keyed by
/// [`lunco_doc::DocumentId`].
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
#[derive(Resource, Default)]
pub struct ModelicaDocumentRegistry {
    hosts: HashMap<DocumentId, DocumentHost<ModelicaDocument>>,
    by_entity: HashMap<Entity, DocumentId>,
    /// Twin-journal handle, wired once the [`JournalResource`](lunco_doc_bevy::JournalResource)
    /// appears (see `wire_modelica_journal_handle`). When set, every host gets
    /// a [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder) so edits —
    /// including undo/redo — auto-record (A3). `None` → no recording.
    journal: Option<lunco_doc_bevy::JournalResource>,
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
    /// Allocate a fresh Untitled [`lunco_doc::DocumentId`] + [`DocumentHost`]
    /// holding `source`. Display name is `Untitled-<id>`. Not linked
    /// to any entity.
    pub fn allocate(&mut self, source: String) -> DocumentId {
        self.next_doc_id = self.next_doc_id.saturating_add(1);
        let id = DocumentId::new(self.next_doc_id);
        let origin = DocumentOrigin::untitled(format!("Untitled-{}", id.raw()));
        let doc = ModelicaDocument::with_origin(id, source, origin);
        self.hosts.insert(id, DocumentHost::new(doc));
        self.attach_recorder(id);
        self.pending_opened.push(id);
        self.pending_changes.push(id);
        id
    }

    /// Allocate a fresh [`lunco_doc::DocumentId`] + [`DocumentHost`] with an
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
        self.attach_recorder(id);
        // One `Opened` for the lifecycle, then one `Changed` so any
        // subscriber that only listens to changes (diagram re-parse,
        // plot variable-list refresh, …) still sees the initial source.
        self.pending_opened.push(id);
        self.pending_changes.push(id);
        id
    }

    /// Allocate a fresh [`lunco_doc::DocumentId`] WITHOUT building the host.
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
        self.attach_recorder(id);
        self.pending_opened.push(id);
        self.pending_changes.push(id);
    }

    /// Wire the Twin-journal handle and retro-fit a recorder onto every
    /// existing host. Called once, reactively, the frame the
    /// [`JournalResource`](lunco_doc_bevy::JournalResource) first appears.
    /// Hosts created afterwards get their recorder at allocation time.
    pub fn set_journal(&mut self, journal: lunco_doc_bevy::JournalResource) {
        self.journal = Some(journal);
        let ids: Vec<_> = self.hosts.keys().copied().collect();
        for id in ids {
            self.attach_recorder(id);
        }
    }

    /// Attach a [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder) to
    /// `id`'s host when a journal is wired and the host lacks one. The A3
    /// auto-bridge seam — every apply/undo/redo records losslessly with no
    /// per-op code in the funnels.
    fn attach_recorder(&mut self, id: DocumentId) {
        if let Some(journal) = &self.journal {
            if let Some(host) = self.hosts.get_mut(&id) {
                if !host.has_recorder() {
                    lunco_doc_bevy::attach_journal_recorder(host, journal);
                }
            }
        }
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
    /// Returns the new [`lunco_doc::DocumentId`] — the caller must also write it into
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
    /// untitled / bundled docs or if no document was opened from that
    /// path. Used by API / scripting to resolve `path` →
    /// `DocumentId`.
    pub fn find_by_path(&self, path: &std::path::Path) -> Option<DocumentId> {
        // `same_file`, NOT `==`: despite its name `canonical_path()` does not
        // canonicalize — it returns the stored path verbatim. An exact compare
        // therefore missed `/a/./b.mo` vs `/a/b.mo` vs a symlink and minted a
        // SECOND document for one file: two tabs, two undo stacks, both saving
        // over each other. That is the exact split-brain this lookup exists to
        // prevent. `same_file` compares cheaply first and only pays for
        // `canonicalize` when the raw paths differ. Shared with USD via
        // `lunco_doc_bevy::DocumentRegistry::doc_for_file`.
        self.hosts.iter().find_map(|(id, host)| {
            host.document()
                .canonical_path()
                .is_some_and(|p| lunco_doc::same_file(p, path))
                .then_some(*id)
        })
    }

    /// Look up a document whose origin is
    /// [`lunco_doc::DocumentOrigin::Bundled`] with the given filename.
    /// Used by the package browser's bundled-doc dedup — the typed
    /// equivalent of `find_by_path` for the bundled variant, which
    /// has no on-disk path.
    pub fn find_bundled(&self, filename: &str) -> Option<DocumentId> {
        self.hosts.iter().find_map(|(id, host)| {
            match host.document().origin() {
                lunco_doc::DocumentOrigin::Bundled { filename: f } if f == filename => Some(*id),
                _ => None,
            }
        })
    }

    /// Iterate every `(entity, doc)` link currently registered.
    /// Used by per-frame snapshot builders that need to project the
    /// full `doc → entity` table for source-backed plot tiles to
    /// resolve their sim at fetch time.
    pub fn iter_doc_for_entity(&self) -> impl Iterator<Item = (Entity, DocumentId)> + '_ {
        self.by_entity.iter().map(|(e, d)| (*e, *d))
    }

    /// Entities currently linked to this document. Typically 0 (editing
    /// without a running sim) or 1; >1 in cosim scenarios.
    pub fn entities_linked_to(&self, doc: DocumentId) -> Vec<Entity> {
        self.by_entity
            .iter()
            .filter_map(|(e, d)| (*d == doc).then_some(*e))
            .collect()
    }

    /// First simulator entity linked to `doc`, or `None` when no
    /// compile has spawned one yet. The canonical lookup for any
    /// view-bound panel that needs sim state for "its" document —
    /// canvas plots, in-canvas input controls, model-view toolbar.
    /// Cosim cases (>1 entity per doc) take the first; if you need
    /// all of them, call [`entities_linked_to`](Self::entities_linked_to).
    pub fn simulator_for(&self, doc: DocumentId) -> Option<Entity> {
        self.by_entity
            .iter()
            .find_map(|(e, d)| (*d == doc).then_some(*e))
    }
}

/// View-bound entry point: the simulator entity for `doc`. Pure
/// function on (world, doc) — no "active tab" magic, no side panel
/// preference. Doc-scoped views (model-view, in-canvas plots,
/// per-doc input controls) call this with their own `doc_id` so two
/// canvases for different models read from the right entity each.
pub fn simulator_for(world: &World, doc: DocumentId) -> Option<Entity> {
    world
        .get_resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.simulator_for(doc))
}

/// `PanelCtx` sibling of [`simulator_for`]. Reads
/// [`ModelicaDocumentRegistry`] through the capability-narrowed panel
/// context so ported panels can resolve their doc's simulator entity
/// during paint without `&World`.
#[cfg(feature = "ui")]
pub fn simulator_for_ctx(
    ctx: &lunco_workbench::PanelCtx,
    doc: DocumentId,
) -> Option<Entity> {
    ctx.resource::<ModelicaDocumentRegistry>()
        .and_then(|r| r.simulator_for(doc))
}

/// Convenience for singleton side panels (Telemetry, Inspector) that
/// follow the active tab by default. Resolves
/// `WorkspaceResource.active_document → simulator_for(doc)`. Returns
/// `None` when there's no active doc or no compiled sim for it.
///
/// The "follow active tab" policy is explicit at the call site —
/// nothing in the lookup itself depends on tab focus. Future panels
/// that pin to a specific doc just call [`simulator_for`] with the
/// pinned id instead.
pub fn active_simulator(world: &World) -> Option<Entity> {
    let active = world
        .get_resource::<lunco_workspace::WorkspaceResource>()?
        .active_document?;
    simulator_for(world, active)
}

/// Empty `impl` block kept so the trailing `}` above closes
/// `ModelicaDocumentRegistry`'s impl cleanly without cargo-culting
/// the existing braces below. The two free functions above belong
/// at module scope, not on the registry — they consult multiple
/// resources.
impl ModelicaDocumentRegistry {

    /// Replace the source on an existing document. Returns `true` if the
    /// document changed (different source), `false` on no-op (identical
    /// source) or unknown id.
    ///
    /// Queues a [`lunco_doc_bevy::DocumentChanged`] notification when the source actually
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
        // API / structured edits are one-shot commits — bypass the
        // typing-debounce so the next ast_refresh tick reparses
        // immediately and the canvas updates without the 2.5 s wait
        // (the debounce exists to coalesce keystroke bursts, which
        // doesn't apply here).
        host.document_mut().waive_ast_debounce();
        self.pending_changes.push(doc);
        true
    }

    /// Explicitly mark a document as changed. Required after direct
    /// mutations through [`host_mut`](Self::host_mut) (undo / redo),
    /// since the registry cannot observe those through a bare `&mut`.
    pub fn mark_changed(&mut self, doc: DocumentId) {
        self.pending_changes.push(doc);
    }

    /// Apply a **journal op** to `doc` for replay (journal→scene projection —
    /// the networked-edit consume path) **without recording it**. Mirror of
    /// [`DocumentRegistry::replay_op`](lunco_doc_bevy::DocumentRegistry::replay_op) — now generic, so this Modelica copy is redundant.
    ///
    /// The op arrived via `Journal::append_remote` and is already in the
    /// journal, so re-recording it (as `DocumentHost::apply` would via the
    /// host's [`JournalOpRecorder`](lunco_doc_bevy::JournalOpRecorder)) would
    /// mint a duplicate local entry. This applies straight to the document
    /// (bypassing the recorder), waives the AST debounce, and marks `doc`
    /// changed so the canvas / engine reproject. `op` is the entry's serialized
    /// [`ModelicaOp`] payload.
    ///
    /// Returns `false` (logged, non-fatal) if the doc is unknown, the payload
    /// isn't a `ModelicaOp`, or the apply is rejected — notably a structural op
    /// (see [`op_needs_fresh_ast_pre_apply`](crate::doc_ops::op_needs_fresh_ast_pre_apply))
    /// whose target the host's stale AST can't resolve yet; the interactive
    /// funnel *defers* those, which cross-peer replay can't (a known limitation
    /// tracked with multi-doc replay wiring).
    pub fn replay_op(&mut self, doc: DocumentId, op: &serde_json::Value) -> bool {
        let parsed = match serde_json::from_value::<ModelicaOp>(op.clone()) {
            Ok(op) => op,
            Err(e) => {
                bevy::log::warn!("[modelica-replay] op payload is not a ModelicaOp: {e}");
                return false;
            }
        };
        let Some(host) = self.hosts.get_mut(&doc) else {
            return false;
        };
        match host.document_mut().apply(parsed) {
            Ok(_) => {
                host.document_mut().waive_ast_debounce();
                self.pending_changes.push(doc);
                true
            }
            Err(e) => {
                bevy::log::warn!("[modelica-replay] apply rejected on doc {doc}: {e:?}");
                false
            }
        }
    }

    /// Drain queued change notifications. The `drain_document_changes`
    /// system calls this each frame and fans the ids out as
    /// [`lunco_doc_bevy::DocumentChanged`] triggers. Deduped per drain
    /// so an N-edit batch fires observers N times across N drains, not
    /// N×N times within one drain.
    pub fn drain_pending_changes(&mut self) -> Vec<DocumentId> {
        let raw = std::mem::take(&mut self.pending_changes);
        let mut seen: std::collections::HashSet<DocumentId> = std::collections::HashSet::new();
        raw.into_iter().filter(|id| seen.insert(*id)).collect()
    }

    /// Drain queued Opened notifications. The same drain-and-fire
    /// system in `ui::mod` turns these into
    /// [`lunco_doc_bevy::DocumentOpened`] triggers.
    /// Drain queued `Opened` ids. Deduplicates so each doc fires its
    /// observers at most once per drain even if the open pipeline
    /// races pushed the same id multiple times (e.g.
    /// `reserve_id` + `install_prebuilt` + an explicit late
    /// `register` from a second observer).
    pub fn drain_pending_opened(&mut self) -> Vec<DocumentId> {
        let raw = std::mem::take(&mut self.pending_opened);
        let mut seen: std::collections::HashSet<DocumentId> = std::collections::HashSet::new();
        raw.into_iter().filter(|id| seen.insert(*id)).collect()
    }

    /// Drain queued Closed notifications. Fanned out as
    /// [`lunco_doc_bevy::DocumentClosed`] triggers. Deduped — Closed
    /// is once-per-lifecycle so duplicates are spurious.
    pub fn drain_pending_closed(&mut self) -> Vec<DocumentId> {
        let raw = std::mem::take(&mut self.pending_closed);
        let mut seen: std::collections::HashSet<DocumentId> = std::collections::HashSet::new();
        raw.into_iter().filter(|id| seen.insert(*id)).collect()
    }

    /// Immutable access to a document host by id.
    pub fn host(&self, doc: DocumentId) -> Option<&DocumentHost<ModelicaDocument>> {
        self.hosts.get(&doc)
    }

    /// Mutable access to a document host by id.
    pub fn host_mut(&mut self, doc: DocumentId) -> Option<&mut DocumentHost<ModelicaDocument>> {
        self.hosts.get_mut(&doc)
    }

    /// Iterate over every known `(DocumentId, &DocumentHost)` pair.
    /// Used by registry-wide scanners (e.g. the debounced AST-refresh
    /// driver) that need to check state on every doc without knowing
    /// ids up front.
    pub fn docs(&self) -> impl Iterator<Item = (DocumentId, &DocumentHost<ModelicaDocument>)> {
        self.hosts.iter().map(|(id, host)| (*id, host))
    }

    /// Which [`lunco_doc::DocumentId`] does `entity` reference, if any.
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
        }
    }
}

// `mirror_active_open_model` deleted (2026-05-08).
// The `WorkbenchState::open_model` cache it kept fresh is gone;
// readers derive source/metadata from
// `ModelicaDocumentRegistry::host(doc).document()` directly.

#[cfg(test)]
mod tests {
    use super::*;

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
        // Fresh doc is gen 1 from construction (placeholder-AST staleness
        // seed); allocate itself applies no op on top of that.
        assert_eq!(host.generation(), 1, "allocate doesn't apply an op");
    }

    #[test]
    fn checkpoint_applies_op_on_change() {
        let mut reg = ModelicaDocumentRegistry::default();
        let doc = reg.allocate("model A end A;".into());

        let changed = reg.checkpoint_source(doc, "model B end B;".into());
        assert!(changed);

        let host = reg.host(doc).unwrap();
        assert_eq!(host.document().source(), "model B end B;");
        assert_eq!(host.generation(), 2); // gen 1 fresh + 1 checkpoint op
        assert!(host.can_undo());
    }

    #[test]
    fn replay_op_applies_without_recording_and_marks_changed() {
        let mut reg = ModelicaDocumentRegistry::default();
        let doc = reg.allocate("model A end A;".into());
        let _ = reg.drain_pending_opened(); // clear allocate-time events
        let _ = reg.drain_pending_changes();
        let gen0 = reg.host(doc).unwrap().generation();

        // A journal op payload = a serialized ModelicaOp.
        let op =
            serde_json::to_value(ModelicaOp::ReplaceSource { new: "model B end B;".into() }).unwrap();
        assert!(reg.replay_op(doc, &op), "valid op replays");

        let host = reg.host(doc).unwrap();
        assert_eq!(host.document().source(), "model B end B;");
        assert!(host.generation() > gen0);
        // Applying via the Document (not the DocumentHost) bypasses the recorder
        // AND the undo stack — so replay pushes no undo entry (idempotent
        // projection of history that's already in the journal).
        assert!(!host.can_undo(), "replay does not record/undo");
        // Marked changed for reprojection.
        assert_eq!(reg.drain_pending_changes(), vec![doc]);
        // A non-ModelicaOp payload and an unknown doc are rejected, not panics.
        assert!(!reg.replay_op(doc, &serde_json::json!({ "nope": 1 })));
        assert!(!reg.replay_op(DocumentId::new(9999), &op));
    }

    #[test]
    fn checkpoint_no_op_when_source_unchanged() {
        let mut reg = ModelicaDocumentRegistry::default();
        let doc = reg.allocate("same".into());

        let changed = reg.checkpoint_source(doc, "same".into());
        assert!(!changed, "re-checkpointing identical source must not bump generation");
        assert_eq!(reg.host(doc).unwrap().generation(), 1); // unchanged fresh gen 1
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
