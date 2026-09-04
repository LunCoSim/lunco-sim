//! Doc-backed twin default scene — web-ready via the twin asset source.
//!
//! This is the **doc-backed live-projection path**: the default twin scene loads
//! through the `twin://` asset source and the async [`UsdLoader`], which
//! re-attaches the scheme so co-located refs (terrain `.glb`) resolve on every
//! platform the source supports. It is made doc-backed by serving the scene
//! document's **composed** (`base ⊕ runtime`) source as a *byte-overlay* on the
//! twin source, so the live world composes from the editable document and
//! runtime spawns/moves appear live.
//!
//! Flow (doc-first: the document exists and its composed source is the overlay
//! BEFORE the scene mounts, so the world is projected exactly once):
//! 1. On `TwinAssetMounted` with a `[usd] default_scene`, kick an async
//!    [`UsdSourceText`] load of `twin://<name>/<scene>` (raw base layer, read
//!    through the twin source — web-ready) and record it in [`PendingTwinDocs`].
//!    The scene mount is admitted after the source asset reaches a terminal
//!    success or failure event.
//! 2. [`drain_pending_twin_docs`] — once the source asset emits its terminal
//!    event, allocate a
//!    [`UsdDocument`](crate::document) for it (origin = the on-disk path, so Save
//!    and dedup work), restore its persisted `.lunco/runtime` overlay, publish
//!    the composed source as the twin overlay, record it in
//!    [`DocBackedTwinScenes`] (synced at the current generation), and only then
//!    fire `LoadScene` — the single mount composes `base ⊕ runtime`.
//! 3. [`sync_twin_overlays`] — on an authored document or stage-lifecycle event
//!    (initial mount, open-time `restore_runtime`, or a later spawn/move), refresh the
//!    twin **overlay** (for persistence / re-open) and **author the delta onto
//!    the live composed stage**: translates and structural spawns/removes are
//!    authored onto the scene's [`CanonicalStage`](lunco_usd_bevy::CanonicalStage)
//!    directly, firing its openusd change sink so `project_stage_changes`
//!    projects the edit in place — no whole-scene asset reload. A referenced
//!    spawn whose asset isn't loaded yet is fetched once through
//!    [`drain_ref_spawns`], then authored the same way.
//!
//! Ownership: a default Twin scene gets a scene lease in
//! [`DocBackedTwinScenes`]. An explicit file open, new document, or authored edit
//! promotes that document to a user lease. Closing a Twin releases its scene
//! leases and removes documents with no remaining user lease; user documents
//! remain available for the document UI and can be reused when the Twin opens
//! again. This keeps runtime projection state bounded without discarding work
//! the user explicitly opened or authored.
//!
//! Scope: the **default Twin scene** uses the document overlay. An explicit
//! file outside the active Twin first opens its owning root and then follows
//! that same doc-first mount. Files inside the active Twin remain document-only;
//! scheme-qualified scene sources enter the typed `LoadScene` path directly.

use crate::document::UsdDocument;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::asset::AssetId;
use bevy::prelude::*;
use lunco_assets::twin_source::TwinRoots;
use lunco_doc::{Document, DocumentId};
use lunco_usd_bevy::{
    UsdAwaitingStage, UsdInstanceProjection, UsdPrimPath, UsdRead, UsdSceneRoot, UsdSourceText,
    UsdStageAsset, UsdVisualProjectionQueued, UsdVisualSynced,
};
use lunco_usd_sim::cosim::LoadScene;

use crate::commands::{EmptyViewportReason, TWIN_SCENE_LOAD_FAILED};
use crate::document::UsdOp;
use lunco_doc::OpenOutcome;
use lunco_doc_bevy::{DocumentChanged, DocumentRegistry};

/// A USD document transitioned from a Twin-only scene lease to a user-facing
/// session lease. The UI uses this to expose a document only after the user
/// explicitly opened, created, or authored it.
#[derive(Event, Clone, Copy, Debug)]
pub struct UsdDocumentUserOwned {
    /// Document whose user-session lease was created.
    pub doc: DocumentId,
}

/// Marks a live prim entity that refreshes its own content **in place**, so the
/// twin projection must NOT structurally despawn/reload it on an attribute-only
/// document change. The DEM terrain sets this: its heavy base grid is retained
/// and re-stamped from the registry document on edits (the sandbox's
/// `refresh_docbacked_terrain_from_doc`), so a whole-scene reload would force a
/// full GeoTIFF re-read per edit. Consumed by [`sync_twin_overlays`], which
/// suppresses the reload when a generation's only structural trigger is
/// attribute edits confined to such a subtree.
#[derive(Component)]
pub struct LiveRebuildExempt;

/// A default-twin-scene document waiting for its base source text to finish
/// loading through the twin source.
struct PendingTwinDoc {
    /// In-flight raw-source load of `twin://<name>/<rel>`.
    handle: Handle<UsdSourceText>,
    /// Twin name (the `twin://` first segment).
    name: String,
    /// Scene path relative to the twin root (the `twin://` remainder).
    rel: String,
    /// On-disk absolute path — the document origin (Save target + dedup key).
    abs_path: PathBuf,
    /// Workspace Twin root that owns this pending projection request. The
    /// document receives a scene lease only after the source is ready and is
    /// retired with this root if the Twin closes first.
    root: PathBuf,
}

/// Default twin scenes whose base source is still loading. Drained by
/// [`drain_pending_twin_docs`].
#[derive(Resource, Default)]
pub struct PendingTwinDocs {
    items: Vec<PendingTwinDoc>,
    ready: HashSet<AssetId<UsdSourceText>>,
    failed: HashMap<AssetId<UsdSourceText>, String>,
}

impl PendingTwinDocs {
    /// Queue a default twin scene for doc-backed projection.
    pub fn push(
        &mut self,
        handle: Handle<UsdSourceText>,
        ready: bool,
        name: String,
        rel: String,
        abs_path: PathBuf,
        root: PathBuf,
    ) {
        if ready {
            self.ready.insert(handle.id());
        }
        self.items.push(PendingTwinDoc {
            handle,
            name,
            rel,
            abs_path,
            root,
        });
    }

    fn mark_ready(&mut self, id: AssetId<UsdSourceText>) {
        if self.items.iter().any(|item| item.handle.id() == id) {
            self.ready.insert(id);
        }
    }

    pub(crate) fn mark_failed(&mut self, id: AssetId<UsdSourceText>, error: String) {
        if self.items.iter().any(|item| item.handle.id() == id) {
            self.failed.insert(id, error);
        }
    }

    fn has_terminal_source_event(&self) -> bool {
        !self.ready.is_empty() || !self.failed.is_empty()
    }

    /// Release pending projection work for a closed Twin.
    pub fn release_root(&mut self, root: &Path) {
        self.items
            .retain(|item| !lunco_doc::same_file(&item.root, root));
        let live_ids: HashSet<_> = self.items.iter().map(|item| item.handle.id()).collect();
        self.ready.retain(|id| live_ids.contains(id));
        self.failed.retain(|id, _| live_ids.contains(id));
    }
}

/// The twin-source coordinates + last-synced generation for a doc-backed twin
/// scene, so [`sync_twin_overlays`] re-serializes only when the document moved.
struct TwinSceneRef {
    /// Workspace Twin roots that own this live projection. This is distinct
    /// from the document origin: a user-opened document may outlive its Twins,
    /// and the same file can be projected by more than one open Twin.
    roots: Vec<PathBuf>,
    name: String,
    rel: String,
    /// Number of open editor preview leases using these coordinates. A
    /// synthetic document projection has no Twin root, so this count is the
    /// lifetime owner for its `TwinRoots` registration.
    preview_leases: usize,
    synced_generation: Option<u64>,
    /// Generation the **persistence overlay** was last serialized at. Tracked apart
    /// from `synced_generation` so the expensive whole-stage serialization happens
    /// at the explicit settle boundary, not for every brush stroke, while live
    /// projection still applies each op immediately.
    overlay_synced_generation: Option<u64>,
}

/// Map of document → the twin scene it backs. Populated by
/// [`drain_pending_twin_docs`], consumed by [`sync_twin_overlays`].
#[derive(Resource, Default)]
pub struct DocBackedTwinScenes {
    map: HashMap<DocumentId, TwinSceneRef>,
    /// Documents explicitly opened, created, or authored by the user. A
    /// document can have both this lease and a live Twin scene lease.
    user_owned: HashSet<DocumentId>,
}

impl DocBackedTwinScenes {
    /// The registry document backing the twin scene at `twin://<name>/<rel>`, if
    /// any. Lets a twin-projected consumer (e.g. a DEM terrain, which carries no
    /// document-backed scene) recover its authoring document from its `twin://` stage
    /// asset path.
    pub fn doc_for(&self, name: &str, rel: &str) -> Option<DocumentId> {
        self.map
            .iter()
            .find(|(_, s)| s.name == name && s.rel == rel)
            .map(|(doc, _)| *doc)
    }

    /// The `twin://` coordinates (`name`, `rel`) a document is already backed
    /// under, if any — so a second consumer (e.g. the editor viewport) reuses
    /// the same overlay + asset instead of registering a duplicate.
    pub fn coords_of(&self, doc: DocumentId) -> Option<(String, String)> {
        self.map.get(&doc).map(|s| (s.name.clone(), s.rel.clone()))
    }

    /// Claim a document for the user-facing document session. Returns `true`
    /// only when this call created the claim, allowing callers to publish one
    /// ownership transition without duplicating lifecycle events.
    pub fn claim_user(&mut self, doc: DocumentId) -> bool {
        self.user_owned.insert(doc)
    }

    /// Whether the document has a user-facing lease.
    pub fn is_user_owned(&self, doc: DocumentId) -> bool {
        self.user_owned.contains(&doc)
    }

    /// Track an already-allocated document as doc-backed under `(name, rel)`, so
    /// [`sync_twin_overlays`] keeps its overlay + live entities in step with the
    /// document generation. Idempotent — a document already tracked (e.g. a
    /// default twin scene) keeps its existing coordinates.
    pub fn track(&mut self, doc: DocumentId, root: PathBuf, name: String, rel: String) {
        if let Some(scene) = self.map.get_mut(&doc) {
            if !scene
                .roots
                .iter()
                .any(|existing| lunco_doc::same_file(existing, &root))
            {
                scene.roots.push(root);
            }
            return;
        }
        self.map.insert(
            doc,
            TwinSceneRef {
                roots: vec![root],
                name,
                rel,
                preview_leases: 0,
                synced_generation: None,
                overlay_synced_generation: None,
            },
        );
    }

    /// Track an editor preview without pretending that it is a workspace Twin
    /// root. Multiple preview leases share these coordinates and therefore
    /// the same `UsdStageAsset` and `CanonicalStage`.
    pub fn track_preview(&mut self, doc: DocumentId, name: String, rel: String) {
        if self.map.contains_key(&doc) {
            return;
        }
        self.map.insert(
            doc,
            TwinSceneRef {
                roots: Vec::new(),
                name,
                rel,
                preview_leases: 0,
                synced_generation: None,
                overlay_synced_generation: None,
            },
        );
    }

    /// Acquire one explicit editor preview lease for a tracked document.
    pub fn acquire_preview(&mut self, doc: DocumentId) {
        if let Some(scene) = self.map.get_mut(&doc) {
            scene.preview_leases = scene.preview_leases.saturating_add(1);
        }
    }

    /// Release one editor preview lease. Returns synthetic Twin coordinates
    /// only when the final preview closes and no workspace Twin owns the
    /// document, allowing the caller to unregister the authority exactly once.
    pub fn release_preview(&mut self, doc: DocumentId) -> Option<(String, String)> {
        let scene = self.map.get_mut(&doc)?;
        if scene.preview_leases == 0 {
            return None;
        }
        scene.preview_leases -= 1;
        if scene.preview_leases == 0 && scene.roots.is_empty() {
            let scene = self.map.remove(&doc)?;
            return Some((scene.name, scene.rel));
        }
        None
    }

    /// Release the scene lease for a closed Twin and return documents that no
    /// longer have any owner. User-owned documents stay in the registry.
    pub fn release_root(&mut self, root: &Path) -> Vec<DocumentId> {
        let mut released = Vec::new();
        self.map.retain(|doc, scene| {
            scene
                .roots
                .retain(|existing| !lunco_doc::same_file(existing, root));
            if scene.roots.is_empty() && scene.preview_leases == 0 {
                released.push(*doc);
                false
            } else {
                true
            }
        });
        released
            .into_iter()
            .filter(|doc| !self.user_owned.contains(doc))
            .collect()
    }

    /// Forget a document after its registry host has been removed.
    pub fn forget_document(&mut self, doc: DocumentId) -> Option<(String, String)> {
        let synthetic = self
            .map
            .remove(&doc)
            .and_then(|scene| scene.roots.is_empty().then_some((scene.name, scene.rel)));
        self.user_owned.remove(&doc);
        synthetic
    }

    /// Drop only the current projection coordinates while retaining preview
    /// leases. The viewport uses this when a Twin authority disappears and
    /// the document must be rehomed under its preview-owned source.
    pub fn detach_projection(&mut self, doc: DocumentId) {
        if let Some(scene) = self.map.get_mut(&doc) {
            if scene.preview_leases > 0 {
                // Keep the preview lease and its stable coordinates alive while
                // the workspace Twin authority is being replaced. The next
                // preview mount re-registers the same synthetic authority.
                scene.roots.clear();
                return;
            }
        }
        self.map.remove(&doc);
    }
}

/// The editable document backing a running scene's stage asset, if the scene is
/// **doc-backed** (loaded as `twin://<name>/<rel>`). Returns `None` for a raw-file
/// scene — which has no savable source document, so a caller (e.g. saving a
/// live-edited scenario back onto its prim) must refuse rather than silently drop
/// the edit. This is the asset↔document bridge that unblocks scenario save-back:
/// a runtime entity carries a `UsdPrimPath { stage_handle, path }`, and this maps
/// that stage handle to the `DocumentRegistry<UsdDocument>` document you can `ApplyUsdOp` on.
pub fn scene_document_for(
    backed: &DocBackedTwinScenes,
    asset_server: &AssetServer,
    scene: AssetId<UsdStageAsset>,
) -> Option<DocumentId> {
    // `AssetPath::path()` is the path WITHOUT the `twin://` source scheme, i.e.
    // `<name>/<rel>`. `rel` may contain slashes (`scenes/luncosim/scene.usda`), so
    // split only on the FIRST one. (Same idiom as `cache_terrain_document`.)
    let asset_path = asset_server.get_path(scene)?;
    let rel_path = asset_path.path().to_string_lossy();
    let (name, rel) = lunco_assets::split_twin_rel(&rel_path)?;
    backed.doc_for(name, rel)
}

/// A **referenced spawn** whose asset closure is being fetched before it can be
/// authored onto the live scene stage. When a structural edit adds a prim that
/// references an asset whose layer bytes aren't loaded into the scene's live
/// resolver yet (a first-of-its-kind rover spawn), [`sync_twin_overlays`] loads
/// that asset as a `UsdStageAsset` (whose loader fetches the full closure,
/// web-ready) and queues this. [`drain_ref_spawns`] injects the fetched bytes
/// into the scene stage's resolver and authors the prim + `references` arc, so
/// the openusd change sink fires and `project_stage_changes` instantiates the
/// composed subtree — no whole-scene reload.
struct RefSpawn {
    /// The scene whose live [`CanonicalStage`](lunco_usd_bevy::CanonicalStage)
    /// the spawn is authored onto.
    scene_id: AssetId<UsdStageAsset>,
    /// The prim path to spawn (e.g. `/World/rover_1`).
    prim_path: String,
    /// The prim's composed `typeName`, authored before the reference.
    type_name: Option<String>,
    /// The reference asset path exactly as authored in the document — PCP
    /// re-derives its canonical id against the scene layer, matching the id the
    /// closure bytes are injected under.
    asset_path: String,
    /// In-flight load of the referenced asset (its loader fetches the closure).
    ref_handle: Handle<UsdStageAsset>,
    /// A SetTranslate may follow AddPrim in the same edit burst. Keep it until
    /// the reference closure is installed; otherwise the edit arrives before
    /// the prim exists on the live stage and the new waypoint stays at origin.
    translate: Option<[f64; 3]>,
    /// Child-scoped edits that arrive while the referenced root is still
    /// loading. They are replayed after the reference and its composed
    /// subtree exist, preserving the original ordered document intent.
    deferred_ops: Vec<UsdOp>,
}

/// Referenced spawns waiting on their asset closure to finish loading.
/// Populated by [`sync_twin_overlays`], drained by [`drain_ref_spawns`].
#[derive(Resource, Default)]
pub struct PendingRefSpawns {
    items: Vec<RefSpawn>,
    ready: HashSet<AssetId<UsdStageAsset>>,
    failed: HashMap<AssetId<UsdStageAsset>, String>,
}

/// Prepared source plans waiting for the live-stage sink to create their
/// corresponding instance root. The key is the canonical scene plus authored
/// prim path, so a plan can never be attached to another Twin or another
/// instance with the same asset.
#[derive(Resource, Default)]
pub(crate) struct PendingInstanceProjections {
    plans: HashMap<(AssetId<UsdStageAsset>, String), UsdInstanceProjection>,
}

impl PendingInstanceProjections {
    fn insert(
        &mut self,
        scene_id: AssetId<UsdStageAsset>,
        prim_path: String,
        projection: UsdInstanceProjection,
    ) {
        self.plans.insert((scene_id, prim_path), projection);
    }

    pub(crate) fn take(
        &mut self,
        scene_id: AssetId<UsdStageAsset>,
        prim_path: &str,
    ) -> Option<UsdInstanceProjection> {
        self.plans.remove(&(scene_id, prim_path.to_string()))
    }

    pub(crate) fn remove(&mut self, scene_id: AssetId<UsdStageAsset>, prim_path: &str) {
        self.plans.remove(&(scene_id, prim_path.to_string()));
    }
}

impl PendingRefSpawns {
    fn push(&mut self, item: RefSpawn, ready: bool) {
        if ready {
            self.ready.insert(item.ref_handle.id());
        }
        self.items.push(item);
    }

    fn mark_ready(&mut self, id: AssetId<UsdStageAsset>) {
        if self.items.iter().any(|item| item.ref_handle.id() == id) {
            self.ready.insert(id);
        }
    }

    fn mark_failed(&mut self, id: AssetId<UsdStageAsset>, error: String) {
        if self.items.iter().any(|item| item.ref_handle.id() == id) {
            self.failed.insert(id, error);
        }
    }

    fn has_terminal_asset_event(&self) -> bool {
        !self.ready.is_empty() || !self.failed.is_empty()
    }
}

/// Clear asynchronous referenced-spawn work owned by the outgoing scene.
///
/// Pending default-scene document loads are owned by their admitted Twin and
/// are released through [`PendingTwinDocs::release_root`] when that Twin closes.
pub(crate) fn reset_scene_projection_state(
    mut pending_refs: ResMut<PendingRefSpawns>,
    mut pending_instances: Option<ResMut<PendingInstanceProjections>>,
) {
    pending_refs.items.clear();
    pending_refs.ready.clear();
    pending_refs.failed.clear();
    if let Some(pending_instances) = pending_instances.as_deref_mut() {
        pending_instances.plans.clear();
    }
}

/// Report a terminal failure of the authoritative default-scene source.
///
/// A doc-backed Twin has exactly one valid mount input: the composed document
/// served through its `twin://` identity. If that source never arrives, the
/// transaction ends empty and diagnosable. Mounting the raw file would create a
/// second projection with different ownership and silently lose runtime edits.
fn report_twin_doc_load_failed(
    empty_reason: &mut EmptyViewportReason,
    commands: &mut Commands,
    twin_path: &str,
    detail: impl Into<String>,
) {
    let detail = detail.into();
    warn!("[usd-e1b] {detail}");
    empty_reason.0 = Some(format!("`{twin_path}` could not be loaded: {detail}"));
    lunco_core::trigger_error(commands, TWIN_SCENE_LOAD_FAILED, detail);
}

/// Transfer source-asset lifecycle events into the pending document transaction.
/// A pending scene is processed only after the source asset is present or has
/// failed; no frame-count or readiness poll is needed.
pub(crate) fn mark_pending_twin_docs(
    mut pending: ResMut<PendingTwinDocs>,
    mut events: MessageReader<bevy::asset::AssetEvent<UsdSourceText>>,
    mut failures: MessageReader<bevy::asset::AssetLoadFailedEvent<UsdSourceText>>,
) {
    for event in events.read() {
        match event {
            bevy::asset::AssetEvent::Added { id }
            | bevy::asset::AssetEvent::Modified { id }
            | bevy::asset::AssetEvent::LoadedWithDependencies { id } => pending.mark_ready(*id),
            bevy::asset::AssetEvent::Removed { id } | bevy::asset::AssetEvent::Unused { id } => {
                pending.mark_failed(*id, "the source asset was removed before mounting".into());
            }
        }
    }
    for failure in failures.read() {
        pending.mark_failed(failure.id, failure.error.to_string());
    }
}

pub(crate) fn pending_twin_docs_ready(pending: Res<PendingTwinDocs>) -> bool {
    pending.has_terminal_source_event()
}

/// Transfer referenced-asset lifecycle events into the pending spawn
/// transactions. The spawn drain consumes only these terminal asset signals.
pub(crate) fn mark_pending_ref_spawns(
    mut pending: ResMut<PendingRefSpawns>,
    mut events: MessageReader<bevy::asset::AssetEvent<UsdStageAsset>>,
    mut failures: MessageReader<bevy::asset::AssetLoadFailedEvent<UsdStageAsset>>,
) {
    for event in events.read() {
        match event {
            bevy::asset::AssetEvent::Added { id }
            | bevy::asset::AssetEvent::Modified { id }
            | bevy::asset::AssetEvent::LoadedWithDependencies { id } => pending.mark_ready(*id),
            bevy::asset::AssetEvent::Removed { id } | bevy::asset::AssetEvent::Unused { id } => {
                pending.mark_failed(
                    *id,
                    "the referenced asset was removed before mounting".into(),
                );
            }
        }
    }
    for failure in failures.read() {
        pending.mark_failed(failure.id, failure.error.to_string());
    }
}

pub(crate) fn pending_ref_spawns_ready(pending: Res<PendingRefSpawns>) -> bool {
    pending.has_terminal_asset_event()
}

/// Allocate the document for each pending twin scene once its base source text
/// has loaded through the twin source, restore its persisted runtime overlay,
/// publish the composed (`base ⊕ runtime`) source as the twin overlay, and then
/// mount the scene ([`LoadScene`]). The async stage load reads the overlay bytes,
/// so the initial projection composes the complete document state. The registry
/// supplies one document for each file origin and preserves its ownership state.
pub(crate) fn drain_pending_twin_docs(
    mut pending: ResMut<PendingTwinDocs>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut backed: ResMut<DocBackedTwinScenes>,
    mut wake: ResMut<TwinProjectionWake>,
    sources: Res<Assets<UsdSourceText>>,
    twin_roots: Res<TwinRoots>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut empty_reason: ResMut<EmptyViewportReason>,
    mut commands: Commands,
) {
    if pending.items.is_empty() {
        return;
    }
    let ready = std::mem::take(&mut pending.ready);
    let failed = std::mem::take(&mut pending.failed);
    let taken = std::mem::take(&mut pending.items);
    let mut still = Vec::new();
    for item in taken {
        let twin_path = lunco_assets::twin_uri(&item.name, &item.rel);
        if let Some(error) = failed.get(&item.handle.id()) {
            report_twin_doc_load_failed(
                &mut empty_reason,
                &mut commands,
                &twin_path,
                format!("the Twin source asset failed to load: {error}"),
            );
            continue;
        }
        if !ready.contains(&item.handle.id()) {
            still.push(item);
            continue;
        }
        let Some(UsdSourceText(source)) = sources.get(&item.handle) else {
            report_twin_doc_load_failed(
                &mut empty_reason,
                &mut commands,
                &twin_path,
                "the source asset emitted a ready event without a stored value",
            );
            continue;
        };
        // The asset pipeline owns source freshness and supplies the bytes.
        let source = source.as_str();

        // The registry owns one-document-per-file deduplication and dirty-document
        // preservation for the source delivered by the asset pipeline.
        let (doc, outcome) = registry.open_file(item.abs_path.clone(), source.to_string());
        match outcome {
            OpenOutcome::KeptUnparsable => {
                report_twin_doc_load_failed(
                    &mut empty_reason,
                    &mut commands,
                    &twin_path,
                    "the source asset is not valid USDA; refusing to mount a stale document",
                );
                continue;
            }
            OpenOutcome::KeptDirty => warn!(
                "[usd-e1b] `{twin_path}` has unsaved edits — keeping them; NOT re-reading from disk"
            ),
            OpenOutcome::Allocated | OpenOutcome::Refreshed => {}
        }
        // Restore the persisted `.lunco/runtime` overlay NOW, before the mount
        // below. The `DocumentOpened` observer restore fires a flush later —
        // after the stage load has already read its bytes. Guarded: whichever
        // runs second is a no-op.
        if let Some(ws) = workspace.as_deref() {
            crate::runtime_persistence::restore_doc_runtime(ws, &mut registry, doc);
        }
        // Publish the composed source as the twin overlay so the stage build
        // reads `base ⊕ runtime`, and mark the scene synced at this generation —
        // every op through it is reflected by the mount itself, so
        // `sync_twin_overlays` only has to project edits made AFTER open.
        let (cur_gen, composed) = match registry.host(doc) {
            Some(h) => (h.document().generation(), h.document().composed_source()),
            None => continue,
        };
        if let Err(error) =
            twin_roots.set_overlay(&item.name, &item.rel, Arc::new(composed.into_bytes()))
        {
            report_twin_doc_load_failed(
                &mut empty_reason,
                &mut commands,
                &twin_path,
                format!("could not publish the composed Twin source: {error}"),
            );
            continue;
        }
        backed.track(doc, item.root.clone(), item.name.clone(), item.rel.clone());
        wake.wake();
        if let Some(scene) = backed.map.get_mut(&doc) {
            scene.synced_generation = Some(cur_gen);
            scene.overlay_synced_generation = Some(cur_gen);
        }
        info!("[usd-e1b] default scene `{twin_path}` is doc-backed ({doc}) — mounting composed");
        commands.trigger(LoadScene {
            path: twin_path,
            root_prim: String::new(),
        });
    }
    pending.items.extend(still);
}

/// Keep each doc-backed twin scene's twin-source overlay and live stage in step
/// with its document. The projection is woken by document and asset lifecycle
/// events; it does not poll every frame. Persistence is serialized once after
/// the live edit reaches the settled generation. Drops entries whose document
/// has closed.
/// Serialize a doc-backed scene's composed source into its twin overlay (the
/// persistence / next-load source) and mark it overlay-synced at `gen`. O(stage) — a
/// whole-stage recompose + serialize — so call it only once the document has SETTLED
/// (see the settle step in [`sync_twin_overlays`]), never on every edit.
fn write_twin_overlay(world: &mut World, doc: DocumentId, name: &str, rel: &str, gen: u64) -> bool {
    let composed_source = world
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)
        .map(|h| h.document().composed_source());
    if let Some(src) = composed_source {
        if let Err(error) =
            world
                .resource::<TwinRoots>()
                .set_overlay(name, rel, Arc::new(src.into_bytes()))
        {
            warn!("[usd-e1b] could not publish composed source for document {doc}: {error}");
            return false;
        }
        if let Some(s) = world
            .resource_mut::<DocBackedTwinScenes>()
            .map
            .get_mut(&doc)
        {
            s.overlay_synced_generation = Some(gen);
        }
        true
    } else {
        false
    }
}

/// A live projection finished an authored generation and may persist it after
/// the current edit burst settles. The message is read at the next
/// `PreUpdate`, which is the explicit settle boundary; no timer or generation
/// polling is needed.
#[derive(Message, Clone, Copy, Debug)]
pub(crate) struct TwinProjectionSettle {
    doc: DocumentId,
    generation: u64,
}

/// Persist settled document overlays in the frame after live projection.
///
/// The queued closure re-checks the generation before serializing. This keeps
/// an older settle message from marking a newer document generation as saved
/// when another edit arrived before deferred commands were flushed.
pub(crate) fn settle_twin_overlays(
    mut messages: MessageReader<TwinProjectionSettle>,
    backed: Res<DocBackedTwinScenes>,
    mut commands: Commands,
) {
    for message in messages.read() {
        let Some((name, rel)) = backed.coords_of(message.doc) else {
            continue;
        };
        let doc = message.doc;
        let generation = message.generation;
        commands.queue(move |world: &mut World| {
            let current_generation = world
                .resource::<DocumentRegistry<UsdDocument>>()
                .host(doc)
                .map(|host| host.document().generation());
            let overlay_generation = world
                .resource::<DocBackedTwinScenes>()
                .map
                .get(&doc)
                .and_then(|scene| scene.overlay_synced_generation);
            if current_generation != Some(generation) || overlay_generation == Some(generation) {
                return;
            }
            write_twin_overlay(world, doc, &name, &rel, generation);
        });
    }
}

pub(crate) fn sync_twin_overlays(world: &mut World) {
    // DocumentChanged, stage-asset lifecycle events, scene mounts, and viewport
    // installs all wake this owner. Consume the wake before inspecting the
    // tracked set so the normal render loop never performs a generation probe.
    world.resource_mut::<TwinProjectionWake>().consume();

    // Snapshot tracked scenes (owned) so no resource borrow is held across the
    // world mutations below.
    let entries: Vec<(DocumentId, String, String, Option<u64>, Option<u64>)> = world
        .resource::<DocBackedTwinScenes>()
        .map
        .iter()
        .map(|(doc, s)| {
            (
                *doc,
                s.name.clone(),
                s.rel.clone(),
                s.synced_generation,
                s.overlay_synced_generation,
            )
        })
        .collect();

    // A twin scene projects only when it is the scene currently mounted.
    // Keeping that admission check here makes projection ownership explicit:
    // exactly one simulation scene and the active editor preview may consume a
    // tracked document at a time.
    //
    // `None` means no scene root exists yet — mid-load, between the old root's
    // despawn and the new one's spawn. Project nothing rather than everything:
    // the incoming scene resumes on the tick its root appears.
    let mounted: Option<AssetId<UsdStageAsset>> = {
        let mut q = world.query_filtered::<&UsdPrimPath, With<UsdSceneRoot>>();
        q.iter(world).next().map(|p| p.stage_handle.id())
    };
    let active_doc: Option<DocumentId> = mounted.and_then(|id| {
        let path = world.resource::<AssetServer>().get_path(id)?;
        let rel = path.path().to_string_lossy().into_owned();
        let (name, rel) = lunco_assets::split_twin_rel(&rel)?;
        world.resource::<DocBackedTwinScenes>().doc_for(name, rel)
    });
    // Editor previews are additional legitimate mounts: their roots are
    // deliberately NOT `UsdSceneRoot` (they are `UsdPreviewOnly`, so sim/avian
    // walkers bail at them), so the query above never sees them. Every open
    // preview document must still be admitted here; otherwise a document edit
    // would update only the focused preview and leave other leases stale.
    // Without the `ui` feature there are no editor mounts to consider.
    #[cfg(feature = "ui")]
    let viewport_docs: HashSet<DocumentId> = world
        .get_resource::<crate::ui::viewport::UsdViewportState>()
        .map(|state| state.preview_docs().collect())
        .unwrap_or_default();
    #[cfg(not(feature = "ui"))]
    let viewport_docs: HashSet<DocumentId> = HashSet::new();

    for (doc, name, rel, synced, overlay_synced) in entries {
        if active_doc != Some(doc) && !viewport_docs.contains(&doc) {
            continue;
        }
        // Read the generation before any whole-stage payload. The composed source
        // is serialized only when this event-driven owner observes a new
        // generation, never on the render loop.
        let cur_gen = match world.resource::<DocumentRegistry<UsdDocument>>().host(doc) {
            Some(h) => h.document().generation(),
            None => {
                if let Err(error) = world.resource::<TwinRoots>().clear_overlay(&name, &rel) {
                    warn!("[usd-e1b] could not clear closed document overlay: {error}");
                }
                world
                    .resource_mut::<DocBackedTwinScenes>()
                    .forget_document(doc);
                continue;
            }
        };
        if Some(cur_gen) == synced {
            // Live projection is already up to date. Persistence is handled by
            // the explicit one-frame settle message, not by rechecking this
            // document on every render frame.
            continue;
        }

        #[cfg(feature = "ui")]
        if let Some(mut viewport) =
            world.get_resource_mut::<crate::ui::viewport::UsdViewportState>()
        {
            // Document generation is not visual readiness. Invalidate every
            // preview lease before the canonical stage applies this generation;
            // the viewport marks it ready only after the USD queue and async
            // mesh phase have both settled.
            viewport.invalidate_projection(doc);
        }

        // Author-once: the scene's live stage is keyed by the cached
        // `twin://name/rel` UsdStageAsset id (AssetServer dedups by path). We
        // replay the **typed ops** the document recorded since the last sync
        // directly onto that stage — the op is the single delta description, so we
        // never re-derive an edit's value by reading it back out of `composed`.
        let twin_path = lunco_assets::twin_uri(&name, &rel);
        let scene_id = world
            .resource::<AssetServer>()
            .load::<UsdStageAsset>(twin_path.clone())
            .id();

        // `None` = the op ring overflowed (more edits than capacity since the last
        // sync) → we can't trust an incremental replay, so rebuild.
        let ops = world
            .resource::<DocumentRegistry<UsdDocument>>()
            .host(doc)
            .and_then(|h| h.document().ops_since(synced.unwrap_or(0)));
        let has_work = synced.is_none() || ops.as_ref().map(|o| !o.is_empty()).unwrap_or(true);

        if synced.is_none() {
            // First mount MUST publish the overlay so the async stage load composes
            // base ⊕ runtime from it. The prepared asset plan already contains
            // this composed document, so initial projection does not need a live
            // `Stage` on the UI thread.
            // Already done at this generation for a twin default scene
            // (`drain_pending_twin_docs` publishes before mounting); still needed
            // here for editor-viewport docs tracked via `track()`.
            if overlay_synced != Some(cur_gen) {
                if !write_twin_overlay(world, doc, &name, &rel, cur_gen) {
                    continue;
                }
            }
            // The prepared plan is the complete initial projection. Runtime
            // edits are not replayed here: the document generation becomes the
            // live-stage edit boundary below, where the canonical stage is
            // created explicitly and the typed journal is applied once.
        } else {
            // Initial materialisation deliberately leaves the non-Send live
            // stage closed. Once an authored generation exists, the edit
            // projector owns the transition to the live canonical stage. The
            // recipe is already resident in `UsdStageAsset`, so this is an
            // explicit authoring operation rather than a second initial-load
            // reader or a per-frame rebuild.
            let stage_ready = world
                .get_non_send::<lunco_usd_bevy::CanonicalStages>()
                .is_some_and(|stages| stages.get(scene_id).is_some());
            if has_work && !stage_ready {
                let recipe = world
                    .resource::<Assets<UsdStageAsset>>()
                    .get(scene_id)
                    .and_then(|asset| asset.recipe.as_ref())
                    .cloned();
                let Some(recipe) = recipe else {
                    // The asset loader has not published the recipe yet. Keep
                    // the document generation pending until the asset boundary
                    // makes the canonical stage available.
                    continue;
                };
                let built = world
                    .get_non_send_mut::<lunco_usd_bevy::CanonicalStages>()
                    .is_some_and(|mut stages| stages.get_or_build(scene_id, &recipe).is_some());
                if !built {
                    continue;
                }
            }

            match ops {
                // Overflow, or a coarse op (ReplaceSource / MovePrim / keyframe /
                // relationship — no incremental stage-author yet, and whole-source
                // undo may change surviving prims' attribute values): rebuild the
                // stage from composed_source + the already-loaded closure. (The
                // overlay is refreshed on the next settled frame.)
                None => {
                    let cs = world
                        .resource::<DocumentRegistry<UsdDocument>>()
                        .host(doc)
                        .map(|h| h.document().composed_source())
                        .unwrap_or_default();
                    rebuild_scene_from_composed(world, scene_id, &cs);
                }
                Some(ops)
                    if ops.iter().any(|op| {
                        let waypoint = match op {
                            UsdOp::SetActive { path, .. } => {
                                is_waypoint_prim(world, scene_id, path)
                            }
                            _ => false,
                        };
                        op_needs_rebuild(op, waypoint)
                    }) =>
                {
                    let cs = world
                        .resource::<DocumentRegistry<UsdDocument>>()
                        .host(doc)
                        .map(|h| h.document().composed_source())
                        .unwrap_or_default();
                    rebuild_scene_from_composed(world, scene_id, &cs);
                }
                // Incremental: replay each op's typed delta onto the live stage. NO
                // whole-stage serialize here — the overlay catches up when settled.
                Some(ops) => {
                    for op in &ops {
                        apply_incremental_op_to_stage(world, scene_id, op);
                    }
                }
            }
        }

        if let Some(s) = world
            .resource_mut::<DocBackedTwinScenes>()
            .map
            .get_mut(&doc)
        {
            s.synced_generation = Some(cur_gen);
        }
        if synced.is_some() && Some(cur_gen) != overlay_synced {
            world.write_message(TwinProjectionSettle {
                doc,
                generation: cur_gen,
            });
        }
    }
}

/// Explicit wake-up for the document-backed live projection owner.
///
/// Projection work is event-driven: a document change, an asset completing,
/// or a scene mount calls [`TwinProjectionWake::wake`]. Keeping this state at
/// the projection boundary gives every producer the same scheduling contract
/// without making any producer duplicate projection logic.
#[derive(Resource, Default)]
pub(crate) struct TwinProjectionWake {
    pending: bool,
}

impl TwinProjectionWake {
    pub(crate) fn wake(&mut self) {
        self.pending = true;
    }

    fn consume(&mut self) {
        self.pending = false;
    }
}

pub(crate) fn twin_projection_ready(wake: Res<TwinProjectionWake>) -> bool {
    wake.pending
}

/// A USD document change is the authoritative input for live projection.
/// Filtering through the USD registry keeps unrelated document kinds from
/// waking this system.
pub(crate) fn wake_twin_projection_on_document_changed(
    trigger: On<DocumentChanged>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    mut wake: ResMut<TwinProjectionWake>,
) {
    if registry.contains(trigger.event().doc) {
        wake.wake();
    }
}

/// Stage asset lifecycle events wake projection when a document edit was
/// waiting for its prepared recipe or when a mounted stage became available.
/// Each reader is independent, so this does not interfere with the pending
/// referenced-spawn transaction that consumes the same Bevy message stream.
pub(crate) fn wake_twin_projection_on_stage_event(
    mut events: MessageReader<bevy::asset::AssetEvent<UsdStageAsset>>,
    mut wake: ResMut<TwinProjectionWake>,
) {
    let mut changed = false;
    for _ in events.read() {
        changed = true;
    }
    if changed {
        wake.wake();
    }
}

/// Whether an op has no incremental live-stage author yet, so the projector must
/// rebuild the scene from the composed source rather than replay it: a
/// whole-source replace, a namespace move (re-keys entities by path; whole-source
/// undo may also change surviving prims' attribute values), a keyframe *removal*
/// (openusd exposes no live-stage sample removal — unlike `SetTimeSample`, which
/// authors incrementally), or a composition-arc edit whose effect is non-local
/// (a variant selection or payload re-composes a whole subtree). The common
/// interactive ops — translate, attribute, spawn, remove, keyframe *authoring*,
/// and now relationship / connection edits — return `false` and replay
/// incrementally via [`apply_incremental_op_to_stage`].
///
/// `SetRelationship` and `SetConnection` use live-stage authors
/// (`CanonicalStage::author_relationship` / `author_connection`). Their consumers
/// (the Avian joint builder and the cosim wire reconcile) re-read on a subtree
/// refresh, so the incremental path fully reconciles them.
///
/// `SetApiSchemas` and `SetActive` do NOT: their effect is which ECS *components* a
/// prim carries (rigid body, collider) and whether its entity exists at all — and
/// the incremental subtree refresh only re-derives an entity's *visual*, not its
/// physics extraction or its presence. So they rebuild, which re-derives both
/// correctly. This is not the hot path: `AttachComponent` emits neither, so
/// building a vehicle from parts stays rebuild-free.
///
/// `SetActive` has ONE carve-out: a prim carrying `LunCoWaypointAPI`. The marker
/// contract is purely visual — a translucent dome (`physics:collisionEnabled =
/// false`) plus an overlap-only non-solid Sensor, never a rigid body — so
/// deactivating it only needs its visual subtree gone, which
/// `refresh_prim_subtree` reconciles. Every other `SetActive` (a vehicle part, a
/// joint, or any other physical prim) still rebuilds, since hiding a physics prim
/// must drop its body/collider, which the visual-only refresh cannot express.
fn op_needs_rebuild(op: &UsdOp, is_waypoint: bool) -> bool {
    // Metadata-only APIs do not add/remove bodies, colliders, or entities. They
    // have live-stage authors below, so they can avoid restarting the scene.
    if let UsdOp::SetApiSchemas { schemas, .. } = op {
        return !incremental_api_schemas(schemas);
    }
    // A waypoint-marker hide reconciles incrementally (see the doc comment); any
    // other `SetActive` is a physics-presence change and must rebuild.
    if matches!(op, UsdOp::SetActive { .. }) {
        return !is_waypoint;
    }
    matches!(
        op,
        UsdOp::ReplaceSource { .. }
            | UsdOp::MovePrim { .. }
            | UsdOp::RemoveTimeSample { .. }
            // Composition-arc changes: value resolution re-composes the prim's
            // subtree wholesale, which the incremental sink can't express.
            | UsdOp::SetVariantSelection { .. }
            | UsdOp::SetPayload { .. }
    )
}

/// Applied schemas with metadata-only runtime consequences. Physical schemas
/// still take the rebuild path because their ECS body/collider presence cannot
/// be reconciled by a visual subtree refresh.
fn incremental_api_schemas(schemas: &[String]) -> bool {
    schemas.iter().all(|schema| {
        matches!(
            schema.as_str(),
            "LunCoProgramAPI" | "LunCoMountAttachmentAPI"
        )
    })
}

/// Whether `path` is a waypoint marker according to the composed USD stage.
/// The marker's `LunCoWaypointAPI` is the authored identity contract; names and
/// hierarchy are intentionally irrelevant so scene authors can organize routes
/// without changing Rust behavior.
fn is_waypoint_prim(world: &World, scene_id: AssetId<UsdStageAsset>, path: &str) -> bool {
    let Ok(path) = openusd::sdf::Path::new(path) else {
        return false;
    };
    world
        .get_non_send::<lunco_usd_bevy::CanonicalStages>()
        .and_then(|stages| stages.get(scene_id))
        .is_some_and(|stage| stage.view().has_api_schema(&path, "LunCoWaypointAPI"))
}

/// A program child is a BT program when its generic `LunCoProgramAPI` source
/// resolves to inline BT.CPP XML or to a supported BT asset. The child name is
/// intentionally irrelevant: `Mission`, `Safety`, `Guidance`, and user-authored
/// names all use the same live projection contract.
pub(crate) fn is_behavior_program(
    world: &World,
    scene_id: AssetId<UsdStageAsset>,
    path: &openusd::sdf::Path,
) -> bool {
    world
        .get_non_send::<lunco_usd_bevy::CanonicalStages>()
        .and_then(|stages| stages.get(scene_id))
        .is_some_and(|stage| {
            let view = stage.view();
            view.has_api_schema(path, "LunCoProgramAPI")
                && matches!(
                    lunco_usd_bevy::program::resolve_behavior_tree_source(&view, path),
                    Ok(Some(_))
                )
        })
}

/// Removal-side companion to [`is_behavior_program`]. Once a source attribute is
/// cleared or changed away from BT.CPP, the composed prim no longer identifies
/// itself as a behaviour source; the runtime provenance component still tells us
/// whether this edit owns the currently projected tree.
fn owns_projected_behavior(
    world: &World,
    scene_id: AssetId<UsdStageAsset>,
    path: &openusd::sdf::Path,
) -> bool {
    world.iter_entities().any(|entity| {
        let Some(prim) = entity.get::<UsdPrimPath>() else {
            return false;
        };
        prim.stage_handle.id() == scene_id
            && entity
                .get::<lunco_autopilot::usd_tree::BehaviorProgramSource>()
                .is_some_and(|source| source.0 == path.as_str())
    })
}

fn projected_behavior_entity(
    world: &World,
    scene_id: AssetId<UsdStageAsset>,
    path: &openusd::sdf::Path,
) -> Option<Entity> {
    world.iter_entities().find_map(|entity| {
        let prim = entity.get::<UsdPrimPath>()?;
        (prim.stage_handle.id() == scene_id
            && entity
                .get::<lunco_autopilot::usd_tree::BehaviorProgramSource>()
                .is_some_and(|source| source.0 == path.as_str()))
        .then_some(entity.id())
    })
}

fn behavior_owner_entity(
    world: &World,
    scene_id: AssetId<UsdStageAsset>,
    path: &openusd::sdf::Path,
) -> Option<Entity> {
    if let Some(owner) = projected_behavior_entity(world, scene_id, path) {
        return Some(owner);
    }
    let owner_path = {
        let stage = world
            .get_non_send::<lunco_usd_bevy::CanonicalStages>()
            .and_then(|stages| stages.get(scene_id))?;
        let view = stage.view();
        let mut current = path.parent();
        let mut result = None;
        while let Some(candidate) = current {
            if view.has_api_schema(&candidate, "PhysxVehicleContextAPI") {
                result = Some(candidate.to_string());
                break;
            }
            current = candidate.parent();
        }
        result
    }?;
    world.iter_entities().find_map(|entity| {
        let prim = entity.get::<UsdPrimPath>()?;
        (prim.stage_handle.id() == scene_id && prim.path == owner_path).then_some(entity.id())
    })
}

/// Replay one **incremental** op's typed delta onto the scene's live
/// `CanonicalStage` — author-once: the value comes straight from the op, never
/// re-read from `composed`. Firing the openusd sink lets
/// [`project_stage_changes`](crate::live_consume::project_stage_changes) reconcile
/// ECS. Only incremental ops reach here; coarse ops ([`op_needs_rebuild`]) rebuild
/// instead. Reads/authors the `!Send` stage under short borrows.
fn apply_incremental_op_to_stage(world: &mut World, scene_id: AssetId<UsdStageAsset>, op: &UsdOp) {
    use lunco_usd_bevy::CanonicalStages;

    // A referenced AddPrim may be waiting on its asset closure. Preserve every
    // later edit whose owner is inside that not-yet-live subtree; otherwise a
    // relationship or metadata op would be accepted by the document and then
    // silently disappear from the live stage. SetTranslate is the one existing
    // fast path that has a dedicated field because it is applied at materialize.
    if !matches!(op, UsdOp::AddPrim { .. }) {
        let owned_path = match op {
            UsdOp::RemovePrim { path, .. }
            | UsdOp::SetTranslate { path, .. }
            | UsdOp::SetRotate { path, .. }
            | UsdOp::SetScale { path, .. }
            | UsdOp::SetAttribute { path, .. }
            | UsdOp::SetRelationship { path, .. }
            | UsdOp::SetConnection { path, .. }
            | UsdOp::SetApiSchemas { path, .. }
            | UsdOp::SetActive { path, .. } => Some(path.as_str()),
            _ => None,
        };
        if let Some(owned_path) = owned_path {
            let pending_index =
                world
                    .resource::<PendingRefSpawns>()
                    .items
                    .iter()
                    .position(|pending| {
                        pending.scene_id == scene_id
                            && (owned_path == pending.prim_path
                                || owned_path
                                    .strip_prefix(&pending.prim_path)
                                    .is_some_and(|suffix| suffix.starts_with('/')))
                    });
            if let Some(index) = pending_index {
                if matches!(op, UsdOp::RemovePrim { .. }) {
                    world.resource_mut::<PendingRefSpawns>().items.remove(index);
                    return;
                }
                let pending = &mut world.resource_mut::<PendingRefSpawns>().items[index];
                if let UsdOp::SetTranslate { value, .. } = op {
                    pending.translate = Some(*value);
                } else {
                    pending.deferred_ops.push(op.clone());
                }
                return;
            }
        }
    }

    match op {
        UsdOp::SetTranslate { path, value, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            if let Some(cs) = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                if let Err(e) = cs.projector().author_translate(&sp, *value) {
                    warn!("[twin] author translate {path}: {e}");
                } else {
                    crate::live_consume::mark_live_transform(
                        world,
                        scene_id,
                        path.clone(),
                        crate::live_consume::TransformEditChannels::translate(),
                    );
                }
            }
        }
        UsdOp::SetRotate { path, value, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            if let Some(cs) = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                if let Err(e) = cs.projector().author_rotate(&sp, *value) {
                    warn!("[twin] author rotate {path}: {e}");
                } else {
                    crate::live_consume::mark_live_transform(
                        world,
                        scene_id,
                        path.clone(),
                        crate::live_consume::TransformEditChannels::rotate(),
                    );
                }
            }
        }
        UsdOp::SetScale { path, value, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            if let Some(cs) = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                if let Err(e) = cs.projector().author_scale(&sp, *value) {
                    warn!("[twin] author scale {path}: {e}");
                } else {
                    crate::live_consume::mark_live_transform(
                        world,
                        scene_id,
                        path.clone(),
                        crate::live_consume::TransformEditChannels::scale(),
                    );
                }
            }
        }
        UsdOp::SetAttribute {
            path,
            name,
            type_name,
            value,
            ..
        } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            // Mirror the document op: a `string` value is RAW (`Value::String`, no
            // literal parse); every other type is a parsed literal.
            let is_string = type_name == "string";
            let v = if is_string {
                openusd::sdf::Value::String(value.clone())
            } else {
                match lunco_usd_bevy::author::parse_attribute_value(type_name, value) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("[twin] parse attribute {path}.{name} ({type_name}): {e}");
                        return;
                    }
                }
            };
            let authored = match world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                Some(cs) => match cs.projector().author_attribute(&sp, name, type_name, v) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("[twin] author attribute {path}.{name}: {e}");
                        false
                    }
                },
                None => false,
            };
            // Wheel/vehicle dynamics attrs: re-derive the spawned wheel
            // components IN PLACE from the composed stage instead of the
            // subtree refresh below — `reinstantiate_entity` on a wheel prim
            // despawns its synthesized `Port` children and visual child
            // while `UsdSimProcessed` survives, leaving a dead solved-torque
            // port and a dangling joint. Checked before the `string`
            // fast-path: authored scalar/string attributes still resync.
            if authored {
                let claimed = world
                    .get_non_send::<CanonicalStages>()
                    .and_then(|s| s.get(scene_id))
                    .is_some_and(|cs| {
                        lunco_usd_sim::wheel_params::claims_edit(&cs.view(), &sp, name)
                    });
                if claimed {
                    lunco_usd_sim::wheel_params::resync_wheels_for_stage(world, scene_id);
                    return;
                }
            }
            // A `string` attribute is non-visual metadata/behavior (`info:sourceCode`,
            // descriptions, a policy's `info:sourceCode`) — no geometry/material
            // consequence, and a refresh would hot-reload a running scenario
            // (resetting its `this`) on a mere save. So author, don't refresh.
            if is_string {
                // A BT program is owned by the parent vessel, not by its child
                // program prim. Updating its source is therefore a component
                // replacement on that already-live vessel, NOT a subtree refresh:
                // re-instantiating the rover here destroys its physics/cosim state.
                if matches!(
                    name.as_str(),
                    "info:implementationSource" | "info:sourceCode" | "info:sourceAsset"
                ) && (is_behavior_program(world, scene_id, &sp)
                    || owns_projected_behavior(world, scene_id, &sp))
                {
                    let source = world
                        .get_non_send::<lunco_usd_bevy::CanonicalStages>()
                        .and_then(|stages| stages.get(scene_id))
                        .map(|stage| {
                            let view = stage.view();
                            crate::program::selected_behavior_source_values(&view, &sp)
                                .unwrap_or_default()
                        });
                    if let Some(owner) = behavior_owner_entity(world, scene_id, &sp) {
                        let mut entity = world.entity_mut(owner);
                        match source {
                            Some((Some(xml), _)) => {
                                entity.insert(lunco_autopilot::usd_tree::BehaviorXml(xml));
                                entity.insert(lunco_autopilot::usd_tree::BehaviorProgramSource(
                                    sp.as_str().to_string(),
                                ));
                                entity.remove::<lunco_autopilot::usd_tree::BehaviorXmlPath>();
                                entity.remove::<lunco_autopilot::usd_tree::BehaviorXmlHandle>();
                            }
                            Some((None, Some(asset))) => {
                                entity.insert(lunco_autopilot::usd_tree::BehaviorXmlPath(asset));
                                entity.insert(lunco_autopilot::usd_tree::BehaviorProgramSource(
                                    sp.as_str().to_string(),
                                ));
                                entity.remove::<lunco_autopilot::usd_tree::BehaviorXml>();
                                entity.remove::<lunco_autopilot::usd_tree::BehaviorXmlHandle>();
                            }
                            _ => {
                                entity.remove::<lunco_autopilot::usd_tree::BehaviorXml>();
                                entity.remove::<lunco_autopilot::usd_tree::BehaviorXmlPath>();
                                entity.remove::<lunco_autopilot::usd_tree::BehaviorXmlHandle>();
                                entity.remove::<lunco_autopilot::usd_tree::BehaviorProgramSource>();
                            }
                        }
                    }
                }
                return;
            }
            // Refresh only what the edit can actually change: a material/shader
            // edit fans out through `material:binding` to meshes anywhere (whole
            // scene), but a geometry/xform attribute edit is local to its own prim
            // — so re-instantiate just that subtree and leave unrelated roots
            // (including live physics bodies) alone.
            if authored {
                let prim_ty = world
                    .get_non_send::<CanonicalStages>()
                    .and_then(|s| s.get(scene_id))
                    .and_then(|cs| cs.view().prim_type_name(&sp));
                if attribute_edit_needs_full_refresh(prim_ty.as_deref()) {
                    // …unless the edit is confined to a `LiveRebuildExempt` subtree.
                    // DEM terrain re-bakes its own content from the registry
                    // document (`refresh_docbacked_terrain_from_doc`), so its
                    // structural scene refresh would be incorrect and needlessly
                    // recreate unrelated stage projections.
                    if !edit_confined_to_exempt_subtree(world, scene_id, path) {
                        refresh_scene_visuals(world, scene_id);
                    }
                } else {
                    refresh_prim_subtree(world, scene_id, path);
                }
            }
        }
        UsdOp::AddPrim {
            parent_path,
            name,
            type_name,
            reference,
            ..
        } => {
            let prim_path = if parent_path == "/" || parent_path.is_empty() {
                format!("/{name}")
            } else {
                format!("{}/{name}", parent_path.trim_end_matches('/'))
            };
            spawn_prim_op(
                world,
                scene_id,
                &prim_path,
                type_name.clone(),
                reference.clone(),
            );
        }
        UsdOp::RemovePrim { path, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            if let Some(cs) = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                if let Err(e) = cs.projector().remove_prim_at(&sp) {
                    warn!("[twin] remove {path}: {e}");
                }
            }
        }
        UsdOp::SetActive { path, active, .. } if is_waypoint_prim(world, scene_id, path) => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let authored = match world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                Some(cs) => match cs.projector().author_active(&sp, *active) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("[twin] author active={active} {path}: {e}");
                        false
                    }
                },
                None => false,
            };
            // A marker carries no rigid body / collider, so toggling its `active`
            // flag only changes whether its visual subtree is present. The live
            // author fires the sink, but the bridge does not despawn on
            // `active = false` the way it does on a spec removal — re-instantiate
            // the prim's subtree so the (now inactive) marker's visual is dropped
            // (or, on reactivation, rebuilt). Mirrors the SetConnection arm.
            if authored {
                refresh_prim_subtree(world, scene_id, path);
            }
        }
        UsdOp::SetTimeSample {
            path,
            name,
            type_name,
            time,
            value,
            ..
        } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let v = match lunco_usd_bevy::author::parse_attribute_value(type_name, value) {
                Ok(v) => v,
                Err(e) => {
                    warn!("[twin] parse keyframe {path}.{name} ({type_name}) @ {time}: {e}");
                    return;
                }
            };
            let authored = match world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                Some(cs) => match cs
                    .projector()
                    .author_time_sample(&sp, name, type_name, *time, v)
                {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("[twin] author keyframe {path}.{name} @ {time}: {e}");
                        false
                    }
                },
                None => false,
            };
            // The per-frame sampler (`sample_usd_animation`) reads the live stage,
            // so a key on an ALREADY-animated prim shows up next tick with no
            // refresh. But the FIRST key turns a static prim animated — its entity
            // isn't `UsdAnimated` yet, so re-instantiate the subtree to let the
            // extractor tag + plan it. Steady-state keyframing stays refresh-free.
            if authored && !prim_entity_is_animated(world, scene_id, path) {
                refresh_prim_subtree(world, scene_id, path);
            }
        }
        UsdOp::SetRelationship {
            path,
            name,
            targets,
            ..
        } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let authored = match world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                Some(cs) => match cs.projector().author_relationship(&sp, name, targets) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("[twin] author relationship {path}.{name}: {e}");
                        false
                    }
                },
                None => false,
            };
            // A relationship is InfoOnly — it never spawns/despawns, so the sink
            // won't reconcile it. Whoever consumes the target (the Avian joint
            // builder reads `physics:body0/1`; a material binding fans out to
            // meshes) is re-run by re-instantiating the owning prim's subtree.
            if authored {
                refresh_relationship_dependents(world, scene_id, path, name);
            }
        }
        UsdOp::SetConnection {
            path,
            name,
            type_name,
            sources,
            ..
        } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let authored = match world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                Some(cs) => match cs
                    .projector()
                    .author_connection(&sp, name, type_name, sources)
                {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("[twin] author connection {path}.{name}: {e}");
                        false
                    }
                },
                None => false,
            };
            // Cosim wires (`SimConnection`) are derived from `connectionPaths` by
            // `reconcile_usd_connections`, which re-scans the composed stage — the
            // subtree refresh re-triggers it for the owning prim.
            if authored {
                refresh_prim_subtree(world, scene_id, path);
            }
        }
        UsdOp::SetApiSchemas { path, schemas, .. } if incremental_api_schemas(schemas) => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let authored = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
                .is_some_and(|cs| cs.projector().author_api_schemas(&sp, schemas).is_ok());
            if !authored {
                warn!("[twin] author program API {path} failed");
            }
        }
        // Coarse ops never reach here (the caller rebuilds for them) — that now
        // includes SetApiSchemas / SetActive, whose ECS effect (physics component
        // set / entity presence) the visual-only subtree refresh can't reconcile.
        _ => {}
    }
}
/// Re-instantiate the subtree(s) that depend on relationship `name` on `path`.
/// A `material:binding` fans out to every mesh it reaches, so a whole-scene visual
/// refresh is honest; any other relationship (physics bodies, collections) is
/// local to its owning prim's consumer, so refresh just that subtree.
fn refresh_relationship_dependents(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    path: &str,
    name: &str,
) {
    if name == "material:binding" {
        refresh_scene_visuals(world, scene_id);
    } else {
        refresh_prim_subtree(world, scene_id, path);
    }
}

/// Whether the live entity projecting `path` in `scene_id` is already tagged
/// [`UsdAnimated`](lunco_usd_bevy::UsdAnimated) — so the per-frame animation
/// sampler already drives it and a fresh keyframe needs no re-instantiation. False
/// when the prim is static (or has no live entity yet), which is when a first
/// keyframe must trigger a subtree refresh to (re-)tag it.
fn prim_entity_is_animated(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    path: &str,
) -> bool {
    let mut q = world.query::<(&UsdPrimPath, Option<&lunco_usd_bevy::UsdAnimated>)>();
    q.iter(world)
        .any(|(upp, anim)| upp.stage_handle.id() == scene_id && upp.path == *path && anim.is_some())
}

/// Author a spawn onto the live stage: a plain prim authors immediately; a
/// referenced prim authors the arc when its asset closure is already loaded, else
/// queues a [`RefSpawn`] fetch that [`drain_ref_spawns`] completes. The
/// short-borrow / pre-decide pattern — the `!Send` stage can't be held across the
/// `AssetServer` fetch or the authoring re-borrow. Shared by typed op replay
/// ([`apply_incremental_op_to_stage`]) and pending-reference completion.
fn spawn_prim_op(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    prim_path: &str,
    type_name: Option<String>,
    reference: Option<String>,
) {
    use lunco_usd_bevy::CanonicalStages;
    let Ok(sp) = openusd::sdf::Path::new(prim_path) else {
        return;
    };

    let Some(asset_path) = reference else {
        // Plain prim — author now.
        if let Some(cs) = world
            .get_non_send::<CanonicalStages>()
            .and_then(|s| s.get(scene_id))
        {
            if let Err(e) = cs.projector().author_prim(&sp, type_name.as_deref()) {
                warn!("[twin] spawn {prim_path}: {e}");
            }
        }
        return;
    };

    // Referenced spawn: the live stage may already have the source bytes, but
    // the standalone asset plan is still the projection input for this
    // instance. Require that plan before authoring so the first frame never
    // falls back to repeated live-stage reads.
    enum Plan {
        Now { projection: UsdInstanceProjection },
        Fetch { ref_handle: Handle<UsdStageAsset> },
    }
    let (ref_id, has_layer_bytes) = {
        let Some(cs) = world
            .get_non_send::<CanonicalStages>()
            .and_then(|s| s.get(scene_id))
        else {
            return;
        };
        let ref_id = cs.canonical_reference_id(&asset_path);
        (ref_id.clone(), cs.has_layer_bytes(&ref_id))
    };
    let ref_handle = world
        .resource::<AssetServer>()
        .load::<UsdStageAsset>(ref_id.clone());
    let plan = if !has_layer_bytes {
        Plan::Fetch { ref_handle }
    } else if let Some(asset) = world
        .resource::<Assets<UsdStageAsset>>()
        .get(ref_handle.id())
    {
        let projection = match asset.projection_plan.for_instance(prim_path) {
            Ok(plan) => UsdInstanceProjection {
                root: None,
                plan: Arc::new(plan),
            },
            Err(error) => {
                error!(
                    "[twin] referenced spawn {prim_path} has an invalid prepared projection plan for `{asset_path}`: {error}"
                );
                return;
            }
        };
        Plan::Now { projection }
    } else {
        Plan::Fetch { ref_handle }
    };
    match plan {
        Plan::Now { projection } => {
            if let Some(cs) = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                let result =
                    cs.projector()
                        .author_referenced_prim(&sp, type_name.as_deref(), &asset_path);
                if let Err(e) = result {
                    warn!("[twin] referenced spawn {prim_path}: {e}");
                } else {
                    world.resource_mut::<PendingInstanceProjections>().insert(
                        scene_id,
                        prim_path.to_string(),
                        projection,
                    );
                }
            }
        }
        Plan::Fetch { ref_handle } => {
            let ref_id = ref_handle.id();
            let ready = world
                .resource::<Assets<UsdStageAsset>>()
                .get(ref_id)
                .is_some();
            let failed = world
                .resource::<AssetServer>()
                .get_load_state(ref_id)
                .is_some_and(|state| state.is_failed());
            world.resource_mut::<PendingRefSpawns>().push(
                RefSpawn {
                    scene_id,
                    prim_path: prim_path.to_string(),
                    type_name,
                    asset_path,
                    ref_handle,
                    translate: None,
                    deferred_ops: Vec::new(),
                },
                ready,
            );
            if failed {
                world.resource_mut::<PendingRefSpawns>().mark_failed(
                    ref_id,
                    "the referenced asset had already failed to load".into(),
                );
            }
        }
    }
}

/// Re-read the whole live scene from the (now-authored) stage. Only an explicit
/// [`UsdSceneRoot`](lunco_usd_bevy::UsdSceneRoot) may seed this rebuild.
/// Before rebuilding, retire every other projection entity for that stage.
///
/// This stage-scoped retirement is essential: a mounted USD camera is
/// intentionally reparented directly to the persistent grid, so it no longer
/// belongs to its USD root's Bevy subtree. Reinstantiating that root alone would
/// create a replacement camera while the detached camera kept rendering.
/// Parentage is therefore never used as scene ownership; the stage handle is.
///
/// Dropping the root's `UsdVisualSynced` marker and children then re-inserting
/// `UsdPrimPath` re-fires `on_usd_prim_added`, rebuilding exactly one subtree so
/// an attribute edit that fans out through a material binding reaches every bound
/// mesh. Structural changes therefore use one explicit, stage-scoped synchronous
/// rebuild.
pub(crate) fn refresh_scene_visuals(world: &mut World, scene_id: AssetId<UsdStageAsset>) {
    let roots: Vec<Entity> = {
        // A live simulation is rooted by `UsdSceneRoot`; each editor preview
        // lease is rooted by `UsdPreviewOnly`. Both are stage ownership roots
        // and must survive a visual refresh. Restricting this to `UsdSceneRoot`
        // silently despawned preview subtrees, leaving no entity to
        // re-instantiate after a material edit.
        let mut q = world.query_filtered::<(Entity, &UsdPrimPath), Or<(
            With<UsdSceneRoot>,
            With<lunco_usd_bevy::UsdPreviewOnly>,
        )>>();
        q.iter(world)
            .filter(|(_, upp)| upp.stage_handle.id() == scene_id)
            .map(|(entity, _)| entity)
            .collect()
    };

    // `reinstantiate_entity` can only recursively despawn ordinary hierarchy
    // children. Camera mounting deliberately breaks that hierarchy for precision,
    // so first remove every non-root entity projected from this stage. This is the
    // same ownership rule used by full scene teardown, kept here because a live
    // document refresh does not pass through that lifecycle command.
    let root_set: std::collections::HashSet<_> = roots.iter().copied().collect();
    let stale: Vec<Entity> = {
        let mut q = world.query::<(Entity, &UsdPrimPath)>();
        q.iter(world)
            .filter(|(entity, prim)| {
                prim.stage_handle.id() == scene_id && !root_set.contains(entity)
            })
            .map(|(entity, _)| entity)
            .collect()
    };
    for entity in stale {
        if let Ok(entity_mut) = world.get_entity_mut(entity) {
            entity_mut.despawn();
        }
    }
    for root in roots {
        reinstantiate_entity(world, root);
    }
}

/// Drop `entity`'s [`UsdVisualSynced`] marker + children and re-insert its
/// [`UsdPrimPath`], re-firing `on_usd_prim_added` so its subtree rebuilds from
/// the (now-authored) live stage. The shared primitive under both the whole-scene
/// [`refresh_scene_visuals`] and the single-prim [`refresh_prim_subtree`].
fn reinstantiate_entity(world: &mut World, entity: Entity) {
    let stage_ready = world
        .get_resource::<Assets<UsdStageAsset>>()
        .is_some_and(|assets| {
            world
                .get::<UsdPrimPath>(entity)
                .is_some_and(|prim| assets.get(&prim.stage_handle).is_some())
        });
    if let Ok(mut em) = world.get_entity_mut(entity) {
        em.remove::<UsdVisualSynced>();
        em.remove::<lunco_usd_sim::shader::UsdShaderResolved>();
        em.despawn_related::<Children>();
        if let Some(pp) = em.take::<UsdPrimPath>() {
            em.insert((pp, UsdAwaitingStage));
            if stage_ready {
                em.insert(UsdVisualProjectionQueued);
            }
        }
    }
}

/// Re-instantiate only the subtree of the single prim at `path` in `scene_id`
/// (the entity whose [`UsdPrimPath`] matches), leaving every other scene root
/// untouched. Used for a geometry/xform attribute edit, whose visual effect is
/// local to its own prim — unlike a material/shader edit, which fans out through
/// `material:binding` to arbitrary meshes and needs the whole-scene refresh. This
/// avoids re-instantiating unrelated roots (including live physics bodies) on
/// every attribute edit.
pub(crate) fn refresh_prim_subtree(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    path: &str,
) {
    let entity = {
        let mut q = world.query::<(Entity, &UsdPrimPath)>();
        q.iter(world)
            .find(|(_, upp)| upp.stage_handle.id() == scene_id && upp.path == *path)
            .map(|(e, _)| e)
    };
    if let Some(e) = entity {
        reinstantiate_entity(world, e);
    }
}

/// Whether a `SetAttribute` on a prim of this type must refresh the **whole**
/// scene rather than just the edited prim's subtree. A material / shader /
/// node-graph opinion propagates through `material:binding` to meshes anywhere in
/// the scene, so the edited prim's own subtree is not where the visual change
/// lands; an unknown type is treated conservatively as needing the full refresh.
/// Every other (geometry/xform) attribute edit is local to its prim, so it takes
/// the cheap [`refresh_prim_subtree`] path.
fn attribute_edit_needs_full_refresh(prim_type: Option<&str>) -> bool {
    match prim_type {
        Some(t) => matches!(t, "Material" | "Shader" | "NodeGraph"),
        None => true,
    }
}

/// Whether an edit at `path` lands inside a [`LiveRebuildExempt`] prim's subtree of
/// `scene_id` — a live prim that refreshes its own content in place (the DEM
/// terrain: it re-bakes off the registry document, so a whole-scene reload would
/// re-bridge the terrain + re-spawn the avatar camera per edit). Matches the
/// exempt prim itself or any descendant. The missing consumer of `LiveRebuildExempt`.
fn edit_confined_to_exempt_subtree(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    path: &str,
) -> bool {
    let mut q = world.query_filtered::<&UsdPrimPath, With<LiveRebuildExempt>>();
    q.iter(world).any(|upp| {
        upp.stage_handle.id() == scene_id
            && (upp.path == path
                || path.starts_with(&format!("{}/", upp.path.trim_end_matches('/'))))
    })
}

/// Rebuild the scene's live `CanonicalStage` from the composed document source
/// (`base ⊕ runtime`) plus the resolver's already-loaded layer closure, then
/// re-instantiate the scene — the coarse whole-source path (Save-As / MovePrim /
/// whole-source undo). A rebuild picks up attribute-value changes on surviving
/// prims too, so undoing a `SetAttribute` (which inverts to a `ReplaceSource`)
/// actually reverts the material/param in the live world. References that were
/// already loaded recompose from the byte snapshot with no re-fetch; a brand-new
/// reference introduced by the edit (rare) would fail to resolve — logged by
/// `CanonicalStages::rebuild`.
fn rebuild_scene_from_composed(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    composed_source: &str,
) {
    use lunco_usd_bevy::{CanonicalStages, StageRecipe};
    // Recipe = the edited composed source as the root layer + every referenced
    // `.usda` the current stage already loaded (keyed by the same canonical ids).
    let (scene_layer, mut bytes) = {
        let Some(cs) = world
            .get_non_send::<CanonicalStages>()
            .and_then(|s| s.get(scene_id))
        else {
            return;
        };
        (cs.scene_layer.clone(), cs.layer_bytes_snapshot())
    };
    bytes.insert(scene_layer.clone(), composed_source.as_bytes().to_vec());
    let recipe = StageRecipe {
        root_id: scene_layer,
        bytes,
    };
    let rebuilt = world
        .get_non_send_mut::<CanonicalStages>()
        .map(|mut stages| stages.rebuild(scene_id, &recipe))
        .unwrap_or(false);
    if rebuilt {
        // Fresh stage (new, empty sink) — re-instantiate every scene root off it.
        refresh_scene_visuals(world, scene_id);
    }
}

/// Complete referenced spawns whose asset closure has finished loading: inject
/// the fetched layer bytes into the scene stage's resolver, then author the prim
/// and its `references` arc so the openusd sink fires and `project_stage_changes`
/// instantiates the composed subtree. Exclusive: authors onto the `!Send`
/// `CanonicalStage`.
pub(crate) fn drain_ref_spawns(world: &mut World) {
    use lunco_usd_bevy::CanonicalStages;
    if world.resource::<PendingRefSpawns>().items.is_empty() {
        return;
    }
    let (ready, failed) = {
        let mut pending = world.resource_mut::<PendingRefSpawns>();
        (
            std::mem::take(&mut pending.ready),
            std::mem::take(&mut pending.failed),
        )
    };
    let pending = std::mem::take(&mut world.resource_mut::<PendingRefSpawns>().items);
    let mut still = Vec::new();
    for mut item in pending {
        if let Some(error) = failed.get(&item.ref_handle.id()) {
            error!(
                "[twin] referenced spawn {} failed to load `{}`: {error}",
                item.prim_path, item.asset_path
            );
            continue;
        }
        if !ready.contains(&item.ref_handle.id()) {
            still.push(item);
            continue;
        }
        let recipe = world
            .resource::<Assets<UsdStageAsset>>()
            .get(item.ref_handle.id())
            .and_then(|a| a.recipe.clone());
        let Some(recipe) = recipe else {
            error!(
                "[twin] referenced spawn {} received a ready event without a usable recipe for `{}`",
                item.prim_path, item.asset_path
            );
            continue;
        };
        let Some(asset) = world
            .resource::<Assets<UsdStageAsset>>()
            .get(item.ref_handle.id())
        else {
            error!(
                "[twin] referenced spawn {} became ready without its prepared asset for `{}`",
                item.prim_path, item.asset_path
            );
            continue;
        };
        let plan = match asset.projection_plan.for_instance(&item.prim_path) {
            Ok(plan) => plan,
            Err(error) => {
                error!(
                    "[twin] referenced spawn {} has an invalid prepared projection plan for `{}`: {error}",
                    item.prim_path, item.asset_path
                );
                continue;
            }
        };
        let projection = UsdInstanceProjection {
            root: None,
            plan: Arc::new(plan),
        };
        let Ok(sp) = openusd::sdf::Path::new(&item.prim_path) else {
            continue;
        };
        let result = {
            let Some(cs) = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(item.scene_id))
            else {
                continue; // scene stage gone — drop the spawn
            };
            // Inject the closure bytes so PCP can resolve the reference, then author.
            cs.add_layer_bytes(recipe.bytes.clone());
            let result = cs.projector().author_referenced_prim(
                &sp,
                item.type_name.as_deref(),
                &item.asset_path,
            );
            if result.is_ok() {
                // Apply the transform after the prim/reference exists. This is the
                // ordering guarantee for first-use referenced markers and spawned
                // vehicles alike.
                if let Some(translate) = item.translate {
                    if let Err(e) = cs.projector().author_translate(&sp, translate) {
                        warn!("[twin] referenced spawn {} translate: {e}", item.prim_path);
                    } else {
                        crate::live_consume::mark_live_transform(
                            world,
                            item.scene_id,
                            item.prim_path.clone(),
                            crate::live_consume::TransformEditChannels::translate(),
                        );
                    }
                }
            }
            result
        };
        if let Err(e) = result {
            warn!(
                "[twin] referenced spawn {} (post-fetch): {e}",
                item.prim_path
            );
            continue;
        }
        world.resource_mut::<PendingInstanceProjections>().insert(
            item.scene_id,
            item.prim_path.clone(),
            projection,
        );
        // Replay child-owned metadata and relationships only after the
        // referenced root exists on the live stage. The document already owns
        // the complete ordered intent; this is just its delayed live-stage
        // projection for first-use references.
        for op in std::mem::take(&mut item.deferred_ops) {
            apply_incremental_op_to_stage(world, item.scene_id, &op);
        }
        crate::live_consume::reproject_physics_if_needed(world, item.scene_id, &item.prim_path);
    }
    world.resource_mut::<PendingRefSpawns>().items.extend(still);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{LayerId, UsdOp};
    use lunco_usd_bevy::UsdVisualSynced;

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    #[test]
    fn projection_wake_coalesces_and_consumes_explicitly() {
        let mut wake = TwinProjectionWake::default();
        assert!(!wake.pending);

        wake.wake();
        wake.wake();
        assert!(wake.pending, "multiple producers share one pending wake");

        wake.consume();
        assert!(!wake.pending, "the projection owner consumes its wake once");
    }

    /// Relationship and connection edits use live-stage authors, while composition
    /// arc edits still require a composed-stage rebuild. This keeps assembly edits
    /// on the incremental path and reserves rebuilding for non-local composition.
    #[test]
    fn op_rebuild_routing_matches_the_incremental_authors() {
        let et = LayerId::root();
        // Incremental now — a joint's two `physics:body` rels and a cosim wire.
        assert!(!op_needs_rebuild(
            &UsdOp::SetRelationship {
                edit_target: et.clone(),
                path: "/J".into(),
                name: "physics:body0".into(),
                targets: vec![],
            },
            false
        ));
        assert!(!op_needs_rebuild(
            &UsdOp::SetConnection {
                edit_target: et.clone(),
                path: "/B".into(),
                name: "inputs:v".into(),
                type_name: "float".into(),
                sources: vec![],
            },
            false
        ));
        // Physical apiSchema / active REBUILD: their effect is a prim's ECS
        // component set / entity presence, which the visual-only subtree refresh
        // can't reconcile.
        assert!(op_needs_rebuild(
            &UsdOp::SetApiSchemas {
                edit_target: et.clone(),
                path: "/W".into(),
                schemas: vec!["PhysicsRigidBodyAPI".into()],
            },
            false
        ));
        // A program API is metadata on an existing `Mission` scope. Its consumer
        // is the vessel's BehaviorXml projection, never the physical rover
        // topology, so it must remain on the live incremental path.
        assert!(!op_needs_rebuild(
            &UsdOp::SetApiSchemas {
                edit_target: et.clone(),
                path: "/W/Mission".into(),
                schemas: vec!["LunCoProgramAPI".into()],
            },
            false
        ));
        // A `SetActive` on a physics prim (a rover part) rebuilds: it changes
        // entity presence / physics component set, which the visual-only subtree
        // refresh can't reconcile.
        assert!(op_needs_rebuild(
            &UsdOp::SetActive {
                edit_target: et.clone(),
                path: "/Rover/Chassis".into(),
                active: false,
            },
            false
        ));
        // A `SetActive` on a prim carrying `LunCoWaypointAPI` reconciles
        // incrementally: the marker is purely visual (a non-colliding dome + an
        // overlap-only Sensor), so hiding/revealing it only needs its visual
        // subtree dropped/rebuilt. Deactivation must NOT reload the scene.
        assert!(!op_needs_rebuild(
            &UsdOp::SetActive {
                edit_target: et.clone(),
                path: "/Rover/Route/W3".into(),
                active: false,
            },
            true
        ));
        // Reactivation is symmetric — also incremental.
        assert!(!op_needs_rebuild(
            &UsdOp::SetActive {
                edit_target: et.clone(),
                path: "/Apollo15/Route/W0".into(),
                active: true,
            },
            true
        ));
        // A normal prim without the authored waypoint schema still rebuilds,
        // regardless of its name or hierarchy.
        assert!(op_needs_rebuild(
            &UsdOp::SetActive {
                edit_target: et.clone(),
                path: "/Rover/Wheels/W0".into(),
                active: false,
            },
            false
        ));
        // Composition-arc edits also rebuild — value resolution recomposes the
        // subtree, which the incremental sink can't express.
        assert!(op_needs_rebuild(
            &UsdOp::SetVariantSelection {
                edit_target: et.clone(),
                path: "/R".into(),
                variant_set: "drivetrain".into(),
                variant: "physical".into(),
            },
            false
        ));
        assert!(op_needs_rebuild(
            &UsdOp::SetPayload {
                edit_target: et.clone(),
                path: "/H".into(),
                asset_paths: vec![],
            },
            false
        ));
        // Pre-existing coarse ops unchanged.
        assert!(op_needs_rebuild(
            &UsdOp::MovePrim {
                edit_target: et,
                from_path: "/a".into(),
                to_path: "/b".into(),
            },
            false
        ));
    }

    /// A material/shader/node-graph attribute edit fans out through
    /// `material:binding` and needs the whole-scene refresh; an unknown type is
    /// treated conservatively the same way; every other (geometry/xform) edit is
    /// local and takes the cheap single-prim path.
    #[test]
    fn attribute_refresh_scope_is_full_only_for_shading_prims() {
        for shading in ["Material", "Shader", "NodeGraph"] {
            assert!(
                attribute_edit_needs_full_refresh(Some(shading)),
                "{shading} binding fan-out needs a whole-scene refresh"
            );
        }
        assert!(
            attribute_edit_needs_full_refresh(None),
            "unknown prim type is refreshed conservatively (whole scene)"
        );
        for local in ["Mesh", "Xform", "Sphere", "Cube", "Camera"] {
            assert!(
                !attribute_edit_needs_full_refresh(Some(local)),
                "{local} attribute edit is local to its own prim subtree"
            );
        }
    }

    /// A mounted camera is reparented directly beneath the persistent world
    /// grid. It still belongs to the stage, but it is outside the root's Bevy
    /// subtree. A full refresh must retire it before rebuilding the root, or
    /// the root creates a replacement avatar alongside it.
    #[test]
    fn full_refresh_does_not_reinstantiate_detached_stage_camera() {
        let mut world = World::new();
        let stage = Handle::<UsdStageAsset>::default();
        let root = world
            .spawn((
                UsdSceneRoot,
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Traverse".into(),
                },
                UsdVisualSynced,
            ))
            .id();
        let detached_camera = world
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Traverse/Avatar".into(),
                },
                UsdVisualSynced,
            ))
            .id();

        refresh_scene_visuals(&mut world, stage.id());

        assert!(
            world.get::<UsdVisualSynced>(root).is_none(),
            "the explicit scene root is refreshed"
        );
        assert!(
            world.get_entity(detached_camera).is_err(),
            "a grid-direct mounted camera is retired before its root rebuilds"
        );
    }

    /// File identity is owned by the document registry, so this test exercises
    /// the registry's canonical file lookup rather than duplicating that rule.
    #[test]
    fn doc_for_file_matches_file_origin_only() {
        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let abs = PathBuf::from("/twins/moonbase/scene.usda");
        let (doc, _) = registry.open_file(abs.clone(), TINY.to_string());
        registry.allocate(
            TINY.to_string(),
            lunco_doc::PathlessOrigin::untitled("Untitled.usda"),
        );

        assert_eq!(registry.doc_for_file(&abs), Some(doc));
        assert_eq!(
            registry.doc_for_file(std::path::Path::new("/twins/x.usda")),
            None
        );
    }

    #[test]
    fn twin_scene_lease_closes_only_unclaimed_documents() {
        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let root = PathBuf::from("/twins/moonbase");
        let (scene_only, _) = registry.open_file(root.join("scene.usda"), TINY.to_string());
        let (user_owned, _) = registry.open_file(root.join("edited.usda"), TINY.to_string());
        let mut backed = DocBackedTwinScenes::default();
        backed.track(
            scene_only,
            root.clone(),
            "moonbase".into(),
            "scene.usda".into(),
        );
        backed.track(
            user_owned,
            root.clone(),
            "moonbase".into(),
            "edited.usda".into(),
        );
        assert!(backed.claim_user(user_owned));

        let released = backed.release_root(&root);
        for doc in released {
            registry.remove(doc);
        }

        assert_eq!(backed.coords_of(scene_only), None);
        assert_eq!(backed.coords_of(user_owned), None);
        assert!(!registry.contains(scene_only));
        assert!(
            registry.contains(user_owned),
            "an explicitly user-owned document survives Twin replacement"
        );
    }

    #[test]
    fn closing_one_of_multiple_twin_leases_keeps_the_document_backed() {
        let root_a = PathBuf::from("/twins/a");
        let root_b = PathBuf::from("/twins/b");
        let doc = DocumentId::new(1);
        let mut backed = DocBackedTwinScenes::default();
        backed.track(doc, root_a.clone(), "shared".into(), "scene.usda".into());
        backed.track(doc, root_b.clone(), "shared".into(), "scene.usda".into());

        assert!(backed.release_root(&root_a).is_empty());
        assert!(backed.coords_of(doc).is_some());
        assert_eq!(backed.release_root(&root_b), vec![doc]);
        assert!(backed.coords_of(doc).is_none());
    }

    #[test]
    fn closing_one_of_multiple_preview_leases_keeps_the_authority() {
        let doc = DocumentId::new(1);
        let mut backed = DocBackedTwinScenes::default();
        backed.track_preview(doc, "assembly".into(), "scene.usda".into());
        backed.acquire_preview(doc);
        backed.acquire_preview(doc);

        assert!(backed.release_preview(doc).is_none());
        assert!(backed.coords_of(doc).is_some());
        assert_eq!(
            backed.release_preview(doc),
            Some(("assembly".into(), "scene.usda".into()))
        );
        assert!(backed.coords_of(doc).is_none());
    }

    #[test]
    fn scene_teardown_preserves_incoming_twin_admission() {
        let mut app = App::new();
        app.init_resource::<PendingTwinDocs>()
            .init_resource::<PendingRefSpawns>()
            .add_systems(lunco_core::SceneTeardown, reset_scene_projection_state);
        app.world_mut().resource_mut::<PendingTwinDocs>().push(
            Handle::default(),
            false,
            "incoming".into(),
            "scene.usda".into(),
            PathBuf::from("/twins/incoming/scene.usda"),
            PathBuf::from("/twins/incoming"),
        );

        lunco_core::run_scene_teardown(app.world_mut());

        assert_eq!(app.world().resource::<PendingTwinDocs>().items.len(), 1);
    }

    #[test]
    fn source_asset_events_advance_pending_twin_docs() {
        let mut app = App::new();
        app.init_resource::<PendingTwinDocs>()
            .add_message::<bevy::asset::AssetEvent<UsdSourceText>>()
            .add_message::<bevy::asset::AssetLoadFailedEvent<UsdSourceText>>()
            .add_systems(Update, mark_pending_twin_docs);
        let handle = Handle::<UsdSourceText>::default();
        app.world_mut().resource_mut::<PendingTwinDocs>().push(
            handle.clone(),
            false,
            "incoming".into(),
            "scene.usda".into(),
            PathBuf::from("/twins/incoming/scene.usda"),
            PathBuf::from("/twins/incoming"),
        );

        assert!(!app
            .world()
            .resource::<PendingTwinDocs>()
            .has_terminal_source_event());
        app.world_mut()
            .resource_mut::<Messages<bevy::asset::AssetEvent<UsdSourceText>>>()
            .write(bevy::asset::AssetEvent::Added { id: handle.id() });
        app.update();

        assert!(app
            .world()
            .resource::<PendingTwinDocs>()
            .has_terminal_source_event());
        assert!(app
            .world()
            .resource::<PendingTwinDocs>()
            .ready
            .contains(&handle.id()));
    }

    #[test]
    fn resident_source_is_ready_when_queued() {
        let mut pending = PendingTwinDocs::default();
        let handle = Handle::<UsdSourceText>::default();
        pending.push(
            handle.clone(),
            true,
            "incoming".into(),
            "scene.usda".into(),
            PathBuf::from("/twins/incoming/scene.usda"),
            PathBuf::from("/twins/incoming"),
        );

        assert!(pending.has_terminal_source_event());
        assert!(pending.ready.contains(&handle.id()));
    }

    #[test]
    fn failed_source_asset_event_is_terminal() {
        let mut pending = PendingTwinDocs::default();
        let handle = Handle::<UsdSourceText>::default();
        pending.push(
            handle.clone(),
            false,
            "incoming".into(),
            "scene.usda".into(),
            PathBuf::from("/twins/incoming/scene.usda"),
            PathBuf::from("/twins/incoming"),
        );

        pending.mark_failed(handle.id(), "source unavailable".into());

        assert!(pending.has_terminal_source_event());
        assert_eq!(
            pending.failed.get(&handle.id()).map(String::as_str),
            Some("source unavailable")
        );
    }

    /// The bytes pushed into the overlay are the document's *composed* source —
    /// so a runtime-layer spawn rides into the live world's composition.
    #[test]
    fn composed_source_overlay_carries_runtime_spawn() {
        let mut registry = DocumentRegistry::<UsdDocument>::default();
        let abs = PathBuf::from("/twins/moonbase/scene.usda");
        let (doc, _) = registry.open_file(abs, TINY.to_string());
        registry
            .host_mut(doc)
            .unwrap()
            .document_mut()
            .apply(UsdOp::AddPrim {
                edit_target: LayerId::runtime(),
                parent_path: "/World".into(),
                name: "rover_1".into(),
                type_name: Some("Xform".into()),
                reference: Some("lunco://vessels/rovers/skid_rover.usda".into()),
            })
            .unwrap();

        let composed = registry.host(doc).unwrap().document().composed_source();
        assert!(
            composed.contains("rover_1"),
            "overlay bytes carry the runtime spawn:\n{composed}"
        );
        assert!(
            composed.contains("@lunco://vessels/rovers/skid_rover.usda@"),
            "and its asset reference (resolved by the async loader at the twin:// anchor)"
        );
    }
}
