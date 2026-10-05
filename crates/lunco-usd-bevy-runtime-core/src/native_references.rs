//! Native USD reference preparation in the existing reference-admission lane.

use bevy::asset::AssetPath;
use bevy::prelude::*;
use lunco_assets_core::asset_path::{PreparedAssetPaths, load_asset_path};
use lunco_core_runtime::{AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority};
use lunco_usd_bevy_stage::canonical::CanonicalStages;
use lunco_usd_bevy_stage::{
    UsdNativeReferenceSource, UsdReferenceSnapshot, UsdStageAsset, UsdStageProjectionPlan,
};
use lunco_usd_compose::recipe::StageRecipe;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

type ReferenceKey = (AssetId<UsdStageAsset>, String);

#[derive(Clone)]
pub(crate) struct PreparedReference {
    pub(crate) handle: Handle<UsdStageAsset>,
    pub(crate) snapshot: Arc<UsdReferenceSnapshot>,
}

#[derive(Clone, PartialEq, Eq)]
struct Owner {
    identity: u64,
    generation: u64,
    origin: Option<AssetPath<'static>>,
}

impl Owner {
    fn capture(world: &World, stage: AssetId<UsdStageAsset>) -> Result<Self, String> {
        let current = world
            .get_non_send::<CanonicalStages>()
            .and_then(|stages| stages.get(stage))
            .ok_or_else(|| "the owning reference stage is unavailable".to_owned())?;
        Ok(Self {
            identity: current.identity(),
            generation: current.generation(),
            origin: world
                .resource::<AssetServer>()
                .get_path(stage)
                .map(AssetPath::into_owned),
        })
    }
}

enum Phase {
    Address,
    Loading {
        handle: Handle<UsdStageAsset>,
        paths: PreparedAssetPaths,
    },
    Preparing {
        handle: Handle<UsdStageAsset>,
        source_recipe: Arc<StageRecipe>,
        source_plan: Arc<UsdStageProjectionPlan>,
        paths: PreparedAssetPaths,
    },
    Ready {
        prepared: PreparedReference,
        source_recipe: Arc<StageRecipe>,
        source_plan: Arc<UsdStageProjectionPlan>,
        paths: PreparedAssetPaths,
    },
    Failed(String),
}

struct Entry {
    owner: Owner,
    phase: Phase,
    work: Option<(AsyncWorkKey, u64)>,
    capacity_revision: Option<u64>,
}

impl Entry {
    fn accepts(&self, operation: u64, owner: Option<&Owner>) -> bool {
        self.work.as_ref().map(|(_, current)| *current) == Some(operation)
            && owner == Some(&self.owner)
    }
}

enum Outcome {
    Failed(String),
    Address(Result<(AssetPath<'static>, PreparedAssetPaths), String>),
    Plan(Result<(Arc<StageRecipe>, Arc<UsdStageProjectionPlan>), String>),
}

struct Completion {
    key: ReferenceKey,
    operation: u64,
    outcome: Outcome,
}

/// One shared preparation per exact stage/reference. Spawn and coarse rebuild
/// consumers retain their existing operation/projection holds until admission.
#[derive(Default)]
pub(crate) struct NativeReferencePreparations {
    entries: HashMap<ReferenceKey, Entry>,
    completions: Arc<Mutex<Vec<Completion>>>,
    completion_ready: Arc<AtomicBool>,
    next_operation: u64,
    dirty: bool,
    retired: Vec<AsyncWorkKey>,
}

impl NativeReferencePreparations {
    pub(crate) fn clear(&mut self, admission: Option<&mut AsyncWorkAdmission>) {
        if let Some(admission) = admission {
            for key in self.retired.drain(..) {
                admission.cancel_queued(key);
            }
            for entry in self.entries.values() {
                if let Some((key, _)) = entry.work {
                    admission.cancel_queued(key);
                }
            }
        }
        self.entries.clear();
        self.retired.clear();
        self.completions = Arc::new(Mutex::new(Vec::new()));
        self.completion_ready = Arc::new(AtomicBool::new(false));
        self.dirty = false;
    }

    pub(crate) fn mark_asset(&mut self, id: AssetId<UsdStageAsset>) {
        if self.entries.values().any(|entry| match &entry.phase {
            Phase::Loading { handle, .. } | Phase::Preparing { handle, .. } => handle.id() == id,
            Phase::Ready { prepared, .. } => prepared.handle.id() == id,
            _ => false,
        }) {
            self.dirty = true;
        }
    }

    pub(crate) fn mark_failed(&mut self, id: AssetId<UsdStageAsset>, error: &str) {
        for entry in self.entries.values_mut() {
            let matches = match &entry.phase {
                Phase::Loading { handle, .. } | Phase::Preparing { handle, .. } => {
                    handle.id() == id
                }
                Phase::Ready { prepared, .. } => prepared.handle.id() == id,
                _ => false,
            };
            if matches {
                if let Some((key, _)) = entry.work.take() {
                    self.retired.push(key);
                }
                entry.phase = Phase::Failed(error.to_owned());
                self.dirty = true;
            }
        }
    }

    pub(crate) fn retire_input(&mut self, stage: AssetId<UsdStageAsset>, reference: &str) {
        if !is_native(reference) {
            return;
        }
        let Ok(reference) = lunco_usd_compose::canonicalize_at(reference, None) else {
            return;
        };
        if let Some(entry) = self.entries.remove(&(stage, reference)) {
            if let Some((key, _)) = entry.work {
                self.retired.push(key);
            }
            self.dirty = true;
        }
    }

    pub(crate) fn needs_work(&self, capacity_revision: u64) -> bool {
        self.dirty
            || self.completion_ready.load(Ordering::Acquire)
            || self.entries.values().any(|entry| {
                entry
                    .capacity_revision
                    .is_some_and(|previous| previous != capacity_revision)
            })
    }

    fn request(
        &mut self,
        key: ReferenceKey,
        owner: Owner,
    ) -> Result<Option<PreparedReference>, String> {
        let entry = self.entries.entry(key).or_insert_with(|| {
            self.dirty = true;
            Entry {
                owner: owner.clone(),
                phase: Phase::Address,
                work: None,
                capacity_revision: None,
            }
        });
        if entry.owner != owner {
            self.dirty = true;
            return Ok(None);
        }
        match &entry.phase {
            Phase::Ready { prepared, .. } => Ok(Some(prepared.clone())),
            Phase::Failed(error) => Err(error.clone()),
            _ => Ok(None),
        }
    }
}

pub(crate) fn is_native(reference: &str) -> bool {
    reference
        .split_once(':')
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("file"))
}

fn run_worker(worker: impl FnOnce() -> Outcome, phase: &str) -> Outcome {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker)) {
        Ok(outcome) => outcome,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            Outcome::Failed(format!(
                "native USD reference {phase} preparation panicked: {detail}"
            ))
        }
    }
}

/// Admit a native reference without native filesystem work on the caller.
pub(crate) fn reference(
    world: &mut World,
    stage: AssetId<UsdStageAsset>,
    reference: &str,
) -> Result<Option<PreparedReference>, String> {
    let owner = Owner::capture(world, stage)?;
    // A live sibling retains the immutable preparation. Resolve only the actual
    // loaded transport handle; no address I/O or re-composition is needed here.
    let cached_path = world
        .get_non_send::<CanonicalStages>()
        .and_then(|stages| stages.get(stage))
        .and_then(|stage| stage.reference_source_path(reference, owner.origin.as_ref()));
    if let Some(path) = cached_path
        && let Some(handle) = world
            .resource::<AssetServer>()
            .get_handle::<UsdStageAsset>(path)
        && let Some((recipe, plan)) = current_source(world, &handle)?
    {
        let snapshot = world
            .get_non_send::<CanonicalStages>()
            .and_then(|stages| stages.get(stage))
            .and_then(|stage| {
                stage.reference_snapshot(reference, owner.origin.as_ref(), &recipe, &plan)
            });
        if let Some(snapshot) = snapshot {
            let source = snapshot
                .native_source
                .as_ref()
                .ok_or_else(|| "cached native reference has no admitted source".to_owned())?;
            source
                .address_paths
                .validate_owner(world.get_resource::<lunco_assets_core::TwinRoots>())
                .map_err(|error| error.to_string())?;
            if let Some(paths) = snapshot.plan.native_asset_paths_snapshot() {
                paths
                    .validate_owner(world.get_resource::<lunco_assets_core::TwinRoots>())
                    .map_err(|error| error.to_string())?;
            }
            return Ok(Some(PreparedReference { handle, snapshot }));
        }
    }
    let reference_key = (stage, reference.to_owned());
    let ready = world
        .resource::<crate::twin_projection::PendingRefSpawns>()
        .native
        .entries
        .get(&reference_key)
        .and_then(|entry| match &entry.phase {
            Phase::Ready {
                prepared,
                source_recipe,
                source_plan,
                paths,
            } => Some((
                prepared.clone(),
                Arc::clone(source_recipe),
                Arc::clone(source_plan),
                paths.clone(),
            )),
            _ => None,
        });
    if let Some((prepared, source_recipe, source_plan, paths)) = ready {
        let next_phase = match paths
            .validate_owner(world.get_resource::<lunco_assets_core::TwinRoots>())
        {
            Err(error) => Some(Phase::Failed(error.to_string())),
            Ok(()) => match current_source(world, &prepared.handle) {
                Ok(Some((recipe, plan)))
                    if Arc::ptr_eq(&recipe, &source_recipe) && Arc::ptr_eq(&plan, &source_plan) =>
                {
                    None
                }
                Ok(_) => Some(Phase::Loading {
                    handle: prepared.handle,
                    paths,
                }),
                Err(error) => Some(Phase::Failed(error)),
            },
        };
        if let Some(phase) = next_phase {
            let mut pending = world.resource_mut::<crate::twin_projection::PendingRefSpawns>();
            if let Some(entry) = pending.native.entries.get_mut(&reference_key) {
                entry.phase = phase;
            }
            pending.native.dirty = true;
        }
    }
    let result = world
        .resource_mut::<crate::twin_projection::PendingRefSpawns>()
        .native
        .request(reference_key, owner);
    if world
        .resource::<crate::twin_projection::PendingRefSpawns>()
        .native
        .dirty
        && let Some(wake) = world.get_resource::<lunco_usd_bevy_twin::TwinProjectionWake>()
    {
        wake.wake();
    }
    result
}

fn current_source(
    world: &World,
    handle: &Handle<UsdStageAsset>,
) -> Result<Option<(Arc<StageRecipe>, Arc<UsdStageProjectionPlan>)>, String> {
    if let Some(state) = world.resource::<AssetServer>().get_load_state(handle.id()) {
        match state {
            bevy::asset::LoadState::Failed(error) => return Err(error.to_string()),
            bevy::asset::LoadState::Loading => return Ok(None),
            _ => {}
        }
    }
    let Some(asset) = world.resource::<Assets<UsdStageAsset>>().get(handle.id()) else {
        return Ok(None);
    };
    let recipe = asset
        .recipe
        .clone()
        .ok_or_else(|| "the referenced asset has no fetched USD recipe".to_owned())?;
    Ok(Some((recipe, Arc::clone(&asset.projection_plan))))
}

/// Consume worker/asset notifications and admit the next bounded work phase.
/// Out-of-date results never release a reference or document-projection hold.
pub(crate) fn advance(world: &mut World) {
    let native = &world
        .resource::<crate::twin_projection::PendingRefSpawns>()
        .native;
    if native.entries.is_empty()
        && !native.dirty
        && !native.completion_ready.load(Ordering::Acquire)
        && native.retired.is_empty()
    {
        return;
    }
    let mut pending = std::mem::take(
        &mut world
            .resource_mut::<crate::twin_projection::PendingRefSpawns>()
            .native,
    );
    pending.dirty = false;
    if let Some(mut admission) = world.get_resource_mut::<AsyncWorkAdmission>() {
        for key in pending.retired.drain(..) {
            admission.cancel_queued(key);
        }
    }
    let completions = if pending.completion_ready.swap(false, Ordering::AcqRel) {
        std::mem::take(
            &mut *pending
                .completions
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    } else {
        Vec::new()
    };
    for completion in completions {
        let Some(entry) = pending.entries.get_mut(&completion.key) else {
            continue;
        };
        let current_owner = Owner::capture(world, completion.key.0).ok();
        if !entry.accepts(completion.operation, current_owner.as_ref()) {
            continue;
        }
        entry.work = None;
        entry.capacity_revision = None;
        entry.phase = match completion.outcome {
            Outcome::Address(Ok((path, paths))) => {
                match paths.validate_owner(world.get_resource::<lunco_assets_core::TwinRoots>()) {
                    Ok(()) => Phase::Loading {
                        handle: world.resource::<AssetServer>().load(path),
                        paths,
                    },
                    Err(error) => Phase::Failed(error.to_string()),
                }
            }
            Outcome::Address(Err(error)) | Outcome::Plan(Err(error)) | Outcome::Failed(error) => {
                Phase::Failed(error)
            }
            Outcome::Plan(Ok((recipe, plan))) => match &entry.phase {
                Phase::Preparing {
                    handle,
                    source_recipe,
                    source_plan,
                    paths,
                } => {
                    match paths.validate_owner(world.get_resource::<lunco_assets_core::TwinRoots>())
                    {
                        Ok(()) => match world.resource::<AssetServer>().get_path(handle.id()) {
                            Some(path) => Phase::Ready {
                                prepared: PreparedReference {
                                    handle: handle.clone(),
                                    snapshot: Arc::new(UsdReferenceSnapshot {
                                        recipe,
                                        plan,
                                        native_source: Some(UsdNativeReferenceSource {
                                            recipe: Arc::clone(source_recipe),
                                            plan: Arc::clone(source_plan),
                                            path: path.into_owned(),
                                            origin: entry.owner.origin.clone(),
                                            address_paths: Arc::new(paths.clone()),
                                        }),
                                    }),
                                },
                                source_recipe: Arc::clone(source_recipe),
                                source_plan: Arc::clone(source_plan),
                                paths: paths.clone(),
                            },
                            None => Phase::Failed(
                                "the prepared reference has no actual source address".into(),
                            ),
                        },
                        Err(error) => Phase::Failed(error.to_string()),
                    }
                }
                _ => Phase::Failed("reference preparation returned an unexpected phase".into()),
            },
        };
    }
    let capacity = world
        .get_resource::<AsyncWorkAdmission>()
        .map(AsyncWorkAdmission::capacity_revision);
    for (reference_key, entry) in &mut pending.entries {
        let current_owner = match Owner::capture(world, reference_key.0) {
            Ok(owner) => owner,
            Err(error) => {
                if let Some((key, _)) = entry.work.take()
                    && let Some(mut admission) = world.get_resource_mut::<AsyncWorkAdmission>()
                {
                    admission.cancel_queued(key);
                }
                entry.phase = Phase::Failed(error);
                continue;
            }
        };
        if current_owner != entry.owner {
            if let Some((key, _)) = entry.work.take()
                && let Some(mut admission) = world.get_resource_mut::<AsyncWorkAdmission>()
            {
                admission.cancel_queued(key);
            }
            let replaced = current_owner.identity != entry.owner.identity
                || current_owner.origin != entry.owner.origin;
            entry.owner = current_owner;
            entry.capacity_revision = None;
            entry.phase = if replaced {
                Phase::Failed("the native reference stage or mount was replaced".into())
            } else {
                Phase::Address
            };
        }
        if let Phase::Ready {
            prepared,
            source_recipe,
            source_plan,
            paths,
        } = &entry.phase
        {
            match current_source(world, &prepared.handle) {
                Ok(Some((recipe, plan)))
                    if Arc::ptr_eq(&recipe, source_recipe) && Arc::ptr_eq(&plan, source_plan) =>
                {
                    if let Err(error) =
                        paths.validate_owner(world.get_resource::<lunco_assets_core::TwinRoots>())
                    {
                        entry.phase = Phase::Failed(error.to_string());
                    }
                }
                Ok(_) => {
                    entry.phase = Phase::Loading {
                        handle: prepared.handle.clone(),
                        paths: paths.clone(),
                    }
                }
                Err(error) => entry.phase = Phase::Failed(error),
            }
        }
        if entry.work.is_some() || matches!(entry.phase, Phase::Failed(_) | Phase::Ready { .. }) {
            continue;
        }
        let Some(capacity) = capacity else {
            entry.phase =
                Phase::Failed("native reference preparation has no worker admission owner".into());
            continue;
        };
        if entry.capacity_revision == Some(capacity) {
            continue;
        }
        let roots = world
            .get_resource::<lunco_assets_core::TwinRoots>()
            .cloned();
        let origin = entry.owner.origin.clone();
        let phase_name = if matches!(entry.phase, Phase::Address) {
            "address"
        } else {
            "composition"
        };
        let worker: Box<dyn FnOnce() -> Outcome + Send> = match &entry.phase {
            Phase::Address => {
                let reference = reference_key.1.clone();
                Box::new(move || {
                    let paths = PreparedAssetPaths::prepare_on_worker(
                        [reference.clone()],
                        origin.clone(),
                        roots.as_ref(),
                    );
                    let result =
                        load_asset_path(&reference, origin.as_ref(), roots.as_ref(), Some(&paths))
                            .map(|path| (path, paths))
                            .map_err(|error| error.to_string());
                    Outcome::Address(result)
                })
            }
            Phase::Loading { handle, paths } => {
                let (source_recipe, source_plan) = match current_source(world, handle) {
                    Ok(Some(source)) => source,
                    Ok(None) => continue,
                    Err(error) => {
                        entry.phase = Phase::Failed(error);
                        continue;
                    }
                };
                let recipe = Arc::clone(&source_recipe);
                let root_id = reference_key.1.clone();
                let next_phase = Phase::Preparing {
                    handle: handle.clone(),
                    source_recipe,
                    source_plan,
                    paths: paths.clone(),
                };
                entry.phase = next_phase;
                Box::new(move || {
                    let result = (|| {
                        let recipe = recipe
                            .reanchor(&root_id)
                            .map_err(|error| error.to_string())?;
                        let mut plan = UsdStageProjectionPlan::from_recipe(&recipe)
                            .map_err(|error| error.to_string())?;
                        plan.prepare_native_asset_paths(origin, roots.as_ref());
                        if let Some(paths) = plan.native_asset_paths_snapshot() {
                            paths
                                .validate_owner(roots.as_ref())
                                .map_err(|error| error.to_string())?;
                        }
                        Ok((Arc::new(recipe), Arc::new(plan)))
                    })();
                    Outcome::Plan(result)
                })
            }
            Phase::Preparing { .. } => {
                entry.phase = Phase::Failed(
                    "native reference plan has no admitted preparation operation".into(),
                );
                continue;
            }
            Phase::Ready { .. } | Phase::Failed(_) => continue,
        };
        let Some(operation) = pending.next_operation.checked_add(1) else {
            entry.phase = Phase::Failed("native reference operation identity exhausted".into());
            continue;
        };
        pending.next_operation = operation;
        let scope = world
            .get_resource::<lunco_core::SceneTransitionCoordinator>()
            .and_then(lunco_core::SceneTransitionCoordinator::lifecycle_generation)
            .unwrap_or_default();
        let key = AsyncWorkKey::new(
            AsyncWorkKind::UsdPreparation,
            scope,
            (1_u128 << 126) | u128::from(entry.owner.identity),
            entry.owner.generation,
            operation,
        );
        let completions = Arc::clone(&pending.completions);
        let ready = Arc::clone(&pending.completion_ready);
        let request = reference_key.clone();
        let wake = world
            .get_resource::<lunco_usd_bevy_twin::TwinProjectionWake>()
            .cloned();
        let job = move || {
            let outcome = run_worker(worker, phase_name);
            completions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Completion {
                    key: request,
                    operation,
                    outcome,
                });
            ready.store(true, Ordering::Release);
            if let Some(wake) = wake {
                wake.wake();
            }
        };
        match world.resource_mut::<AsyncWorkAdmission>().submit(
            AsyncWorkPriority::SimulationRequired,
            key,
            job,
        ) {
            Ok(()) => {
                entry.work = Some((key, operation));
                entry.capacity_revision = None;
            }
            Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                entry.capacity_revision = Some(capacity);
                if let Phase::Preparing { handle, paths, .. } = &entry.phase {
                    entry.phase = Phase::Loading {
                        handle: handle.clone(),
                        paths: paths.clone(),
                    };
                }
            }
            Err(error) => {
                entry.phase = Phase::Failed(format!("native reference work rejected: {error:?}"))
            }
        }
    }
    world
        .resource_mut::<crate::twin_projection::PendingRefSpawns>()
        .native = pending;
    if let Some(wake) = world.get_resource::<lunco_usd_bevy_twin::TwinProjectionWake>() {
        wake.wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetApp;
    use lunco_core_runtime::{SimulationProgress, SimulationProgressKey, SimulationProgressOwner};
    use lunco_usd_bevy_stage::canonical::CanonicalStage;

    #[test]
    fn native_reference_worker_panic_is_a_terminal_completion() {
        let outcome = run_worker(|| panic!("generic worker failure"), "composition");
        let Outcome::Failed(error) = outcome else {
            panic!("worker panic must be a terminal error");
        };
        assert!(error.contains("composition preparation panicked"));
        assert!(error.contains("generic worker failure"));
    }

    #[test]
    fn stale_native_reference_completion_preserves_current_work_and_hold() {
        let recipe = Arc::new(StageRecipe::from_source(
            "scene.usda",
            "#usda 1.0\ndef Scope \"Root\" {}\n",
        ));
        let canonical = CanonicalStage::from_recipe(&recipe).unwrap();
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<UsdStageAsset>();
        app.init_resource::<crate::twin_projection::PendingRefSpawns>();
        app.init_resource::<AsyncWorkAdmission>();
        app.init_resource::<SimulationProgress>();
        let source = UsdStageAsset::from_recipe(recipe.as_ref().clone()).unwrap();
        let plan = Arc::clone(&source.projection_plan);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(source);
        let mut stages = CanonicalStages::default();
        stages.insert(handle.id(), canonical);
        app.world_mut().insert_non_send(stages);
        let owner = Owner::capture(app.world(), handle.id()).unwrap();
        let reference =
            lunco_storage::file_path_to_uri(&std::env::temp_dir().join("generic reference.usda"))
                .unwrap();
        let reference_key = (handle.id(), reference);
        let key = AsyncWorkKey::new(
            AsyncWorkKind::UsdPreparation,
            0,
            u128::from(owner.identity),
            owner.generation,
            2,
        );
        let hold = SimulationProgressKey {
            owner: SimulationProgressOwner::SceneReferences,
            operation_id: 2,
        };
        app.world_mut()
            .resource_mut::<SimulationProgress>()
            .acquire(hold, "pending native reference");
        {
            let mut pending = app
                .world_mut()
                .resource_mut::<crate::twin_projection::PendingRefSpawns>();
            pending.native.entries.insert(
                reference_key.clone(),
                Entry {
                    owner: owner.clone(),
                    phase: Phase::Preparing {
                        handle,
                        source_recipe: recipe,
                        source_plan: plan,
                        paths: PreparedAssetPaths::for_origin(None),
                    },
                    work: Some((key, 2)),
                    capacity_revision: None,
                },
            );
            pending.native.completions.lock().unwrap().push(Completion {
                key: reference_key.clone(),
                operation: 1,
                outcome: Outcome::Plan(Err("retired operation failed".into())),
            });
            pending
                .native
                .completion_ready
                .store(true, Ordering::Release);
        }
        advance(app.world_mut());
        let pending = app
            .world()
            .resource::<crate::twin_projection::PendingRefSpawns>();
        let entry = &pending.native.entries[&reference_key];
        assert_eq!(entry.work, Some((key, 2)));
        assert!(matches!(entry.phase, Phase::Preparing { .. }));
        assert!(app.world().resource::<SimulationProgress>().is_held());
        assert!(entry.accepts(2, Some(&owner)));
        let mut changed_owner = owner.clone();
        changed_owner.generation += 1;
        assert!(!entry.accepts(2, Some(&changed_owner)));
        changed_owner = owner.clone();
        changed_owner.identity += 1;
        assert!(!entry.accepts(2, Some(&changed_owner)));
        changed_owner = owner;
        changed_owner.origin =
            Some(AssetPath::from_path_buf("other-mount/scene.usda".into()).with_source("twin"));
        assert!(!entry.accepts(2, Some(&changed_owner)));
        let mut pending = app
            .world_mut()
            .resource_mut::<crate::twin_projection::PendingRefSpawns>();
        let outgoing = Arc::clone(&pending.native.completions);
        let outgoing_ready = Arc::clone(&pending.native.completion_ready);
        pending.native.clear(None);
        assert!(!Arc::ptr_eq(&outgoing, &pending.native.completions));
        outgoing_ready.store(true, Ordering::Release);
        assert!(!pending.native.completion_ready.load(Ordering::Acquire));
        pending.native.completions.lock().unwrap().push(Completion {
            key: reference_key,
            operation: 2,
            outcome: Outcome::Failed("retired operation completed late".into()),
        });
        pending
            .native
            .completion_ready
            .store(true, Ordering::Release);
        drop(pending);
        advance(app.world_mut());
        assert!(
            !app.world()
                .resource::<crate::twin_projection::PendingRefSpawns>()
                .native
                .needs_work(0)
        );
        assert!(app.world().resource::<SimulationProgress>().is_held());
    }
}
