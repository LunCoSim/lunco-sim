//! Revision-fenced native address preparation before live USD consumption.

use crate::live_consume::TransformEditChannels;
use bevy::asset::AssetPath;
use bevy::prelude::*;
use lunco_assets_core::asset_path::PreparedAssetPaths;
use lunco_core_runtime::{
    AsyncWorkAdmission, AsyncWorkKey, AsyncWorkKind, AsyncWorkPriority, SimulationProgress,
    SimulationProgressKey, SimulationProgressOwner,
};
use lunco_usd_bevy_stage::UsdStageAsset;
use lunco_usd_bevy_stage::canonical::{CanonicalStages, RawStageChange};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

type Changes = (
    AssetId<UsdStageAsset>,
    Vec<RawStageChange>,
    HashMap<String, TransformEditChannels>,
);

#[derive(Resource, Default)]
pub(crate) struct PendingNativeAssetPaths {
    stages: BTreeMap<AssetId<UsdStageAsset>, PendingStage>,
    completions: Arc<Mutex<Vec<Completion>>>,
    completion_ready: Arc<AtomicBool>,
    next_operation: u64,
}

#[derive(Default)]
struct PendingStage {
    identity: Option<u64>,
    changes: Vec<RawStageChange>,
    hints: HashMap<String, TransformEditChannels>,
    work: Option<Work>,
    capacity_revision: Option<u64>,
    failed: bool,
    ready_for: Option<(u64, u64)>,
    progress_key: Option<SimulationProgressKey>,
}

struct Work {
    key: AsyncWorkKey,
    operation: u64,
    identity: u64,
    generation: u64,
    origin: Option<AssetPath<'static>>,
}

struct Completion {
    stage: AssetId<UsdStageAsset>,
    operation: u64,
    prepared: PreparedAssetPaths,
}

impl PendingNativeAssetPaths {
    pub(crate) fn clear(
        &mut self,
        admission: Option<&mut AsyncWorkAdmission>,
        progress: Option<&mut SimulationProgress>,
    ) {
        if let Some(admission) = admission {
            for stage in self.stages.values() {
                if let Some(work) = &stage.work {
                    admission.cancel_queued(work.key);
                }
            }
        }
        if let Some(progress) = progress {
            for stage in self.stages.values() {
                if let Some(key) = stage.progress_key {
                    progress.release(key);
                }
            }
        }
        self.stages.clear();
        self.completions = Arc::new(Mutex::new(Vec::new()));
        self.completion_ready = Arc::new(AtomicBool::new(false));
    }

    pub(crate) fn contains(&self, stage: AssetId<UsdStageAsset>) -> bool {
        self.stages.contains_key(&stage)
    }
}

fn retire_stage(world: &mut World, stage: PendingStage) {
    if let Some(work) = stage.work
        && let Some(mut admission) = world.get_resource_mut::<AsyncWorkAdmission>()
    {
        admission.cancel_queued(work.key);
    }
    if let Some(key) = stage.progress_key
        && let Some(mut progress) = world.get_resource_mut::<SimulationProgress>()
    {
        progress.release(key);
    }
}

/// Hold complete sink batches and transform hints until all native addresses
/// required by their current composed revision have worker preparation. A newer
/// generation replaces the job, while its retained changes remain accumulated.
pub(crate) fn admit_stage_changes(world: &mut World, incoming: Vec<Changes>) -> Vec<Changes> {
    if incoming.is_empty()
        && world
            .get_resource::<PendingNativeAssetPaths>()
            .is_none_or(|pending| {
                pending.stages.is_empty() && !pending.completion_ready.load(Ordering::Acquire)
            })
    {
        return Vec::new();
    }
    world.init_resource::<PendingNativeAssetPaths>();
    let mut pending = world
        .remove_resource::<PendingNativeAssetPaths>()
        .expect("initialized native admission");
    for (id, changes, hints) in incoming {
        let identity = world
            .get_non_send::<CanonicalStages>()
            .and_then(|stages| stages.get(id))
            .map(|stage| stage.identity());
        if pending
            .stages
            .get(&id)
            .is_some_and(|stage| stage.identity != identity)
            && let Some(stage) = pending.stages.remove(&id)
        {
            retire_stage(world, stage);
        }
        let stage = pending.stages.entry(id).or_default();
        stage.identity = identity;
        stage.changes.extend(changes);
        for (path, channels) in hints {
            stage.hints.entry(path).or_default().merge(channels);
        }
        stage.capacity_revision = None;
        stage.failed = false;
        stage.ready_for = None;
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
        let Some(stage) = pending.stages.get_mut(&completion.stage) else {
            continue;
        };
        let Some(work) = stage.work.as_ref() else {
            continue;
        };
        if work.operation != completion.operation {
            continue;
        }
        let origin = world
            .get_resource::<AssetServer>()
            .and_then(|server| server.get_path(completion.stage).map(AssetPath::into_owned));
        let current = world
            .get_non_send::<CanonicalStages>()
            .and_then(|stages| stages.get(completion.stage));
        let matches = current.is_some_and(|current| {
            current.identity() == work.identity && current.generation() == work.generation
        }) && origin == work.origin;
        if matches {
            let roots = world.get_resource::<lunco_assets_core::TwinRoots>();
            if let Err(error) = completion.prepared.validate_owner(roots) {
                report_failure(
                    world,
                    format!("native asset preparation owner retired: {error}"),
                );
                stage.failed = true;
            } else {
                let installation =
                    world
                        .get_non_send_mut::<CanonicalStages>()
                        .and_then(|mut stages| {
                            let current = stages.get_mut(completion.stage)?;
                            let mut paths = current
                                .cached_native_asset_paths()
                                .map(|paths| paths.as_ref().clone())
                                .unwrap_or_else(|| PreparedAssetPaths::for_origin(origin));
                            Some(
                                paths
                                    .merge(completion.prepared)
                                    .map(|()| current.set_native_asset_paths(Arc::new(paths))),
                            )
                        });
                if let Some(Err(error)) = installation {
                    report_failure(world, format!("native asset preparation rejected: {error}"));
                    stage.failed = true;
                }
            }
        }
        stage.work = None;
        stage.capacity_revision = None;
    }
    let mut ready = Vec::new();
    let ids = pending.stages.keys().copied().collect::<Vec<_>>();
    for id in ids {
        let Some((identity, generation)) = world
            .get_non_send::<CanonicalStages>()
            .and_then(|stages| stages.get(id))
            .map(|current| (current.identity(), current.generation()))
        else {
            if let Some(stage) = pending.stages.remove(&id) {
                retire_stage(world, stage);
            }
            continue;
        };
        if pending
            .stages
            .get(&id)
            .is_some_and(|stage| stage.identity != Some(identity))
        {
            if let Some(stage) = pending.stages.remove(&id) {
                retire_stage(world, stage);
            }
            continue;
        }
        let origin = world
            .get_resource::<AssetServer>()
            .and_then(|server| server.get_path(id).map(AssetPath::into_owned));
        let Some(stage) = pending.stages.get_mut(&id) else {
            continue;
        };
        if stage.failed || stage.ready_for.is_some() {
            continue;
        }
        if let Some(work) = stage.work.as_ref() {
            if work.identity == identity && work.generation == generation && work.origin == origin {
                continue;
            }
            if let Some(mut admission) = world.get_resource_mut::<AsyncWorkAdmission>() {
                admission.cancel_queued(work.key);
            }
            stage.work = None;
            stage.capacity_revision = None;
        }
        let Some(current) = world
            .get_non_send::<CanonicalStages>()
            .and_then(|stages| stages.get(id))
        else {
            continue;
        };
        let existing = current
            .cached_native_asset_paths()
            .cloned()
            .or_else(|| {
                world
                    .get_resource::<Assets<UsdStageAsset>>()
                    .and_then(|assets| assets.get(id))
                    .and_then(|asset| asset.projection_plan.native_asset_paths_snapshot())
            })
            .filter(|paths| paths.is_for_origin(origin.as_ref()))
            .unwrap_or_else(|| Arc::new(PreparedAssetPaths::for_origin(origin.clone())));
        let references = lunco_usd_bevy_stage::native_paths::changed_native_references(
            &current.view(),
            &stage.changes,
        );
        let missing = references
            .into_iter()
            .filter(|reference| !existing.contains(reference))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            if let Err(error) =
                existing.validate_owner(world.get_resource::<lunco_assets_core::TwinRoots>())
            {
                report_failure(
                    world,
                    format!("native asset preparation owner retired: {error}"),
                );
                stage.failed = true;
                continue;
            }
            if let Some(mut stages) = world.get_non_send_mut::<CanonicalStages>()
                && let Some(current) = stages.get_mut(id)
            {
                current.set_native_asset_paths(existing);
            }
            stage.ready_for = Some((identity, generation));
            ready.push((
                id,
                std::mem::take(&mut stage.changes),
                std::mem::take(&mut stage.hints),
            ));
            continue;
        }
        if let Some(mut stages) = world.get_non_send_mut::<CanonicalStages>()
            && let Some(current) = stages.get_mut(id)
        {
            current.hold_native_asset_paths(existing);
        }
        if stage.progress_key.is_none()
            && crate::twin_projection::is_authoritative_scene_stage(world, id)
        {
            let Some(operation) = pending.next_operation.checked_add(1) else {
                report_failure(
                    world,
                    "native asset preparation operation identity exhausted".into(),
                );
                stage.failed = true;
                continue;
            };
            pending.next_operation = operation;
            let key = SimulationProgressKey {
                owner: SimulationProgressOwner::UsdNativeAssetPreparation,
                operation_id: operation,
            };
            if let Some(mut progress) = world.get_resource_mut::<SimulationProgress>() {
                progress.acquire(
                    key,
                    format!("Preparing native assets for USD stage {id:?} generation {generation}"),
                );
                stage.progress_key = Some(key);
            } else {
                report_failure(
                    world,
                    "native asset preparation has no simulation-progress owner".into(),
                );
                stage.failed = true;
                continue;
            }
        }
        let Some(capacity_revision) = world
            .get_resource::<AsyncWorkAdmission>()
            .map(|admission| admission.capacity_revision())
        else {
            report_failure(
                world,
                "native asset preparation has no worker admission owner".into(),
            );
            stage.failed = true;
            continue;
        };
        if stage.capacity_revision == Some(capacity_revision) {
            continue;
        }
        let Some(operation) = pending.next_operation.checked_add(1) else {
            report_failure(
                world,
                "native asset preparation operation identity exhausted".into(),
            );
            stage.failed = true;
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
            (1_u128 << 127) | u128::from(identity),
            generation,
            operation,
        );
        let roots = world
            .get_resource::<lunco_assets_core::TwinRoots>()
            .cloned();
        let queue = Arc::clone(&pending.completions);
        let completion_ready = Arc::clone(&pending.completion_ready);
        let worker_origin = origin.clone();
        let wake = world
            .get_resource::<lunco_usd_bevy_twin::TwinProjectionWake>()
            .cloned();
        let job = move || {
            let prepared =
                PreparedAssetPaths::prepare_on_worker(missing, worker_origin, roots.as_ref());
            queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Completion {
                    stage: id,
                    operation,
                    prepared,
                });
            completion_ready.store(true, Ordering::Release);
            if let Some(wake) = wake {
                wake.wake()
            }
        };
        match world.resource_mut::<AsyncWorkAdmission>().submit(
            AsyncWorkPriority::SimulationRequired,
            key,
            job,
        ) {
            Ok(()) => {
                stage.work = Some(Work {
                    key,
                    operation,
                    identity,
                    generation,
                    origin,
                });
                stage.capacity_revision = None;
            }
            Err(lunco_core_runtime::AsyncWorkRejection::QueueFull) => {
                stage.capacity_revision = Some(capacity_revision);
            }
            Err(error) => {
                stage.failed = true;
                report_failure(
                    world,
                    format!("native asset preparation could not be admitted: {error:?}"),
                );
            }
        }
    }
    world.insert_resource(pending);
    ready
}

/// Release only after the matching sink batch has been consumed into live ECS.
pub(crate) fn finish_stage_projection(world: &mut World, id: AssetId<UsdStageAsset>) {
    let current = world
        .get_non_send::<CanonicalStages>()
        .and_then(|stages| stages.get(id))
        .map(|stage| (stage.identity(), stage.generation()));
    let finished = world
        .get_resource_mut::<PendingNativeAssetPaths>()
        .and_then(|mut pending| {
            let stage = pending.stages.get_mut(&id)?;
            let Some(ready_for) = stage.ready_for else {
                return None;
            };
            if Some(ready_for) != current {
                stage.ready_for = None;
                return None;
            }
            pending.stages.remove(&id)
        });
    if let Some(stage) = finished
        && let Some(key) = stage.progress_key
        && let Some(mut progress) = world.get_resource_mut::<SimulationProgress>()
    {
        progress.release(key);
    }
}

fn report_failure(world: &mut World, message: String) {
    world.trigger(lunco_core::RuntimeError {
        name: "usd-native-asset-preparation-failed".into(),
        message,
    });
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use lunco_usd_bevy_stage::canonical::CanonicalStage;
    use lunco_usd_compose::recipe::StageRecipe;
    use openusd::sdf::Path as SdfPath;

    #[test]
    fn stale_native_completion_preserves_newer_changes_and_transform_hints() {
        let file = tempfile::tempdir().expect("native folder");
        let reference =
            lunco_storage::file_path_to_uri(&file.path().join("payload.txt")).expect("file URI");
        let source =
            format!("#usda 1.0\n\ndef Xform \"Root\"\n{{\n    asset native = @{reference}@\n}}\n");
        let recipe = StageRecipe::from_source("scene.usda", &source);
        let mut canonical = CanonicalStage::from_recipe(&recipe).expect("inline stage");
        canonical.generation = 2;
        let identity = canonical.identity();
        let mut world = World::new();
        world.init_resource::<Assets<UsdStageAsset>>();
        world.init_resource::<AsyncWorkAdmission>();
        world.init_resource::<SimulationProgress>();
        let handle = world
            .resource_mut::<Assets<UsdStageAsset>>()
            .add(UsdStageAsset::from_recipe(recipe).expect("inline prepared stage"));
        let scene_root = world
            .spawn(lunco_usd_bevy_scene::UsdPrimPath {
                stage_handle: handle.clone(),
                path: "/Root".into(),
            })
            .id();
        let mut mounts = lunco_core::SceneMountState::default();
        mounts.register_root(scene_root, true);
        world.insert_resource(mounts);
        let mut stages = CanonicalStages::default();
        stages.insert(handle.id(), canonical);
        world.insert_non_send(stages);
        let old_key = AsyncWorkKey::new(
            AsyncWorkKind::UsdPreparation,
            0,
            (1_u128 << 127) | u128::from(identity),
            1,
            1,
        );
        let changes = vec![RawStageChange {
            info_only: vec![SdfPath::new("/Root.native").expect("property")],
            ..Default::default()
        }];
        let old_hints = HashMap::from([("/Root".into(), TransformEditChannels::translate())]);
        let progress_key = SimulationProgressKey {
            owner: SimulationProgressOwner::UsdNativeAssetPreparation,
            operation_id: 1,
        };
        world
            .resource_mut::<SimulationProgress>()
            .acquire(progress_key, "native preparation");
        let mut pending = PendingNativeAssetPaths {
            next_operation: 1,
            ..Default::default()
        };
        pending.stages.insert(
            handle.id(),
            PendingStage {
                identity: Some(identity),
                changes,
                hints: old_hints,
                work: Some(Work {
                    key: old_key,
                    operation: 1,
                    identity,
                    generation: 1,
                    origin: None,
                }),
                progress_key: Some(progress_key),
                ..Default::default()
            },
        );
        pending.completions.lock().expect("queue").push(Completion {
            stage: handle.id(),
            operation: 1,
            prepared: PreparedAssetPaths::for_origin(None),
        });
        pending.completion_ready.store(true, Ordering::Release);
        world.insert_resource(pending);
        let incoming = vec![(
            handle.id(),
            vec![RawStageChange {
                info_only: vec![SdfPath::new("/Root.xformOp:rotateXYZ").expect("property")],
                ..Default::default()
            }],
            HashMap::from([("/Root".into(), TransformEditChannels::rotate())]),
        )];
        assert!(admit_stage_changes(&mut world, incoming).is_empty());
        let pending = world.resource::<PendingNativeAssetPaths>();
        let retained = pending
            .stages
            .get(&handle.id())
            .expect("held current owner");
        assert_eq!(retained.changes.len(), 2);
        assert!(retained.hints["/Root"].translate);
        assert!(retained.hints["/Root"].rotate);
        let work = retained.work.as_ref().expect("new generation preparation");
        assert_eq!(work.generation, 2);
        assert_eq!(work.operation, 2);
        assert_ne!(work.key, old_key);
        assert!(
            !world
                .non_send::<CanonicalStages>()
                .get(handle.id())
                .expect("current stage")
                .native_asset_paths_ready()
        );
        assert!(world.resource::<SimulationProgress>().is_held());
        assert!(
            world
                .resource::<SimulationProgress>()
                .contains(progress_key)
        );
        // No unresolved preparation may release the authoritative physics clock.
        finish_stage_projection(&mut world, handle.id());
        assert!(
            world
                .resource::<SimulationProgress>()
                .contains(progress_key)
        );
        // The old completion must not make the current native reference readable.
        assert!(
            !world
                .non_send::<CanonicalStages>()
                .get(handle.id())
                .expect("current stage")
                .cached_native_asset_paths()
                .expect("held table")
                .contains(&lunco_assets_core::asset_path::AssetReference::Native(
                    reference.clone()
                ))
        );
        let mut pending = world
            .remove_resource::<PendingNativeAssetPaths>()
            .expect("pending owner");
        let outgoing_completions = Arc::clone(&pending.completions);
        let outgoing_ready = Arc::clone(&pending.completion_ready);
        let mut admission = world
            .remove_resource::<AsyncWorkAdmission>()
            .expect("worker owner");
        let mut progress = world
            .remove_resource::<SimulationProgress>()
            .expect("progress owner");
        pending.clear(Some(&mut admission), Some(&mut progress));
        assert!(!progress.is_held());
        assert!(pending.stages.is_empty());
        outgoing_completions
            .lock()
            .expect("outgoing channel")
            .push(Completion {
                stage: handle.id(),
                operation: 2,
                prepared: PreparedAssetPaths::for_origin(None),
            });
        outgoing_ready.store(true, Ordering::Release);
        assert!(!pending.completion_ready.load(Ordering::Acquire));
        assert!(pending.completions.lock().expect("new channel").is_empty());
    }
}
