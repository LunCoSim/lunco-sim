//! Doc-backed twin default scene — web-ready via the twin asset source.
//!
//! The document projection lease publishes codec-correct base bytes and a
//! separate native runtime/view/session stack to the async [`UsdLoader`].
//! Native USD composition preserves source identity and opinion strength;
//! disposable view opinions have a separate live projection cursor.
//!
//! Flow:
//! 1. Twin loading policy dispatches `OpenTwinScene`. This owner loads exact
//!    source bytes through the Twin asset source and records `PendingTwinDocs`.
//! 2. `AsyncWorkAdmission` prepares the native source revision. The document
//!    registry commits it under its path identity and dirty-document policy.
//!    After runtime persistence restore, a worker captures the native stage
//!    input. Its generation fence guards lease publication and `LoadScene`.
//! 3. [`sync_twin_overlays`] — later document edits author typed deltas onto the
//!    live composed stage: translates and structural spawns/removes are
//!    authored onto the scene's [`CanonicalStage`](lunco_usd_bevy_stage::canonical::CanonicalStage)
//!    directly, firing its openusd change sink so `project_stage_changes`
//!    projects the edit in place — no whole-scene asset reload. The runtime
//!    persistence plugin saves coalesced runtime-layer snapshots off-thread;
//!    dependent stages are refreshed only when their composition uses the
//!    changed document. A reference batch admits its native closure before
//!    ordered authoring, and the native change sink owns bounded ECS projection.
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
use std::path::PathBuf;
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
use lunco_usd_bevy_stage::{UsdStageAsset, UsdStageProjectionPlan, source::UsdSourceBytes};
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
    handle: Handle<UsdSourceBytes>,
    twin: lunco_workspace::TwinId,
    /// Twin name (the `twin://` first segment).
    name: String,
    /// Scene path relative to the twin root (the `twin://` remainder).
    rel: String,
    /// On-disk absolute path — the document origin (Save target + dedup key).
    abs_path: PathBuf,
    /// Captured transport root, checked with the exact Twin ID and mount before
    /// publication. The admitted Twin ID owns retirement of this request.
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
    AwaitingStageInput {
        doc: DocumentId,
        generation: u64,
    },
    PreparingStageInput {
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
    StageInput(Result<lunco_usd_document::document::DocumentStageInput, String>),
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
    ready: HashSet<AssetId<UsdSourceBytes>>,
    failed: HashMap<AssetId<UsdSourceBytes>, String>,
    completions: Arc<Mutex<Vec<TwinDocCompletion>>>,
    next_operation: u64,
}

impl PendingTwinDocs {
    /// Queue a default twin scene for doc-backed projection.
    pub(crate) fn push(
        &mut self,
        handle: Handle<UsdSourceBytes>,
        twin: lunco_workspace::TwinId,
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
            twin,
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

    fn mark_ready(&mut self, id: AssetId<UsdSourceBytes>) {
        if self.items.iter().any(|item| item.handle.id() == id) {
            self.ready.insert(id);
        }
    }

    pub(crate) fn mark_failed(&mut self, id: AssetId<UsdSourceBytes>, error: String) {
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
            TwinDocPreparationStage::AwaitingStageInput { .. } => {
                item.capacity_revision != Some(capacity_revision)
            }
            TwinDocPreparationStage::PreparingSource { .. }
            | TwinDocPreparationStage::PreparingStageInput { .. }
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
    pub(crate) fn release_twin(&mut self, twin: lunco_workspace::TwinId) -> Vec<AsyncWorkKey> {
        let mut cancelled = Vec::new();
        self.items.retain(|item| {
            let keep = item.twin != twin;
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
                | TwinDocPreparationStage::PreparingStageInput { operation, .. } => Some(operation),
                TwinDocPreparationStage::AwaitingSource
                | TwinDocPreparationStage::AwaitingStageInput { .. }
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

/// Native reference closure preparation and retained asset residency.
#[derive(Resource, Default)]
pub(crate) struct ReferenceLayerInputs {
    pub(crate) native: crate::native_references::NativeReferencePreparations,
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

/// Validate a captured reference closure through its installed Rhai policy.
/// Every projection path consumes this decision before publishing native bytes.
fn validate_reference_composition(
    world: &World,
    scene_id: AssetId<UsdStageAsset>,
    asset_path: &str,
    recipe: &lunco_usd_compose::recipe::StageRecipe,
) -> Result<(), String> {
    let Some(first_missing) = recipe.dependency_diagnostics.first() else {
        return Ok(());
    };
    match evaluate_reference_composition_policy(world, scene_id, &recipe.dependency_diagnostics) {
        Ok(IncompleteCompositionDecision::AllowPartial) => Ok(()),
        Ok(IncompleteCompositionDecision::RejectScene) => Err(format!(
            "the `{INCOMPLETE_COMPOSITION_POLICY_HOOK}` policy rejected referenced asset `{asset_path}` with {} unresolved USD composition dependencies; first: {first_missing}",
            recipe.dependency_diagnostics.len()
        )),
        Err(error) => Err(format!(
            "the `{INCOMPLETE_COMPOSITION_POLICY_HOOK}` policy could not admit referenced asset `{asset_path}`: {error}"
        )),
    }
}

/// Clear native reference preparation and projection owned by the outgoing scene.
///
/// Pending default-scene document loads are owned by their admitted Twin and
/// are released through [`PendingTwinDocs::release_twin`] when that Twin closes.
pub(crate) fn reset_scene_projection_state(
    mut pending_refs: ResMut<ReferenceLayerInputs>,
    mut pending_document_projections: Option<ResMut<PendingDocumentProjectionAdmissions>>,
    mut pending_dependent_refreshes: Option<ResMut<PendingDependentStageRefreshes>>,
    mut pending_stage_projections: Option<ResMut<crate::live_consume::PendingStageProjections>>,
    mut admission: Option<ResMut<AsyncWorkAdmission>>,
    mut pending_native_paths: Option<ResMut<crate::native_assets::PendingNativeAssetPaths>>,
    mut progress: Option<ResMut<SimulationProgress>>,
) {
    if let Some(progress) = progress.as_deref_mut() {
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
    pending_refs.retained_assets.clear();
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
    mut events: MessageReader<bevy::asset::AssetEvent<UsdSourceBytes>>,
    mut failures: MessageReader<bevy::asset::AssetLoadFailedEvent<UsdSourceBytes>>,
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

/// Native source preparation consumes the asset owner's terminal signals.
pub(crate) fn mark_reference_layer_inputs(
    mut inputs: ResMut<ReferenceLayerInputs>,
    mut events: MessageReader<bevy::asset::AssetEvent<UsdStageAsset>>,
    mut failures: MessageReader<bevy::asset::AssetLoadFailedEvent<UsdStageAsset>>,
) {
    for event in events.read() {
        match event {
            bevy::asset::AssetEvent::Added { id }
            | bevy::asset::AssetEvent::Modified { id }
            | bevy::asset::AssetEvent::LoadedWithDependencies { id } => {
                inputs.native.mark_asset(*id)
            }
            bevy::asset::AssetEvent::Removed { id } | bevy::asset::AssetEvent::Unused { id } => {
                inputs
                    .native
                    .mark_failed(*id, "native reference asset was retired before admission")
            }
        }
    }
    for failure in failures.read() {
        inputs
            .native
            .mark_failed(failure.id, &failure.error.to_string());
    }
}

pub(crate) fn native_reference_inputs_ready(
    inputs: Res<ReferenceLayerInputs>,
    admission: Res<AsyncWorkAdmission>,
) -> bool {
    inputs.native.needs_work(admission.capacity_revision())
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
    sources: Res<Assets<UsdSourceBytes>>,
    asset_server: Res<AssetServer>,
    twin_roots: Res<TwinRoots>,
    mut workspace: Option<ResMut<lunco_workspace::WorkspaceResource>>,
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
        if !workspace.as_deref().is_some_and(|workspace| {
            workspace
                .twin(item.twin)
                .is_some_and(|twin| twin.root == item.root)
        }) || !twin_roots
            .root_for(&item.name)
            .is_ok_and(|root| root.as_ref() == Some(&item.root))
        {
            item.stage = TwinDocPreparationStage::Finished;
            if let Some(key) = item.work_key.take() {
                admission.cancel_queued(key);
            }
            warn!(
                "[usd-e1b] refusing source completion for retired Twin {}",
                item.twin.raw()
            );
            continue;
        }
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
                        .map(|UsdSourceBytes(source)| source.as_slice());
                    if current_source != Some(prepared.source_bytes()) {
                        item.stage = TwinDocPreparationStage::AwaitingSource;
                        continue;
                    }
                    let runtime = lunco_workspace::DocumentRuntimeOwner::LocalTwin(item.twin);
                    if let Some(doc) = registry.doc_for_file(&item.abs_path)
                        && registry
                            .host(doc)
                            .is_some_and(|host| host.document().is_dirty())
                        && workspace.as_deref().is_some_and(|workspace| {
                            workspace
                                .document(doc)
                                .is_none_or(|entry| entry.runtime_context != runtime)
                        })
                    {
                        report_twin_doc_load_failed(
                            &mut empty_reason,
                            &mut commands,
                            &lunco_assets_core::twin_uri(&item.name, &item.rel),
                            "dirty resident USD source belongs to a different runtime owner",
                        );
                        item.stage = TwinDocPreparationStage::Finished;
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
                                "the source asset is not valid USD; refusing to mount a stale document",
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
                    if let Some(workspace) = workspace.as_deref_mut()
                        && let Some(host) = registry.host(doc)
                    {
                        let origin = host.document().origin().clone();
                        workspace.add_document(lunco_workspace::DocumentEntry {
                            id: doc,
                            kind: lunco_workspace::DocumentKindId::new("usd"),
                            title: origin.display_name(),
                            origin,
                            runtime_context: runtime,
                            dirty: host.document().is_dirty(),
                        });
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
                    backed.track(doc, item.twin, item.name.clone(), item.rel.clone());
                    item.doc = Some(doc);
                    item.stage = TwinDocPreparationStage::AwaitingStageInput {
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
                TwinDocPreparationStage::PreparingStageInput {
                    operation,
                    doc,
                    generation,
                },
                TwinDocCompletionResult::StageInput(result),
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
                    item.stage = TwinDocPreparationStage::AwaitingStageInput {
                        doc,
                        generation: current_generation,
                    };
                    continue;
                }
                let twin_path = lunco_assets_core::twin_uri(&item.name, &item.rel);
                let input = match result {
                    Ok(input) => input,
                    Err(error) => {
                        report_twin_doc_load_failed(
                            &mut empty_reason,
                            &mut commands,
                            &twin_path,
                            format!("native USD layer preparation failed: {error}"),
                        );
                        item.stage = TwinDocPreparationStage::Failed;
                        continue;
                    }
                };
                if let Err(error) =
                    backed.publish_stage_input(doc, &twin_roots, &asset_server, input)
                {
                    report_twin_doc_load_failed(
                        &mut empty_reason,
                        &mut commands,
                        &twin_path,
                        error,
                    );
                    item.stage = TwinDocPreparationStage::Failed;
                    continue;
                }
                wake.wake();
                backed.mark_initial_projection(doc, generation);
                info!(
                    "[usd-e1b] default scene `{twin_path}` is doc-backed ({doc}) — mounting native layers"
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
        if !workspace.as_deref().is_some_and(|workspace| {
            workspace
                .twin(item.twin)
                .is_some_and(|twin| twin.root == item.root)
        }) || !twin_roots
            .root_for(&item.name)
            .is_ok_and(|root| root.as_ref() == Some(&item.root))
        {
            if let Some(key) = item.work_key.take() {
                admission.cancel_queued(key);
            }
            continue;
        }
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
                let Some(UsdSourceBytes(source)) = sources.get(&item.handle) else {
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
                let key = twin_usd_work_key(&item, lunco_hash::fnv1a64(source), operation);
                let completions = Arc::clone(&pending.completions);
                let source = source.clone();
                let job = move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        PreparedUsdSource::from_bytes(source)
                    }))
                    .map_err(|_| "USD decoder panicked".to_owned())
                    .and_then(|result| result);
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
            TwinDocPreparationStage::AwaitingStageInput { doc, generation: _ } => {
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
                let root_id = twin_path.clone();
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
                            .capture_stage_input(&root_id, false)
                            .map_err(|error| error.to_string())
                    }))
                    .unwrap_or_else(|_| Err("native USD layer preparation panicked".to_owned()));
                    completions
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(TwinDocCompletion {
                            operation,
                            result: TwinDocCompletionResult::StageInput(result),
                        });
                };
                match admission.submit(AsyncWorkPriority::SimulationRequired, key, job) {
                    Ok(()) | Err(lunco_core_runtime::AsyncWorkRejection::DuplicateKey) => {
                        item.stage = TwinDocPreparationStage::PreparingStageInput {
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
                            TwinDocPreparationStage::AwaitingStageInput { doc, generation };
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
            | TwinDocPreparationStage::PreparingStageInput { .. } => still.push(item),
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
            | TwinDocPreparationStage::PreparingStageInput { operation, .. } => Some(operation),
            TwinDocPreparationStage::AwaitingSource
            | TwinDocPreparationStage::AwaitingStageInput { .. }
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
/// Publish the document's native input under its admitted Twin
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
    let root_id = lunco_assets_core::twin_uri(name, rel);
    let input = world
        .resource::<DocumentRegistry<UsdDocument>>()
        .host(doc)
        .ok_or_else(|| format!("document {doc} is not open"))
        .and_then(|host| {
            host.document()
                .capture_stage_input(&root_id, false)
                .map_err(|error| error.to_string())
        });
    let input = match input {
        Ok(input) => input,
        Err(error) => {
            warn!("[usd-e1b] {error}");
            return false;
        }
    };
    let roots = world.resource::<TwinRoots>().clone();
    let server = world.resource::<AssetServer>().clone();
    let result = world
        .resource_mut::<DocBackedTwinScenes>()
        .publish_stage_input(doc, &roots, &server, input);
    if let Err(error) = result {
        warn!("[usd-e1b] {error}");
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
        // Restart owns the prepared replacement of the active document. Its
        // refreshed source is already in the resolver overlay; replaying the
        // reset into the outgoing canonical stage would bypass preparation.
        if active_twin_document
            && world.contains_resource::<crate::scene::SceneLoadInFlight>()
            && world
                .resource::<lunco_core::SceneTransitionCoordinator>()
                .active()
                .is_some_and(|transition| {
                    matches!(transition, lunco_core::SceneTransition::Restart { .. })
                })
        {
            continue;
        }
        // Read the generation before any whole-stage payload. The composed source
        // is serialized only when this event-driven owner observes a new
        // generation, never on the render loop.
        let (cur_gen, pending_ops) = {
            let Some(host) = world.resource::<DocumentRegistry<UsdDocument>>().host(doc) else {
                // DocumentClosed owns projection bookkeeping and private mount retirement.
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
            // A journal gap requires the complete native authored stack;
            // its admitted dependency closure preserves source identities.
            None => {
                let _span =
                    bevy::log::info_span!("usd_twin_projection_rebuild_history_gap").entered();
                if !rebuild_scene_from_document(world, scene_id, doc) {
                    continue;
                }
            }
            Some(ops) if ops.iter().any(op_needs_rebuild) => {
                let _span =
                    bevy::log::info_span!("usd_twin_projection_rebuild_coarse_ops").entered();
                if !ensure_reference_layers_for_projection(world, scene_id, &ops) {
                    // Keep the document generation pending until every new
                    // reference closure is available to the live resolver.
                    // Rebuilding first would permanently open a stage whose root
                    // source contains the arc but whose resolver cannot resolve
                    // it; a later variant/metadata edit would preserve the
                    // incomplete composition.
                    continue;
                }
                if !rebuild_scene_from_document(world, scene_id, doc) {
                    continue;
                }
            }
            // Incremental: replay each op's typed delta onto the live stage.
            // The runtime persistence owner snapshots authored edits
            // asynchronously; the in-memory Twin overlay is rebuilt on the
            // next mount rather than serializing the whole stage here.
            Some(ops) => {
                // Admit the batch's complete reference closure before any
                // native changes can project its sibling relationships/joints.
                // Worker completion must not expose an intermediate topology.
                if !ensure_reference_layers_for_projection(world, scene_id, &ops) {
                    continue;
                }
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
        .resource::<ReferenceLayerInputs>()
        .retained_assets
        .iter()
        .filter(|((stage, _), _)| *stage == changed_scene)
        .map(|(_, handle)| (handle.id(), handle.clone()))
        .collect::<HashMap<_, _>>();
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
            'resolve: {
                let mut recipes = Vec::with_capacity(reference_handles.len());
                for handle in &reference_handles {
                    if let Some(recipe) = assets
                        .get(handle.id())
                        .and_then(|asset| asset.recipe.as_ref())
                    {
                        recipes.push(Arc::clone(recipe));
                    } else if let ReferenceAssetState::Failed(error) =
                        reference_asset_state(world, handle.id())
                    {
                        break 'resolve DependentReferenceRecipes::Failed(error);
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
/// `CanonicalStage` — values come straight from typed ops; property removal projects the
/// document’s newly resolved opinion because the live root combines its layers.
/// Firing the openusd sink lets
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
            | UsdOp::RestoreProperty { path, .. }
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
            | UsdOp::RemoveRelationship { path, .. }
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

fn document_projection_source(
    world: &World,
    scene_id: AssetId<UsdStageAsset>,
    edit_target: &lunco_usd_document::document::LayerId,
) -> Result<Arc<openusd::sdf::Data>, String> {
    let document = world
        .get_resource::<DocBackedTwinScenes>()
        .zip(world.get_resource::<AssetServer>())
        .and_then(|(backed, assets)| scene_document_for(backed, assets, scene_id))
        .and_then(|doc| {
            world
                .get_resource::<DocumentRegistry<UsdDocument>>()?
                .host(doc)
        })
        .ok_or_else(|| "live property projection has no authoritative document".to_owned())?;
    if let Some(error) = document.document().parse_error() {
        return Err(format!(
            "live property projection has invalid document source: {error}"
        ));
    }
    document
        .document()
        .authored_layer(edit_target)
        .map_err(|error| error.to_string())
}

fn apply_incremental_op_to_stage(world: &mut World, scene_id: AssetId<UsdStageAsset>, op: &UsdOp) {
    let _span = bevy::log::info_span!("usd_twin_projection_apply_incremental_op").entered();
    use lunco_usd_bevy_stage::canonical::CanonicalStages;

    let native_stage = world
        .get_non_send::<CanonicalStages>()
        .and_then(|stages| stages.get(scene_id))
        .map(|stage| stage.stage().clone());
    let _edit_context = match native_stage.as_ref() {
        Some(stage) => {
            let root_id = stage.root_layer().identifier().to_owned();
            let ids = lunco_usd_authoring::author::DocumentLayerIds::for_root(&root_id);
            let target = op.edit_target();
            let identifier = if target.is_root() {
                ids.base
            } else if target.is_runtime() {
                ids.runtime
            } else if target.is_view() {
                ids.view
            } else {
                report_stage_projection_reset_failure(
                    world,
                    scene_id,
                    format!("unknown USD projection target {}", target.as_str()),
                );
                return;
            };
            match stage.edit_context(openusd::usd::EditTarget::for_layer(identifier)) {
                Ok(context) => Some(context),
                Err(error) => {
                    report_stage_projection_reset_failure(
                        world,
                        scene_id,
                        format!("native document edit target unavailable: {error}"),
                    );
                    return;
                }
            }
        }
        None => {
            report_stage_projection_reset_failure(
                world,
                scene_id,
                "native stage missing for document projection".to_owned(),
            );
            return;
        }
    };

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
                .map(|stage| {
                    let source = document_projection_source(world, scene_id, op.edit_target())?;
                    stage
                        .projector()
                        .reconcile_document_property(&sp, name, &source)
                        .map_err(|error| error.to_string())
                });
            match removed {
                Some(Ok(())) => refresh_prim_subtree(world, scene_id, path),
                Some(Err(error)) => report_stage_projection_reset_failure(
                    world,
                    scene_id,
                    format!("remove attribute {path}.{name}: {error}"),
                ),
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
        UsdOp::RestoreProperty { path, name, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let restored = world
                .get_non_send::<CanonicalStages>()
                .and_then(|stages| stages.get(scene_id))
                .map(|stage| {
                    let source = document_projection_source(world, scene_id, op.edit_target())?;
                    stage
                        .projector()
                        .reconcile_document_property(&sp, name, &source)
                        .map_err(|error| error.to_string())
                });
            match restored {
                Some(Ok(())) => refresh_relationship_dependents(world, scene_id, path, name),
                Some(Err(error)) => report_stage_projection_reset_failure(
                    world,
                    scene_id,
                    format!("restore property {path}.{name}: {error}"),
                ),
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
        UsdOp::RemoveRelationship { path, name, .. } => {
            let Ok(sp) = openusd::sdf::Path::new(path) else {
                return;
            };
            let removed = world
                .get_non_send::<CanonicalStages>()
                .and_then(|stages| stages.get(scene_id))
                .map(|stage| {
                    let source = document_projection_source(world, scene_id, op.edit_target())?;
                    stage
                        .projector()
                        .reconcile_document_property(&sp, name, &source)
                        .map_err(|error| error.to_string())
                });
            match removed {
                Some(Ok(())) => refresh_relationship_dependents(world, scene_id, path, name),
                Some(Err(error)) => report_stage_projection_reset_failure(
                    world,
                    scene_id,
                    format!("remove relationship {path}.{name}: {error}"),
                ),
                None => {}
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
            if !cleared {
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

/// Author a spawn onto the live native stage using its admitted closure.
/// The native arc is visible to every subsequent opinion in the same ordered
/// document batch; its change sink owns bounded ECS materialization.
fn spawn_prim_op(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    prim_path: &str,
    type_name: Option<String>,
    reference: Option<String>,
    reference_prim_path: Option<String>,
) {
    let result = (|| -> Result<(), String> {
        let stages = world
            .get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
            .ok_or_else(|| "native stage registry missing".to_owned())?;
        let stage = stages
            .get(scene_id)
            .ok_or_else(|| "native stage missing for document projection".to_owned())?;
        let path = openusd::sdf::Path::new(prim_path).map_err(|error| error.to_string())?;
        if let Some(asset) = reference.as_deref() {
            if !asset.is_empty() {
                let id = stage
                    .canonical_reference_id(asset)
                    .map_err(|error| error.to_string())?;
                if !stage.has_layer_bytes(&id) {
                    return Err(format!(
                        "reference closure `{id}` was not admitted before its document batch"
                    ));
                }
            }
            stage
                .projector()
                .author_referenced_prim(
                    &path,
                    type_name.as_deref(),
                    asset,
                    reference_prim_path
                        .as_deref()
                        .filter(|path| !path.is_empty()),
                )
                .map_err(|error| error.to_string())
        } else {
            stage
                .projector()
                .author_prim(&path, type_name.as_deref())
                .map_err(|error| error.to_string())
        }
    })();
    if let Err(error) = result {
        report_stage_projection_reset_failure(world, scene_id, error);
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

pub(crate) fn report_stage_projection_reset_failure(
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

/// Replace the live stage with the document's exact native authored stack and
/// admitted dependency closure. The replacement's root notice owns address
/// admission and ECS projection before document synchronization completes.
fn rebuild_scene_from_document(
    world: &mut World,
    scene_id: AssetId<UsdStageAsset>,
    doc: DocumentId,
) -> bool {
    use lunco_usd_bevy_stage::canonical::{CanonicalStage, CanonicalStages};
    let snapshot = {
        let _span = bevy::log::info_span!("usd_twin_rebuild_snapshot_live_layers").entered();
        let Some(cs) = world
            .get_non_send::<CanonicalStages>()
            .and_then(|s| s.get(scene_id))
        else {
            return false;
        };
        cs.admitted_recipe()
    };
    let replacement = match {
        let _span = bevy::log::info_span!("usd_twin_rebuild_prepare_live_stage").entered();
        snapshot.and_then(|recipe| {
            let document = world
                .resource::<DocumentRegistry<UsdDocument>>()
                .host(doc)
                .ok_or_else(|| {
                    std::io::Error::other(format!("document {doc} closed before native rebuild"))
                })?;
            let input = lunco_usd_authoring::author::document_stage_recipe(
                document.document().data(),
                document.document().runtime_data(),
                document.document().view_data(),
                Some(&recipe),
            )?;
            CanonicalStage::from_recipe(&input)
        })
    } {
        Ok(stage) => stage,
        Err(error) => {
            report_stage_projection_reset_failure(
                world,
                scene_id,
                format!("native document stack could not rebuild the live stage: {error}"),
            );
            return false;
        }
    };
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
        // The replacement's root notice enters native address admission before
        // the live consumer resets and reprojects stage-owned ECS state.
        true
    } else {
        report_stage_projection_reset_failure(
            world,
            scene_id,
            "prepared native document stack could not replace the canonical stage".to_owned(),
        );
        false
    }
}

/// Make every newly referenced asset in an edit batch available to the live
/// stage before applying its ordered opinions. The async `UsdStageAsset` loader owns
/// the available transitive layer closure; this only transfers those bytes into
/// the existing canonical resolver. If a closure is still loading, leave the
/// document generation unsynced so the asset event resumes the same batch
/// with the available resolver instead of publishing a stage before its root
/// asset is ready.
fn ensure_reference_layers_for_projection(
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
            } if !reference.is_empty() => Some(reference.clone()),
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
                    format!("invalid USD projection reference {asset_path:?}: {error}"),
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
                    if let Err(detail) = validate_reference_composition(
                        world,
                        scene_id,
                        &asset_path,
                        &prepared.snapshot.recipe,
                    ) {
                        report_stage_projection_reset_failure(world, scene_id, detail);
                        return false;
                    }
                    extra.extend(prepared.snapshot.recipe.bytes.clone());
                    world
                        .resource_mut::<ReferenceLayerInputs>()
                        .retained_assets
                        .insert((scene_id, reference_id.clone()), prepared.handle);
                    admitted_native.push(reference_id);
                }
                Ok(None) => return false,
                Err(error) => {
                    report_stage_projection_reset_failure(
                        world,
                        scene_id,
                        format!("native USD projection reference {asset_path:?} failed: {error}"),
                    );
                    return false;
                }
            }
            continue;
        }
        let handle = if let Some(handle) = world
            .resource::<ReferenceLayerInputs>()
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
                        format!("invalid USD projection reference {asset_path:?}: {error}"),
                    );
                    return false;
                }
            };
            let handle = world.resource::<AssetServer>().load::<UsdStageAsset>(path);
            world
                .resource_mut::<ReferenceLayerInputs>()
                .retained_assets
                .insert((scene_id, reference_id.clone()), handle.clone());
            handle
        };
        if let ReferenceAssetState::Failed(error) = reference_asset_state(world, handle.id()) {
            report_stage_projection_reset_failure(
                world,
                scene_id,
                format!("USD projection reference `{asset_path}` failed: {error}"),
            );
            return false;
        }
        let Some(asset) = world.resource::<Assets<UsdStageAsset>>().get(handle.id()) else {
            return false;
        };
        let Some(recipe) = asset.recipe.as_ref() else {
            report_stage_projection_reset_failure(
                world,
                scene_id,
                format!("referenced asset `{asset_path}` has no native layer recipe"),
            );
            return false;
        };
        if let Err(detail) = validate_reference_composition(world, scene_id, &asset_path, recipe) {
            report_stage_projection_reset_failure(world, scene_id, detail);
            return false;
        }
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
                .resource_mut::<ReferenceLayerInputs>()
                .native
                .retire_input(scene_id, &reference);
        }
    }
    admitted
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_usd_bevy_scene::UsdSceneProjected;
    use lunco_usd_document::document::{LayerId, UsdOp};

    const TINY: &str = "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n}\n";

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
    fn source_asset_events_advance_pending_twin_docs() {
        let mut app = App::new();
        app.init_resource::<PendingTwinDocs>()
            .add_message::<bevy::asset::AssetEvent<UsdSourceBytes>>()
            .add_message::<bevy::asset::AssetLoadFailedEvent<UsdSourceBytes>>()
            .add_systems(Update, mark_pending_twin_docs);
        let handle = Handle::<UsdSourceBytes>::default();
        app.world_mut().resource_mut::<PendingTwinDocs>().push(
            handle.clone(),
            lunco_workspace::TwinId::new(1),
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
            .resource_mut::<Messages<bevy::asset::AssetEvent<UsdSourceBytes>>>()
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
        let handle = Handle::<UsdSourceBytes>::default();
        pending.push(
            handle.clone(),
            lunco_workspace::TwinId::new(1),
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
        let handle = Handle::<UsdSourceBytes>::default();
        pending.push(
            handle.clone(),
            lunco_workspace::TwinId::new(1),
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
        let handle = Handle::<UsdSourceBytes>::default();
        pending.push(
            handle.clone(),
            lunco_workspace::TwinId::new(1),
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
    fn scene_teardown_preserves_incoming_twin_admission() {
        let mut app = App::new();
        app.init_resource::<PendingTwinDocs>()
            .init_resource::<ReferenceLayerInputs>()
            .add_systems(lunco_core::SceneTeardown, reset_scene_projection_state);
        app.world_mut().resource_mut::<PendingTwinDocs>().push(
            Handle::default(),
            lunco_workspace::TwinId::new(1),
            false,
            "incoming".into(),
            "scene.usda".into(),
            PathBuf::from("/twins/incoming/scene.usda"),
            PathBuf::from("/twins/incoming"),
        );

        lunco_core::run_scene_teardown(app.world_mut());

        assert_eq!(app.world().resource::<PendingTwinDocs>().items.len(), 1);
    }
}
