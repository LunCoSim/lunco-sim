//! Doc-backed twin default scene — web-ready via the twin asset source.
//!
//! This is the **doc-backed live-projection path**: the default twin scene loads
//! through the `twin://` asset source and the async [`UsdLoader`], which
//! re-attaches the scheme so co-located refs (terrain `.glb`) resolve on every
//! platform the source supports. It is made doc-backed by serving the scene
//! document's **persistent** (`base ⊕ runtime`) source as a *byte-overlay* on
//! the Twin source, so runtime edits appear in the initial mount. Disposable
//! `view` opinions stay out of that overlay and are replayed to the canonical
//! stage through their separate projection cursor.
//!
//! Flow (doc-first: the document exists and its persistent source is the overlay
//! BEFORE the scene mounts, so the world is projected exactly once):
//! 1. The authored Twin loading policy selects a scene and dispatches
//!    `OpenTwinScene`; this owner kicks an async [`UsdSourceText`] load of
//!    `twin://<name>/<scene>` (raw base layer, read
//!    through the twin source — web-ready) and record it in [`PendingTwinDocs`].
//!    The scene mount is admitted after the source asset reaches a terminal
//!    success or failure event.
//! 2. [`drain_pending_twin_docs`] — once the source asset emits its terminal
//!    event, admit parsing of that exact text revision through
//!    `AsyncWorkAdmission`. The document registry commits the prepared source
//!    under its path identity and dirty-document policy. After restoring the
//!    persisted `.lunco/runtime` overlay, a cloned document snapshot is
//!    serialized on the worker pool; the owner accepts it only while its
//!    generation remains current. It then publishes the persistent source as
//!    the Twin overlay, records the document lease, and fires `LoadScene` —
//!    the single mount composes `base ⊕ runtime`.
//! 3. [`sync_twin_overlays`] — later document edits author typed deltas onto the
//!    live composed stage: translates and structural spawns/removes are
//!    authored onto the scene's [`CanonicalStage`](lunco_usd_bevy_stage::canonical::CanonicalStage)
//!    directly, firing its openusd change sink so `project_stage_changes`
//!    projects the edit in place — no whole-scene asset reload. The runtime
//!    persistence plugin saves coalesced runtime-layer snapshots off-thread;
//!    dependent stages are refreshed only when their composition uses the
//!    changed document. A referenced spawn whose asset isn't loaded yet is
//!    fetched once through [`drain_ref_spawns`]; its prepared per-instance plan
//!    drives the initial subtree while the live stage receives only its root.
//!    The first later edit that needs composed instance facts promotes that
//!    reference into the canonical stage, and root deletion never needs that
//!    composition.
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

use lunco_usd_document::document::UsdDocument;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use crate::scene::{
    INCOMPLETE_COMPOSITION_POLICY_HOOK, IncompleteCompositionDecision, LoadScene,
    evaluate_incomplete_composition_policy,
};
use bevy::asset::AssetId;
use bevy::prelude::*;
use lunco_assets_core::twin_source::TwinRoots;
use lunco_core_runtime::{
    AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority, SimulationProgress,
    SimulationProgressKey, SimulationProgressOwner,
};
use lunco_doc::{Document, DocumentId};
use lunco_usd_bevy_scene::{
    UsdPrimPath, UsdSceneAwaitingStage, UsdSceneProjected, UsdSceneProjectionQueued,
    UsdSceneProjectionReset, UsdSceneRoot,
};
use lunco_usd_bevy_stage::{
    UsdInstanceProjection, UsdRead, UsdStageAsset, UsdStageProjectionPlan, euler_xyz_deg_to_quat,
    source::UsdSourceText,
};
use lunco_usd_bevy_twin::{
    DocBackedTwinScenes, LiveRebuildExempt, TwinProjectionWake, scene_document_for,
};

use crate::scene_runtime::TWIN_SCENE_LOAD_FAILED;
use lunco_doc::OpenOutcome;
use lunco_doc_bevy::{DocumentChanged, DocumentRegistry};
use lunco_hooks::HookValue;
use lunco_usd_core::commands::EmptyViewportReason;
use lunco_usd_document::document::PreparedUsdSource;
use lunco_usd_document::document::UsdOp;

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
    doc: Option<DocumentId>,
    failure_reported: bool,
    stage: TwinDocPreparationStage,
    capacity_revision: Option<u64>,
    work_key: Option<AsyncWorkKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TwinDocPreparationStage {
    Finished,
    Failed,
    AwaitingSource,
    PreparingSource {
        operation: u64,
    },
    AwaitingPersistentSource {
        doc: DocumentId,
        generation: u64,
    },
    PreparingPersistentSource {
        operation: u64,
        doc: DocumentId,
        generation: u64,
    },
}

struct TwinDocCompletion {
    operation: u64,
    result: TwinDocCompletionResult,
}

enum TwinDocCompletionResult {
    Source(Result<PreparedUsdSource, String>),
    PersistentSource(Result<String, String>),
}

/// Coalesced dependent-layer patches and worker-prepared plans for later mounts.
#[derive(Resource, Default)]
pub(crate) struct PendingDependentStageRefreshes {
    by_stage: HashMap<AssetId<UsdStageAsset>, PendingDependentStageRefresh>,
    completions: Arc<Mutex<Vec<DependentStageRefreshCompletion>>>,
    next_operation: u64,
}

struct PendingDependentStageRefresh {
    operation: u64,
    target_plan: Arc<UsdStageProjectionPlan>,
    base_recipe: Arc<lunco_usd_compose::recipe::StageRecipe>,
    changed_layers: BTreeMap<String, Arc<DependentStageLayerSource>>,
    layer_patches: BTreeMap<String, DependentStageLayerPatch>,
    reference_assets: HashMap<AssetId<UsdStageAsset>, Handle<UsdStageAsset>>,
    desired_revision: u64,
    submitted_revision: Option<u64>,
    phase: DependentStageRefreshPhase,
    live_patch_applied: bool,
    plan_failure: Option<String>,
    work_key: Option<AsyncWorkKey>,
    capacity_revision: Option<u64>,
    work_identity: u128,
    work_order: u64,
    progress_key: Option<SimulationProgressKey>,
    priority: AsyncWorkPriority,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct DependentStageLayerPatch {
    rebuild_stage: bool,
    prim_subtrees: BTreeSet<String>,
    property_subtrees: BTreeSet<String>,
    fields: BTreeSet<(String, String)>,
}

impl DependentStageLayerPatch {
    fn merge(&mut self, other: Self) {
        if self.rebuild_stage || other.rebuild_stage {
            *self = Self {
                rebuild_stage: true,
                ..Self::default()
            };
            return;
        }
        self.prim_subtrees.extend(other.prim_subtrees);
        self.property_subtrees.extend(other.property_subtrees);
        self.fields.extend(other.fields);
    }
}

#[derive(Clone)]
enum DependentStageRefreshMode {
    Incremental(DependentStageLayerPatch),
    Rebuild,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DependentStageRefreshPhase {
    Patch,
    Plan,
    Rebuild,
}

fn dependent_stage_work_priority(
    phase: DependentStageRefreshPhase,
    active: bool,
    inactive_priority: AsyncWorkPriority,
) -> AsyncWorkPriority {
    match phase {
        DependentStageRefreshPhase::Plan => AsyncWorkPriority::Background,
        DependentStageRefreshPhase::Patch | DependentStageRefreshPhase::Rebuild if active => {
            AsyncWorkPriority::SimulationRequired
        }
        DependentStageRefreshPhase::Patch | DependentStageRefreshPhase::Rebuild => {
            inactive_priority
        }
    }
}

struct PreparedDependentLayer {
    layer_id: String,
    source: Option<Arc<openusd::sdf::Data>>,
    patch: DependentStageLayerPatch,
}

struct PreparedDependentStagePlan {
    recipe: lunco_usd_compose::recipe::StageRecipe,
    projection_plan: UsdStageProjectionPlan,
}

struct PreparedDependentStagePatch {
    layers: Vec<PreparedDependentLayer>,
    reference_recipes: Vec<Arc<lunco_usd_compose::recipe::StageRecipe>>,
}

struct DependentStageLayerSource {
    doc: DocumentId,
    persistent_revision: (u64, u64),
    snapshot: Arc<UsdDocument>,
    data: OnceLock<Result<Arc<openusd::sdf::Data>, Arc<str>>>,
    serialized: OnceLock<Result<Arc<[u8]>, Arc<str>>>,
}

impl DependentStageLayerSource {
    fn persistent_data(&self) -> Result<Arc<openusd::sdf::Data>, String> {
        self.data
            .get_or_init(|| {
                let _span = bevy::log::info_span!(
                    "usd_twin_projection_dependent_source_compose",
                    doc = %self.doc,
                    base_revision = self.persistent_revision.0,
                    runtime_revision = self.persistent_revision.1
                )
                .entered();
                self.snapshot
                    .persistent_composed_data()
                    .map(Arc::new)
                    .map_err(|error| Arc::<str>::from(error.to_string()))
            })
            .clone()
            .map_err(|error| error.to_string())
    }

    fn persistent_bytes(&self) -> Result<Arc<[u8]>, String> {
        self.serialized
            .get_or_init(|| {
                let _span = bevy::log::info_span!(
                    "usd_twin_projection_dependent_source_serialize",
                    doc = %self.doc,
                    base_revision = self.persistent_revision.0,
                    runtime_revision = self.persistent_revision.1
                )
                .entered();
                self.persistent_data()
                    .and_then(|data| {
                        lunco_usd_authoring::author::data_to_usda(&data)
                            .map_err(|error| error.to_string())
                    })
                    .map(|source| Arc::from(source.into_bytes()))
                    .map_err(|error| Arc::<str>::from(error.to_string()))
            })
            .clone()
            .map_err(|error| error.to_string())
    }
}

enum DependentReferenceRecipes {
    Ready(Vec<Arc<lunco_usd_compose::recipe::StageRecipe>>),
    Waiting,
    Failed(String),
}

struct DependentStageRefreshCompletion {
    stage: AssetId<UsdStageAsset>,
    operation: u64,
    revision: u64,
    kind: DependentStageRefreshCompletionKind,
}

enum DependentStageRefreshCompletionKind {
    Patch(Result<PreparedDependentStagePatch, String>),
    Plan(Result<Option<PreparedDependentStagePlan>, String>),
}

impl PendingDependentStageRefreshes {
    fn allocate_operation(&mut self) -> Option<u64> {
        let operation = self.next_operation.checked_add(1)?;
        self.next_operation = operation;
        Some(operation)
    }

    fn has_admission_retry(&self, capacity_revision: u64) -> bool {
        self.by_stage.values().any(|pending| {
            pending.plan_failure.is_none()
                && pending.work_key.is_none()
                && pending
                    .capacity_revision
                    .is_none_or(|revision| revision != capacity_revision)
        })
    }
}

/// Default twin scenes whose base source is still loading. Drained by
/// [`drain_pending_twin_docs`].
#[derive(Resource, Default)]
pub(crate) struct PendingTwinDocs {
    items: Vec<PendingTwinDoc>,
    ready: HashSet<AssetId<UsdSourceText>>,
    failed: HashMap<AssetId<UsdSourceText>, String>,
    completions: Arc<Mutex<Vec<TwinDocCompletion>>>,
    next_operation: u64,
}

impl PendingTwinDocs {
    /// Queue a default twin scene for doc-backed projection.
    pub(crate) fn push(
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
            doc: None,
            failure_reported: false,
            stage: TwinDocPreparationStage::AwaitingSource,
            capacity_revision: None,
            work_key: None,
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

    #[cfg(test)]
    fn has_terminal_source_event(&self) -> bool {
        !self.ready.is_empty() || !self.failed.is_empty()
    }

    fn has_preparation_work(&self, capacity_revision: u64) -> bool {
        if !self
            .completions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
        {
            return true;
        }
        self.items.iter().any(|item| match item.stage {
            TwinDocPreparationStage::AwaitingSource => {
                self.failed.contains_key(&item.handle.id())
                    || (self.ready.contains(&item.handle.id())
                        && item.capacity_revision != Some(capacity_revision))
            }
            TwinDocPreparationStage::AwaitingPersistentSource { .. } => {
                item.capacity_revision != Some(capacity_revision)
            }
            TwinDocPreparationStage::PreparingSource { .. }
            | TwinDocPreparationStage::PreparingPersistentSource { .. }
            | TwinDocPreparationStage::Finished
            | TwinDocPreparationStage::Failed => false,
        })
    }

    fn allocate_operation(&mut self) -> Option<u64> {
        let operation = self.next_operation;
        self.next_operation = operation.checked_add(1)?;
        Some(operation)
    }

    fn take_completions(&mut self) -> Vec<TwinDocCompletion> {
        let mut completions = self
            .completions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut ready = std::mem::take(&mut *completions);
        ready.sort_by_key(|completion| completion.operation);
        ready
    }

    /// Release pending projection work for a closed Twin.
    pub(crate) fn release_root(&mut self, root: &Path) -> Vec<AsyncWorkKey> {
        let mut cancelled = Vec::new();
        self.items.retain(|item| {
            let keep = !lunco_doc::same_file(&item.root, root);
            if !keep && let Some(key) = item.work_key {
                cancelled.push(key);
            }
            keep
        });
        let live_ids: HashSet<_> = self.items.iter().map(|item| item.handle.id()).collect();
        self.ready.retain(|id| live_ids.contains(id));
        self.failed.retain(|id, _| live_ids.contains(id));
        let live_operations: HashSet<_> = self
            .items
            .iter()
            .filter_map(|item| match item.stage {
                TwinDocPreparationStage::PreparingSource { operation }
                | TwinDocPreparationStage::PreparingPersistentSource { operation, .. } => {
                    Some(operation)
                }
                TwinDocPreparationStage::AwaitingSource
                | TwinDocPreparationStage::AwaitingPersistentSource { .. }
                | TwinDocPreparationStage::Finished
                | TwinDocPreparationStage::Failed => None,
            })
            .collect();
        self.completions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|completion| live_operations.contains(&completion.operation));
        cancelled
    }
}

/// A **referenced spawn** whose asset closure is being fetched before it can be
/// authored onto the live scene stage. When a structural edit adds a prim that
/// references an asset whose layer bytes aren't loaded into the scene's live
/// resolver yet (a first-of-its-kind rover spawn), [`sync_twin_overlays`] loads
/// that asset as a `UsdStageAsset` (whose loader fetches the available closure,
/// web-ready) and queues this. [`drain_ref_spawns`] injects the fetched bytes
/// into the scene stage's resolver and authors the prim + `references` arc, so
/// the openusd change sink fires and `project_stage_changes` instantiates the
/// composed subtree — no whole-scene reload.
struct RefSpawn {
    /// Stable identity for this reference admission operation.
    progress_key: SimulationProgressKey,
    /// The scene whose live [`CanonicalStage`](lunco_usd_bevy_stage::canonical::CanonicalStage)
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
    /// Optional explicit prim path inside the referenced layer. `None` uses
    /// the loaded asset's default prim.
    reference_prim_path: Option<String>,
    /// In-flight load of the referenced asset (its loader fetches the closure).
    ref_handle: Option<Handle<UsdStageAsset>>,
    /// A SetTranslate may follow AddPrim in the same edit burst. Keep it until
    /// the reference closure is installed; otherwise the edit arrives before
    /// the prim exists on the live stage and the new prim stays at origin.
    translate: Option<[f64; 3]>,
    /// Child-scoped edits that arrive while the referenced root is still
    /// loading. They are replayed after the reference and its composed
    /// subtree exist, preserving the original ordered document intent.
    deferred_ops: Vec<UsdOp>,
    /// Current root activation state while the live reference transaction is
    /// pending. A root that is inactive before its asset arrives is not
    /// materialized into the live stage; a later reactivation keeps the same
    /// transaction valid without authoring an inactive intermediate prim.
    active: bool,
    /// Whether this operation currently gates time for the mounted primary scene.
    held: bool,
    /// Whether the asset event has already been consumed while the root was
    /// inactive. The loaded asset remains available for a later activation.
    asset_ready: bool,
    /// Terminal closure/projection failure, retained until the authored root
    /// is replaced or removed.
    failure: Option<String>,
    /// Whether the retained failure has been published to the runtime fault
    /// and diagnostic owners while the root is active.
    failure_reported: bool,
    /// The document removed this pending root before its reference became
    /// live. Keep the transaction until the asset event is consumed so later
    /// descendant edits cannot leak onto the live stage; a new AddPrim at the
    /// same path replaces this tombstone.
    removed: bool,
}

/// Referenced spawns waiting on their asset closure to finish loading.
/// Populated by [`sync_twin_overlays`], drained by [`drain_ref_spawns`].
#[derive(Resource, Default)]
pub(crate) struct PendingRefSpawns {
    pub(crate) native: crate::native_references::NativeReferencePreparations,
    items: Vec<RefSpawn>,
    ready: HashSet<AssetId<UsdStageAsset>>,
    failed: HashMap<AssetId<UsdStageAsset>, String>,
    next_operation_id: u64,
    held_keys: HashSet<SimulationProgressKey>,
    /// Strong handles held while a coarse document rebuild waits for a newly
    /// referenced closure. Without this retention the load becomes `Unused`
    /// before the async loader can publish its prepared asset.
    retained_assets: HashMap<(AssetId<UsdStageAsset>, String), Handle<UsdStageAsset>>,
}

/// One coalesced authoritative document-projection operation per document.
/// The key uses the stable document identity; `generations` fences completion
/// so projecting an older committed revision cannot release a newer edit.
#[derive(Resource, Default)]
pub(crate) struct PendingDocumentProjectionAdmissions {
    generations: HashMap<DocumentId, u64>,
}

impl PendingDocumentProjectionAdmissions {
    fn admit(&mut self, doc: DocumentId, generation: u64, progress: &mut SimulationProgress) {
        let key = SimulationProgressKey::usd_document_projection(doc.raw());
        match self.generations.get_mut(&doc) {
            Some(target) => {
                *target = (*target).max(generation);
                progress.update_reason(
                    key,
                    format!("Project USD document {doc} generation {}", *target),
                );
            }
            None => {
                self.generations.insert(doc, generation);
                progress.acquire(
                    key,
                    format!("Project USD document {doc} generation {generation}"),
                );
            }
        }
    }

    fn complete(&mut self, doc: DocumentId, generation: u64, progress: &mut SimulationProgress) {
        if self
            .generations
            .get(&doc)
            .is_some_and(|target| generation >= *target)
        {
            self.generations.remove(&doc);
            progress.release(SimulationProgressKey::usd_document_projection(doc.raw()));
        }
    }

    fn clear(&mut self, progress: &mut SimulationProgress) {
        for doc in self.generations.keys().copied().collect::<Vec<_>>() {
            progress.release(SimulationProgressKey::usd_document_projection(doc.raw()));
        }
        self.generations.clear();
    }
}

/// Preserve authored reference order when asset outcomes arrive in a different
/// order. Preparation remains parallel; live commits and terminal failure
/// publication for the authoritative scene wait behind earlier unresolved
/// references.
#[derive(Default)]
struct PrimaryReferenceCommitOrder {
    blocked_scenes: HashSet<AssetId<UsdStageAsset>>,
}

impl PrimaryReferenceCommitOrder {
    fn must_defer(
        &mut self,
        scene_id: AssetId<UsdStageAsset>,
        authoritative: bool,
        result_available: bool,
    ) -> bool {
        if !authoritative {
            return false;
        }
        if self.blocked_scenes.contains(&scene_id) {
            return true;
        }
        if !result_available {
            self.blocked_scenes.insert(scene_id);
            return true;
        }
        false
    }

    fn block_successors(&mut self, scene_id: AssetId<UsdStageAsset>, authoritative: bool) {
        if authoritative {
            self.blocked_scenes.insert(scene_id);
        }
    }
}

/// Prepared source plans waiting for the live-stage sink to create their
/// corresponding instance root. The key is the canonical scene plus authored
/// prim path, so a plan can never be attached to another Twin or another
/// instance with the same asset.
#[derive(Resource, Default)]
pub(crate) struct PendingInstanceProjections {
    plans: HashMap<(AssetId<UsdStageAsset>, String), PendingInstanceProjection>,
}

/// Whether an affected referenced subtree has reached its live ECS owner.
/// Unrelated pending references in the mounted stage do not delay this
/// document edit's completion boundary.
pub(crate) fn has_pending_reference_projection(
    world: &World,
    scene_id: AssetId<UsdStageAsset>,
    required_reference_paths: &std::collections::BTreeSet<String>,
) -> bool {
    let waiting_for_reference = world
        .get_resource::<PendingRefSpawns>()
        .is_some_and(|pending| {
            pending.items.iter().any(|item| {
                item.scene_id == scene_id
                    && item.active
                    && !item.removed
                    && required_reference_paths.contains(&item.prim_path)
            })
        });
    let waiting_for_instance = world
        .get_resource::<PendingInstanceProjections>()
        .is_some_and(|pending| {
            pending.plans.keys().any(|(stage_id, path)| {
                *stage_id == scene_id && required_reference_paths.contains(path)
            })
        });
    waiting_for_reference || waiting_for_instance
}

pub(crate) struct PendingInstanceProjection {
    pub(crate) projection: UsdInstanceProjection,
    pub(crate) progress_key: SimulationProgressKey,
    pub(crate) failure_reported: bool,
}

impl PendingInstanceProjections {
    fn insert(
        &mut self,
        scene_id: AssetId<UsdStageAsset>,
        prim_path: String,
        projection: UsdInstanceProjection,
        progress_key: SimulationProgressKey,
    ) {
        self.plans.insert(
            (scene_id, prim_path),
            PendingInstanceProjection {
                projection,
                progress_key,
                failure_reported: false,
            },
        );
    }

    pub(crate) fn take(
        &mut self,
        scene_id: AssetId<UsdStageAsset>,
        prim_path: &str,
    ) -> Option<PendingInstanceProjection> {
        self.plans.remove(&(scene_id, prim_path.to_string()))
    }

    pub(crate) fn remove(
        &mut self,
        scene_id: AssetId<UsdStageAsset>,
        prim_path: &str,
    ) -> Option<PendingInstanceProjection> {
        self.plans.remove(&(scene_id, prim_path.to_string()))
    }

    fn progress_key(
        &self,
        scene_id: AssetId<UsdStageAsset>,
        prim_path: &str,
    ) -> Option<SimulationProgressKey> {
        self.plans
            .get(&(scene_id, prim_path.to_string()))
            .map(|pending| pending.progress_key)
    }
}

impl PendingRefSpawns {
    fn allocate_progress_key(&mut self) -> Option<SimulationProgressKey> {
        let operation_id = self.next_operation_id;
        self.next_operation_id = operation_id.checked_add(1)?;
        Some(SimulationProgressKey {
            owner: SimulationProgressOwner::SceneReferences,
            operation_id,
        })
    }

    fn push(&mut self, item: RefSpawn, ready: bool) {
        if ready && let Some(handle) = &item.ref_handle {
            self.ready.insert(handle.id());
        }
        self.items.push(item);
    }

    fn replace_path(
        &mut self,
        scene_id: AssetId<UsdStageAsset>,
        prim_path: &str,
    ) -> Vec<SimulationProgressKey> {
        let mut released = Vec::new();
        let retired = self
            .items
            .iter()
            .filter(|item| item.scene_id == scene_id && item.prim_path == prim_path)
            .map(|item| item.asset_path.clone())
            .collect::<Vec<_>>();
        for reference in retired {
            self.native.retire_input(scene_id, &reference);
        }
        self.items.retain(|item| {
            let keep = !(item.scene_id == scene_id && item.prim_path == prim_path);
            if !keep && !item.failure_reported {
                released.push(item.progress_key);
            }
            keep
        });
        released
    }

    fn index_for_path(&self, scene_id: AssetId<UsdStageAsset>, path: &str) -> Option<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.scene_id == scene_id
                    && (path == item.prim_path
                        || path
                            .strip_prefix(&item.prim_path)
                            .is_some_and(|suffix| suffix.starts_with('/')))
            })
            .max_by_key(|(_, item)| item.prim_path.len())
            .map(|(index, _)| index)
    }

    fn mark_ready(&mut self, id: AssetId<UsdStageAsset>) {
        self.native.mark_asset(id);
        if self.items.iter().any(|item| {
            item.ref_handle
                .as_ref()
                .is_some_and(|handle| handle.id() == id)
        }) {
            self.ready.insert(id);
        }
    }

    fn mark_failed(&mut self, id: AssetId<UsdStageAsset>, error: String) {
        self.native.mark_failed(id, &error);
        if self.items.iter().any(|item| {
            item.ref_handle
                .as_ref()
                .is_some_and(|handle| handle.id() == id)
        }) {
            self.failed.insert(id, error);
        }
    }

    fn has_terminal_asset_event(&self) -> bool {
        !self.ready.is_empty() || !self.failed.is_empty()
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReferenceAssetState {
    Prepared,
    Loading,
    Failed(String),
}

/// Inspect the authoritative asset owners when a reference operation enters
/// the queue. Asset events may already have been consumed before the authored
/// edit is projected, so queue admission also observes the current store and
/// load state.
fn reference_asset_state(world: &World, id: AssetId<UsdStageAsset>) -> ReferenceAssetState {
    let load_state = world.resource::<AssetServer>().get_load_state(id);
    let failure = load_state.as_ref().and_then(|state| match state {
        bevy::asset::LoadState::Failed(error) => Some(error.to_string()),
        _ => None,
    });
    let loading = load_state.as_ref().is_some_and(|state| state.is_loading());
    let prepared = world.resource::<Assets<UsdStageAsset>>().get(id).is_some();
    classify_reference_asset_state(prepared, loading, failure)
}

fn classify_reference_asset_state(
    prepared: bool,
    loading: bool,
    failure: Option<String>,
) -> ReferenceAssetState {
    if let Some(error) = failure {
        ReferenceAssetState::Failed(error)
    } else if loading {
        ReferenceAssetState::Loading
    } else if prepared {
        ReferenceAssetState::Prepared
    } else {
        ReferenceAssetState::Loading
    }
}

/// Admit a referenced spawn using the asset state that exists when its
/// structural edit reaches the live-stage owner. Lifecycle messages can be
/// consumed before this operation is queued, so the current store and load
/// state determine its initial readiness or failure.
fn enqueue_reference_spawn(world: &mut World, mut item: RefSpawn) {
    let Some(id) = item.ref_handle.as_ref().map(Handle::id) else {
        world.resource_mut::<PendingRefSpawns>().push(item, false);
        return;
    };
    let state = reference_asset_state(world, id);
    let asset_ready = matches!(&state, ReferenceAssetState::Prepared);
    item.asset_ready = asset_ready;
    let failure = match state {
        ReferenceAssetState::Failed(error) => Some(error),
        ReferenceAssetState::Prepared | ReferenceAssetState::Loading => None,
    };
    world
        .resource_mut::<PendingRefSpawns>()
        .push(item, asset_ready);
    if let Some(error) = failure {
        world
            .resource_mut::<PendingRefSpawns>()
            .mark_failed(id, error);
    }
}

pub(crate) fn is_authoritative_scene_stage(
    world: &World,
    scene_id: AssetId<UsdStageAsset>,
) -> bool {
    let Some(root) = world
        .get_resource::<lunco_core::SceneMountState>()
        .and_then(lunco_core::SceneMountState::active_root)
    else {
        return false;
    };
    world
        .get::<UsdPrimPath>(root)
        .is_some_and(|path| path.stage_handle.id() == scene_id)
}

fn acquire_reference_progress(
    world: &mut World,
    key: SimulationProgressKey,
    scene_id: AssetId<UsdStageAsset>,
    reason: String,
) -> bool {
    if !is_authoritative_scene_stage(world, scene_id) {
        return false;
    }
    world
        .resource_mut::<SimulationProgress>()
        .acquire(key, reason);
    world
        .resource_mut::<PendingRefSpawns>()
        .held_keys
        .insert(key);
    true
}

pub(crate) fn release_reference_progress(world: &mut World, key: SimulationProgressKey) {
    if let Some(mut progress) = world.get_resource_mut::<SimulationProgress>() {
        progress.release(key);
    }
    if let Some(mut pending) = world.get_resource_mut::<PendingRefSpawns>() {
        pending.held_keys.remove(&key);
    }
}

fn activate_reference_progress(world: &mut World, item: &mut RefSpawn) {
    if !item.held {
        let reason = format!(
            "Preparing USD reference {} from `{}`",
            item.prim_path, item.asset_path
        );
        item.held = acquire_reference_progress(world, item.progress_key, item.scene_id, reason);
    }
}

fn deactivate_reference_progress(world: &mut World, item: &mut RefSpawn) {
    if item.held && !item.failure_reported {
        release_reference_progress(world, item.progress_key);
        item.held = false;
    }
}

fn set_pending_reference_active(world: &mut World, index: usize, active: bool) {
    let state = {
        let mut pending = world.resource_mut::<PendingRefSpawns>();
        let Some(item) = pending.items.get_mut(index) else {
            return;
        };
        if item.removed {
            return;
        }
        let changed = item.active != active;
        item.active = active;
        (
            item.progress_key,
            item.ref_handle.as_ref().map(Handle::id),
            item.held,
            item.asset_ready,
            item.failure.clone(),
            item.failure_reported,
            item.scene_id,
            item.prim_path.clone(),
            item.asset_path.clone(),
            changed,
        )
    };
    let (
        key,
        asset_id,
        held,
        asset_ready,
        failure,
        failure_reported,
        scene_id,
        prim_path,
        asset_path,
        changed,
    ) = state;
    if !active {
        if held && !failure_reported {
            release_reference_progress(world, key);
            if let Some(item) = world
                .resource_mut::<PendingRefSpawns>()
                .items
                .get_mut(index)
            {
                item.held = false;
            }
        }
        return;
    }
    if !held {
        let acquired = acquire_reference_progress(
            world,
            key,
            scene_id,
            format!("Preparing USD reference {prim_path} from `{asset_path}`"),
        );
        if let Some(item) = world
            .resource_mut::<PendingRefSpawns>()
            .items
            .get_mut(index)
        {
            item.held = acquired;
        }
    }
    if changed {
        let mut pending = world.resource_mut::<PendingRefSpawns>();
        if asset_ready && let Some(asset_id) = asset_id {
            pending.ready.insert(asset_id);
        }
        if let Some(error) = &failure
            && let Some(asset_id) = asset_id
        {
            pending.failed.insert(asset_id, error.clone());
        }
    }
    if let Some(error) = failure.filter(|_| !failure_reported) {
        report_reference_failure(world, key, scene_id, &prim_path, &asset_path, &error);
        if let Some(item) = world
            .resource_mut::<PendingRefSpawns>()
            .items
            .get_mut(index)
        {
            item.failure_reported = true;
        }
    }
}

fn cancel_pending_reference(world: &mut World, index: usize) {
    let retired = world
        .resource::<PendingRefSpawns>()
        .items
        .get(index)
        .map(|item| (item.scene_id, item.asset_path.clone()));
    if let Some((scene, reference)) = retired {
        world
            .resource_mut::<PendingRefSpawns>()
            .native
            .retire_input(scene, &reference);
    }
    let (key, held) = {
        let mut pending = world.resource_mut::<PendingRefSpawns>();
        let Some(item) = pending.items.get_mut(index) else {
            return;
        };
        item.removed = true;
        item.translate = None;
        item.deferred_ops.clear();
        (item.progress_key, item.held)
    };
    let failure_reported = world.resource::<PendingRefSpawns>().items[index].failure_reported;
    if held && !failure_reported {
        release_reference_progress(world, key);
        if let Some(item) = world
            .resource_mut::<PendingRefSpawns>()
            .items
            .get_mut(index)
        {
            item.held = false;
        }
    }
}

fn fail_reference_spawn(world: &mut World, item: &mut RefSpawn, detail: String) {
    item.failure = Some(detail.clone());
    if item.active && !item.failure_reported {
        item.held = report_reference_failure(
            world,
            item.progress_key,
            item.scene_id,
            &item.prim_path,
            &item.asset_path,
            &detail,
        );
        item.failure_reported = true;
    } else if !item.failure_reported {
        item.failure_reported = true;
    }
}

fn evaluate_reference_composition_policy(
    world: &World,
    scene_id: AssetId<UsdStageAsset>,
    diagnostics: &[lunco_usd_compose::recipe::StageDependencyDiagnostic],
) -> Result<IncompleteCompositionDecision, String> {
    let scene_path = world
        .get_resource::<AssetServer>()
        .and_then(|asset_server| asset_server.get_path(scene_id))
        .map(|path| path.to_string())
        .ok_or_else(|| {
            format!("the mounted scene asset {scene_id:?} has no registered source address")
        })?;
    let coordinator = world
        .get_resource::<lunco_core::SceneTransitionCoordinator>()
        .ok_or_else(|| "the scene transition coordinator is unavailable".to_owned())?;
    evaluate_incomplete_composition_policy(&scene_path, diagnostics, coordinator)
}

fn deferred_op_is_represented_by_instance_plan(op: &UsdOp, root_path: &str) -> bool {
    match op {
        UsdOp::SetRotate { path, .. } | UsdOp::SetScale { path, .. } => path == root_path,
        UsdOp::SetAttribute {
            path,
            name,
            type_name,
            ..
        } => path == root_path && name == "lunco:catalogId" && type_name == "string",
        _ => false,
    }
}

/// Keep every failed document projection diagnosable, while only a failure on
/// the mounted primary scene faults and holds authoritative simulation time.
pub(crate) fn report_reference_failure(
    world: &mut World,
    key: SimulationProgressKey,
    scene_id: AssetId<UsdStageAsset>,
    prim_path: &str,
    asset_path: &str,
    detail: &str,
) -> bool {
    const PRODUCER: &str = "usd-reference-admission";
    let subject = format!("{scene_id:?}:{prim_path}");
    let reason = format!("USD reference {prim_path} could not be admitted: {detail}");
    let authoritative = acquire_reference_progress(world, key, scene_id, reason.clone());
    if authoritative {
        world
            .get_resource_or_insert_with(lunco_core::RuntimeFaults::default)
            .raise("usd-reference-admission", None, subject.clone(), detail);
    }
    let mut diagnostics =
        world.get_resource_or_insert_with(lunco_core::RuntimeDiagnostics::default);
    diagnostics
        .findings
        .retain(|finding| !(finding.producer == PRODUCER && finding.subject == subject));
    diagnostics.findings.push(lunco_core::RuntimeDiagnostic {
        code: "usd-reference-admission".to_owned(),
        severity: lunco_core::DiagnosticSeverity::Error,
        producer: PRODUCER.to_owned(),
        subject,
        message: format!("`{asset_path}`: {detail}"),
    });
    diagnostics
        .findings
        .sort_by(|left, right| left.subject.cmp(&right.subject));
    error!("[twin] {reason}");
    authoritative
}

pub(crate) fn fail_pending_instance_projection(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    prim_path: &str,
    detail: &str,
) {
    let key = world
        .get_resource::<PendingInstanceProjections>()
        .and_then(|pending| pending.progress_key(scene_id, prim_path));
    if let Some(key) = key {
        report_reference_failure(
            world,
            key,
            scene_id,
            prim_path,
            "prepared USD instance projection",
            detail,
        );
        if let Some(mut pending) = world.get_resource_mut::<PendingInstanceProjections>() {
            if let Some(pending) = pending.plans.get_mut(&(scene_id, prim_path.to_owned())) {
                pending.failure_reported = true;
            }
        }
    }
}

/// Clear asynchronous referenced-spawn work owned by the outgoing scene.
///
/// Pending default-scene document loads are owned by their admitted Twin and
/// are released through [`PendingTwinDocs::release_root`] when that Twin closes.
pub(crate) fn reset_scene_projection_state(
    mut pending_refs: ResMut<PendingRefSpawns>,
    mut pending_document_projections: Option<ResMut<PendingDocumentProjectionAdmissions>>,
    mut pending_dependent_refreshes: Option<ResMut<PendingDependentStageRefreshes>>,
    mut pending_instances: Option<ResMut<PendingInstanceProjections>>,
    mut pending_stage_projections: Option<ResMut<crate::live_consume::PendingStageProjections>>,
    mut admission: Option<ResMut<AsyncWorkAdmission>>,
    mut pending_native_paths: Option<ResMut<crate::native_assets::PendingNativeAssetPaths>>,
    mut progress: Option<ResMut<SimulationProgress>>,
) {
    if let Some(progress) = progress.as_deref_mut() {
        for key in pending_refs.held_keys.drain() {
            progress.release(key);
        }
        if let Some(admissions) = pending_document_projections.as_deref_mut() {
            admissions.clear(progress);
        }
    }
    if let Some(pending) = pending_dependent_refreshes.as_deref_mut() {
        for refresh in pending.by_stage.values() {
            if let (Some(admission), Some(key)) = (admission.as_deref_mut(), refresh.work_key) {
                admission.cancel_queued(key);
            }
            if let (Some(progress), Some(key)) = (progress.as_deref_mut(), refresh.progress_key) {
                progress.release(key);
            }
        }
        pending.by_stage.clear();
        pending
            .completions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
    if let Some(pending) = pending_native_paths.as_deref_mut() {
        pending.clear(admission.as_deref_mut(), progress.as_deref_mut());
    }
    pending_refs.native.clear(admission.as_deref_mut());
    pending_refs.items.clear();
    pending_refs.ready.clear();
    pending_refs.failed.clear();
    pending_refs.retained_assets.clear();
    pending_refs.held_keys.clear();
    if let Some(pending_instances) = pending_instances.as_deref_mut() {
        pending_instances.plans.clear();
    }
    if let Some(pending_stage_projections) = pending_stage_projections.as_deref_mut() {
        pending_stage_projections.clear();
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
    lunco_core::trigger_runtime_error(commands, TWIN_SCENE_LOAD_FAILED, detail);
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

pub(crate) fn pending_twin_docs_ready(
    pending: Res<PendingTwinDocs>,
    admission: Res<AsyncWorkAdmission>,
) -> bool {
    pending.has_preparation_work(admission.capacity_revision())
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

pub(crate) fn pending_ref_spawns_ready(
    pending: Res<PendingRefSpawns>,
    admission: Option<Res<AsyncWorkAdmission>>,
) -> bool {
    pending.has_terminal_asset_event()
        || pending.native.needs_work(
            admission
                .as_ref()
                .map_or(0, |admission| admission.capacity_revision()),
        )
}

/// Publish non-fatal USD closure misses for the stage identities that are
/// currently represented in the scene. OpenUSD has already dropped only the
/// unresolved arc; the rest of the stage remains usable. Keeping this as a
/// runtime diagnostic makes the authored URI actionable without turning a
/// recoverable composition issue into a second scene-load failure.
pub(crate) fn sync_stage_dependency_diagnostics(
    mut events: MessageReader<bevy::asset::AssetEvent<UsdStageAsset>>,
    added_prims: Query<(), Added<UsdPrimPath>>,
    prims: Query<&UsdPrimPath>,
    stages: Option<Res<Assets<UsdStageAsset>>>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    const PRODUCER: &str = "usd-composition";
    const MISSING_DEPENDENCY: &str = "USD_COMPOSITION_MISSING_DEPENDENCY";

    let mut saw_stage_event = false;
    let mut stage_ids = events
        .read()
        .filter_map(|event| {
            saw_stage_event = true;
            match event {
                bevy::asset::AssetEvent::Added { id }
                | bevy::asset::AssetEvent::Modified { id }
                | bevy::asset::AssetEvent::LoadedWithDependencies { id } => Some(*id),
                bevy::asset::AssetEvent::Removed { .. }
                | bevy::asset::AssetEvent::Unused { .. } => None,
            }
        })
        .collect::<HashSet<_>>();
    if !saw_stage_event && added_prims.is_empty() {
        return;
    }
    let Some(stages) = stages else {
        return;
    };

    stage_ids.extend(prims.iter().map(|prim| prim.stage_handle.id()));
    let mut findings = Vec::new();
    for stage_id in stage_ids {
        let Some(asset) = stages.get(stage_id) else {
            continue;
        };
        let Some(recipe) = asset.recipe.as_ref() else {
            continue;
        };
        for missing in &recipe.dependency_diagnostics {
            let message = missing.to_string();
            warn!("[{PRODUCER}] {message}");
            findings.push(lunco_core::RuntimeDiagnostic {
                code: MISSING_DEPENDENCY.to_owned(),
                severity: lunco_core::DiagnosticSeverity::Warning,
                producer: PRODUCER.to_owned(),
                subject: format!("{} -> {}", missing.referring_layer, missing.dependency),
                message,
            });
        }
    }
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        diagnostics.replace_producer(PRODUCER, findings);
    }
}

/// Prepare each pending Twin source through shared worker admission, commit the
/// exact parsed revision under the registry's file identity and dirty-state
/// rules, restore its runtime overlay, and asynchronously serialize the
/// persistent snapshot before publishing it to the Twin source. The async stage
/// load reads the overlay bytes, so the initial projection composes the complete
/// document state.
pub(crate) fn drain_pending_twin_docs(
    mut pending: ResMut<PendingTwinDocs>,
    mut admission: ResMut<AsyncWorkAdmission>,
    mut registry: ResMut<DocumentRegistry<UsdDocument>>,
    mut backed: ResMut<DocBackedTwinScenes>,
    wake: Res<TwinProjectionWake>,
    sources: Res<Assets<UsdSourceText>>,
    twin_roots: Res<TwinRoots>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    runtime_saves: Res<lunco_usd_bevy_runtime_persistence::RuntimeSaveJobs>,
    mut empty_reason: ResMut<EmptyViewportReason>,
    mut commands: Commands,
) {
    if pending.items.is_empty() {
        pending.take_completions();
        return;
    }
    let mut items = std::mem::take(&mut pending.items);
    for completion in pending.take_completions() {
        let Some(item) = items
            .iter_mut()
            .find(|item| item.operation() == Some(completion.operation))
        else {
            continue;
        };
        item.work_key = None;
        item.capacity_revision = None;
        match (&item.stage, completion.result) {
            (
                TwinDocPreparationStage::PreparingSource { operation },
                TwinDocCompletionResult::Source(result),
            ) if *operation == completion.operation => match result {
                Ok(prepared) => {
                    let current_source = sources
                        .get(&item.handle)
                        .map(|UsdSourceText(source)| source.as_str());
                    if current_source != Some(prepared.source_text()) {
                        item.stage = TwinDocPreparationStage::AwaitingSource;
                        continue;
                    }
                    let (doc, outcome) =
                        registry.open_prepared_file(item.abs_path.clone(), prepared, true);
                    match outcome {
                        OpenOutcome::KeptUnparsable => {
                            report_twin_doc_load_failed(
                                &mut empty_reason,
                                &mut commands,
                                &lunco_assets_core::twin_uri(&item.name, &item.rel),
                                "the source asset is not valid USDA; refusing to mount a stale document",
                            );
                            item.stage = TwinDocPreparationStage::Finished;
                            continue;
                        }
                        OpenOutcome::KeptDirty => warn!(
                            "[usd-e1b] `{}` has unsaved edits — keeping them; NOT re-reading from disk",
                            lunco_assets_core::twin_uri(&item.name, &item.rel)
                        ),
                        OpenOutcome::Allocated | OpenOutcome::Refreshed => {}
                    }
                    if let Some(ws) = workspace.as_deref() {
                        lunco_usd_bevy_runtime_persistence::restore_doc_runtime_with_pending(
                            ws,
                            &mut registry,
                            &runtime_saves,
                            doc,
                        );
                    }
                    let Some(host) = registry.host(doc) else {
                        report_twin_doc_load_failed(
                            &mut empty_reason,
                            &mut commands,
                            &lunco_assets_core::twin_uri(&item.name, &item.rel),
                            "the USD document closed before its Twin source was prepared",
                        );
                        item.stage = TwinDocPreparationStage::Finished;
                        continue;
                    };
                    backed.track(doc, item.root.clone(), item.name.clone(), item.rel.clone());
                    item.doc = Some(doc);
                    item.stage = TwinDocPreparationStage::AwaitingPersistentSource {
                        doc,
                        generation: host.document().generation(),
                    };
                }
                Err(error) => {
                    report_twin_doc_load_failed(
                        &mut empty_reason,
                        &mut commands,
                        &lunco_assets_core::twin_uri(&item.name, &item.rel),
                        format!("USDA preparation failed: {error}"),
                    );
                    item.stage = TwinDocPreparationStage::Finished;
                }
            },
            (
                TwinDocPreparationStage::PreparingPersistentSource {
                    operation,
                    doc,
                    generation,
                },
                TwinDocCompletionResult::PersistentSource(result),
            ) if *operation == completion.operation => {
                let (doc, generation) = (*doc, *generation);
                let Some(current_generation) =
                    registry.host(doc).map(|host| host.document().generation())
                else {
                    report_twin_doc_load_failed(
                        &mut empty_reason,
                        &mut commands,
                        &lunco_assets_core::twin_uri(&item.name, &item.rel),
                        "the USD document closed before its Twin source was prepared",
                    );
                    item.stage = TwinDocPreparationStage::Failed;
                    continue;
                };
                if current_generation != generation {
                    item.stage = TwinDocPreparationStage::AwaitingPersistentSource {
                        doc,
                        generation: current_generation,
                    };
                    continue;
                }
                let twin_path = lunco_assets_core::twin_uri(&item.name, &item.rel);
                let composed = match result {
                    Ok(composed) => composed,
                    Err(error) => {
                        report_twin_doc_load_failed(
                            &mut empty_reason,
                            &mut commands,
                            &twin_path,
                            format!("persistent USD source serialization failed: {error}"),
                        );
                        item.stage = TwinDocPreparationStage::Failed;
                        continue;
                    }
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
                    item.stage = TwinDocPreparationStage::Failed;
                    continue;
                }
                wake.wake();
                backed.mark_initial_projection(doc, generation);
                info!(
                    "[usd-e1b] default scene `{twin_path}` is doc-backed ({doc}) — mounting composed"
                );
                commands.trigger(LoadScene {
                    path: twin_path,
                    root_prim: String::new(),
                });
                item.stage = TwinDocPreparationStage::Finished;
            }
            _ => {}
        }
    }

    let capacity_revision = admission.capacity_revision();
    let mut still = Vec::new();
    for mut item in items {
        let twin_path = lunco_assets_core::twin_uri(&item.name, &item.rel);
        if item.stage == TwinDocPreparationStage::Finished {
            continue;
        }
        if let Some(error) = pending.failed.get(&item.handle.id()) {
            if let Some(key) = item.work_key.take() {
                admission.cancel_queued(key);
            }
            if !item.failure_reported {
                report_twin_doc_load_failed(
                    &mut empty_reason,
                    &mut commands,
                    &twin_path,
                    format!("the Twin source asset failed to load: {error}"),
                );
                item.failure_reported = true;
            }
            if item.doc.is_some() {
                item.stage = TwinDocPreparationStage::Failed;
                still.push(item);
            }
            continue;
        }

        match item.stage {
            TwinDocPreparationStage::AwaitingSource => {
                if !pending.ready.contains(&item.handle.id()) {
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
                if item.capacity_revision == Some(capacity_revision) {
                    still.push(item);
                    continue;
                }
                let Some(operation) = pending.allocate_operation() else {
                    report_twin_doc_load_failed(
                        &mut empty_reason,
                        &mut commands,
                        &twin_path,
                        "USD preparation operation id exhausted",
                    );
                    continue;
                };
                let key =
                    twin_usd_work_key(&item, lunco_hash::fnv1a64(source.as_bytes()), operation);
                let completions = Arc::clone(&pending.completions);
                let source = source.clone();
                let job = move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        PreparedUsdSource::parse(source)
                    }))
                    .map_err(|_| "USDA parser panicked".to_owned());
                    completions
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(TwinDocCompletion {
                            operation,
                            result: TwinDocCompletionResult::Source(result),
                        });
                };
                match admission.submit(AsyncWorkPriority::SimulationRequired, key, job) {
                    Ok(()) | Err(lunco_core_runtime::AsyncWorkRejection::DuplicateKey) => {
                        item.stage = TwinDocPreparationStage::PreparingSource { operation };
                        item.capacity_revision = None;
                        item.work_key = Some(key);
                    }
                    Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                        item.capacity_revision = Some(capacity_revision);
                    }
                    Err(lunco_core_runtime::AsyncWorkRejection::NativeDispatcherUnavailable) => {
                        report_twin_doc_load_failed(
                            &mut empty_reason,
                            &mut commands,
                            &twin_path,
                            "USD source preparation requires a worker transport on this host",
                        );
                        continue;
                    }
                }
                still.push(item);
            }
            TwinDocPreparationStage::AwaitingPersistentSource { doc, generation: _ } => {
                if item.capacity_revision == Some(capacity_revision) {
                    still.push(item);
                    continue;
                }
                let Some(host) = registry.host(doc) else {
                    report_twin_doc_load_failed(
                        &mut empty_reason,
                        &mut commands,
                        &twin_path,
                        "the USD document closed before its Twin source was prepared",
                    );
                    item.stage = TwinDocPreparationStage::Failed;
                    still.push(item);
                    continue;
                };
                let generation = host.document().generation();
                let snapshot = host.document().clone();
                let Some(operation) = pending.allocate_operation() else {
                    report_twin_doc_load_failed(
                        &mut empty_reason,
                        &mut commands,
                        &twin_path,
                        "USD preparation operation id exhausted",
                    );
                    item.stage = TwinDocPreparationStage::Failed;
                    still.push(item);
                    continue;
                };
                let key = twin_usd_work_key(&item, generation, operation);
                let completions = Arc::clone(&pending.completions);
                let job = move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        snapshot
                            .persistent_composed_source()
                            .map_err(|error| error.to_string())
                    }))
                    .unwrap_or_else(|_| {
                        Err("persistent USD source serializer panicked".to_owned())
                    });
                    completions
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(TwinDocCompletion {
                            operation,
                            result: TwinDocCompletionResult::PersistentSource(result),
                        });
                };
                match admission.submit(AsyncWorkPriority::SimulationRequired, key, job) {
                    Ok(()) | Err(lunco_core_runtime::AsyncWorkRejection::DuplicateKey) => {
                        item.stage = TwinDocPreparationStage::PreparingPersistentSource {
                            operation,
                            doc,
                            generation,
                        };
                        item.capacity_revision = None;
                        item.work_key = Some(key);
                    }
                    Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                        item.capacity_revision = Some(capacity_revision);
                        item.stage =
                            TwinDocPreparationStage::AwaitingPersistentSource { doc, generation };
                    }
                    Err(lunco_core_runtime::AsyncWorkRejection::NativeDispatcherUnavailable) => {
                        report_twin_doc_load_failed(
                            &mut empty_reason,
                            &mut commands,
                            &twin_path,
                            "USD source serialization requires a worker transport on this host",
                        );
                        item.stage = TwinDocPreparationStage::Failed;
                    }
                }
                still.push(item);
            }
            TwinDocPreparationStage::Finished => {}
            TwinDocPreparationStage::Failed => still.push(item),
            TwinDocPreparationStage::PreparingSource { .. }
            | TwinDocPreparationStage::PreparingPersistentSource { .. } => still.push(item),
        }
    }
    pending.items.extend(still);
    let live_ids: HashSet<_> = pending.items.iter().map(|item| item.handle.id()).collect();
    pending.ready.retain(|id| live_ids.contains(id));
    pending.failed.retain(|id, _| live_ids.contains(id));
}

impl PendingTwinDoc {
    fn operation(&self) -> Option<u64> {
        match self.stage {
            TwinDocPreparationStage::PreparingSource { operation }
            | TwinDocPreparationStage::PreparingPersistentSource { operation, .. } => {
                Some(operation)
            }
            TwinDocPreparationStage::AwaitingSource
            | TwinDocPreparationStage::AwaitingPersistentSource { .. }
            | TwinDocPreparationStage::Finished
            | TwinDocPreparationStage::Failed => None,
        }
    }
}

fn twin_usd_work_key(item: &PendingTwinDoc, source_revision: u64, operation: u64) -> AsyncWorkKey {
    AsyncWorkKey::new(
        AsyncWorkKind::UsdPreparation,
        lunco_hash::fnv1a64(item.root.to_string_lossy().as_bytes()),
        lunco_hash::fnv1a64(item.abs_path.to_string_lossy().as_bytes()) as u128,
        source_revision,
        operation,
    )
}

/// Keep each doc-backed Twin scene's in-memory next-mount source and live stage
/// in step with its document. The projection is woken by document and asset
/// lifecycle events; it does not poll every frame. Durable runtime-layer I/O
/// is owned separately by the asynchronous persistence plugin. Drops entries
/// whose document has closed.
/// Publish an already-serialized persistent source into the in-memory Twin
/// asset overlay for the next stage mount. Default Twin startup prepares that
/// source on the worker pool before calling this path; editor-preview initial
/// snapshots remain event-driven. Ordinary edits use typed incremental
/// operations on the live stage.
fn write_twin_overlay(
    world: &mut World,
    doc: DocumentId,
    name: &str,
    rel: &str,
    generation: u64,
) -> bool {
    let _span = bevy::log::info_span!("usd_twin_overlay_serialize_publish").entered();
    let source = world
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)
        .map(|h| h.document().persistent_composed_source());
    let Some(source) = source else { return false };
    let source = match source {
        Ok(source) => source,
        Err(error) => {
            warn!("[usd-e1b] cannot serialize persistent source for document {doc}: {error}");
            return false;
        }
    };
    if let Err(error) =
        world
            .resource::<TwinRoots>()
            .set_overlay(name, rel, Arc::new(source.into_bytes()))
    {
        warn!("[usd-e1b] could not publish persistent source for document {doc}: {error}");
        return false;
    }
    world
        .resource_mut::<DocBackedTwinScenes>()
        .mark_overlay_synced(doc, generation);
    true
}

pub(crate) fn sync_twin_overlays(world: &mut World) {
    // DocumentChanged, stage-asset lifecycle events, scene mounts, and viewport
    // installs all wake this owner. Consume the wake before inspecting the
    // tracked set so the normal render loop never performs a generation probe.
    world.resource_mut::<TwinProjectionWake>().consume();
    poll_dependent_stage_refreshes(world);

    // Snapshot tracked scenes (owned) so no resource borrow is held across the
    // world mutations below.
    let entries: Vec<(
        DocumentId,
        String,
        String,
        Option<u64>,
        Option<u64>,
        Option<u64>,
    )> = {
        let _span = bevy::log::info_span!("usd_twin_projection_snapshot_documents").entered();
        world.resource::<DocBackedTwinScenes>().entries().collect()
    };
    let preparing_docs: HashSet<_> = world
        .resource::<PendingTwinDocs>()
        .items
        .iter()
        .filter_map(|item| item.doc)
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
        let _span = bevy::log::info_span!("usd_twin_projection_find_mounted_scene").entered();
        let mut q = world.query_filtered::<&UsdPrimPath, With<UsdSceneRoot>>();
        q.iter(world).next().map(|p| p.stage_handle.id())
    };
    let active_doc: Option<DocumentId> = {
        let _span = bevy::log::info_span!("usd_twin_projection_resolve_active_document").entered();
        mounted.and_then(|id| {
            let path = world.resource::<AssetServer>().get_path(id)?;
            let rel = path.path().to_string_lossy().into_owned();
            let (name, rel) = lunco_assets_core::split_twin_rel(&rel)?;
            world.resource::<DocBackedTwinScenes>().doc_for(name, rel)
        })
    };
    // A preview may edit a referenced component document while another preview
    // is already showing an assembly that contains it. Keep those views live as
    // one graph by patching the component's `twin://` layer into each loaded
    // dependent canonical stage. The viewport state is deliberately untouched.
    for (doc, name, rel, applied, view_applied, overlay_synced) in entries {
        let _document_span = bevy::log::info_span!("usd_twin_projection_document_sync").entered();
        if preparing_docs.contains(&doc) {
            continue;
        }
        let preview_owned = world
            .resource::<DocBackedTwinScenes>()
            .has_preview_lease(doc);
        if active_doc != Some(doc) && !preview_owned {
            continue;
        }
        let active_twin_document = active_doc == Some(doc);
        // Read the generation before any whole-stage payload. The composed source
        // is serialized only when this event-driven owner observes a new
        // generation, never on the render loop.
        let (cur_gen, pending_ops) = {
            let Some(host) = world.resource::<DocumentRegistry<UsdDocument>>().host(doc) else {
                if let Err(error) = world.resource::<TwinRoots>().clear_overlay(&name, &rel) {
                    warn!("[usd-e1b] could not clear closed document overlay: {error}");
                }
                world
                    .resource_mut::<DocBackedTwinScenes>()
                    .forget_document(doc);
                continue;
            };
            let document = host.document();
            let cur_gen = document.generation();
            let view_layer_is_empty = document.view_data().is_empty();
            let pending_ops = if view_applied.is_none() {
                // The first mounted recipe already contains the current base
                // and runtime layers. Replay only view-layer ops from that
                // source's empty view baseline; unrelated edits and runtime
                // restores must not expire this presentation suffix.
                if view_layer_is_empty {
                    Some(Vec::new())
                } else {
                    document.view_ops_since_source_baseline()
                }
            } else {
                let persistent_cursor = applied.unwrap_or(cur_gen);
                let view_cursor = view_applied.unwrap_or(persistent_cursor);
                let history_cursor = persistent_cursor.min(view_cursor);
                document.ops_since(history_cursor).map(|ops| {
                    ops.into_iter()
                        .enumerate()
                        .filter_map(|(index, op)| {
                            let generation = history_cursor + index as u64 + 1;
                            let cursor = if op.edit_target().is_view() {
                                view_cursor
                            } else {
                                persistent_cursor
                            };
                            (generation > cursor).then_some(op)
                        })
                        .collect::<Vec<_>>()
                })
            };
            (cur_gen, pending_ops)
        };
        if Some(cur_gen) == applied && Some(cur_gen) == view_applied {
            // The live stage is current. Durable runtime-layer persistence is
            // scheduled independently from DocumentChanged; there is no stage
            // projection or whole-source serialization to do here.
            continue;
        }

        // Author-once: the scene's live stage is keyed by the cached
        // `twin://name/rel` UsdStageAsset id (AssetServer dedups by path). We
        // replay the **typed ops** the document recorded since the last sync
        // directly onto that stage — the op is the single delta description, so we
        // never re-derive an edit's value by reading it back out of `composed`.
        let twin_path = lunco_assets_core::twin_uri(&name, &rel);
        let source_path =
            match lunco_assets_core::asset_path::load_asset_path(&twin_path, None, None, None) {
                Ok(path) => path,
                Err(error) => {
                    world.trigger(lunco_core::RuntimeError {
                        name: "usd-source-address-invalid".into(),
                        message: format!(
                            "document {doc} has invalid composed source `{twin_path}`: {error}"
                        ),
                    });
                    continue;
                }
            };
        let scene_id = world
            .resource::<AssetServer>()
            .load::<UsdStageAsset>(source_path)
            .id();

        // The initial scene recipe contains base + runtime, but omits the
        // disposable view layer. Its retained operation suffix was selected
        // against the view source baseline above, independent of persistent
        // edit volume.
        // `None` means the relevant journal suffix expired or spans a full
        // source reset; the composed document remains the authoritative rebuild.
        let required_reference_paths = pending_ops
            .as_ref()
            .map(|ops| referenced_add_prim_paths(ops))
            .unwrap_or_default();
        let dependent_reference_paths = required_reference_paths.clone();
        let dependent_refresh = match pending_ops.as_deref() {
            None => Some(DependentStageRefreshMode::Rebuild),
            Some(ops) => dependent_stage_refresh_mode(ops),
        };
        let has_work = pending_ops
            .as_ref()
            .map(|ops| !ops.is_empty())
            .unwrap_or(true);

        if applied.is_none() {
            // First mount MUST publish the overlay so the async stage load composes
            // base ⊕ runtime from it. The prepared asset plan already contains
            // this composed document, so initial projection does not need a live
            // `Stage` on the UI thread.
            // Already done at this generation for a twin default scene
            // (`drain_pending_twin_docs` publishes before mounting); still needed
            // here for editor-viewport docs tracked via `track()`.
            if overlay_synced != Some(cur_gen)
                && !write_twin_overlay(world, doc, &name, &rel, cur_gen)
            {
                continue;
            }
            // The prepared plan is the complete persistent projection. View
            // operations remain pending until their separate cursor is consumed
            // below; replaying a view op into the persistent Twin overlay would
            // incorrectly make presentation durable.
        }

        // A pending transient presentation edit may be the first post-mount
        // change. Open the canonical stage only when there is an actual delta
        // to consume; stable Twin scenes retain the prepared-stage path.
        let stage_ready = world
            .get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
            .is_some_and(|stages| stages.get(scene_id).is_some());
        if has_work && !stage_ready {
            let recipe = {
                let _span =
                    bevy::log::info_span!("usd_twin_projection_clone_stage_recipe").entered();
                world
                    .resource::<Assets<UsdStageAsset>>()
                    .get(scene_id)
                    .and_then(|asset| asset.recipe.as_ref())
                    .cloned()
            };
            let Some(recipe) = recipe else {
                // The asset loader has not published the recipe yet. Keep the
                // document generation pending until the asset boundary makes
                // the canonical stage available.
                continue;
            };
            let built = {
                let _span =
                    bevy::log::info_span!("usd_twin_projection_get_or_build_live_stage").entered();
                world
                    .get_non_send_mut::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
                    .is_some_and(|mut stages| stages.get_or_build(scene_id, &recipe).is_some())
            };
            if !built {
                continue;
            }
        }

        let _delta_span =
            bevy::log::info_span!("usd_twin_projection_commit_document_delta").entered();
        match pending_ops {
            // Overflow, or a coarse op (ReplaceSource / MovePrim / keyframe
            // removal / composition arc — no incremental stage-author yet,
            // and whole-source undo may change surviving prims' values): rebuild the
            // stage from composed_source + the already-loaded closure. The
            // next mount rebuilds its in-memory Twin overlay from the
            // document's persistent source.
            None => {
                let _span =
                    bevy::log::info_span!("usd_twin_projection_rebuild_history_gap").entered();
                let cs = world
                    .resource::<DocumentRegistry<UsdDocument>>()
                    .host(doc)
                    .map(|h| h.document().composed_source())
                    .unwrap_or_default();
                if !rebuild_scene_from_composed(world, scene_id, &cs) {
                    continue;
                }
            }
            Some(ops) if ops.iter().any(op_needs_rebuild) => {
                let _span =
                    bevy::log::info_span!("usd_twin_projection_rebuild_coarse_ops").entered();
                if !ensure_reference_layers_for_rebuild(world, scene_id, &ops) {
                    // Keep the document generation pending until every new
                    // reference closure is available to the live resolver.
                    // Rebuilding first would permanently open a stage whose root
                    // source contains the arc but whose resolver cannot resolve
                    // it; a later variant/metadata edit would preserve the
                    // incomplete composition.
                    continue;
                }
                let cs = world
                    .resource::<DocumentRegistry<UsdDocument>>()
                    .host(doc)
                    .map(|h| h.document().composed_source())
                    .unwrap_or_default();
                if !rebuild_scene_from_composed(world, scene_id, &cs) {
                    continue;
                }
            }
            // Incremental: replay each op's typed delta onto the live stage.
            // The runtime persistence owner snapshots authored edits
            // asynchronously; the in-memory Twin overlay is rebuilt on the
            // next mount rather than serializing the whole stage here.
            Some(ops) => {
                let _span =
                    bevy::log::info_span!("usd_twin_projection_apply_incremental_ops").entered();
                for op in &ops {
                    apply_incremental_op_to_stage(world, scene_id, op);
                }
            }
        }
        drop(_delta_span);

        {
            let _span = bevy::log::info_span!("usd_twin_projection_commit_cursors").entered();
            world
                .resource_mut::<DocBackedTwinScenes>()
                .mark_applied(doc, scene_id, cur_gen);
            world
                .resource_mut::<DocBackedTwinScenes>()
                .mark_view_applied(doc, scene_id, cur_gen);
        }
        // A standalone Editor preview has no `UsdSceneRoot`, so the live ECS
        // sink cannot publish its cursor through `live_consume`.  The preview
        // renders the canonical stage directly; mark that stage consumed here
        // after its typed ops/rebuild have completed.  Mounted Twin scenes
        // deliberately stay on the live-consume boundary so a query can never
        // observe a generation before ECS projection has finished.
        if active_doc != Some(doc) {
            world
                .resource_mut::<DocBackedTwinScenes>()
                .mark_stage_projected(scene_id);
        } else {
            let _span =
                bevy::log::info_span!("usd_twin_projection_queue_live_stage_projection").entered();
            crate::live_consume::queue_stage_projection(
                world,
                doc,
                scene_id,
                cur_gen,
                required_reference_paths,
            );
        }
        // The live stage now owns this generation. Do not serialize the whole
        // composed scene into the asset overlay for ordinary edits. A later
        // mount publishes the current document once; loaded dependent stages
        // receive the affected layer through a subtree patch when the typed
        // operation has a bounded authored footprint.
        if let Some(mode) = dependent_refresh {
            let _span = bevy::log::info_span!("usd_twin_projection_refresh_dependent_stage_assets")
                .entered();
            refresh_dependent_stage_assets(
                world,
                doc,
                scene_id,
                &twin_path,
                active_twin_document,
                mode,
                &dependent_reference_paths,
            );
        }
    }
    submit_pending_dependent_stage_refreshes(world);
}

/// Propagate an edited component layer into already-loaded assembly stages.
///
/// `UsdStageAsset` recipes are intentionally immutable snapshots at the async
/// loader boundary.  That is the right property for deterministic loading, but
/// it means a separately opened component document cannot otherwise be observed
/// by an assembly whose resolver already captured the old bytes. Bounded typed
/// edits patch the changed authored layer in the existing canonical stage and
/// reconcile its affected paths immediately. Worker plan composition updates the
/// immutable asset snapshot for a future mount; it does not gate the live patch.
/// Coarse composition edits retain the full-stage reset path.
fn refresh_dependent_stage_assets(
    world: &mut World,
    changed_doc: DocumentId,
    changed_scene: AssetId<UsdStageAsset>,
    layer_id: &str,
    active_twin_document: bool,
    mode: DependentStageRefreshMode,
    required_reference_paths: &[String],
) {
    let candidates: Vec<(AssetId<UsdStageAsset>, String)> = {
        let _span = bevy::log::info_span!("usd_twin_projection_dependent_candidate_scan").entered();
        let assets = world.resource::<Assets<UsdStageAsset>>();
        assets
            .iter()
            .filter_map(|(id, asset)| {
                if id == changed_scene {
                    return None;
                }
                let recipe = asset.recipe.as_ref()?;
                recipe_depends_on_changed_layer(recipe, layer_id)
                    .then(|| (id, recipe.root_id.clone()))
            })
            .collect()
    };

    let candidate_roots: Vec<&str> = candidates
        .iter()
        .map(|(_, root_id)| root_id.as_str())
        .collect();
    info!(
        "[usd-live] component layer changed: doc={changed_doc} layer={layer_id} dependent_candidates={} roots={candidate_roots:?}",
        candidate_roots.len()
    );

    if candidates.is_empty() {
        return;
    }
    let policy_context = if lunco_hooks::get(COMPONENT_REFRESH_POLICY_HOOK).is_some() {
        let generation = world
            .get_resource::<lunco_core::SceneTransitionCoordinator>()
            .and_then(lunco_core::SceneTransitionCoordinator::lifecycle_generation);
        match component_refresh_runtime_context(active_twin_document, generation) {
            Ok(context) => Some(context),
            Err(error) => {
                warn!(
                    "[usd-live] hook {COMPONENT_REFRESH_POLICY_HOOK} has no valid owner context; dependent stages will not refresh: {error}"
                );
                return;
            }
        }
    } else {
        None
    };
    let Some(layer_source) = world
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(changed_doc)
        .map(|host| {
            let snapshot = Arc::new(host.document().clone());
            Arc::new(DependentStageLayerSource {
                doc: changed_doc,
                persistent_revision: (snapshot.base_revision(), snapshot.runtime_revision()),
                snapshot,
                data: OnceLock::new(),
                serialized: OnceLock::new(),
            })
        })
    else {
        return;
    };
    let reference_assets = world
        .get_resource::<PendingRefSpawns>()
        .map(|pending| {
            pending
                .items
                .iter()
                .filter(|item| {
                    item.scene_id == changed_scene
                        && required_reference_paths
                            .iter()
                            .any(|path| path == &item.prim_path)
                })
                .filter_map(|item| {
                    item.ref_handle
                        .as_ref()
                        .map(|handle| (handle.id(), handle.clone()))
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let layer_patch = match mode {
        DependentStageRefreshMode::Incremental(patch) => patch,
        DependentStageRefreshMode::Rebuild => DependentStageLayerPatch {
            rebuild_stage: true,
            ..DependentStageLayerPatch::default()
        },
    };

    for (stage_id, _) in candidates {
        match component_refresh_decision(changed_doc, layer_id, stage_id, policy_context) {
            ComponentRefreshDecision::Propagate => {}
            ComponentRefreshDecision::Defer => {
                info!(
                    "[usd-live] component refresh deferred by hook for dependent stage {stage_id:?} (layer {layer_id})"
                );
                continue;
            }
            ComponentRefreshDecision::Reject => {
                warn!(
                    "[usd-live] component refresh rejected by hook for dependent stage {stage_id:?} (layer {layer_id})"
                );
                continue;
            }
        }
        let (base_recipe, target_plan) = {
            let Some(asset) = world.resource::<Assets<UsdStageAsset>>().get(stage_id) else {
                continue;
            };
            let Some(recipe) = asset.recipe.as_ref() else {
                continue;
            };
            (recipe.clone(), asset.projection_plan.clone())
        };
        request_dependent_stage_refresh(
            world,
            stage_id,
            base_recipe,
            target_plan,
            layer_id,
            Arc::clone(&layer_source),
            layer_patch.clone(),
            reference_assets.clone(),
        );
    }
}

/// A changed root document is already projected by its owning canonical
/// stage. Only a distinct root recipe that composes the changed layer is a
/// dependent stage and needs refresh.
fn recipe_depends_on_changed_layer(
    recipe: &lunco_usd_compose::recipe::StageRecipe,
    layer_id: &str,
) -> bool {
    recipe.root_id != layer_id && recipe.bytes.contains_key(layer_id)
}

fn request_dependent_stage_refresh(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
    base_recipe: Arc<lunco_usd_compose::recipe::StageRecipe>,
    target_plan: Arc<UsdStageProjectionPlan>,
    layer_id: &str,
    layer_source: Arc<DependentStageLayerSource>,
    layer_patch: DependentStageLayerPatch,
    reference_assets: HashMap<AssetId<UsdStageAsset>, Handle<UsdStageAsset>>,
) {
    let compatible = world
        .resource::<PendingDependentStageRefreshes>()
        .by_stage
        .get(&stage_id)
        .is_some_and(|pending| Arc::ptr_eq(&pending.target_plan, &target_plan));
    if compatible {
        let Some(pending) = world
            .resource::<PendingDependentStageRefreshes>()
            .by_stage
            .get(&stage_id)
        else {
            return;
        };
        let source_changed = pending.changed_layers.get(layer_id).is_none_or(|current| {
            current.doc != layer_source.doc
                || current.persistent_revision != layer_source.persistent_revision
        });
        let previous_patch = pending
            .layer_patches
            .get(layer_id)
            .cloned()
            .unwrap_or_default();
        let mut merged_patch = previous_patch.clone();
        merged_patch.merge(layer_patch);
        let patch_changed = merged_patch != previous_patch;
        let new_reference_assets = reference_assets
            .into_iter()
            .filter(|(id, _)| !pending.reference_assets.contains_key(id))
            .collect::<HashMap<_, _>>();
        if !source_changed && !patch_changed && new_reference_assets.is_empty() {
            return;
        }
        let active = is_authoritative_scene_stage(world, stage_id);
        let requires_rebuild = merged_patch.rebuild_stage
            || pending
                .layer_patches
                .iter()
                .any(|(id, patch)| id != layer_id && patch.rebuild_stage)
            || pending.layer_patches.keys().any(|id| id != layer_id);
        let Some(desired_revision) = pending.desired_revision.checked_add(1) else {
            warn!("[usd-live] dependent stage refresh revision exhausted for {stage_id:?}");
            return;
        };
        let mut progress_key_to_release = None;
        let mut progress_key_to_acquire = None;
        let mut refreshes = world.resource_mut::<PendingDependentStageRefreshes>();
        if let Some(pending) = refreshes.by_stage.get_mut(&stage_id) {
            if source_changed {
                pending
                    .changed_layers
                    .insert(layer_id.to_owned(), layer_source);
            }
            pending
                .layer_patches
                .insert(layer_id.to_owned(), merged_patch);
            pending.reference_assets.extend(new_reference_assets);
            pending.desired_revision = desired_revision;
            pending.phase = if requires_rebuild {
                DependentStageRefreshPhase::Rebuild
            } else {
                DependentStageRefreshPhase::Patch
            };
            pending.live_patch_applied = false;
            pending.plan_failure = None;
            pending.priority = if active {
                AsyncWorkPriority::SimulationRequired
            } else {
                AsyncWorkPriority::Interactive
            };
            if active {
                let key = pending.progress_key.unwrap_or(SimulationProgressKey {
                    owner: SimulationProgressOwner::UsdDependentStageProjection,
                    operation_id: pending.operation,
                });
                pending.progress_key = Some(key);
                progress_key_to_acquire = Some(key);
            } else {
                progress_key_to_release = pending.progress_key.take();
            }
        }
        drop(refreshes);
        if let Some(key) = progress_key_to_acquire
            && let Some(mut progress) = world.get_resource_mut::<SimulationProgress>()
        {
            progress.acquire(
                key,
                format!("Preparing updated dependent USD stage {stage_id:?}"),
            );
        }
        release_dependent_stage_progress(world, progress_key_to_release);
        return;
    }

    if let Some(retired) = world
        .resource_mut::<PendingDependentStageRefreshes>()
        .by_stage
        .remove(&stage_id)
    {
        if let Some(key) = retired.work_key {
            world
                .resource_mut::<AsyncWorkAdmission>()
                .cancel_queued(key);
        }
        release_dependent_stage_progress(world, retired.progress_key);
    }
    let desired_revision = 1;

    let Some(operation) = world
        .resource_mut::<PendingDependentStageRefreshes>()
        .allocate_operation()
    else {
        warn!("[usd-live] dependent stage refresh operation id exhausted");
        return;
    };
    let active = is_authoritative_scene_stage(world, stage_id);
    let progress_key = active.then_some(SimulationProgressKey {
        owner: SimulationProgressOwner::UsdDependentStageProjection,
        operation_id: operation,
    });
    if let (Some(key), Some(mut progress)) =
        (progress_key, world.get_resource_mut::<SimulationProgress>())
    {
        progress.acquire(
            key,
            format!("Preparing updated dependent USD stage {stage_id:?}"),
        );
    }
    let identity = u128::from(lunco_hash::fnv1a64(base_recipe.root_id.as_bytes()));
    let work_order = lunco_hash::fnv1a64(base_recipe.root_id.as_bytes());
    let phase = if layer_patch.rebuild_stage {
        DependentStageRefreshPhase::Rebuild
    } else {
        DependentStageRefreshPhase::Patch
    };

    let pending = PendingDependentStageRefresh {
        operation,
        target_plan,
        base_recipe,
        changed_layers: BTreeMap::from([(layer_id.to_owned(), layer_source)]),
        layer_patches: BTreeMap::from([(layer_id.to_owned(), layer_patch)]),
        reference_assets,
        desired_revision,
        submitted_revision: None,
        phase,
        live_patch_applied: false,
        plan_failure: None,
        work_key: None,
        capacity_revision: None,
        work_identity: identity,
        work_order,
        progress_key,
        priority: if active {
            AsyncWorkPriority::SimulationRequired
        } else {
            AsyncWorkPriority::Interactive
        },
    };
    world
        .resource_mut::<PendingDependentStageRefreshes>()
        .by_stage
        .insert(stage_id, pending);
}

fn apply_dependent_layer_patch(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
    refresh: &PreparedDependentStagePatch,
) -> Result<bool, String> {
    use lunco_usd_bevy_stage::canonical::CanonicalStages;

    if refresh.layers.len() != 1 {
        return Ok(false);
    }
    let layer = &refresh.layers[0];
    if layer.patch.rebuild_stage {
        return Ok(false);
    }
    let Some(source) = layer.source.as_ref() else {
        return Err(format!(
            "incremental dependent patch for `{}` has no prepared source layer",
            layer.layer_id
        ));
    };

    let mut spec_paths = BTreeSet::new();
    spec_paths.extend(layer.patch.prim_subtrees.iter().cloned());
    spec_paths.extend(layer.patch.property_subtrees.iter().cloned());
    let spec_paths = spec_paths
        .into_iter()
        .map(|path| {
            openusd::sdf::Path::new(&path)
                .map_err(|error| format!("invalid dependent patch path `{path}`: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let fields = layer
        .patch
        .fields
        .iter()
        .map(|(path, field)| {
            openusd::sdf::Path::new(path)
                .map(|path| (path, field.clone()))
                .map_err(|error| format!("invalid dependent field path `{path}`: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let Some(mut stages) = world.get_non_send_mut::<CanonicalStages>() else {
        return Ok(false);
    };
    let Some(stage) = stages.get_mut(stage_id) else {
        return Ok(false);
    };
    for recipe in &refresh.reference_recipes {
        if !stage.add_layer_recipe(recipe) {
            return Err(format!(
                "dependent stage {:?} cannot accept referenced layer closure `{}`",
                stage_id, recipe.root_id
            ));
        }
    }
    stage
        .projector()
        .patch_layer(&layer.layer_id, source, &spec_paths, &fields)
        .map_err(|error| {
            format!(
                "failed to patch dependent layer `{}` in stage {:?}: {error}",
                layer.layer_id, stage_id
            )
        })?;
    Ok(true)
}

fn poll_dependent_stage_refreshes(world: &mut World) {
    let completions = {
        let pending = world.resource::<PendingDependentStageRefreshes>();
        std::mem::take(
            &mut *pending
                .completions
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    };
    for completion in completions {
        let layer_sources = {
            let refreshes = world.resource::<PendingDependentStageRefreshes>();
            let Some(current) = refreshes.by_stage.get(&completion.stage) else {
                continue;
            };
            if current.operation != completion.operation
                || current.submitted_revision != Some(completion.revision)
            {
                continue;
            }
            current.changed_layers.values().cloned().collect::<Vec<_>>()
        };
        let document_state = {
            let registry = world.resource::<DocumentRegistry<UsdDocument>>();
            layer_sources.iter().find_map(|source| {
                let Some(host) = registry.host(source.doc) else {
                    return Some((source.doc, false));
                };
                let document = host.document();
                ((document.base_revision(), document.runtime_revision())
                    != source.persistent_revision)
                    .then_some((source.doc, true))
            })
        };
        if let Some((doc, changed)) = document_state {
            if !changed {
                let retired = {
                    let mut refreshes = world.resource_mut::<PendingDependentStageRefreshes>();
                    refreshes
                        .by_stage
                        .get(&completion.stage)
                        .is_some_and(|pending| pending.operation == completion.operation)
                        .then(|| refreshes.by_stage.remove(&completion.stage))
                        .flatten()
                };
                if let Some(retired) = retired {
                    if let Some(key) = retired.work_key {
                        world
                            .resource_mut::<AsyncWorkAdmission>()
                            .cancel_queued(key);
                    }
                    release_dependent_stage_progress(world, retired.progress_key);
                }
                warn!(
                    "[usd-live] dependent stage {:?} refresh ignored because source document {doc} closed before commit",
                    completion.stage
                );
            } else if let Some(pending) = world
                .resource_mut::<PendingDependentStageRefreshes>()
                .by_stage
                .get_mut(&completion.stage)
                && pending.operation == completion.operation
                && pending.submitted_revision == Some(completion.revision)
            {
                pending.work_key = None;
                pending.submitted_revision = None;
                pending.capacity_revision = None;
            }
            continue;
        }

        if let DependentStageRefreshCompletionKind::Patch(result) = &completion.kind {
            let patch_is_current = world
                .resource::<PendingDependentStageRefreshes>()
                .by_stage
                .get(&completion.stage)
                .is_some_and(|pending| {
                    pending.operation == completion.operation
                        && pending.submitted_revision == Some(completion.revision)
                        && pending.desired_revision == completion.revision
                });
            if !patch_is_current {
                if let Some(pending) = world
                    .resource_mut::<PendingDependentStageRefreshes>()
                    .by_stage
                    .get_mut(&completion.stage)
                    && pending.operation == completion.operation
                    && pending.submitted_revision == Some(completion.revision)
                {
                    pending.work_key = None;
                    pending.submitted_revision = None;
                    pending.capacity_revision = None;
                }
                continue;
            }
            match result {
                Err(error) => {
                    warn!(
                        "[usd-e1b] dependent stage {:?} live patch preparation failed: {error}",
                        completion.stage
                    );
                    report_stage_projection_reset_failure(world, completion.stage, error.clone());
                    let pending = world
                        .resource_mut::<PendingDependentStageRefreshes>()
                        .by_stage
                        .remove(&completion.stage);
                    if let Some(pending) = pending {
                        retain_dependent_stage_plan_failure(
                            world,
                            completion.stage,
                            pending,
                            error.clone(),
                        );
                    }
                }
                Ok(patch) => {
                    let _patch_span = bevy::log::info_span!(
                        "usd_twin_projection_dependent_layer_patch_commit",
                        stage = ?completion.stage,
                        revision = completion.revision,
                    )
                    .entered();
                    let live_stage_updated =
                        match apply_dependent_layer_patch(world, completion.stage, &patch) {
                            Ok(updated) => updated,
                            Err(error) => {
                                report_stage_projection_reset_failure(
                                    world,
                                    completion.stage,
                                    error.clone(),
                                );
                                let pending = world
                                    .resource_mut::<PendingDependentStageRefreshes>()
                                    .by_stage
                                    .remove(&completion.stage);
                                if let Some(pending) = pending {
                                    retain_dependent_stage_plan_failure(
                                        world,
                                        completion.stage,
                                        pending,
                                        error,
                                    );
                                }
                                continue;
                            }
                        };
                    let authoritative = is_authoritative_scene_stage(world, completion.stage);
                    if !live_stage_updated && authoritative {
                        report_stage_projection_reset_failure(
                            world,
                            completion.stage,
                            "authoritative dependent stage has no canonical live stage to patch"
                                .to_owned(),
                        );
                        let pending = world
                            .resource_mut::<PendingDependentStageRefreshes>()
                            .by_stage
                            .remove(&completion.stage);
                        if let Some(pending) = pending {
                            retain_dependent_stage_plan_failure(
                                world,
                                completion.stage,
                                pending,
                                "authoritative dependent stage has no canonical live stage to patch"
                                    .to_owned(),
                            );
                        }
                        continue;
                    }
                    if live_stage_updated {
                        // Make the changed subtree visible before the worker
                        // composes the immutable plan used by a future mount.
                        let _projection_span = bevy::log::info_span!(
                            "usd_twin_projection_dependent_patch_live_consume",
                            stage = ?completion.stage,
                            revision = completion.revision,
                        )
                        .entered();
                        crate::live_consume::project_stage_changes(world);
                    }
                    let progress_key = {
                        let mut refreshes = world.resource_mut::<PendingDependentStageRefreshes>();
                        refreshes
                            .by_stage
                            .get_mut(&completion.stage)
                            .filter(|pending| {
                                pending.operation == completion.operation
                                    && pending.submitted_revision == Some(completion.revision)
                            })
                            .map(|pending| {
                                pending.live_patch_applied = live_stage_updated;
                                pending.phase = DependentStageRefreshPhase::Plan;
                                pending.work_key = None;
                                pending.submitted_revision = None;
                                pending.capacity_revision = None;
                                pending.plan_failure = None;
                                pending.progress_key.take()
                            })
                            .flatten()
                    };
                    release_dependent_stage_progress(world, progress_key);
                }
            }
            continue;
        }

        let DependentStageRefreshCompletionKind::Plan(result) = completion.kind else {
            continue;
        };

        let prepared = {
            let mut pending = world.resource_mut::<PendingDependentStageRefreshes>();
            let Some(current) = pending.by_stage.get_mut(&completion.stage) else {
                continue;
            };
            if current.operation != completion.operation
                || current.submitted_revision != Some(completion.revision)
            {
                continue;
            }
            current.work_key = None;
            current.submitted_revision = None;
            current.capacity_revision = None;
            if current.desired_revision != completion.revision {
                continue;
            }
            pending.by_stage.remove(&completion.stage)
        };
        let Some(mut pending_refresh) = prepared else {
            continue;
        };
        let requires_full_rebuild = pending_refresh.layer_patches.len() != 1
            || pending_refresh
                .layer_patches
                .values()
                .any(|patch| patch.rebuild_stage);
        if requires_full_rebuild && is_authoritative_scene_stage(world, completion.stage) {
            let key = pending_refresh
                .progress_key
                .unwrap_or(SimulationProgressKey {
                    owner: SimulationProgressOwner::UsdDependentStageProjection,
                    operation_id: pending_refresh.operation,
                });
            if let Some(mut progress) = world.get_resource_mut::<SimulationProgress>() {
                progress.acquire(
                    key,
                    format!(
                        "Committing updated dependent USD stage {:?}",
                        completion.stage
                    ),
                );
            }
            pending_refresh.progress_key = Some(key);
        }
        match result {
            Err(error) => {
                if pending_refresh.live_patch_applied {
                    report_dependent_stage_plan_failure(world, completion.stage, &error);
                    retain_dependent_stage_plan_failure(
                        world,
                        completion.stage,
                        pending_refresh,
                        error,
                    );
                } else {
                    report_stage_projection_reset_failure(world, completion.stage, error.clone());
                    retain_dependent_stage_plan_failure(
                        world,
                        completion.stage,
                        pending_refresh,
                        error,
                    );
                }
            }
            Ok(None) => {
                release_dependent_stage_progress(world, pending_refresh.progress_key);
                wake_reference_spawns_for_asset(world, completion.stage);
            }
            Ok(Some(refresh)) => {
                if let Some(paths) = refresh.projection_plan.native_asset_paths_snapshot()
                    && let Err(error) = paths.validate_owner(world.get_resource::<TwinRoots>())
                {
                    report_stage_projection_reset_failure(
                        world,
                        completion.stage,
                        error.to_string(),
                    );
                    retain_dependent_stage_plan_failure(
                        world,
                        completion.stage,
                        pending_refresh,
                        error.to_string(),
                    );
                    continue;
                }
                let current_plan = world
                    .resource::<Assets<UsdStageAsset>>()
                    .get(completion.stage)
                    .map(|asset| Arc::ptr_eq(&asset.projection_plan, &pending_refresh.target_plan));
                if current_plan != Some(true) {
                    release_dependent_stage_progress(world, pending_refresh.progress_key);
                    wake_reference_spawns_for_asset(world, completion.stage);
                    continue;
                }
                if !requires_full_rebuild {
                    if is_authoritative_scene_stage(world, completion.stage)
                        && !pending_refresh.live_patch_applied
                    {
                        report_stage_projection_reset_failure(
                            world,
                            completion.stage,
                            "incremental dependent stage plan completed before its live layer patch"
                                .to_owned(),
                        );
                        release_dependent_stage_progress(world, pending_refresh.progress_key);
                        wake_reference_spawns_for_asset(world, completion.stage);
                        continue;
                    }
                    let recipe = Arc::new(refresh.recipe);
                    let projection_plan = Arc::new(refresh.projection_plan);
                    let asset_updated = if let Some(mut asset) = world
                        .resource_mut::<Assets<UsdStageAsset>>()
                        .get_mut(completion.stage)
                    {
                        asset.recipe = Some(recipe);
                        asset.projection_plan = Arc::clone(&projection_plan);
                        true
                    } else {
                        false
                    };
                    if asset_updated && pending_refresh.live_patch_applied {
                        if let Some(mut stages) = world
                            .get_non_send_mut::<lunco_usd_bevy_stage::canonical::CanonicalStages>(
                        ) {
                            stages.mark_prepared_plan_snapshot(completion.stage, projection_plan);
                        }
                    }
                    release_dependent_stage_progress(world, pending_refresh.progress_key);
                    wake_reference_spawns_for_asset(world, completion.stage);
                    continue;
                }

                let replacement = match {
                    let _span =
                        bevy::log::info_span!("usd_twin_projection_dependent_live_stage_build")
                            .entered();
                    lunco_usd_bevy_stage::canonical::CanonicalStage::from_recipe(&refresh.recipe)
                } {
                    Ok(stage) => stage,
                    Err(error) => {
                        warn!(
                            "[usd-e1b] dependent stage {:?} live rebuild failed: {error}",
                            completion.stage
                        );
                        report_stage_projection_reset_failure(
                            world,
                            completion.stage,
                            error.to_string(),
                        );
                        retain_dependent_stage_plan_failure(
                            world,
                            completion.stage,
                            pending_refresh,
                            error.to_string(),
                        );
                        continue;
                    }
                };
                {
                    let _span =
                        bevy::log::info_span!("usd_twin_projection_dependent_reset_prepare")
                            .entered();
                    if !prepare_stage_projection_reset(world, completion.stage) {
                        retain_dependent_stage_plan_failure(
                            world,
                            completion.stage,
                            pending_refresh,
                            "dependent stage reset could not prepare its live projection"
                                .to_owned(),
                        );
                        continue;
                    }
                }

                let replaced = world
                    .get_non_send_mut::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
                    .is_some_and(|mut stages| {
                        stages.replace_rebuilt(completion.stage, replacement);
                        true
                    });
                if replaced {
                    let recipe = Arc::new(refresh.recipe);
                    let projection_plan = Arc::new(refresh.projection_plan);
                    let asset_updated = if let Some(mut asset) = world
                        .resource_mut::<Assets<UsdStageAsset>>()
                        .get_mut(completion.stage)
                    {
                        asset.recipe = Some(recipe);
                        asset.projection_plan = Arc::clone(&projection_plan);
                        true
                    } else {
                        false
                    };
                    if asset_updated {
                        if let Some(mut stages) = world
                            .get_non_send_mut::<lunco_usd_bevy_stage::canonical::CanonicalStages>(
                        ) {
                            stages.mark_prepared_plan_snapshot(completion.stage, projection_plan);
                        }
                    }
                    let _span =
                        bevy::log::info_span!("usd_twin_projection_dependent_visual_refresh")
                            .entered();
                    refresh_scene_visuals_prepared(world, completion.stage);
                } else {
                    let error =
                        "dependent stage plan was ready but its canonical stage could not be replaced"
                            .to_owned();
                    report_stage_projection_reset_failure(world, completion.stage, error.clone());
                    retain_dependent_stage_plan_failure(
                        world,
                        completion.stage,
                        pending_refresh,
                        error,
                    );
                    continue;
                }
                release_dependent_stage_progress(world, pending_refresh.progress_key);
                wake_reference_spawns_for_asset(world, completion.stage);
            }
        }
    }
}

fn release_dependent_stage_progress(
    world: &mut World,
    progress_key: Option<SimulationProgressKey>,
) {
    if let (Some(key), Some(mut progress)) =
        (progress_key, world.get_resource_mut::<SimulationProgress>())
    {
        progress.release(key);
    }
}

fn wake_reference_spawns_for_asset(world: &mut World, asset: AssetId<UsdStageAsset>) {
    if let Some(mut pending) = world.get_resource_mut::<PendingRefSpawns>() {
        pending.mark_ready(asset);
    }
}

fn retain_dependent_stage_plan_failure(
    world: &mut World,
    stage: AssetId<UsdStageAsset>,
    mut pending: PendingDependentStageRefresh,
    error: String,
) {
    pending.plan_failure = Some(error);
    pending.work_key = None;
    pending.submitted_revision = None;
    pending.capacity_revision = None;
    release_dependent_stage_progress(world, pending.progress_key.take());
    world
        .resource_mut::<PendingDependentStageRefreshes>()
        .by_stage
        .insert(stage, pending);
    wake_reference_spawns_for_asset(world, stage);
}

fn submit_pending_dependent_stage_refreshes(world: &mut World) {
    let capacity_revision = world.resource::<AsyncWorkAdmission>().capacity_revision();
    let mut requests = world
        .resource::<PendingDependentStageRefreshes>()
        .by_stage
        .iter()
        .filter(|(_, pending)| {
            pending.plan_failure.is_none()
                && pending.work_key.is_none()
                && pending
                    .capacity_revision
                    .is_none_or(|revision| revision != capacity_revision)
        })
        .map(|(stage, pending)| {
            let changed_layers = pending
                .changed_layers
                .iter()
                .map(|(layer_id, source)| (layer_id.clone(), Arc::clone(source)))
                .collect::<Vec<_>>();
            let layer_patches = pending.layer_patches.clone();
            let reference_assets = pending
                .reference_assets
                .values()
                .cloned()
                .collect::<Vec<_>>();
            (
                *stage,
                pending.operation,
                pending.desired_revision,
                pending.phase,
                pending.work_identity,
                pending.work_order,
                if pending.phase == DependentStageRefreshPhase::Plan {
                    AsyncWorkPriority::Background
                } else {
                    pending.priority
                },
                Arc::clone(&pending.base_recipe),
                changed_layers,
                layer_patches,
                reference_assets,
                pending.progress_key,
            )
        })
        .collect::<Vec<_>>();
    requests.sort_by_key(
        |(_, operation, _, _, identity, order, priority, _, _, _, _, _)| {
            (*priority, *order, *identity, *operation)
        },
    );

    for (
        stage,
        operation,
        revision,
        phase,
        identity,
        order,
        priority,
        base_recipe,
        changed_layers,
        layer_patches,
        reference_handles,
        progress_key,
    ) in requests
    {
        let reference_recipes = {
            let assets = world.resource::<Assets<UsdStageAsset>>();
            let pending_refs = world.get_resource::<PendingRefSpawns>();
            'resolve: {
                let mut recipes = Vec::with_capacity(reference_handles.len());
                for handle in &reference_handles {
                    if let Some(recipe) = assets
                        .get(handle.id())
                        .and_then(|asset| asset.recipe.as_ref())
                    {
                        recipes.push(Arc::clone(recipe));
                    } else if let Some(error) =
                        pending_refs.and_then(|pending| pending.failed.get(&handle.id()))
                    {
                        break 'resolve DependentReferenceRecipes::Failed(error.clone());
                    } else {
                        break 'resolve DependentReferenceRecipes::Waiting;
                    }
                }
                DependentReferenceRecipes::Ready(recipes)
            }
        };
        let reference_recipes = match reference_recipes {
            DependentReferenceRecipes::Ready(recipes) => recipes,
            DependentReferenceRecipes::Waiting => {
                let mut refreshes = world.resource_mut::<PendingDependentStageRefreshes>();
                if let Some(pending) = refreshes.by_stage.get_mut(&stage)
                    && pending.operation == operation
                    && pending.desired_revision == revision
                {
                    pending.capacity_revision = Some(capacity_revision);
                }
                continue;
            }
            DependentReferenceRecipes::Failed(error) => {
                warn!(
                    "[usd-live] dependent stage {stage:?} refresh stopped because its added reference failed: {error}"
                );
                let retired = world
                    .resource_mut::<PendingDependentStageRefreshes>()
                    .by_stage
                    .remove(&stage);
                if let Some(retired) = retired {
                    release_dependent_stage_progress(world, retired.progress_key);
                }
                continue;
            }
        };

        let active = is_authoritative_scene_stage(world, stage);
        let priority = dependent_stage_work_priority(phase, active, priority);
        let progress_key = if active && phase != DependentStageRefreshPhase::Plan {
            let key = progress_key.unwrap_or(SimulationProgressKey {
                owner: SimulationProgressOwner::UsdDependentStageProjection,
                operation_id: operation,
            });
            if let Some(mut progress) = world.get_resource_mut::<SimulationProgress>() {
                progress.acquire(
                    key,
                    format!("Preparing updated dependent USD stage {stage:?}"),
                );
            }
            Some(key)
        } else if phase == DependentStageRefreshPhase::Plan {
            None
        } else {
            progress_key
        };
        let key = AsyncWorkKey::new(
            AsyncWorkKind::UsdPreparation,
            world
                .get_resource::<lunco_core::SceneTransitionCoordinator>()
                .and_then(lunco_core::SceneTransitionCoordinator::lifecycle_generation)
                .unwrap_or_default(),
            identity,
            revision,
            operation,
        );
        let (completions, wake) = {
            let pending = world.resource::<PendingDependentStageRefreshes>();
            (
                Arc::clone(&pending.completions),
                world.resource::<TwinProjectionWake>().clone(),
            )
        };
        let worker_origin = world
            .get_resource::<AssetServer>()
            .and_then(|server| server.get_path(stage).map(|path| path.into_owned()));
        let worker_roots = world.get_resource::<TwinRoots>().cloned();
        let worker_base_recipe = Arc::clone(&base_recipe);
        let worker_layer_patches = layer_patches.clone();
        let worker_reference_recipes = reference_recipes.clone();
        let worker_completions = Arc::clone(&completions);
        let worker_wake = wake.clone();
        let job = move || {
            let kind = match phase {
                DependentStageRefreshPhase::Patch => {
                    let _span = bevy::log::info_span!(
                        "usd_twin_projection_dependent_layer_patch_prepare",
                        revision
                    )
                    .entered();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut layers = Vec::with_capacity(changed_layers.len());
                        for (layer_id, source) in changed_layers {
                            let patch = worker_layer_patches
                                .get(&layer_id)
                                .cloned()
                                .unwrap_or(DependentStageLayerPatch {
                                    rebuild_stage: true,
                                    ..DependentStageLayerPatch::default()
                                });
                            if patch.rebuild_stage {
                                return Err(format!(
                                    "incremental dependent refresh for `{layer_id}` was classified as a rebuild"
                                ));
                            }
                            layers.push(PreparedDependentLayer {
                                layer_id,
                                source: Some(source.persistent_data()?),
                                patch,
                            });
                        }
                        Ok(PreparedDependentStagePatch {
                            layers,
                            reference_recipes: worker_reference_recipes,
                        })
                    }))
                    .unwrap_or_else(|_| {
                        Err("dependent USD layer patch preparation panicked".to_owned())
                    });
                    DependentStageRefreshCompletionKind::Patch(result)
                }
                DependentStageRefreshPhase::Plan | DependentStageRefreshPhase::Rebuild => {
                    let _span = bevy::log::info_span!(
                        "usd_twin_projection_dependent_plan_prepare",
                        revision
                    )
                    .entered();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut recipe = (*worker_base_recipe).clone();
                        let mut changed = false;
                        for reference_recipe in &worker_reference_recipes {
                            for (layer_id, bytes) in &reference_recipe.bytes {
                                if recipe
                                    .bytes
                                    .get(layer_id)
                                    .is_some_and(|existing| existing == bytes)
                                {
                                    continue;
                                }
                                recipe.bytes.insert(layer_id.clone(), bytes.clone());
                                changed = true;
                            }
                        }
                        for (layer_id, source) in changed_layers {
                            let bytes = source.persistent_bytes()?;
                            let layer_changed = !recipe
                                .bytes
                                .get(&layer_id)
                                .is_some_and(|existing| existing.as_slice() == bytes.as_ref());
                            if layer_changed {
                                recipe.bytes.insert(layer_id, bytes.to_vec());
                                changed = true;
                            }
                        }
                        if !changed && phase != DependentStageRefreshPhase::Rebuild {
                            return Ok(None);
                        }
                        let projection_plan = {
                            let _span = bevy::log::info_span!(
                                "usd_twin_projection_dependent_plan_compose",
                                revision
                            )
                            .entered();
                            let mut plan = UsdStageProjectionPlan::from_recipe(&recipe)
                                .map_err(|error| error.to_string())?;
                            plan.prepare_native_asset_paths(
                                worker_origin.clone(),
                                worker_roots.as_ref(),
                            );
                            plan
                        };
                        Ok(Some(PreparedDependentStagePlan {
                            recipe,
                            projection_plan,
                        }))
                    }))
                    .unwrap_or_else(|_| {
                        Err("dependent USD stage plan preparation panicked".to_owned())
                    });
                    DependentStageRefreshCompletionKind::Plan(result)
                }
            };
            worker_completions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(DependentStageRefreshCompletion {
                    stage,
                    operation,
                    revision,
                    kind,
                });
            worker_wake.wake();
        };

        {
            let mut refreshes = world.resource_mut::<PendingDependentStageRefreshes>();
            let Some(pending) = refreshes.by_stage.get_mut(&stage) else {
                continue;
            };
            if pending.operation != operation || pending.desired_revision != revision {
                continue;
            }
            pending.work_key = Some(key);
            pending.submitted_revision = Some(revision);
            pending.capacity_revision = None;
            pending.priority = priority;
            pending.progress_key = progress_key;
        }

        match world
            .resource_mut::<AsyncWorkAdmission>()
            .submit_ordered(priority, key, order, job)
        {
            Ok(()) | Err(lunco_core_runtime::AsyncWorkRejection::DuplicateKey) => {}
            Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                let capacity_revision = world.resource::<AsyncWorkAdmission>().capacity_revision();
                if let Some(pending) = world
                    .resource_mut::<PendingDependentStageRefreshes>()
                    .by_stage
                    .get_mut(&stage)
                    && pending.operation == operation
                    && pending.submitted_revision == Some(revision)
                {
                    pending.work_key = None;
                    pending.submitted_revision = None;
                    pending.capacity_revision = Some(capacity_revision);
                }
            }
            Err(lunco_core_runtime::AsyncWorkRejection::NativeDispatcherUnavailable) => {
                let error =
                    "dependent USD stage preparation requires a worker transport".to_owned();
                let kind = if phase == DependentStageRefreshPhase::Patch {
                    DependentStageRefreshCompletionKind::Patch(Err(error))
                } else {
                    DependentStageRefreshCompletionKind::Plan(Err(error))
                };
                world
                    .resource::<PendingDependentStageRefreshes>()
                    .completions
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(DependentStageRefreshCompletion {
                        stage,
                        operation,
                        revision,
                        kind,
                    });
                world.resource::<TwinProjectionWake>().wake();
            }
        }
    }
}

/// Rhai-overridable decision for a live component refresh.
///
/// The default is `propagate`: a component document is authoritative for every
/// already-mounted stage whose recipe references that `twin://` layer.  A local
/// policy hook may return `#{action: "propagate"|"defer"|"reject"}`.  Hook
/// failures and malformed results are loud and conservative: no dependent
/// stage is rebuilt.  This is a UI/local projection policy, not simulation
/// state, so the hook is intentionally not required to be deterministic.
const COMPONENT_REFRESH_POLICY_HOOK: &str = "usd.component_refresh";

lunco_hooks::declare_hook! {
    id: COMPONENT_REFRESH_POLICY_HOOK,
    owner: "lunco-usd-bevy-runtime-core",
    description: "Choose whether a changed USD component layer propagates to dependent stages.",
    signature: [ctx: Map],
    output: Map,
    deterministic: false,
    required: false,
    installable: true,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComponentRefreshDecision {
    Propagate,
    Defer,
    Reject,
}

fn component_refresh_decision(
    changed_doc: DocumentId,
    layer_id: &str,
    dependent_stage: AssetId<UsdStageAsset>,
    runtime_context: Option<lunco_core::RuntimeExecutionContext>,
) -> ComponentRefreshDecision {
    let args = [HookValue::map([
        ("changed_document", HookValue::Int(changed_doc.0 as i64)),
        ("changed_layer", HookValue::str(layer_id)),
        (
            "dependent_stage",
            HookValue::str(format!("{dependent_stage:?}")),
        ),
        ("default_action", HookValue::str("propagate")),
        ("camera_policy", HookValue::str("preserve")),
    ])];

    let Some(runtime_context) = runtime_context else {
        return ComponentRefreshDecision::Propagate;
    };
    let Some(result) =
        lunco_hooks::invoke_with_context(COMPONENT_REFRESH_POLICY_HOOK, &args, runtime_context)
    else {
        return ComponentRefreshDecision::Propagate;
    };

    let value = match result {
        Ok(value) => value,
        Err(error) => {
            warn!(
                "[usd-live] hook {COMPONENT_REFRESH_POLICY_HOOK} failed; dependent stage will not refresh: {error}"
            );
            return ComponentRefreshDecision::Reject;
        }
    };
    match parse_component_refresh_decision(&value) {
        Ok(decision) => decision,
        Err(error) => {
            warn!(
                "[usd-live] hook {COMPONENT_REFRESH_POLICY_HOOK} returned invalid policy ({error}); dependent stage will not refresh: {value:?}"
            );
            ComponentRefreshDecision::Reject
        }
    }
}

fn component_refresh_runtime_context(
    active_twin_document: bool,
    twin_generation: Option<u64>,
) -> Result<lunco_core::RuntimeExecutionContext, &'static str> {
    let route = if active_twin_document {
        let generation = twin_generation
            .ok_or("the mounted Twin document has no active or committed lifecycle generation")?;
        lunco_core::RuntimeRoute::twin(lunco_core::RuntimeCycle::Lifecycle, generation)
    } else {
        lunco_core::RuntimeRoute::application(lunco_core::RuntimeCycle::Lifecycle)
    };
    Ok(lunco_core::RuntimeExecutionContext {
        route: Some(route),
        phase: lunco_core::RuntimePhase::Preparation,
        clock: lunco_core::RuntimeClock::None,
        time_seconds: None,
        delta_seconds: None,
        sequence: None,
        producer: None,
    })
}

fn parse_component_refresh_decision(
    value: &HookValue,
) -> Result<ComponentRefreshDecision, &'static str> {
    let Some(action) = value.get("action").and_then(HookValue::as_str) else {
        return Err("expected map with action=propagate|defer|reject");
    };
    match action {
        "propagate" => Ok(ComponentRefreshDecision::Propagate),
        "defer" => Ok(ComponentRefreshDecision::Defer),
        "reject" => Ok(ComponentRefreshDecision::Reject),
        _ => Err("unknown action; expected propagate|defer|reject"),
    }
}

pub(crate) fn twin_projection_ready(
    wake: Res<TwinProjectionWake>,
    pending: Res<PendingDependentStageRefreshes>,
    admission: Res<AsyncWorkAdmission>,
) -> bool {
    wake.is_pending() || pending.has_admission_retry(admission.capacity_revision())
}

/// A USD document change is the authoritative input for live projection.
/// Filtering through the USD registry keeps unrelated document kinds from
/// waking this system.
pub(crate) fn wake_twin_projection_on_document_changed(
    trigger: On<DocumentChanged>,
    registry: Res<DocumentRegistry<UsdDocument>>,
    wake: Res<TwinProjectionWake>,
) {
    if registry.contains(trigger.event().doc) {
        wake.wake();
    }
}

/// Admit the current authoritative document generation before fixed time can
/// advance. `DocumentRegistry` change detection avoids scanning on steady
/// frames; the FixedLast installation catches edits issued inside a fixed
/// Rhai/event pass, while the PreUpdate installation catches edits from UI and
/// command cycles before the next fixed loop.
pub(crate) fn admit_pending_primary_document_projection(world: &mut World) {
    let Some(root) = world
        .get_resource::<lunco_core::SceneMountState>()
        .and_then(lunco_core::SceneMountState::active_root)
    else {
        return;
    };
    let Some(stage_id) = world
        .get::<UsdPrimPath>(root)
        .map(|path| path.stage_handle.id())
    else {
        return;
    };
    let Some(doc) = world
        .get_resource::<DocBackedTwinScenes>()
        .zip(world.get_resource::<AssetServer>())
        .and_then(|(backed, asset_server)| scene_document_for(backed, asset_server, stage_id))
    else {
        return;
    };
    let Some(projected_generation) = world
        .resource::<DocBackedTwinScenes>()
        .synced_generation(doc)
    else {
        // Initial scene loading owns its own lifecycle hold until its first
        // projection commits.
        return;
    };
    let Some((generation, has_simulation_ops)) = world
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)
        .map(|host| {
            let document = host.document();
            let generation = document.generation();
            let has_simulation_ops = generation > projected_generation
                && document
                    .ops_since(projected_generation)
                    .is_none_or(|ops| ops.iter().any(|op| !op.edit_target().is_view()));
            (generation, has_simulation_ops)
        })
    else {
        return;
    };
    if !has_simulation_ops {
        return;
    }

    world.resource_scope(
        |world, mut admissions: Mut<PendingDocumentProjectionAdmissions>| {
            let mut progress = world.resource_mut::<SimulationProgress>();
            admissions.admit(doc, generation, &mut progress);
        },
    );
    if let Some(wake) = world.get_resource::<TwinProjectionWake>() {
        wake.wake();
    }
}

/// Release the exact mounted-document projection hold only after the live
/// stage and its ECS projection cursor both reach the admitted target revision.
pub(crate) fn release_document_projection_progress(
    world: &mut World,
    doc: DocumentId,
    generation: u64,
) {
    world.resource_scope(
        |world, mut admissions: Mut<PendingDocumentProjectionAdmissions>| {
            let mut progress = world.resource_mut::<SimulationProgress>();
            admissions.complete(doc, generation, &mut progress);
        },
    );
}

/// Stage asset lifecycle events wake projection when a document edit was
/// waiting for its prepared recipe or when a mounted stage became available.
/// Each reader is independent, so this does not interfere with the pending
/// referenced-spawn transaction that consumes the same Bevy message stream.
pub(crate) fn wake_twin_projection_on_stage_event(
    mut events: MessageReader<bevy::asset::AssetEvent<UsdStageAsset>>,
    wake: Res<TwinProjectionWake>,
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
/// relationship/connection edits, API-schema edits, and kind edits — return
/// `false` and replay incrementally via [`apply_incremental_op_to_stage`].
///
/// `SetRelationship` and `SetConnection` use live-stage authors
/// (`CanonicalStage::author_relationship` / `author_connection`). Their consumers
/// (the Avian joint builder and the cosim wire reconcile) re-read on a subtree
/// refresh, so the incremental path fully reconciles them.
///
/// `SetApiSchemas`, `SetPrimKind`, and `SetPrimOrder` use live authoring too. A
/// schema edit refreshes only the affected prim subtree so physical ECS
/// components are rebuilt at the smallest safe scope; kind and child-order
/// metadata need no ECS refresh. None of these operations tears down unrelated
/// simulation state.
///
/// Active state is structural, but the generic structural reconciler already
/// owns exactly that operation: it despawns an inactive subtree and spawns it
/// again when reactivated. Keeping `SetActive` incremental prevents a route
/// annotation edit from rebuilding unrelated live vessels and their models.
fn op_needs_rebuild(op: &UsdOp) -> bool {
    matches!(
        op,
        UsdOp::ReplaceSource { .. }
            | UsdOp::MovePrim { .. }
            | UsdOp::RemoveTimeSample { .. }
            // Composition-arc changes: value resolution re-composes the prim's
            // subtree wholesale, which the incremental sink can't express.
            | UsdOp::SetVariantSelection { .. }
            | UsdOp::SetPayload { .. }
            | UsdOp::SetReferenceArcs { .. }
            | UsdOp::SetDefaultPrim { .. }
            // Stage metrics affect every canonical transform and require the
            // composed stage and all projected spatial state to be rebuilt.
            | UsdOp::SetStageMetrics { .. }
    )
}

/// Classify a schema list whose effects are metadata-only.  An empty list is
/// deliberately treated as physical/unknown: clearing a previously applied
/// physics schema must refresh the prim subtree so its ECS components disappear.
fn incremental_api_schemas(schemas: &[String]) -> bool {
    !schemas.is_empty()
        && schemas.iter().all(|schema| {
            matches!(
                schema.as_str(),
                "LunCoProgramAPI" | "LunCoMountAttachmentAPI" | "LunCoUiSchemaAPI"
            )
        })
}

/// Replay one **incremental** op's typed delta onto the scene's live
/// `CanonicalStage` — author-once: the value comes straight from the op, never
/// re-read from `composed`. Firing the openusd sink lets
/// [`project_stage_changes`](crate::live_consume::project_stage_changes) reconcile
/// ECS. Only incremental ops reach here; coarse ops ([`op_needs_rebuild`]) rebuild
/// instead. Reads/authors the `!Send` stage under short borrows.
fn child_prim_path(parent_path: &str, name: &str) -> String {
    if parent_path == "/" || parent_path.is_empty() {
        format!("/{name}")
    } else {
        format!("/{}/{}", parent_path.trim_matches('/'), name)
    }
}

fn parent_prim_path(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".to_owned(),
        Some((parent, _)) => parent.to_owned(),
    }
}

fn dependent_stage_refresh_mode(ops: &[UsdOp]) -> Option<DependentStageRefreshMode> {
    let persistent = ops
        .iter()
        .filter(|op| !op.edit_target().is_view())
        .collect::<Vec<_>>();
    if persistent.is_empty() {
        return None;
    }

    let mut patch = DependentStageLayerPatch::default();
    for op in persistent {
        if op_needs_rebuild(op) {
            return Some(DependentStageRefreshMode::Rebuild);
        }
        match op {
            UsdOp::AddPrim {
                parent_path, name, ..
            } => {
                let path = child_prim_path(parent_path, name);
                patch.prim_subtrees.insert(path);
                patch
                    .fields
                    .insert((parent_path.clone(), "primChildren".to_owned()));
            }
            UsdOp::RemovePrim { path, .. } | UsdOp::RestorePrim { path, .. } => {
                patch.prim_subtrees.insert(path.clone());
                patch
                    .fields
                    .insert((parent_prim_path(path), "primChildren".to_owned()));
            }
            UsdOp::SetPrimOrder { path, .. } => {
                patch.fields.insert((path.clone(), "primOrder".to_owned()));
            }
            UsdOp::SetStageDocumentation { .. } => {
                return Some(DependentStageRefreshMode::Rebuild);
            }
            UsdOp::SetTranslate { path, .. }
            | UsdOp::RemoveXformOp { path, .. }
            | UsdOp::RemoveAttribute { path, .. }
            | UsdOp::RestoreAttribute { path, .. }
            | UsdOp::RestoreXformOp { path, .. }
            | UsdOp::SetRotate { path, .. }
            | UsdOp::SetScale { path, .. }
            | UsdOp::SetAttribute { path, .. }
            | UsdOp::SetAttributeDocumentation { path, .. }
            | UsdOp::RevolveProfileMesh { path, .. }
            | UsdOp::ExtrudeProfileMesh { path, .. }
            | UsdOp::TaperedBeamMesh { path, .. }
            | UsdOp::SetTimeSample { path, .. }
            | UsdOp::RemoveTimeSample { path, .. }
            | UsdOp::SetRelationship { path, .. }
            | UsdOp::SetConnection { path, .. }
            | UsdOp::SetPrimDocumentation { path, .. }
            | UsdOp::SetPrimKind { path, .. }
            | UsdOp::SetApiSchemas { path, .. }
            | UsdOp::SetActive { path, .. }
            | UsdOp::ClearActive { path, .. } => {
                patch.prim_subtrees.insert(path.clone());
            }
            UsdOp::ReplaceSource { .. }
            | UsdOp::MovePrim { .. }
            | UsdOp::SetDefaultPrim { .. }
            | UsdOp::SetStageMetrics { .. }
            | UsdOp::SetVariantSelection { .. }
            | UsdOp::SetPayload { .. }
            | UsdOp::SetReferenceArcs { .. } => {
                return Some(DependentStageRefreshMode::Rebuild);
            }
        }
    }
    Some(DependentStageRefreshMode::Incremental(patch))
}

fn referenced_add_prim_paths(ops: &[UsdOp]) -> Vec<String> {
    ops.iter()
        .filter_map(|op| match op {
            UsdOp::AddPrim {
                parent_path,
                name,
                reference: Some(_),
                ..
            } => Some(child_prim_path(parent_path, name)),
            _ => None,
        })
        .collect()
}

fn promote_referenced_instance_for_op(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    owned_path: &str,
    op: &UsdOp,
) -> Result<(), String> {
    use lunco_usd_bevy_stage::canonical::CanonicalStages;

    let mut search_path = match op {
        UsdOp::AddPrim { parent_path, .. } => parent_path.as_str(),
        _ => owned_path,
    };
    loop {
        let entity = crate::live_consume::find_live_entity(world, scene_id, search_path);
        if let Some(entity) = entity
            && let Some(projection) = world.get::<UsdInstanceProjection>(entity).cloned()
        {
            let Some(root_entity) = projection.root else {
                return Err(format!(
                    "referenced instance at `{search_path}` has no live root identity"
                ));
            };
            let Some(root_path) = world
                .get::<UsdPrimPath>(root_entity)
                .map(|prim| prim.path.clone())
            else {
                return Err(format!(
                    "referenced instance root entity {root_entity} has no USD prim path"
                ));
            };
            let within_instance = owned_path == root_path
                || owned_path
                    .strip_prefix(&root_path)
                    .is_some_and(|suffix| root_path.ends_with('/') || suffix.starts_with('/'));
            if !within_instance {
                return Err(format!(
                    "edit target `{owned_path}` does not belong to referenced instance `{root_path}`"
                ));
            }
            // Removing the lightweight root needs no reference composition. The
            // normal structural sink removes its complete ECS projection.
            if matches!(op, UsdOp::RemovePrim { path, .. } if path == &root_path) {
                return Ok(());
            }
            if projection.is_promoted() {
                return Ok(());
            }
            let root_sdf_path = openusd::sdf::Path::new(&root_path).map_err(|error| {
                format!("invalid referenced instance root `{root_path}`: {error}")
            })?;
            let _promotion_span = bevy::log::info_span!(
                "usd_reference_instance_promotion",
                instance_root = %root_path,
            )
            .entered();
            let promoted = world
                .get_non_send::<CanonicalStages>()
                .and_then(|stages| stages.get(scene_id))
                .ok_or_else(|| "the owning scene stage is unavailable".to_owned())?
                .projector()
                .author_referenced_prim(
                    &root_sdf_path,
                    projection.type_name.as_deref(),
                    &projection.asset_path,
                    projection.reference_prim_path.as_deref(),
                )
                .map_err(|error| {
                    format!("failed to compose referenced instance `{root_path}`: {error}")
                });
            promoted?;
            projection.mark_promoted();
            return Ok(());
        }
        let Some((parent, _)) = search_path.rsplit_once('/') else {
            return Ok(());
        };
        if parent.is_empty() {
            search_path = "/";
        } else {
            search_path = parent;
        }
        if search_path == "/" {
            return Ok(());
        }
    }
}

fn report_instance_promotion_failure(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    path: &str,
    detail: String,
) {
    const PRODUCER: &str = "usd-instance-promotion";
    let subject = format!("{scene_id:?}:{path}");
    if is_authoritative_scene_stage(world, scene_id) {
        world
            .get_resource_or_insert_with(lunco_core::RuntimeFaults::default)
            .raise(PRODUCER, None, subject.clone(), detail.clone());
    }
    let mut diagnostics =
        world.get_resource_or_insert_with(lunco_core::RuntimeDiagnostics::default);
    diagnostics
        .findings
        .retain(|finding| !(finding.producer == PRODUCER && finding.subject == subject));
    diagnostics.findings.push(lunco_core::RuntimeDiagnostic {
        code: PRODUCER.to_owned(),
        severity: lunco_core::DiagnosticSeverity::Error,
        producer: PRODUCER.to_owned(),
        subject: subject.clone(),
        message: detail.clone(),
    });
    diagnostics
        .findings
        .sort_by(|left, right| left.subject.cmp(&right.subject));
    error!("[usd-live] referenced instance promotion {subject} failed: {detail}");
}

fn apply_incremental_op_to_stage(world: &mut World, scene_id: AssetId<UsdStageAsset>, op: &UsdOp) {
    let _span = bevy::log::info_span!("usd_twin_projection_apply_incremental_op").entered();
    use lunco_usd_bevy_stage::canonical::CanonicalStages;

    // A referenced AddPrim may be waiting on its asset closure. Preserve every
    // later edit whose owner is inside that not-yet-live subtree; otherwise a
    // relationship or metadata op would be accepted by the document and then
    // silently disappear from the live stage. SetTranslate is the one existing
    // fast path that has a dedicated field because it is applied at materialize.
    // Root activation/removal is transaction state rather than a deferred
    // operation: a transient delete must not be replayed against a later
    // AddPrim that reuses the same path.
    let owned_path = match op {
        UsdOp::AddPrim {
            parent_path, name, ..
        } => Some(child_prim_path(parent_path, name)),
        UsdOp::RemovePrim { path, .. }
        | UsdOp::RestorePrim { path, .. }
        | UsdOp::SetTranslate { path, .. }
        | UsdOp::RemoveXformOp { path, .. }
        | UsdOp::RestoreXformOp { path, .. }
        | UsdOp::RemoveAttribute { path, .. }
        | UsdOp::RestoreAttribute { path, .. }
        | UsdOp::SetRotate { path, .. }
        | UsdOp::SetScale { path, .. }
        | UsdOp::SetAttribute { path, .. }
        | UsdOp::SetRelationship { path, .. }
        | UsdOp::SetConnection { path, .. }
        | UsdOp::SetApiSchemas { path, .. }
        | UsdOp::SetPrimOrder { path, .. }
        | UsdOp::SetPrimKind { path, .. }
        | UsdOp::SetActive { path, .. }
        | UsdOp::ClearActive { path, .. } => Some(path.clone()),
        _ => None,
    };
    if let Some(owned_path) = owned_path.as_deref() {
        let pending_index = world
            .resource::<PendingRefSpawns>()
            .index_for_path(scene_id, owned_path);
        if let Some(index) = pending_index {
            let exact_root =
                world.resource::<PendingRefSpawns>().items[index].prim_path == owned_path;
            if matches!(op, UsdOp::RestorePrim { .. }) && exact_root {
                // Undoing a delete while a referenced spawn is still pending
                // restores the exact authored root only after its dependency
                // closure is injected into the live stage. Keep the pending
                // activation state from before the delete; the snapshot owns
                // the exact authored root state.
                let mut pending = world.resource_mut::<PendingRefSpawns>();
                if let Some(item) = pending.items.get_mut(index) {
                    item.removed = false;
                    item.translate = None;
                    item.deferred_ops.clear();
                    item.deferred_ops.push(op.clone());
                }
                return;
            } else if matches!(op, UsdOp::AddPrim { .. }) && exact_root {
                // A new authored root is a new transaction. Drop the old
                // transaction, including any stale activation state, and let
                // the normal AddPrim path queue/materialize this one.
                let removed = {
                    let mut pending = world.resource_mut::<PendingRefSpawns>();
                    pending.items.get_mut(index).map(|item| {
                        let state = (item.progress_key, item.held);
                        item.removed = true;
                        item.active = false;
                        item.held = false;
                        state
                    })
                };
                if let Some((key, held)) = removed {
                    if held {
                        release_reference_progress(world, key);
                    }
                    world.resource_mut::<PendingRefSpawns>().items.remove(index);
                }
            } else {
                if world.resource::<PendingRefSpawns>().items[index].removed {
                    return;
                }
                match op {
                    UsdOp::RemovePrim { .. } if exact_root => {
                        cancel_pending_reference(world, index);
                    }
                    UsdOp::SetActive { active, .. } if exact_root => {
                        set_pending_reference_active(world, index, *active);
                    }
                    UsdOp::ClearActive { .. } if exact_root => {
                        set_pending_reference_active(world, index, true);
                    }
                    UsdOp::SetTranslate { value, .. } if exact_root => {
                        world.resource_mut::<PendingRefSpawns>().items[index].translate =
                            Some(*value);
                    }
                    _ => world.resource_mut::<PendingRefSpawns>().items[index]
                        .deferred_ops
                        .push(op.clone()),
                }
                return;
            }
        }
    }

    if let Some(owned_path) = owned_path.as_deref()
        && let Err(detail) = promote_referenced_instance_for_op(world, scene_id, owned_path, op)
    {
        report_instance_promotion_failure(world, scene_id, owned_path, detail);
        return;
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
        UsdOp::RemoveXformOp { path, name, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            if let Some(cs) = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                if let Err(e) = cs.projector().remove_xform_op(&sp, name) {
                    warn!("[twin] remove xform operation {path}.{name}: {e}");
                } else if let Some(channel) =
                    crate::live_consume::TransformEditChannels::for_attribute(name)
                {
                    crate::live_consume::mark_live_transform(
                        world,
                        scene_id,
                        path.clone(),
                        channel,
                    );
                }
            }
        }
        UsdOp::RestoreXformOp {
            path,
            name,
            value,
            order,
            ..
        } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            if let Some(cs) = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                if let Err(e) = cs
                    .projector()
                    .restore_xform_op(&sp, name, *value, order.as_deref())
                {
                    warn!("[twin] restore xform operation {path}.{name}: {e}");
                } else if let Some(channel) =
                    crate::live_consume::TransformEditChannels::for_attribute(name)
                {
                    crate::live_consume::mark_live_transform(
                        world,
                        scene_id,
                        path.clone(),
                        channel,
                    );
                }
            }
        }
        UsdOp::RemoveAttribute { path, name, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let removed = world
                .get_non_send::<CanonicalStages>()
                .and_then(|stages| stages.get(scene_id))
                .map(|stage| stage.projector().remove_attribute(&sp, name));
            match removed {
                Some(Ok(())) => refresh_prim_subtree(world, scene_id, path),
                Some(Err(error)) => warn!("[twin] remove attribute {path}.{name}: {error}"),
                None => {}
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
                match lunco_usd_authoring::author::parse_attribute_value(type_name, value) {
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
                let live_edit_owners = world
                    .get_resource::<lunco_usd_bevy_core::live_edit::UsdLiveEditRegistry>()
                    .map(|registry| registry.snapshot())
                    .unwrap_or_default();
                let claimed_owner = world
                    .get_non_send::<CanonicalStages>()
                    .and_then(|s| s.get(scene_id))
                    .map(|cs| {
                        live_edit_owners
                            .iter()
                            .filter(|owner| owner.claims_edit(&cs.view(), &sp, name))
                            .copied()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if !claimed_owner.is_empty() {
                    for owner in claimed_owner {
                        owner.refresh_stage(world, scene_id);
                    }
                    return;
                }
            }
            // A `string` attribute is non-visual metadata/behavior (`info:sourceCode`,
            // descriptions, a policy's `info:sourceCode`) — no geometry/material
            // consequence, and a refresh would hot-reload a running scenario
            // (resetting its `this`) on a mere save. So author, don't refresh.
            if is_string {
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
            reference_prim_path,
            ..
        } => {
            let prim_path = child_prim_path(parent_path, name);
            spawn_prim_op(
                world,
                scene_id,
                &prim_path,
                type_name.clone(),
                reference.clone(),
                reference_prim_path.clone(),
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
        UsdOp::RestorePrim {
            path,
            snapshot_usda,
            sibling_order,
            ..
        } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            if let Some(cs) = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                if let Err(e) =
                    cs.projector()
                        .restore_prim_subtree(&sp, snapshot_usda, sibling_order)
                {
                    warn!("[twin] restore prim subtree {path}: {e}");
                }
            }
        }
        UsdOp::RestoreAttribute {
            path,
            name,
            snapshot_usda,
            sibling_order,
            ..
        } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let restored = world
                .get_non_send::<CanonicalStages>()
                .and_then(|stages| stages.get(scene_id))
                .map(|stage| {
                    stage.projector().restore_attribute_subtree(
                        &sp,
                        name,
                        snapshot_usda,
                        sibling_order,
                    )
                });
            match restored {
                Some(Ok(())) => refresh_prim_subtree(world, scene_id, path),
                Some(Err(error)) => warn!("[twin] restore attribute {path}.{name}: {error}"),
                None => {}
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
            let v = match lunco_usd_authoring::author::parse_attribute_value(type_name, value) {
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
            // The per-frame sampler in `lunco-usd-bevy-animation` reads the live stage,
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
        UsdOp::SetApiSchemas { path, schemas, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let authored = match world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                Some(cs) => match cs.projector().author_api_schemas(&sp, schemas) {
                    Ok(()) => true,
                    Err(e) => {
                        warn!("[twin] author API schemas at {path}: {e}");
                        false
                    }
                },
                None => {
                    warn!("[twin] no canonical stage while authoring API schemas at {path}");
                    false
                }
            };
            // Metadata-only Twin schemas do not affect ECS. Physical/unknown
            // schemas do, so refresh only this prim's subtree after the live
            // author succeeds; unrelated scene roots and running scripts stay
            // intact.
            if authored && !incremental_api_schemas(schemas) {
                refresh_prim_subtree(world, scene_id, path);
            }
        }
        UsdOp::SetPrimOrder { path, order, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            match world
                .get_non_send::<CanonicalStages>()
                .and_then(|stages| stages.get(scene_id))
            {
                Some(cs) => {
                    if let Err(error) = cs.projector().author_prim_order(&sp, order.as_deref()) {
                        warn!("[twin] author child order at {path}: {error}");
                    }
                }
                None => warn!("[twin] no canonical stage while ordering children at {path}"),
            }
        }
        UsdOp::SetPrimKind { path, kind, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            match world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
            {
                Some(cs) => {
                    if let Err(e) = cs.projector().author_kind(&sp, kind.as_deref()) {
                        warn!("[twin] author kind at {path}: {e}");
                    }
                }
                None => warn!("[twin] no canonical stage while authoring kind at {path}"),
            }
        }
        UsdOp::SetActive { path, active, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let authored = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
                .is_some_and(|cs| cs.projector().author_active(&sp, *active).is_ok());
            if !authored {
                warn!("[twin] author active={active} at {path} failed");
            }
        }
        UsdOp::ClearActive { path, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let cleared = world
                .get_non_send::<CanonicalStages>()
                .and_then(|s| s.get(scene_id))
                .is_some_and(|cs| cs.projector().clear_active(&sp).is_ok());
            if cleared {
                // OpenUSD 0.5 exposes metadata erasure through Sdf data editing;
                // unlike Prim::set_active, that low-level edit is not classified
                // as a structural resync by the pinned sink. Reuse the shared
                // structural reconciler at the same stage boundary so undoing a
                // local active=false opinion immediately reveals the weaker
                // composed prim without a scene rebuild.
                crate::live_consume::reconcile_structural_live(
                    world,
                    scene_id,
                    std::slice::from_ref(path),
                );
            } else {
                warn!("[twin] clear active at {path} failed");
            }
        }
        // Coarse composition ops never reach here (the caller rebuilds for
        // them). Metadata and kind are handled above on the live stage.
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
/// [`UsdAnimated`](lunco_usd_bevy_scene::UsdAnimated) — so the per-frame animation
/// sampler already drives it and a fresh keyframe needs no re-instantiation. False
/// when the prim is static (or has no live entity yet), which is when a first
/// keyframe must trigger a subtree refresh to (re-)tag it.
fn prim_entity_is_animated(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    path: &str,
) -> bool {
    let mut q = world.query::<(&UsdPrimPath, Option<&lunco_usd_bevy_scene::UsdAnimated>)>();
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
    reference_prim_path: Option<String>,
) {
    use lunco_usd_bevy_stage::canonical::CanonicalStages;
    let reference_prim_path = reference_prim_path.filter(|path| !path.is_empty());
    let Some(asset_path) = reference else {
        let Ok(sp) = openusd::sdf::Path::new(prim_path) else {
            return;
        };
        for key in world
            .resource_mut::<PendingRefSpawns>()
            .replace_path(scene_id, prim_path)
        {
            release_reference_progress(world, key);
        }
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

    let Some(progress_key) = world
        .resource_mut::<PendingRefSpawns>()
        .allocate_progress_key()
    else {
        let detail = "USD reference operation identity space is exhausted";
        let exhaustion_key = SimulationProgressKey {
            owner: SimulationProgressOwner::SceneReferences,
            operation_id: u64::MAX,
        };
        report_reference_failure(
            world,
            exhaustion_key,
            scene_id,
            prim_path,
            &asset_path,
            detail,
        );
        return;
    };
    for key in world
        .resource_mut::<PendingRefSpawns>()
        .replace_path(scene_id, prim_path)
    {
        release_reference_progress(world, key);
    }

    if openusd::sdf::Path::new(prim_path).is_err() {
        let detail = format!("invalid USD prim path `{prim_path}`");
        let mut item = failed_ref_spawn(
            progress_key,
            scene_id,
            prim_path,
            type_name,
            &asset_path,
            reference_prim_path,
            detail.clone(),
        );
        item.held = report_reference_failure(
            world,
            progress_key,
            scene_id,
            prim_path,
            &asset_path,
            &detail,
        );
        item.failure_reported = true;
        world.resource_mut::<PendingRefSpawns>().push(item, false);
        return;
    };

    let reference_id = {
        let Some(cs) = world
            .get_non_send::<CanonicalStages>()
            .and_then(|s| s.get(scene_id))
        else {
            let detail = "the owning scene stage is unavailable";
            let mut item = failed_ref_spawn(
                progress_key,
                scene_id,
                prim_path,
                type_name,
                &asset_path,
                reference_prim_path,
                detail.to_owned(),
            );
            item.held = report_reference_failure(
                world,
                progress_key,
                scene_id,
                prim_path,
                &asset_path,
                detail,
            );
            item.failure_reported = true;
            world.resource_mut::<PendingRefSpawns>().push(item, false);
            return;
        };
        cs.canonical_reference_id(&asset_path)
            .map_err(|error| error.to_string())
    };
    let ref_handle = reference_id.and_then(|ref_id| {
        if crate::native_references::is_native(&ref_id) {
            crate::native_references::reference(world, scene_id, &ref_id)
                .map(|prepared| prepared.map(|prepared| prepared.handle))
        } else {
            let path = {
                let prepared = world
                    .get_non_send::<CanonicalStages>()
                    .and_then(|stages| stages.get(scene_id))
                    .and_then(|stage| stage.native_asset_paths());
                lunco_usd_bevy_stage::asset::resolve_stage_asset_path(
                    world.resource::<AssetServer>(),
                    scene_id,
                    &ref_id,
                    world.get_resource::<TwinRoots>(),
                    prepared,
                )
                .map_err(|error| error.to_string())
            }?;
            Ok(Some(
                world.resource::<AssetServer>().load::<UsdStageAsset>(path),
            ))
        }
    });
    let ref_handle = match ref_handle {
        Ok(handle) => handle,
        Err(error) => {
            let detail = format!("invalid USD reference identifier: {error}");
            let mut item = failed_ref_spawn(
                progress_key,
                scene_id,
                prim_path,
                type_name,
                &asset_path,
                reference_prim_path,
                detail.clone(),
            );
            item.held = report_reference_failure(
                world,
                progress_key,
                scene_id,
                prim_path,
                &asset_path,
                &detail,
            );
            item.failure_reported = true;
            world.resource_mut::<PendingRefSpawns>().push(item, false);
            return;
        }
    };
    let reason = format!("Preparing USD reference {prim_path} from `{asset_path}`");
    let held = acquire_reference_progress(world, progress_key, scene_id, reason);
    enqueue_reference_spawn(
        world,
        RefSpawn {
            progress_key,
            scene_id,
            prim_path: prim_path.to_string(),
            type_name,
            asset_path,
            reference_prim_path,
            ref_handle,
            translate: None,
            deferred_ops: Vec::new(),
            active: true,
            held,
            asset_ready: false,
            failure: None,
            failure_reported: false,
            removed: false,
        },
    );
}

fn failed_ref_spawn(
    progress_key: SimulationProgressKey,
    scene_id: AssetId<UsdStageAsset>,
    prim_path: &str,
    type_name: Option<String>,
    asset_path: &str,
    reference_prim_path: Option<String>,
    detail: String,
) -> RefSpawn {
    RefSpawn {
        progress_key,
        scene_id,
        prim_path: prim_path.to_owned(),
        type_name,
        asset_path: asset_path.to_owned(),
        reference_prim_path,
        ref_handle: None,
        translate: None,
        deferred_ops: Vec::new(),
        active: true,
        held: false,
        asset_ready: false,
        failure: Some(detail),
        failure_reported: true,
        removed: false,
    }
}

/// Re-read the whole live scene from the (now-authored) stage. Only an explicit
/// [`UsdSceneRoot`](lunco_usd_bevy_scene::UsdSceneRoot) may seed this rebuild.
/// Before rebuilding, retire every other projection entity for that stage.
///
/// This stage-scoped retirement is essential: a mounted USD camera is
/// intentionally reparented directly to the persistent grid, so it no longer
/// belongs to its USD root's Bevy subtree. Reinstantiating that root alone would
/// create a replacement camera while the detached camera kept rendering.
/// Parentage is therefore never used as scene ownership; the stage handle is.
///
/// Dropping the root's `UsdSceneProjected` marker and children then re-inserting
/// `UsdPrimPath` re-fires `on_usd_prim_added`, rebuilding exactly one subtree so
/// an attribute edit that fans out through a material binding reaches every bound
/// mesh. Structural changes therefore use one explicit, stage-scoped synchronous
/// rebuild.
pub(crate) fn refresh_scene_visuals(world: &mut World, scene_id: AssetId<UsdStageAsset>) -> bool {
    if !prepare_stage_projection_reset(world, scene_id) {
        return false;
    }
    refresh_scene_visuals_prepared(world, scene_id)
}

fn refresh_scene_visuals_prepared(world: &mut World, scene_id: AssetId<UsdStageAsset>) -> bool {
    let roots: Vec<Entity> = {
        // A live simulation is rooted by `UsdSceneRoot`; each editor preview
        // lease is rooted by `UsdPreviewOnly`. Both are stage ownership roots
        // and must survive a visual refresh. Restricting this to `UsdSceneRoot`
        // silently despawned preview subtrees, leaving no entity to
        // re-instantiate after a material edit.
        let mut q = world.query_filtered::<(Entity, &UsdPrimPath), Or<(
            With<UsdSceneRoot>,
            With<lunco_usd_bevy_scene::UsdPreviewOnly>,
        )>>();
        q.iter(world)
            .filter(|(_, upp)| upp.stage_handle.id() == scene_id)
            .map(|(entity, _)| entity)
            .collect()
    };
    if roots.is_empty() {
        return true;
    }

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
    true
}

/// Let every registered live-edit owner retire its stage-derived state before
/// the generic projector replaces entities. Owners run in stable id order; a
/// failure leaves the current ECS projection intact and faults the active stage
/// so simulation cannot continue against a partially reset topology.
fn prepare_stage_projection_reset(world: &mut World, stage_id: AssetId<UsdStageAsset>) -> bool {
    let mut owners = world
        .get_resource::<lunco_usd_bevy_core::live_edit::UsdLiveEditRegistry>()
        .map(|registry| registry.snapshot())
        .unwrap_or_default();
    owners.sort_by_key(|owner| owner.id());
    for owner in owners {
        if let Err(error) = owner.prepare_stage_projection_reset(world, stage_id) {
            report_stage_projection_reset_failure(world, stage_id, error);
            return false;
        }
    }
    true
}

fn subtree_requires_stage_projection_reset(
    world: &World,
    stage_id: AssetId<UsdStageAsset>,
    prim_path: &str,
) -> bool {
    world
        .get_resource::<lunco_usd_bevy_core::live_edit::UsdLiveEditRegistry>()
        .is_some_and(|registry| {
            registry
                .snapshot()
                .iter()
                .any(|owner| owner.requires_stage_projection_reset(world, stage_id, prim_path))
        })
}

fn report_stage_projection_reset_failure(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
    detail: String,
) {
    const PRODUCER: &str = "usd-stage-projection-reset";
    let subject = format!("{stage_id:?}");
    let active_stage = {
        let mut roots = world.query_filtered::<&UsdPrimPath, With<UsdSceneRoot>>();
        roots
            .iter(world)
            .any(|path| path.stage_handle.id() == stage_id)
    };
    if active_stage {
        world
            .get_resource_or_insert_with(lunco_core::RuntimeFaults::default)
            .raise(PRODUCER, None, subject.clone(), detail.clone());
    }
    let mut diagnostics =
        world.get_resource_or_insert_with(lunco_core::RuntimeDiagnostics::default);
    diagnostics
        .findings
        .retain(|finding| !(finding.producer == PRODUCER && finding.subject == subject));
    diagnostics.findings.push(lunco_core::RuntimeDiagnostic {
        code: PRODUCER.to_owned(),
        severity: lunco_core::DiagnosticSeverity::Error,
        producer: PRODUCER.to_owned(),
        subject: subject.clone(),
        message: detail.clone(),
    });
    diagnostics
        .findings
        .sort_by(|left, right| left.subject.cmp(&right.subject));
    error!("[usd-live] stage projection reset {subject} failed: {detail}");
}

fn report_dependent_stage_plan_failure(
    world: &mut World,
    stage_id: AssetId<UsdStageAsset>,
    detail: &str,
) {
    const PRODUCER: &str = "usd-dependent-stage-plan";
    let subject = format!("{stage_id:?}");
    let mut diagnostics =
        world.get_resource_or_insert_with(lunco_core::RuntimeDiagnostics::default);
    diagnostics
        .findings
        .retain(|finding| !(finding.producer == PRODUCER && finding.subject == subject));
    diagnostics.findings.push(lunco_core::RuntimeDiagnostic {
        code: PRODUCER.to_owned(),
        severity: lunco_core::DiagnosticSeverity::Error,
        producer: PRODUCER.to_owned(),
        subject: subject.clone(),
        message: detail.to_owned(),
    });
    diagnostics
        .findings
        .sort_by(|left, right| left.subject.cmp(&right.subject));
    error!("[usd-live] dependent stage plan {subject} failed: {detail}");
}

/// Notify domain projections, then drop `entity`'s [`UsdSceneProjected`] marker
/// and children and re-insert its [`UsdPrimPath`], re-firing
/// `on_usd_prim_added` so its subtree rebuilds from the (now-authored) live
/// stage. The shared primitive under both the whole-scene
/// [`refresh_scene_visuals`] and the single-prim [`refresh_prim_subtree`].
fn reinstantiate_entity(world: &mut World, entity: Entity) {
    let stage_ready = world
        .get_resource::<Assets<UsdStageAsset>>()
        .is_some_and(|assets| {
            world
                .get::<UsdPrimPath>(entity)
                .is_some_and(|prim| assets.get(&prim.stage_handle).is_some())
        });
    world.write_message(UsdSceneProjectionReset { entity });
    if let Ok(mut em) = world.get_entity_mut(entity) {
        em.remove::<UsdSceneProjected>();
        em.despawn_related::<Children>();
        if let Some(pp) = em.take::<UsdPrimPath>() {
            em.insert((pp, UsdSceneAwaitingStage));
            if stage_ready {
                em.insert(UsdSceneProjectionQueued);
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
    if subtree_requires_stage_projection_reset(world, scene_id, path) {
        info!(
            "[usd-live] prim refresh for {scene_id:?}:{path} promoted to a stage reset by a projection owner"
        );
        refresh_scene_visuals(world, scene_id);
        return;
    }

    let entity = crate::live_consume::find_stage_entity(world, scene_id, path);
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
) -> bool {
    use lunco_usd_bevy_stage::canonical::{CanonicalStage, CanonicalStages};
    use lunco_usd_compose::recipe::StageRecipe;
    // Recipe = the edited composed source as the root layer + every referenced
    // `.usda` the current stage already loaded (keyed by the same canonical ids).
    let (scene_layer, mut bytes) = {
        let _span = bevy::log::info_span!("usd_twin_rebuild_snapshot_live_layers").entered();
        let Some(cs) = world
            .get_non_send::<CanonicalStages>()
            .and_then(|s| s.get(scene_id))
        else {
            return false;
        };
        (cs.scene_layer.clone(), cs.layer_bytes_snapshot())
    };
    bytes.insert(scene_layer.clone(), composed_source.as_bytes().to_vec());
    let recipe = StageRecipe::new(scene_layer, bytes);
    let replacement = match {
        let _span = bevy::log::info_span!("usd_twin_rebuild_prepare_live_stage").entered();
        CanonicalStage::from_recipe(&recipe)
    } {
        Ok(stage) => stage,
        Err(error) => {
            report_stage_projection_reset_failure(
                world,
                scene_id,
                format!("edited composed source could not rebuild the live stage: {error}"),
            );
            return false;
        }
    };
    let reset_ready = {
        let _span = bevy::log::info_span!("usd_twin_rebuild_prepare_world_reset").entered();
        prepare_stage_projection_reset(world, scene_id)
    };
    if !reset_ready {
        return false;
    }
    let rebuilt = {
        let _span = bevy::log::info_span!("usd_twin_rebuild_commit_live_stage").entered();
        if let Some(mut stages) = world.get_non_send_mut::<CanonicalStages>() {
            stages.replace_rebuilt(scene_id, replacement);
            true
        } else {
            false
        }
    };
    if rebuilt {
        // Fresh stage (new, empty sink) — re-instantiate every scene root off it.
        let _span = bevy::log::info_span!("usd_twin_rebuild_refresh_scene_visuals").entered();
        refresh_scene_visuals_prepared(world, scene_id)
    } else {
        report_stage_projection_reset_failure(
            world,
            scene_id,
            "prepared composed source could not replace the canonical stage".to_owned(),
        );
        false
    }
}

/// Make every newly referenced asset in a coarse edit available to the live
/// stage before rebuilding it. The async `UsdStageAsset` loader already owns
/// the available transitive layer closure; this only transfers those bytes into
/// the existing canonical resolver. If a closure is still loading, leave the
/// document generation unsynced so the asset event retries the same rebuild
/// with the available resolver instead of publishing a stage before its root
/// asset is ready.
fn ensure_reference_layers_for_rebuild(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    ops: &[UsdOp],
) -> bool {
    let references: Vec<String> = ops
        .iter()
        .filter_map(|op| match op {
            UsdOp::AddPrim {
                reference: Some(reference),
                ..
            } => Some(reference.clone()),
            _ => None,
        })
        .collect();
    if references.is_empty() {
        return true;
    }

    let mut extra = HashMap::new();
    let mut admitted_native = Vec::new();
    for asset_path in references {
        let reference_id = {
            let Some(cs) = world
                .get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
                .and_then(|stages| stages.get(scene_id))
            else {
                return false;
            };
            cs.canonical_reference_id(&asset_path).map(|id| {
                let present = cs.has_layer_bytes(&id);
                (id, present)
            })
        };
        let (reference_id, present) = match reference_id {
            Ok(reference) => reference,
            Err(error) => {
                report_stage_projection_reset_failure(
                    world,
                    scene_id,
                    format!("invalid USD rebuild reference {asset_path:?}: {error}"),
                );
                return false;
            }
        };
        if present {
            continue;
        }
        if crate::native_references::is_native(&reference_id) {
            match crate::native_references::reference(world, scene_id, &reference_id) {
                Ok(Some(prepared)) => {
                    extra.extend(prepared.recipe.bytes.clone());
                    world
                        .resource_mut::<PendingRefSpawns>()
                        .retained_assets
                        .insert((scene_id, reference_id.clone()), prepared.handle);
                    admitted_native.push(reference_id);
                }
                Ok(None) => return false,
                Err(error) => {
                    report_stage_projection_reset_failure(
                        world,
                        scene_id,
                        format!("native USD rebuild reference {asset_path:?} failed: {error}"),
                    );
                    return false;
                }
            }
            continue;
        }
        let handle = if let Some(handle) = world
            .resource::<PendingRefSpawns>()
            .retained_assets
            .get(&(scene_id, reference_id.clone()))
            .cloned()
        {
            handle
        } else {
            let path = {
                let prepared = world
                    .get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
                    .and_then(|stages| stages.get(scene_id))
                    .and_then(|stage| stage.native_asset_paths());
                lunco_usd_bevy_stage::asset::resolve_stage_asset_path(
                    world.resource::<AssetServer>(),
                    scene_id,
                    &reference_id,
                    world.get_resource::<TwinRoots>(),
                    prepared,
                )
            };
            let path = match path {
                Ok(path) => path,
                Err(error) => {
                    report_stage_projection_reset_failure(
                        world,
                        scene_id,
                        format!("invalid USD rebuild reference {asset_path:?}: {error}"),
                    );
                    return false;
                }
            };
            let handle = world.resource::<AssetServer>().load::<UsdStageAsset>(path);
            world
                .resource_mut::<PendingRefSpawns>()
                .retained_assets
                .insert((scene_id, reference_id.clone()), handle.clone());
            handle
        };
        let Some(asset) = world.resource::<Assets<UsdStageAsset>>().get(handle.id()) else {
            return false;
        };
        let Some(recipe) = asset.recipe.as_ref() else {
            bevy::log::warn!("[twin] referenced rebuild asset `{asset_path}` has no layer recipe");
            return false;
        };
        extra.extend(recipe.bytes.clone());
    }
    if extra.is_empty() {
        return true;
    }
    let Some(mut stages) =
        world.get_non_send_mut::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
    else {
        return false;
    };
    let Some(cs) = stages.get_mut(scene_id) else {
        return false;
    };
    let admitted = cs.add_layer_bytes(extra);
    drop(stages);
    if admitted {
        for reference in admitted_native {
            world
                .resource_mut::<PendingRefSpawns>()
                .native
                .retire_input(scene_id, &reference);
        }
    }
    admitted
}

/// Complete referenced spawns whose asset closure has finished loading. The
/// authored document already owns the reference arc; its prepared asset plan
/// projects the live subtree. The authoritative scene stage gets only the
/// lightweight root needed for path reconciliation, and composes the reference
/// when a later edit requires live-stage ownership.
pub(crate) fn drain_ref_spawns(world: &mut World) {
    use lunco_usd_bevy_stage::canonical::CanonicalStages;
    crate::native_references::advance(world);
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
    let mut pending = std::mem::take(&mut world.resource_mut::<PendingRefSpawns>().items);
    pending.sort_by_key(|item| item.progress_key.operation_id);
    let _batch_span = bevy::log::info_span!(
        "usd_reference_spawn_commit_batch",
        item_count = pending.len(),
    )
    .entered();
    let mut still = Vec::new();
    let mut commit_order = PrimaryReferenceCommitOrder::default();
    for mut item in pending {
        if item.removed {
            deactivate_reference_progress(world, &mut item);
            continue;
        }
        let mut prepared_native = None;
        let mut native_reference = false;
        if item.failure.is_none() {
            let reference_id = world
                .get_non_send::<CanonicalStages>()
                .and_then(|stages| stages.get(item.scene_id))
                .map(|stage| {
                    stage
                        .canonical_reference_id(&item.asset_path)
                        .map_err(|error| error.to_string())
                });
            match reference_id {
                Some(Ok(reference_id)) if crate::native_references::is_native(&reference_id) => {
                    native_reference = true;
                    match crate::native_references::reference(world, item.scene_id, &reference_id) {
                        Ok(Some(prepared)) => {
                            item.ref_handle = Some(prepared.handle.clone());
                            item.asset_ready = true;
                            prepared_native = Some(prepared);
                        }
                        Ok(None) => item.asset_ready = false,
                        Err(error) => item.failure = Some(error),
                    }
                }
                Some(Err(error)) => item.failure = Some(error),
                None if crate::native_references::is_native(&item.asset_path) => {
                    item.failure = Some("the owning native reference stage is unavailable".into());
                }
                _ => {}
            }
        }
        let asset_id = item.ref_handle.as_ref().map(Handle::id);
        let ready_event = asset_id.is_some_and(|id| ready.contains(&id))
            && (!native_reference || prepared_native.is_some());
        let failed_event = asset_id.and_then(|id| failed.get(&id));
        if !item.active {
            if ready_event {
                item.asset_ready = true;
            }
            if let Some(error) = failed_event {
                item.failure = Some(error.clone());
            }
            deactivate_reference_progress(world, &mut item);
            still.push(item);
            continue;
        }
        activate_reference_progress(world, &mut item);
        let authoritative = is_authoritative_scene_stage(world, item.scene_id);
        if commit_order.must_defer(
            item.scene_id,
            authoritative,
            item.failure.is_some() || failed_event.is_some() || item.asset_ready || ready_event,
        ) {
            item.asset_ready |= ready_event;
            if let Some(error) = failed_event {
                item.failure = Some(error.clone());
            }
            still.push(item);
            continue;
        }
        if let Some(error) = item.failure.clone() {
            if !item.failure_reported {
                fail_reference_spawn(world, &mut item, error);
            }
            commit_order.block_successors(item.scene_id, authoritative);
            still.push(item);
            continue;
        }
        if let Some(error) = failed_event {
            fail_reference_spawn(world, &mut item, error.clone());
            item.failure = Some(error.clone());
            commit_order.block_successors(item.scene_id, authoritative);
            still.push(item);
            continue;
        }
        if !item.asset_ready && !ready_event {
            still.push(item);
            continue;
        }
        if native_reference && prepared_native.is_none() {
            still.push(item);
            continue;
        }
        let Some(ref_handle) = item.ref_handle.as_ref().cloned() else {
            fail_reference_spawn(
                world,
                &mut item,
                "a ready reference has no admitted source asset".into(),
            );
            commit_order.block_successors(item.scene_id, authoritative);
            still.push(item);
            continue;
        };
        item.asset_ready = true;
        let dependent_plan_state = world
            .resource::<PendingDependentStageRefreshes>()
            .by_stage
            .get(&ref_handle.id())
            .map(|pending| pending.plan_failure.clone());
        match dependent_plan_state {
            Some(Some(error)) => {
                fail_reference_spawn(
                    world,
                    &mut item,
                    format!(
                        "the referenced asset's current projection plan is unavailable: {error}"
                    ),
                );
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
            Some(None) => {
                still.push(item);
                continue;
            }
            None => {}
        }
        let recipe = prepared_native
            .as_ref()
            .map(|prepared| Arc::clone(&prepared.recipe))
            .or_else(|| {
                world
                    .resource::<Assets<UsdStageAsset>>()
                    .get(ref_handle.id())
                    .and_then(|a| a.recipe.clone())
            });
        let Some(recipe) = recipe else {
            fail_reference_spawn(
                world,
                &mut item,
                "the ready referenced asset has no usable layer recipe".to_owned(),
            );
            commit_order.block_successors(item.scene_id, authoritative);
            still.push(item);
            continue;
        };
        if !recipe.dependency_diagnostics.is_empty() {
            let decision = evaluate_reference_composition_policy(
                world,
                item.scene_id,
                &recipe.dependency_diagnostics,
            );
            let rejection = match decision {
                Ok(IncompleteCompositionDecision::AllowPartial) => None,
                Ok(IncompleteCompositionDecision::RejectScene) => {
                    let first_missing = recipe
                        .dependency_diagnostics
                        .first()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "no dependency details were supplied".to_owned());
                    Some(format!(
                        "the `{}` policy rejected referenced asset `{}` with {} unresolved USD composition dependencies; first: {first_missing}",
                        INCOMPLETE_COMPOSITION_POLICY_HOOK,
                        item.asset_path,
                        recipe.dependency_diagnostics.len()
                    ))
                }
                Err(error) => Some(format!(
                    "the `{}` policy could not admit referenced asset `{}`: {error}",
                    INCOMPLETE_COMPOSITION_POLICY_HOOK, item.asset_path
                )),
            };
            if let Some(detail) = rejection {
                fail_reference_spawn(world, &mut item, detail);
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
        }
        let source_plan = prepared_native
            .as_ref()
            .map(|prepared| Arc::clone(&prepared.plan))
            .or_else(|| {
                world
                    .resource::<Assets<UsdStageAsset>>()
                    .get(ref_handle.id())
                    .map(|asset| Arc::clone(&asset.projection_plan))
            });
        let Some(source_plan) = source_plan else {
            fail_reference_spawn(
                world,
                &mut item,
                "the asset-ready event has no prepared USD asset".to_owned(),
            );
            commit_order.block_successors(item.scene_id, authoritative);
            still.push(item);
            continue;
        };
        let mut plan = match {
            let _span = bevy::log::info_span!("usd_reference_instance_plan_remap").entered();
            source_plan.for_instance(&item.prim_path)
        } {
            Ok(plan) => plan,
            Err(error) => {
                fail_reference_spawn(
                    world,
                    &mut item,
                    format!("invalid prepared projection plan: {error}"),
                );
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
        };
        let Ok(instance_root) = openusd::sdf::Path::new(&item.prim_path) else {
            let detail = format!("invalid USD prim path `{}`", item.prim_path);
            fail_reference_spawn(world, &mut item, detail);
            commit_order.block_successors(item.scene_id, authoritative);
            still.push(item);
            continue;
        };
        let mut root_transform = match UsdRead::local_transform_at(&plan, &instance_root, 0.0) {
            Ok(transform) => transform.unwrap_or_default(),
            Err(error) => {
                fail_reference_spawn(
                    world,
                    &mut item,
                    format!("prepared instance root transform is invalid: {error}"),
                );
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
        };
        let mut changed_pose = false;
        if let Some(translate) = item.translate {
            if translate.iter().any(|component| !component.is_finite()) {
                fail_reference_spawn(
                    world,
                    &mut item,
                    "prepared instance root translation is not finite".to_owned(),
                );
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
            root_transform.translation = Vec3::new(
                translate[0] as f32,
                translate[1] as f32,
                translate[2] as f32,
            );
            if !root_transform.translation.is_finite() {
                fail_reference_spawn(
                    world,
                    &mut item,
                    "prepared instance root translation exceeds the spatial render range"
                        .to_owned(),
                );
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
            changed_pose = true;
        }
        let root_rotation = item.deferred_ops.iter().rev().find_map(|op| match op {
            UsdOp::SetRotate { path, value, .. } if path == &item.prim_path => Some(*value),
            _ => None,
        });
        if let Some(rotate) = root_rotation {
            if rotate.iter().any(|component| !component.is_finite())
                || rotate
                    .iter()
                    .any(|component| (*component as f32).is_infinite())
            {
                fail_reference_spawn(
                    world,
                    &mut item,
                    "prepared instance root rotation is not representable".to_owned(),
                );
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
            root_transform.rotation = euler_xyz_deg_to_quat(Vec3::new(
                rotate[0] as f32,
                rotate[1] as f32,
                rotate[2] as f32,
            ));
            changed_pose = true;
        }
        if changed_pose {
            if let Err(error) = plan.set_instance_root_pose(root_transform) {
                fail_reference_spawn(
                    world,
                    &mut item,
                    format!("prepared instance root pose is invalid: {error}"),
                );
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
        }
        if let Some(scale) = item.deferred_ops.iter().rev().find_map(|op| match op {
            UsdOp::SetScale { path, value, .. } if path == &item.prim_path => Some(*value),
            _ => None,
        }) {
            if let Err(error) = plan.set_instance_root_scale(scale) {
                fail_reference_spawn(
                    world,
                    &mut item,
                    format!("prepared instance root scale is invalid: {error}"),
                );
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
        }
        if let Some(catalog_id) = item.deferred_ops.iter().rev().find_map(|op| match op {
            UsdOp::SetAttribute {
                path,
                name,
                type_name,
                value,
                ..
            } if path == &item.prim_path && name == "lunco:catalogId" && type_name == "string" => {
                Some(value.clone())
            }
            _ => None,
        }) {
            if let Err(error) =
                plan.set_instance_root_string_attribute("lunco:catalogId", catalog_id)
            {
                fail_reference_spawn(
                    world,
                    &mut item,
                    format!("prepared instance catalog identity is invalid: {error}"),
                );
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
        }
        let root_restore = item.deferred_ops.iter().find_map(|op| match op {
            UsdOp::RestorePrim {
                path,
                snapshot_usda,
                sibling_order,
                ..
            } if path == &item.prim_path => Some((snapshot_usda.clone(), sibling_order.clone())),
            _ => None,
        });
        let compose_reference = !authoritative
            || root_restore.is_some()
            || item
                .deferred_ops
                .iter()
                .any(|op| !deferred_op_is_represented_by_instance_plan(op, &item.prim_path));
        let projection = UsdInstanceProjection::new(
            ref_handle,
            Arc::clone(&recipe),
            Arc::new(plan),
            item.asset_path.clone(),
            item.reference_prim_path.clone(),
            item.type_name.clone(),
        );
        if compose_reference {
            projection.mark_promoted();
        }
        let Ok(sp) = openusd::sdf::Path::new(&item.prim_path) else {
            let detail = format!("invalid USD prim path `{}`", item.prim_path);
            fail_reference_spawn(world, &mut item, detail);
            commit_order.block_successors(item.scene_id, authoritative);
            still.push(item);
            continue;
        };
        let stage_result = {
            let _span = bevy::log::info_span!(
                "usd_reference_layer_closure_merge",
                layer_count = recipe.bytes.len(),
            )
            .entered();
            let prepared_paths = prepared_native
                .as_ref()
                .and_then(|prepared| prepared.plan.native_asset_paths_snapshot());
            match world.get_non_send_mut::<CanonicalStages>() {
                Some(mut stages) => match stages.get_mut(item.scene_id) {
                    Some(cs) => {
                        let preparation = if let Some(paths) = prepared_paths {
                            let mut paths_for_stage = cs
                                .cached_native_asset_paths()
                                .map(|current| current.as_ref().clone())
                                .unwrap_or_else(|| paths.as_ref().clone());
                            paths_for_stage
                                .merge(paths.as_ref().clone())
                                .map(|()| cs.set_native_asset_paths(Arc::new(paths_for_stage)))
                                .map_err(|error| error.to_string())
                        } else {
                            Ok(())
                        };
                        if let Err(error) = preparation {
                            Some(Err(format!(
                                "native reference preparation owner mismatch: {error}"
                            )))
                        } else if !cs.add_layer_recipe(&recipe) {
                            Some(Err(
                                "the owning stage cannot accept referenced layer bytes".to_owned()
                            ))
                        } else {
                            let _author_span =
                                bevy::log::info_span!("usd_reference_root_author").entered();
                            let result = if let Some((snapshot_usda, sibling_order)) =
                                root_restore.as_ref()
                            {
                                cs.projector()
                                    .restore_prim_subtree(&sp, snapshot_usda, sibling_order)
                                    .map_err(|error| {
                                        format!("failed to restore the prim subtree: {error}")
                                    })
                            } else if authoritative && compose_reference {
                                cs.projector()
                                    .author_referenced_prim(
                                        &sp,
                                        item.type_name.as_deref(),
                                        &item.asset_path,
                                        item.reference_prim_path.as_deref(),
                                    )
                                    .map_err(|error| {
                                        format!("failed to author the reference: {error}")
                                    })
                            } else if authoritative {
                                cs.projector()
                                    .author_prim(&sp, item.type_name.as_deref())
                                    .map_err(|error| {
                                        format!("failed to author the instance root: {error}")
                                    })
                            } else {
                                cs.projector()
                                    .author_referenced_prim(
                                        &sp,
                                        item.type_name.as_deref(),
                                        &item.asset_path,
                                        item.reference_prim_path.as_deref(),
                                    )
                                    .map_err(|error| {
                                        format!("failed to author the reference: {error}")
                                    })
                            };
                            Some(result.and_then(|()| {
                                let translate =
                                    root_restore.is_none().then_some(item.translate).flatten();
                                if let Some(translate) = translate {
                                    cs.projector().author_translate(&sp, translate).map_err(
                                        |error| {
                                            format!("failed to apply the spawn transform: {error}")
                                        },
                                    )?;
                                }
                                Ok(translate.is_some())
                            }))
                        }
                    }
                    None => None,
                },
                None => None,
            }
        };
        let translated = match stage_result {
            None => {
                fail_reference_spawn(
                    world,
                    &mut item,
                    "the owning scene stage disappeared before projection".to_owned(),
                );
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
            Some(Ok(translated)) => translated,
            Some(Err(error)) => {
                fail_reference_spawn(world, &mut item, error);
                commit_order.block_successors(item.scene_id, authoritative);
                still.push(item);
                continue;
            }
        };
        if translated {
            crate::live_consume::mark_live_transform(
                world,
                item.scene_id,
                item.prim_path.clone(),
                crate::live_consume::TransformEditChannels::translate(),
            );
        }
        world.resource_mut::<PendingInstanceProjections>().insert(
            item.scene_id,
            item.prim_path.clone(),
            projection,
            item.progress_key,
        );
        if native_reference {
            world
                .resource_mut::<PendingRefSpawns>()
                .native
                .retire_input(item.scene_id, &item.asset_path);
        }
        // Replay child-owned metadata and relationships only after the
        // referenced root exists on the live stage. The document already owns
        // the complete ordered intent; this is just its delayed live-stage
        // projection for first-use references.
        for op in std::mem::take(&mut item.deferred_ops) {
            if matches!(&op, UsdOp::RestorePrim { path, .. } if path == &item.prim_path) {
                continue;
            }
            apply_incremental_op_to_stage(world, item.scene_id, &op);
        }
        crate::live_consume::reproject_physics_if_needed(world, item.scene_id, &item.prim_path);
    }
    world.resource_mut::<PendingRefSpawns>().items.extend(still);
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_usd_bevy_scene::UsdSceneProjected;
    use lunco_usd_document::document::{LayerId, UsdOp};

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

    struct RejectCompositionPolicyHook(
        std::sync::Arc<
            std::sync::Mutex<
                Option<(
                    Vec<lunco_hooks::HookValue>,
                    lunco_core::RuntimeExecutionContext,
                )>,
            >,
        >,
    );

    impl lunco_hooks::ScriptHook for RejectCompositionPolicyHook {
        fn invoke(&self, invocation: &lunco_hooks::HookInvocation<'_>) -> lunco_hooks::HookResult {
            *self.0.lock().expect("composition hook observation lock") =
                Some((invocation.args.to_vec(), invocation.context));
            Ok(lunco_hooks::HookValue::map([(
                "action",
                lunco_hooks::HookValue::str("reject_scene"),
            )]))
        }
    }

    struct RestoreCompositionPolicyHook(Option<std::sync::Arc<lunco_hooks::RegisteredHook>>);

    impl Drop for RestoreCompositionPolicyHook {
        fn drop(&mut self) {
            lunco_hooks::unregister(INCOMPLETE_COMPOSITION_POLICY_HOOK);
            if let Some(previous) = self.0.take() {
                lunco_hooks::register(lunco_hooks::RegisteredHook {
                    id: previous.id.clone(),
                    backend: previous.backend.clone(),
                    deterministic: previous.deterministic,
                    hook: std::sync::Arc::clone(&previous.hook),
                });
            }
        }
    }

    #[test]
    fn changed_root_recipe_is_not_scheduled_as_its_own_dependent_stage() {
        use lunco_usd_compose::recipe::StageRecipe;

        let root = StageRecipe::new(
            "scene.usda",
            HashMap::from([("scene.usda".to_owned(), Vec::new())]),
        );
        let dependent = StageRecipe::new(
            "assembly.usda",
            HashMap::from([
                ("assembly.usda".to_owned(), Vec::new()),
                ("scene.usda".to_owned(), Vec::new()),
            ]),
        );

        assert!(!recipe_depends_on_changed_layer(&root, "scene.usda"));
        assert!(recipe_depends_on_changed_layer(&dependent, "scene.usda"));
    }

    #[test]
    fn waypoint_spawn_move_order_and_delete_use_bounded_dependent_patches() {
        let route = "/Traverse/Route";
        let waypoint = "/Traverse/Route/W6";
        let ops = [
            UsdOp::AddPrim {
                edit_target: LayerId::root(),
                parent_path: route.to_owned(),
                name: "W6".to_owned(),
                type_name: Some("Xform".to_owned()),
                reference: Some("../markers/route_point.usda".to_owned()),
                reference_prim_path: None,
            },
            UsdOp::SetTranslate {
                edit_target: LayerId::runtime(),
                path: waypoint.to_owned(),
                value: [12.0, 4.0, -7.0],
            },
            UsdOp::SetPrimOrder {
                edit_target: LayerId::runtime(),
                path: route.to_owned(),
                order: Some((0..=6).map(|index| format!("W{index}")).collect()),
            },
        ];
        let Some(DependentStageRefreshMode::Incremental(patch)) =
            dependent_stage_refresh_mode(&ops)
        else {
            panic!("waypoint edits must use an incremental dependent patch");
        };
        assert!(patch.prim_subtrees.contains(waypoint));
        assert!(
            patch
                .fields
                .contains(&(route.to_owned(), "primChildren".to_owned()))
        );
        assert!(
            patch
                .fields
                .contains(&(route.to_owned(), "primOrder".to_owned()))
        );

        let delete = UsdOp::RemovePrim {
            edit_target: LayerId::runtime(),
            path: waypoint.to_owned(),
        };
        let Some(DependentStageRefreshMode::Incremental(delete_patch)) =
            dependent_stage_refresh_mode(&[delete])
        else {
            panic!("waypoint deletion must use an incremental dependent patch");
        };
        assert!(delete_patch.prim_subtrees.contains(waypoint));
        assert!(
            delete_patch
                .fields
                .contains(&(route.to_owned(), "primChildren".to_owned()))
        );

        let view_only = UsdOp::SetTranslate {
            edit_target: LayerId::view(),
            path: waypoint.to_owned(),
            value: [0.0; 3],
        };
        assert!(dependent_stage_refresh_mode(&[view_only]).is_none());
    }

    #[test]
    fn dependent_stage_plan_refresh_runs_below_live_patch_work() {
        assert_eq!(
            dependent_stage_work_priority(
                DependentStageRefreshPhase::Patch,
                true,
                AsyncWorkPriority::Interactive,
            ),
            AsyncWorkPriority::SimulationRequired,
        );
        assert_eq!(
            dependent_stage_work_priority(
                DependentStageRefreshPhase::Patch,
                false,
                AsyncWorkPriority::Interactive,
            ),
            AsyncWorkPriority::Interactive,
        );
        assert_eq!(
            dependent_stage_work_priority(
                DependentStageRefreshPhase::Plan,
                true,
                AsyncWorkPriority::SimulationRequired,
            ),
            AsyncWorkPriority::Background,
        );
        assert_eq!(
            dependent_stage_work_priority(
                DependentStageRefreshPhase::Rebuild,
                true,
                AsyncWorkPriority::Interactive,
            ),
            AsyncWorkPriority::SimulationRequired,
        );
    }

    #[test]
    fn first_authored_edit_promotes_only_its_prepared_reference_instance() {
        use lunco_usd_bevy_stage::canonical::{CanonicalStage, CanonicalStages};
        use lunco_usd_compose::recipe::StageRecipe;
        use lunco_usd_document::document::LayerId;

        let scene_recipe = StageRecipe::from_source(
            "scene.usda",
            "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\" {}\n",
        );
        let reference_recipe = Arc::new(StageRecipe::from_source(
            "reference.usda",
            "#usda 1.0\n(\n    defaultPrim = \"Asset\"\n)\ndef Xform \"Asset\"\n{\n    def Cube \"Box\"\n    {\n        double size = 2\n    }\n}\n",
        ));
        let stage_handle = Handle::<UsdStageAsset>::default();
        let mut canonical = CanonicalStage::from_recipe(&scene_recipe).expect("open scene");
        assert!(canonical.add_layer_recipe(&reference_recipe));
        let instance_root = openusd::sdf::Path::new("/World/Spawned").unwrap();
        canonical
            .projector()
            .author_prim(&instance_root, Some("Xform"))
            .expect("author lightweight root");

        let mut app = App::new();
        crate::live_consume::install_live_prim_entity_index(&mut app);
        app.init_non_send::<CanonicalStages>();
        app.world_mut()
            .get_non_send_mut::<CanonicalStages>()
            .expect("canonical stage resource")
            .insert(stage_handle.id(), canonical);
        let root_entity = app
            .world_mut()
            .spawn(UsdPrimPath {
                stage_handle: stage_handle.clone(),
                path: instance_root.to_string(),
            })
            .id();
        let source =
            UsdStageAsset::from_recipe((*reference_recipe).clone()).expect("prepare source asset");
        let plan = source
            .projection_plan
            .for_instance(instance_root.as_str())
            .expect("remap source plan");
        let mut projection = UsdInstanceProjection::new(
            Handle::default(),
            Arc::clone(&reference_recipe),
            Arc::new(plan),
            "reference.usda",
            None,
            Some("Xform".to_owned()),
        );
        projection.root = Some(root_entity);
        app.world_mut()
            .entity_mut(root_entity)
            .insert(projection.clone());

        let op = UsdOp::SetPrimKind {
            edit_target: LayerId::runtime(),
            path: "/World/Spawned/Box".to_owned(),
            kind: Some("component".to_owned()),
        };
        promote_referenced_instance_for_op(
            app.world_mut(),
            stage_handle.id(),
            "/World/Spawned/Box",
            &op,
        )
        .expect("promote referenced instance before applying edit");

        assert!(
            projection.is_promoted(),
            "cloned readers share promotion state"
        );
        let stages = app.world().non_send::<CanonicalStages>();
        let live = stages.get(stage_handle.id()).expect("live stage remains");
        assert!(
            live.view()
                .has_prim(&openusd::sdf::Path::new("/World/Spawned/Box").unwrap())
        );
    }

    #[test]
    fn dependency_diagnostics_publish_without_projected_prims() {
        use bevy::asset::AssetApp;
        use lunco_usd_compose::recipe::{StageDependencyDiagnostic, StageRecipe};

        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>()
            .init_resource::<lunco_core::RuntimeDiagnostics>()
            .add_message::<bevy::asset::AssetEvent<UsdStageAsset>>()
            .add_systems(Update, sync_stage_dependency_diagnostics);

        let mut recipe = StageRecipe::from_source("scene.usda", TINY);
        recipe
            .dependency_diagnostics
            .push(StageDependencyDiagnostic::missing(
                "scene.usda",
                "lunco://markers/route_point.usda",
            ));
        let handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset::from_recipe(recipe).expect("prepare stage asset"));
        app.world_mut()
            .resource_mut::<Messages<bevy::asset::AssetEvent<UsdStageAsset>>>()
            .write(bevy::asset::AssetEvent::Added { id: handle.id() });

        app.update();

        let diagnostics = &app
            .world()
            .resource::<lunco_core::RuntimeDiagnostics>()
            .findings;
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "USD_COMPOSITION_MISSING_DEPENDENCY");
        assert_eq!(
            diagnostics[0].subject,
            "scene.usda -> lunco://markers/route_point.usda"
        );

        app.world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .remove(handle.id());
        app.update();
        assert!(
            app.world()
                .resource::<lunco_core::RuntimeDiagnostics>()
                .findings
                .is_empty()
        );
    }

    #[test]
    fn component_refresh_policy_accepts_only_explicit_actions() {
        assert_eq!(
            parse_component_refresh_decision(&HookValue::map([(
                "action",
                HookValue::str("propagate"),
            )])),
            Ok(ComponentRefreshDecision::Propagate)
        );
        assert_eq!(
            parse_component_refresh_decision(&HookValue::map([(
                "action",
                HookValue::str("defer"),
            )])),
            Ok(ComponentRefreshDecision::Defer)
        );
        assert_eq!(
            parse_component_refresh_decision(&HookValue::map([(
                "action",
                HookValue::str("reject"),
            )])),
            Ok(ComponentRefreshDecision::Reject)
        );
        assert!(parse_component_refresh_decision(&HookValue::Unit).is_err());
        assert!(
            parse_component_refresh_decision(&HookValue::map([(
                "action",
                HookValue::str("unknown"),
            )]))
            .is_err()
        );
    }

    #[test]
    fn component_refresh_context_tracks_the_projection_owner() {
        let application = component_refresh_runtime_context(false, None)
            .expect("an editor preview uses the Application lifecycle route");
        assert_eq!(
            application.route,
            Some(lunco_core::RuntimeRoute::application(
                lunco_core::RuntimeCycle::Lifecycle,
            ))
        );
        assert_eq!(application.phase, lunco_core::RuntimePhase::Preparation);
        assert_eq!(application.clock, lunco_core::RuntimeClock::None);
        assert!(application.validate().is_ok());

        let twin = component_refresh_runtime_context(true, Some(17))
            .expect("a mounted Twin uses its lifecycle generation");
        assert_eq!(
            twin.route,
            Some(lunco_core::RuntimeRoute::twin(
                lunco_core::RuntimeCycle::Lifecycle,
                17,
            ))
        );
        assert_eq!(twin.phase, lunco_core::RuntimePhase::Preparation);
        assert_eq!(twin.clock, lunco_core::RuntimeClock::None);
        assert!(twin.validate().is_ok());

        assert!(component_refresh_runtime_context(true, None).is_err());
    }

    #[test]
    fn projection_wake_coalesces_and_consumes_explicitly() {
        let wake = TwinProjectionWake::default();
        assert!(!wake.is_pending());

        wake.wake();
        wake.wake();
        assert!(
            wake.is_pending(),
            "multiple producers share one pending wake"
        );

        wake.consume();
        assert!(
            !wake.is_pending(),
            "the projection owner consumes its wake once"
        );
    }

    #[test]
    fn referenced_add_prim_paths_name_only_added_reference_roots() {
        let paths = referenced_add_prim_paths(&[
            UsdOp::AddPrim {
                edit_target: lunco_usd_document::document::LayerId::runtime(),
                parent_path: "/World/Route".into(),
                name: "W5".into(),
                type_name: Some("Xform".into()),
                reference: Some("lunco://markers/route_point.usda".into()),
                reference_prim_path: None,
            },
            UsdOp::AddPrim {
                edit_target: lunco_usd_document::document::LayerId::runtime(),
                parent_path: "/World".into(),
                name: "Plain".into(),
                type_name: Some("Xform".into()),
                reference: None,
                reference_prim_path: None,
            },
        ]);

        assert_eq!(paths, ["/World/Route/W5"]);
    }

    #[test]
    fn document_projection_hold_coalesces_revisions_and_rejects_stale_completion() {
        let doc = DocumentId::fresh();
        let key = SimulationProgressKey::usd_document_projection(doc.raw());
        let mut admissions = PendingDocumentProjectionAdmissions::default();
        let mut progress = SimulationProgress::default();

        admissions.admit(doc, 4, &mut progress);
        admissions.admit(doc, 7, &mut progress);
        assert_eq!(admissions.generations.get(&doc), Some(&7));
        assert_eq!(progress.blockers().count(), 1);
        assert_eq!(
            progress.blockers().next().map(|blocker| blocker.key),
            Some(key)
        );

        admissions.complete(doc, 6, &mut progress);
        assert!(
            progress.is_held(),
            "an older projection cannot release the latest revision"
        );

        admissions.complete(doc, 7, &mut progress);
        assert!(!progress.is_held());
    }

    /// Relationship and connection edits use live-stage authors, while composition
    /// arc edits still require a composed-stage rebuild. This keeps assembly edits
    /// on the incremental path and reserves rebuilding for non-local composition.
    #[test]
    fn op_rebuild_routing_matches_the_incremental_authors() {
        let et = LayerId::root();
        // Incremental now — a joint's two `physics:body` rels and a cosim wire.
        assert!(!op_needs_rebuild(&UsdOp::SetRelationship {
            edit_target: et.clone(),
            path: "/J".into(),
            name: "physics:body0".into(),
            targets: vec![],
        }));
        assert!(!op_needs_rebuild(&UsdOp::SetConnection {
            edit_target: et.clone(),
            path: "/B".into(),
            name: "inputs:v".into(),
            type_name: "float".into(),
            sources: vec![],
        }));
        assert!(!op_needs_rebuild(&UsdOp::SetPrimOrder {
            edit_target: et.clone(),
            path: "/W/Route".into(),
            order: Some(vec!["P1".into(), "P0".into()]),
        }));
        // Physical API schemas are authored live and refresh only the affected
        // prim subtree, so unrelated scripts and bodies keep running.
        assert!(!op_needs_rebuild(&UsdOp::SetApiSchemas {
            edit_target: et.clone(),
            path: "/W".into(),
            schemas: vec!["PhysicsRigidBodyAPI".into()],
        }));
        // Metadata-only APIs on an existing scope remain on the live
        // incremental path.
        assert!(!op_needs_rebuild(&UsdOp::SetApiSchemas {
            edit_target: et.clone(),
            path: "/W/Mission".into(),
            schemas: vec!["LunCoProgramAPI".into()],
        }));
        assert!(!op_needs_rebuild(&UsdOp::SetApiSchemas {
            edit_target: et.clone(),
            path: "/W/Mission".into(),
            schemas: vec!["LunCoUiSchemaAPI".into()],
        }));
        // Active state is handled by the generic structural reconciler for both
        // physical and visual prims: it despawns absent entities and spawns them
        // from the canonical stage when they become active.
        assert!(!op_needs_rebuild(&UsdOp::SetActive {
            edit_target: et.clone(),
            path: "/Rover/Chassis".into(),
            active: false,
        }));
        assert!(!op_needs_rebuild(&UsdOp::SetActive {
            edit_target: et.clone(),
            path: "/Rover/Route/W3".into(),
            active: false,
        }));
        assert!(!op_needs_rebuild(&UsdOp::SetActive {
            edit_target: et.clone(),
            path: "/Apollo15/Route/W0".into(),
            active: true,
        }));
        assert!(!op_needs_rebuild(&UsdOp::SetActive {
            edit_target: et.clone(),
            path: "/Rover/Wheels/W0".into(),
            active: false,
        }));
        // Stage and prim metadata are read from the rebuilt composed snapshot
        // so root/runtime opinions and canonical-stage reads stay coherent.
        assert!(op_needs_rebuild(&UsdOp::SetDefaultPrim {
            edit_target: et.clone(),
            default_prim: Some("World".into()),
        }));
        assert!(!op_needs_rebuild(&UsdOp::SetPrimKind {
            edit_target: et.clone(),
            path: "/Rover".into(),
            kind: Some("component".into()),
        }));
        // Composition-arc edits also rebuild — value resolution recomposes the
        // subtree, which the incremental sink can't express.
        assert!(op_needs_rebuild(&UsdOp::SetVariantSelection {
            edit_target: et.clone(),
            path: "/R".into(),
            variant_set: "drivetrain".into(),
            variant: "physical".into(),
        }));
        assert!(op_needs_rebuild(&UsdOp::SetPayload {
            edit_target: et.clone(),
            path: "/H".into(),
            asset_paths: vec![],
        }));
        // Pre-existing coarse ops unchanged.
        assert!(op_needs_rebuild(&UsdOp::MovePrim {
            edit_target: et,
            from_path: "/a".into(),
            to_path: "/b".into(),
        }));
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
        world.init_resource::<Messages<UsdSceneProjectionReset>>();
        let stage = Handle::<UsdStageAsset>::default();
        let root = world
            .spawn((
                UsdSceneRoot,
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Traverse".into(),
                },
                UsdSceneProjected,
            ))
            .id();
        let detached_camera = world
            .spawn((
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Traverse/Embodiment".into(),
                },
                UsdSceneProjected,
            ))
            .id();

        refresh_scene_visuals(&mut world, stage.id());

        assert!(
            world.get::<UsdSceneProjected>(root).is_none(),
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

        assert!(
            !app.world()
                .resource::<PendingTwinDocs>()
                .has_terminal_source_event()
        );
        app.world_mut()
            .resource_mut::<Messages<bevy::asset::AssetEvent<UsdSourceText>>>()
            .write(bevy::asset::AssetEvent::Added { id: handle.id() });
        app.update();

        assert!(
            app.world()
                .resource::<PendingTwinDocs>()
                .has_terminal_source_event()
        );
        assert!(
            app.world()
                .resource::<PendingTwinDocs>()
                .ready
                .contains(&handle.id())
        );
    }

    #[test]
    fn twin_document_preparation_wakes_on_source_capacity_and_completion() {
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
        assert!(!pending.has_preparation_work(0));

        pending.mark_ready(handle.id());
        assert!(pending.has_preparation_work(0));
        pending.items[0].capacity_revision = Some(0);
        assert!(!pending.has_preparation_work(0));
        assert!(pending.has_preparation_work(1));

        pending.items[0].stage = TwinDocPreparationStage::PreparingSource { operation: 7 };
        pending.items[0].capacity_revision = None;
        assert!(!pending.has_preparation_work(1));
        pending.completions.lock().unwrap().push(TwinDocCompletion {
            operation: 7,
            result: TwinDocCompletionResult::Source(Ok(PreparedUsdSource::parse(
                "#usda 1.0\n".into(),
            ))),
        });
        assert!(pending.has_preparation_work(1));
        assert_eq!(pending.take_completions().len(), 1);
        assert!(!pending.has_preparation_work(1));
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

    #[test]
    fn reference_admission_tracks_activity_and_publishes_terminal_failure() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PendingRefSpawns::default());
        world.insert_resource(PendingInstanceProjections::default());
        world.insert_resource(SimulationProgress::default());

        let key = world
            .resource_mut::<PendingRefSpawns>()
            .allocate_progress_key()
            .expect("reference operation identity");
        let scene_id = AssetId::<UsdStageAsset>::default();
        let root = world
            .spawn((
                UsdSceneRoot,
                UsdPrimPath {
                    stage_handle: Handle::default(),
                    path: "/World".to_owned(),
                },
            ))
            .id();
        let mut mounts = lunco_core::SceneMountState::default();
        mounts.register_root(root, true);
        world.insert_resource(mounts);
        let path = "/World/Rover";
        let asset = "lunco://vessels/rover.usda";
        let detail = "referenced asset failed to load";
        let mut item = failed_ref_spawn(
            key,
            scene_id,
            path,
            Some("Xform".to_owned()),
            asset,
            None,
            detail.to_owned(),
        );
        item.held = report_reference_failure(&mut world, key, scene_id, path, asset, detail);
        item.failure_reported = true;
        world.resource_mut::<PendingRefSpawns>().push(item, false);

        assert!(world.resource::<SimulationProgress>().is_held());
        assert!(world.resource::<lunco_core::RuntimeFaults>().active());
        assert_eq!(
            world.resource::<lunco_core::RuntimeDiagnostics>().findings[0].code,
            "usd-reference-admission"
        );

        set_pending_reference_active(&mut world, 0, false);
        assert!(world.resource::<SimulationProgress>().is_held());
        set_pending_reference_active(&mut world, 0, true);
        assert!(world.resource::<SimulationProgress>().is_held());

        cancel_pending_reference(&mut world, 0);
        assert!(world.resource::<SimulationProgress>().is_held());
        assert!(
            world
                .resource_mut::<PendingRefSpawns>()
                .replace_path(scene_id, path)
                .is_empty()
        );
        assert!(world.resource::<SimulationProgress>().is_held());

        world.run_system_once(reset_scene_projection_state).unwrap();
        assert!(!world.resource::<SimulationProgress>().is_held());
    }

    #[test]
    fn incomplete_dynamic_reference_policy_rejection_holds_primary_and_blocks_successors() {
        use bevy::asset::{AssetApp, AssetPath, AssetServer};
        use lunco_usd_bevy_stage::UsdStageAsset;
        use lunco_usd_compose::recipe::{StageDependencyDiagnostic, StageRecipe};

        let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
        let previous = lunco_hooks::get(INCOMPLETE_COMPOSITION_POLICY_HOOK);
        lunco_hooks::register(lunco_hooks::RegisteredHook {
            id: INCOMPLETE_COMPOSITION_POLICY_HOOK.to_owned(),
            backend: "test".to_owned(),
            deterministic: true,
            hook: std::sync::Arc::new(RejectCompositionPolicyHook(std::sync::Arc::clone(
                &observed,
            ))),
        });
        let _restore_hook = RestoreCompositionPolicyHook(previous);

        let mut app = App::new();
        app.add_plugins(bevy::prelude::MinimalPlugins)
            .add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>();

        let scene_handle = app
            .world()
            .resource::<AssetServer>()
            .load::<UsdStageAsset>(AssetPath::parse("composition-test/scene.usda").into_owned());
        let rejected_handle = app
            .world()
            .resource::<AssetServer>()
            .load::<UsdStageAsset>(AssetPath::parse("composition-test/partial.usda").into_owned());
        let successor_handle = app
            .world()
            .resource::<AssetServer>()
            .load::<UsdStageAsset>(AssetPath::parse("composition-test/complete.usda").into_owned());

        let scene_recipe = StageRecipe::from_source("composition-test/scene.usda", TINY);
        let mut rejected_recipe = StageRecipe::from_source("composition-test/partial.usda", TINY);
        rejected_recipe
            .dependency_diagnostics
            .push(StageDependencyDiagnostic::missing(
                "composition-test/partial.usda",
                "composition-test/missing-first.usda",
            ));
        rejected_recipe
            .dependency_diagnostics
            .push(StageDependencyDiagnostic::missing(
                "composition-test/partial.usda",
                "composition-test/missing-second.usda",
            ));
        let successor_recipe = StageRecipe::from_source("composition-test/complete.usda", TINY);
        {
            let mut assets = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
            assets
                .insert(
                    scene_handle.id(),
                    UsdStageAsset::from_recipe(scene_recipe).expect("prepare scene asset"),
                )
                .expect("insert scene under its AssetServer identity");
            assets
                .insert(
                    rejected_handle.id(),
                    UsdStageAsset::from_recipe(rejected_recipe)
                        .expect("prepare incomplete referenced asset"),
                )
                .expect("insert incomplete asset under its AssetServer identity");
            assets
                .insert(
                    successor_handle.id(),
                    UsdStageAsset::from_recipe(successor_recipe)
                        .expect("prepare complete successor asset"),
                )
                .expect("insert successor under its AssetServer identity");
        }

        app.init_resource::<PendingRefSpawns>()
            .init_resource::<PendingDependentStageRefreshes>()
            .init_resource::<PendingInstanceProjections>()
            .init_resource::<SimulationProgress>()
            .init_resource::<lunco_core::SceneTransitionCoordinator>();
        let root = app
            .world_mut()
            .spawn((
                UsdSceneRoot,
                UsdPrimPath {
                    stage_handle: scene_handle.clone(),
                    path: "/World".to_owned(),
                },
            ))
            .id();
        let mut mounts = lunco_core::SceneMountState::default();
        mounts.register_root(root, true);
        app.world_mut().insert_resource(mounts);

        let scene_id = scene_handle.id();
        let first_key = app
            .world_mut()
            .resource_mut::<PendingRefSpawns>()
            .allocate_progress_key()
            .expect("first reference operation identity");
        let second_key = app
            .world_mut()
            .resource_mut::<PendingRefSpawns>()
            .allocate_progress_key()
            .expect("successor reference operation identity");
        let make_spawn = |progress_key, prim_path: &str, asset_path: &str, ref_handle| RefSpawn {
            progress_key,
            scene_id,
            prim_path: prim_path.to_owned(),
            type_name: Some("Xform".to_owned()),
            asset_path: asset_path.to_owned(),
            reference_prim_path: None,
            ref_handle: Some(ref_handle),
            translate: None,
            deferred_ops: Vec::new(),
            active: true,
            held: false,
            asset_ready: false,
            failure: None,
            failure_reported: false,
            removed: false,
        };
        {
            let mut pending = app.world_mut().resource_mut::<PendingRefSpawns>();
            pending.push(
                make_spawn(
                    first_key,
                    "/World/Partial",
                    "twin://composition-test/partial.usda",
                    rejected_handle,
                ),
                true,
            );
            pending.push(
                make_spawn(
                    second_key,
                    "/World/Complete",
                    "twin://composition-test/complete.usda",
                    successor_handle,
                ),
                true,
            );
        }

        drain_ref_spawns(app.world_mut());

        let (args, context) = observed
            .lock()
            .expect("composition hook observation lock")
            .clone()
            .expect("incomplete dynamic reference reaches the required Rhai policy");
        assert_eq!(args.len(), 1);
        let facts = &args[0];
        assert_eq!(
            facts
                .get("scene_path")
                .and_then(lunco_hooks::HookValue::as_str),
            Some("composition-test/scene.usda")
        );
        let missing = match facts.get("missing_dependencies") {
            Some(lunco_hooks::HookValue::Array(missing)) => missing,
            _ => panic!("policy receives ordered unresolved dependency facts"),
        };
        assert_eq!(missing.len(), 2);
        assert_eq!(
            missing[0]
                .get("dependency")
                .and_then(lunco_hooks::HookValue::as_str),
            Some("composition-test/missing-first.usda")
        );
        assert_eq!(
            missing[1]
                .get("dependency")
                .and_then(lunco_hooks::HookValue::as_str),
            Some("composition-test/missing-second.usda")
        );
        assert_eq!(
            context.route.map(|route| route.scope),
            Some(lunco_core::RuntimeScope::Application)
        );
        assert_eq!(
            context.route.expect("policy route is classified").cycle,
            lunco_core::RuntimeCycle::Lifecycle
        );
        assert_eq!(context.phase, lunco_core::RuntimePhase::Preparation);

        let pending = app.world().resource::<PendingRefSpawns>();
        assert_eq!(pending.items.len(), 2);
        assert!(pending.items[0].failure_reported);
        assert!(pending.items[0].failure.as_deref().is_some_and(|detail| {
            detail.contains("rejected referenced asset")
                && detail.contains("composition-test/missing-first.usda")
        }));
        assert!(pending.items[0].held && pending.items[1].held);
        assert!(pending.items[1].asset_ready);
        assert!(app.world().resource::<SimulationProgress>().is_held());
        assert!(app.world().resource::<lunco_core::RuntimeFaults>().active());
        assert!(
            app.world()
                .resource::<PendingInstanceProjections>()
                .plans
                .is_empty()
        );
        assert!(
            app.world()
                .resource::<lunco_core::RuntimeDiagnostics>()
                .findings
                .iter()
                .any(|finding| finding.code == "usd-reference-admission")
        );
    }

    #[test]
    fn primary_reference_commit_order_holds_ready_successors_until_the_prefix_is_ready() {
        let scene_id = AssetId::<UsdStageAsset>::default();
        let mut order = PrimaryReferenceCommitOrder::default();

        assert!(order.must_defer(scene_id, true, false));
        assert!(
            order.must_defer(scene_id, true, true),
            "later completed results wait behind the unresolved operation"
        );
        assert!(
            !order.must_defer(scene_id, false, false),
            "preview projection does not wait on the primary simulation boundary"
        );

        let mut next_drain = PrimaryReferenceCommitOrder::default();
        assert!(!next_drain.must_defer(scene_id, true, true));
        assert!(!next_drain.must_defer(scene_id, true, true));

        next_drain.block_successors(scene_id, true);
        assert!(
            next_drain.must_defer(scene_id, true, true),
            "a terminal failure prevents later operations from committing"
        );
    }

    #[test]
    fn failed_reference_load_dominates_a_retained_prepared_asset() {
        assert_eq!(
            classify_reference_asset_state(true, false, Some("reload failed".to_owned())),
            ReferenceAssetState::Failed("reload failed".to_owned()),
            "a failed reload must not commit a stale retained asset"
        );
        assert_eq!(
            classify_reference_asset_state(true, false, None),
            ReferenceAssetState::Prepared
        );
        assert_eq!(
            classify_reference_asset_state(false, false, None),
            ReferenceAssetState::Loading
        );
    }

    #[test]
    fn loading_reference_reload_dominates_a_retained_prepared_asset() {
        assert_eq!(
            classify_reference_asset_state(true, true, None),
            ReferenceAssetState::Loading,
            "an in-progress reload must not commit a stale retained asset"
        );
    }

    #[test]
    fn prepared_reference_asset_survives_its_event_preceding_spawn_admission() {
        use bevy::asset::AssetApp;
        use bevy::prelude::*;
        use lunco_usd_bevy_stage::canonical::CanonicalStages;
        use lunco_usd_bevy_stage::read::UsdRead;
        use openusd::sdf::Path as SdfPath;

        const REFERENCE: &str =
            "#usda 1.0\n(\n    defaultPrim = \"Vehicle\"\n)\ndef Xform \"Vehicle\"\n{\n}\n";

        let scene_recipe = lunco_usd_compose::recipe::StageRecipe::from_source("scene.usda", TINY);
        let mut app = App::new();
        crate::live_consume::install_live_prim_entity_index(&mut app);
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>()
            .init_non_send::<CanonicalStages>()
            .init_resource::<PendingRefSpawns>()
            .init_resource::<PendingDependentStageRefreshes>()
            .init_resource::<PendingInstanceProjections>()
            .init_resource::<SimulationProgress>()
            .add_systems(Update, mark_pending_ref_spawns);

        let scene_handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset::from_recipe(scene_recipe.clone()).expect("prepare scene"));
        let scene_id = scene_handle.id();
        let reference_id = app
            .world_mut()
            .non_send_mut::<CanonicalStages>()
            .get_or_build(scene_id, &scene_recipe)
            .expect("open the live scene stage")
            .canonical_reference_id("vehicle.usda")
            .expect("valid reference");
        let reference_recipe =
            lunco_usd_compose::recipe::StageRecipe::from_source(reference_id, REFERENCE);
        let reference_handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset::from_recipe(reference_recipe).expect("prepare reference"));
        let reference_asset_id = reference_handle.id();

        // Assets publishes Added after insertion. Let the runtime consume that
        // event while no authored spawn exists yet.
        app.update();
        app.update();
        assert!(app.world().resource::<PendingRefSpawns>().ready.is_empty());

        let progress_key = app
            .world_mut()
            .resource_mut::<PendingRefSpawns>()
            .allocate_progress_key()
            .expect("reference operation identity");
        app.world_mut().resource_mut::<PendingRefSpawns>().push(
            RefSpawn {
                progress_key,
                scene_id,
                prim_path: "/World/Vehicle".to_owned(),
                type_name: Some("Xform".to_owned()),
                asset_path: "vehicle.usda".to_owned(),
                reference_prim_path: None,
                ref_handle: Some(reference_handle),
                translate: None,
                deferred_ops: Vec::new(),
                active: true,
                held: false,
                asset_ready: false,
                failure: None,
                failure_reported: false,
                removed: false,
            },
            false,
        );

        // The event reader does not replay the old Added message for a later
        // operation. Queue admission must recover readiness from asset state.
        app.update();
        assert!(app.world().resource::<PendingRefSpawns>().ready.is_empty());
        let item = app
            .world_mut()
            .resource_mut::<PendingRefSpawns>()
            .items
            .remove(0);
        enqueue_reference_spawn(app.world_mut(), item);
        assert!(
            app.world()
                .resource::<PendingRefSpawns>()
                .ready
                .contains(&reference_asset_id)
        );
        assert!(app.world().resource::<PendingRefSpawns>().items[0].asset_ready);

        drain_ref_spawns(app.world_mut());

        let stage = app
            .world()
            .non_send::<CanonicalStages>()
            .get(scene_id)
            .expect("live scene stage remains open");
        assert!(
            stage
                .view()
                .has_prim(&SdfPath::new("/World/Vehicle").unwrap())
        );
        assert!(app.world().resource::<PendingRefSpawns>().items.is_empty());
        assert!(
            app.world()
                .resource::<PendingInstanceProjections>()
                .plans
                .contains_key(&(scene_id, "/World/Vehicle".into()))
        );
    }

    #[derive(Resource, Default)]
    struct ProjectedDocumentEvents(Vec<lunco_telemetry_core::TelemetryValue>);

    fn capture_projected_document_event(
        event: On<lunco_telemetry_core::StampedTelemetryEvent>,
        mut events: ResMut<ProjectedDocumentEvents>,
    ) {
        if event.event().name == "usd.document.projected" {
            events.0.push(event.event().data.clone());
        }
    }

    #[test]
    fn document_projection_event_waits_for_affected_reference_only() {
        use bevy::prelude::*;
        use lunco_core_runtime::pacing::{SimulationProgressKey, SimulationProgressOwner};

        let mut app = App::new();
        app.add_plugins(lunco_telemetry_core::LunCoTelemetryCorePlugin);
        app.init_resource::<crate::live_consume::PendingStageProjections>()
            .init_resource::<PendingRefSpawns>()
            .init_resource::<PendingInstanceProjections>()
            .init_resource::<PendingDocumentProjectionAdmissions>()
            .init_resource::<SimulationProgress>()
            .init_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .init_resource::<ProjectedDocumentEvents>()
            .add_observer(capture_projected_document_event);

        let doc = lunco_doc::DocumentId(42);
        let stage_id = AssetId::<UsdStageAsset>::default();
        let generation = 7;
        {
            let mut backed = app
                .world_mut()
                .resource_mut::<lunco_usd_bevy_twin::DocBackedTwinScenes>();
            backed.track_preview(doc, "projection-test".into(), "scene.usda".into());
            backed.mark_applied(doc, stage_id, generation);
        }
        crate::live_consume::queue_stage_projection(
            app.world_mut(),
            doc,
            stage_id,
            generation,
            ["/World/Route/W5".to_owned()],
        );
        assert!(crate::live_consume::accumulate_pending_stage_changes(
            app.world_mut(),
            stage_id,
            ["/World/Route".to_owned(), "/World/Route/W5".to_owned(),],
        ));

        let reference_key = SimulationProgressKey {
            owner: SimulationProgressOwner::SceneReferences,
            operation_id: 1,
        };
        let document_key = SimulationProgressKey::usd_document_projection(doc.raw());
        app.world_mut()
            .resource_mut::<PendingDocumentProjectionAdmissions>()
            .generations
            .insert(doc, generation);
        {
            let mut progress = app.world_mut().resource_mut::<SimulationProgress>();
            progress.acquire(document_key, "waiting for edited route point");
            progress.acquire(reference_key, "waiting for edited route point asset");
        }
        app.world_mut()
            .resource_mut::<PendingRefSpawns>()
            .items
            .push(RefSpawn {
                progress_key: reference_key,
                scene_id: stage_id,
                prim_path: "/World/Route/W5".into(),
                type_name: Some("Xform".into()),
                asset_path: "lunco://markers/route_point.usda".into(),
                reference_prim_path: None,
                ref_handle: None,
                translate: Some([1.0, 2.0, 3.0]),
                deferred_ops: Vec::new(),
                active: true,
                held: false,
                asset_ready: false,
                failure: None,
                failure_reported: false,
                removed: false,
            });

        crate::live_consume::publish_pending_stage_projections(app.world_mut());
        app.world_mut().flush();
        assert!(
            app.world()
                .resource::<ProjectedDocumentEvents>()
                .0
                .is_empty()
        );
        assert!(app.world().resource::<SimulationProgress>().is_held());
        assert_eq!(
            app.world()
                .resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
                .synced_generation(doc),
            None,
            "the live document cursor must remain behind while the reference is unresolved"
        );

        app.world_mut()
            .resource_mut::<PendingRefSpawns>()
            .items
            .clear();
        app.world_mut()
            .resource_mut::<PendingInstanceProjections>()
            .plans
            .insert(
                (stage_id, "/World/Route/W5".into()),
                PendingInstanceProjection {
                    projection: UsdInstanceProjection::new(
                        Handle::default(),
                        Arc::new(lunco_usd_compose::recipe::StageRecipe::from_source(
                            "reference.usda",
                            TINY,
                        )),
                        Arc::new(UsdStageProjectionPlan::default()),
                        "",
                        None,
                        None,
                    ),
                    progress_key: reference_key,
                    failure_reported: false,
                },
            );
        assert!(crate::live_consume::accumulate_pending_stage_changes(
            app.world_mut(),
            stage_id,
            ["/World/Route/W5".to_owned()],
        ));
        crate::live_consume::publish_pending_stage_projections(app.world_mut());
        app.world_mut().flush();
        assert!(
            app.world()
                .resource::<ProjectedDocumentEvents>()
                .0
                .is_empty()
        );

        app.world_mut()
            .resource_mut::<PendingInstanceProjections>()
            .plans
            .clear();
        app.world_mut()
            .resource_mut::<SimulationProgress>()
            .release(reference_key);

        // A separate reference admission elsewhere in the mounted stage must
        // not hold this document edit's completion or document-projection hold.
        // That reference retains its own SceneReferences simulation hold.
        let unrelated_reference_key = SimulationProgressKey {
            owner: SimulationProgressOwner::SceneReferences,
            operation_id: 2,
        };
        app.world_mut()
            .resource_mut::<SimulationProgress>()
            .acquire(
                unrelated_reference_key,
                "waiting for unrelated rover reference",
            );
        app.world_mut()
            .resource_mut::<PendingRefSpawns>()
            .items
            .push(RefSpawn {
                progress_key: unrelated_reference_key,
                scene_id: stage_id,
                prim_path: "/World/Rover".into(),
                type_name: Some("Xform".into()),
                asset_path: "lunco://vessels/rover.usda".into(),
                reference_prim_path: None,
                ref_handle: None,
                translate: None,
                deferred_ops: Vec::new(),
                active: true,
                held: false,
                asset_ready: false,
                failure: None,
                failure_reported: false,
                removed: false,
            });
        crate::live_consume::publish_pending_stage_projections(app.world_mut());
        app.world_mut().flush();

        let events = &app.world().resource::<ProjectedDocumentEvents>().0;
        assert_eq!(events.len(), 1, "one document generation publishes once");
        let lunco_telemetry_core::TelemetryValue::Map(data) = &events[0] else {
            panic!("projection event data is not a map");
        };
        assert_eq!(
            data.get("changed_prim_paths"),
            Some(&lunco_telemetry_core::TelemetryValue::Array(vec![
                lunco_telemetry_core::TelemetryValue::String("/World/Route".into()),
                lunco_telemetry_core::TelemetryValue::String("/World/Route/W5".into()),
            ])),
            "the completion event includes every structural batch that was reconciled"
        );
        assert_eq!(
            app.world()
                .resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
                .synced_generation(doc),
            Some(generation)
        );
        let progress = app.world().resource::<SimulationProgress>();
        assert!(progress.contains(unrelated_reference_key));
        assert!(!progress.contains(document_key));
        assert!(
            progress.is_held(),
            "the document hold is released while the unrelated reference retains its own hold"
        );
    }

    #[test]
    fn drain_ref_spawns_commits_a_ready_successor_after_its_unready_prefix() {
        use bevy::asset::AssetApp;
        use bevy::prelude::*;
        use lunco_usd_bevy_stage::canonical::CanonicalStages;
        use lunco_usd_bevy_stage::read::UsdRead;
        use openusd::sdf::Path as SdfPath;

        const FIRST_REFERENCE: &str =
            "#usda 1.0\n(\n    defaultPrim = \"FirstAsset\"\n)\ndef Xform \"FirstAsset\"\n{\n}\n";
        const SECOND_REFERENCE: &str =
            "#usda 1.0\n(\n    defaultPrim = \"SecondAsset\"\n)\ndef Xform \"SecondAsset\"\n{\n}\n";

        let scene_recipe = lunco_usd_compose::recipe::StageRecipe::from_source("scene.usda", TINY);
        let mut app = App::new();
        crate::live_consume::install_live_prim_entity_index(&mut app);
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>()
            .init_non_send::<CanonicalStages>()
            .init_resource::<PendingRefSpawns>()
            .init_resource::<PendingDependentStageRefreshes>()
            .init_resource::<PendingInstanceProjections>()
            .init_resource::<SimulationProgress>()
            .add_systems(Update, mark_pending_ref_spawns);

        let scene_handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset::from_recipe(scene_recipe.clone()).expect("prepare scene"));
        let scene_id = scene_handle.id();
        let (first_reference_id, second_reference_id) = {
            let mut stages = app.world_mut().non_send_mut::<CanonicalStages>();
            let stage = stages
                .get_or_build(scene_id, &scene_recipe)
                .expect("open the live scene stage");
            (
                stage
                    .canonical_reference_id("first.usda")
                    .expect("valid first reference"),
                stage
                    .canonical_reference_id("second.usda")
                    .expect("valid second reference"),
            )
        };
        let first_reference_recipe = lunco_usd_compose::recipe::StageRecipe::from_source(
            first_reference_id,
            FIRST_REFERENCE,
        );
        let second_reference_recipe = lunco_usd_compose::recipe::StageRecipe::from_source(
            second_reference_id,
            SECOND_REFERENCE,
        );
        let first_handle = app.world_mut().resource_mut::<Assets<UsdStageAsset>>().add(
            UsdStageAsset::from_recipe(first_reference_recipe).expect("prepare first reference"),
        );
        let second_handle = app.world_mut().resource_mut::<Assets<UsdStageAsset>>().add(
            UsdStageAsset::from_recipe(second_reference_recipe).expect("prepare second reference"),
        );

        let root = app
            .world_mut()
            .spawn((
                UsdSceneRoot,
                UsdPrimPath {
                    stage_handle: scene_handle,
                    path: "/World".to_owned(),
                },
            ))
            .id();
        let mut mounts = lunco_core::SceneMountState::default();
        mounts.register_root(root, true);
        app.world_mut().insert_resource(mounts);

        // Consume insertion events before either authored reference operation
        // is admitted; the test below controls their completion order.
        app.update();
        app.update();

        let (first_key, second_key) = {
            let mut pending = app.world_mut().resource_mut::<PendingRefSpawns>();
            let first_key = pending
                .allocate_progress_key()
                .expect("first reference operation identity");
            let second_key = pending
                .allocate_progress_key()
                .expect("second reference operation identity");
            let make_spawn =
                |progress_key, prim_path: &str, asset_path: &str, ref_handle| RefSpawn {
                    progress_key,
                    scene_id,
                    prim_path: prim_path.to_owned(),
                    type_name: Some("Xform".to_owned()),
                    asset_path: asset_path.to_owned(),
                    reference_prim_path: None,
                    ref_handle: Some(ref_handle),
                    translate: None,
                    deferred_ops: Vec::new(),
                    active: true,
                    held: false,
                    asset_ready: false,
                    failure: None,
                    failure_reported: false,
                    removed: false,
                };
            pending.push(
                make_spawn(
                    first_key,
                    "/World/First",
                    "first.usda",
                    first_handle.clone(),
                ),
                false,
            );
            pending.push(
                make_spawn(
                    second_key,
                    "/World/Second",
                    "second.usda",
                    second_handle.clone(),
                ),
                false,
            );
            (first_key, second_key)
        };

        // The later authored operation completes first through the same event
        // reader used by the runtime.
        app.world_mut()
            .resource_mut::<Messages<bevy::asset::AssetEvent<UsdStageAsset>>>()
            .write(bevy::asset::AssetEvent::LoadedWithDependencies {
                id: second_handle.id(),
            });
        app.update();
        assert!(
            app.world()
                .resource::<PendingRefSpawns>()
                .ready
                .contains(&second_handle.id())
        );
        assert!(
            !app.world()
                .resource::<PendingRefSpawns>()
                .ready
                .contains(&first_handle.id())
        );

        drain_ref_spawns(app.world_mut());

        let stages = app.world().non_send::<CanonicalStages>();
        let stage = stages.get(scene_id).expect("live scene stage remains open");
        assert!(
            !stage
                .view()
                .has_prim(&SdfPath::new("/World/First").unwrap())
        );
        assert!(
            !stage
                .view()
                .has_prim(&SdfPath::new("/World/Second").unwrap())
        );
        let pending = app.world().resource::<PendingRefSpawns>();
        assert_eq!(pending.items.len(), 2);
        assert!(pending.items[0].held && pending.items[1].held);
        assert!(pending.items[1].asset_ready);
        assert!(app.world().resource::<SimulationProgress>().is_held());

        app.world_mut()
            .resource_mut::<Messages<bevy::asset::AssetEvent<UsdStageAsset>>>()
            .write(bevy::asset::AssetEvent::LoadedWithDependencies {
                id: first_handle.id(),
            });
        app.update();
        drain_ref_spawns(app.world_mut());

        let stage = app
            .world()
            .non_send::<CanonicalStages>()
            .get(scene_id)
            .expect("live scene stage remains open");
        assert!(
            stage
                .view()
                .has_prim(&SdfPath::new("/World/First").unwrap())
        );
        assert!(
            stage
                .view()
                .has_prim(&SdfPath::new("/World/Second").unwrap())
        );
        let changes = app
            .world_mut()
            .non_send_mut::<CanonicalStages>()
            .get_mut(scene_id)
            .expect("live scene stage remains open")
            .drain_changes();
        let reference_commit_order = changes
            .iter()
            .flat_map(|change| change.resynced.iter())
            .filter_map(|path| match path.to_string().as_str() {
                "/World/First" => Some(first_key.operation_id),
                "/World/Second" => Some(second_key.operation_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            reference_commit_order,
            [first_key.operation_id, second_key.operation_id]
        );
        let projections = app.world().resource::<PendingInstanceProjections>();
        assert!(
            projections
                .plans
                .contains_key(&(scene_id, "/World/First".into()))
        );
        assert!(
            projections
                .plans
                .contains_key(&(scene_id, "/World/Second".into()))
        );
        assert!(app.world().resource::<SimulationProgress>().is_held());
    }

    #[test]
    fn preview_reference_failure_does_not_hold_the_active_simulation() {
        let mut world = World::new();
        world.insert_resource(PendingRefSpawns::default());
        world.insert_resource(SimulationProgress::default());

        let active_root = world
            .spawn((
                UsdSceneRoot,
                UsdPrimPath {
                    stage_handle: Handle::default(),
                    path: "/World".to_owned(),
                },
            ))
            .id();
        let mut mounts = lunco_core::SceneMountState::default();
        mounts.register_root(active_root, true);
        world.insert_resource(mounts);

        let key = world
            .resource_mut::<PendingRefSpawns>()
            .allocate_progress_key()
            .expect("reference operation identity");
        report_reference_failure(
            &mut world,
            key,
            AssetId::invalid(),
            "/World/PreviewRover",
            "lunco://vessels/rover.usda",
            "referenced asset failed to load",
        );

        assert!(!world.resource::<SimulationProgress>().is_held());
        assert!(
            !world
                .get_resource::<lunco_core::RuntimeFaults>()
                .is_some_and(lunco_core::RuntimeFaults::active)
        );
        assert_eq!(
            world
                .resource::<lunco_core::RuntimeDiagnostics>()
                .findings
                .len(),
            1,
            "the owning document still receives a visible diagnostic"
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
                reference_prim_path: None,
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
