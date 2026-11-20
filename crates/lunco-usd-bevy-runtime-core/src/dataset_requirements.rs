//! Admit only datasets consumed by the current composed scene. USD owns the
//! dependency snapshot; the dataset registry owns identity and availability.

use bevy::prelude::*;
use lunco_assets_datasets::{DatasetRegistry, DatasetScope, SceneDatasetRequirements};
use lunco_core::{AppSceneExt, SceneResource};
use lunco_core_runtime::{SimulationProgress, SimulationProgressKey};
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneRoot};
use lunco_usd_bevy_stage::{UsdStageAsset, UsdStageProjectionPlan};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Asset identity resolved against the current scene's Twin mounts. Registry
/// updates change availability independently of this source snapshot.
struct Dependency {
    uri: String,
    runtime_required: bool,
    address: Result<Option<(DatasetScope, PathBuf)>, String>,
}

#[derive(Resource, Default)]
struct Inputs {
    root: Option<Entity>,
    plan: Option<Arc<UsdStageProjectionPlan>>,
    mounts_revision: Option<u64>,
    live: Option<std::collections::BTreeMap<String, std::collections::BTreeSet<String>>>,
    dependencies: Vec<Dependency>,
}
impl SceneResource for Inputs {}

fn update(
    mount: Res<lunco_core::SceneMountState>,
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    stages: Res<Assets<UsdStageAsset>>,
    prims: Query<&UsdPrimPath, With<UsdSceneRoot>>,
    registry: Option<Res<DatasetRegistry>>,
    runtime: Option<Res<lunco_assets_datasets::RuntimeAssetRequirements>>,
    mut inputs: ResMut<Inputs>,
    mut requirements: ResMut<SceneDatasetRequirements>,
    mut progress: ResMut<SimulationProgress>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
    mut commands: Commands,
) {
    let root = mount.active_root();
    let plan = root
        .and_then(|root| prims.get(root).ok())
        .and_then(|prim| stages.get(&prim.stage_handle))
        .map(|stage| &stage.projection_plan);
    let revision = roots.as_ref().and_then(|roots| roots.revision().ok());
    let scene_changed = root != inputs.root
        || !match (&inputs.plan, plan) {
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        };
    let dependencies_changed = scene_changed
        || revision != inputs.mounts_revision
        || inputs.is_changed()
        || runtime.as_ref().is_some_and(|runtime| runtime.is_changed());
    if !dependencies_changed
        && !registry
            .as_ref()
            .is_some_and(|registry| registry.is_changed())
        && !requirements.is_changed()
    {
        return;
    }
    if dependencies_changed {
        let _span = bevy::log::info_span!("usd_scene_dataset_dependencies").entered();
        if scene_changed {
            inputs.live = None;
        }
        inputs.root = root;
        inputs.plan = plan.cloned();
        inputs.mounts_revision = revision;
        let composed = inputs
            .live
            .as_ref()
            .or_else(|| plan.map(|plan| plan.asset_identifiers()));
        let mut seen = HashSet::new();
        let composed = composed
            .into_iter()
            .flat_map(|paths| paths.values().flatten())
            .filter(|uri| seen.insert(uri.as_str()))
            .map(|uri| (uri, false));
        let runtime = runtime
            .as_deref()
            .filter(|runtime| runtime.scene == root)
            .into_iter()
            .flat_map(|runtime| &runtime.uris)
            .map(|uri| (uri, true));
        inputs.dependencies = composed
            .chain(runtime)
            .map(|(uri, runtime_required)| Dependency {
                uri: uri.clone(),
                runtime_required,
                address: DatasetScope::for_asset_uri(uri, roots.as_deref()),
            })
            .collect();
    }
    let mut next = SceneDatasetRequirements::default();
    if let Some(registry) = registry.as_deref() {
        next.scene = plan.and(root);
        for dependency in &inputs.dependencies {
            let reference = &dependency.uri;
            let (scope, relative) = match &dependency.address {
                Ok(Some((scope, relative))) => (scope, relative),
                Ok(None) if !dependency.runtime_required => continue,
                Ok(None) => {
                    faults.raise("required-asset-undeclared", root, "Runtime asset dependency",
                        format!("No artifact is declared for {reference}. Correct the URI or declare it in Assets.toml."));
                    continue;
                }
                Err(error) => {
                    faults.raise(
                        "required-asset-invalid",
                        root,
                        "Runtime asset dependency",
                        error,
                    );
                    continue;
                }
            };
            let scanned = registry.is_scope_scanned(scope);
            if !scanned {
                next.pending_scopes.insert(scope.id());
            }
            if let Some(entry) = registry.declared_artifact(scope, relative) {
                let Some(uri) = scope.mounted_uri(relative) else {
                    faults.raise(
                        "required-asset-invalid",
                        root,
                        "Runtime asset dependency",
                        "A referenced Twin source has no mounted asset address.",
                    );
                    continue;
                };
                next.artifacts
                    .entry(entry.id.clone())
                    .or_default()
                    .insert(uri);
            } else if dependency.runtime_required && scanned {
                faults.raise("required-asset-undeclared", root, "Runtime asset dependency",
                    format!("No artifact is declared for {reference}. Correct the URI or declare it in Assets.toml."));
            }
        }
    }
    let missing: Vec<_> = next
        .artifacts
        .keys()
        .filter_map(|id| registry.as_ref()?.entry(id))
        .filter(|entry| !entry.state.is_installed())
        .map(|entry| entry.name.as_str())
        .collect();
    let key = SimulationProgressKey::scene_datasets();
    if !next.pending_scopes.is_empty() || !missing.is_empty() {
        let reason = if !next.pending_scopes.is_empty() {
            "Discovering scene data requirements".to_owned()
        } else {
            format!(
                "Scene data required: {}. Open Settings > Data & libraries to download or retry.",
                missing.join(", ")
            )
        };
        if progress.acquire(key, reason.clone()) || *requirements != next {
            commands.trigger(lunco_telemetry_core::TelemetryEvent {
                name: "SCENE_DATA_REQUIRED".into(),
                source: 0,
                severity: lunco_telemetry_core::Severity::Warning,
                data: lunco_telemetry_core::TelemetryValue::String(reason.clone()),
                timestamp: 0.0,
                sim_secs: 0.0,
                sim_tick: 0,
            });
        }
        progress.update_reason(key, reason);
    } else if progress.release(key) && plan.is_some() {
        commands.trigger(lunco_telemetry_core::TelemetryEvent {
            name: "SCENE_DATA_READY".into(), source: 0,
            severity: lunco_telemetry_core::Severity::Info,
            data: lunco_telemetry_core::TelemetryValue::String("Scene data ready. Simulation continues after remaining scene preparation; manual pause is preserved.".into()),
            timestamp: 0.0, sim_secs: 0.0, sim_tick: 0,
        });
    }
    if *requirements != next {
        *requirements = next;
    }
}

/// Re-read only asset properties and structurally affected subtrees of an
/// admitted live edit. Ordinary transform/time edits do not scan the stage.
pub(crate) fn stage_changed(
    world: &mut World,
    id: AssetId<UsdStageAsset>,
    changes: &[lunco_usd_bevy_stage::canonical::RawStageChange],
) {
    use lunco_usd_bevy_stage::native_paths::{
        composed_layer_identifiers_for_active_prims, consumed_asset_identifiers_for_active_prims,
    };
    use lunco_usd_bevy_stage::{UsdReadObject, canonical::CanonicalStages};
    let Some(root) = world
        .get_resource::<lunco_core::SceneMountState>()
        .and_then(|m| m.active_root())
    else {
        return;
    };
    if world
        .get::<UsdPrimPath>(root)
        .is_none_or(|p| p.stage_handle.id() != id)
    {
        return;
    }
    let Some(stage) = world
        .get_non_send::<CanonicalStages>()
        .and_then(|s| s.get(id))
    else {
        return;
    };
    let view = stage.view();
    let mut paths = std::collections::BTreeSet::new();
    let mut subtrees = Vec::new();
    for change in changes {
        for path in change.info_only.iter().chain(change.resynced.iter()) {
            if let Some((prim, name)) = path.as_str().split_once('.') {
                let Ok(prim) = openusd::sdf::Path::new(prim) else {
                    continue;
                };
                if matches!(
                    view.attr_type_name(&prim, name).as_deref(),
                    Some("asset" | "asset[]")
                ) || change.resynced.contains(path)
                {
                    paths.insert(prim);
                }
            } else if change.resynced.contains(path) {
                subtrees.push(path.to_string());
            }
        }
    }
    if paths.is_empty() && subtrees.is_empty() {
        return;
    }
    let mut stack: Vec<_> = subtrees
        .iter()
        .filter_map(|p| openusd::sdf::Path::new(p).ok())
        .collect();
    while let Some(path) = stack.pop() {
        if paths.insert(path.clone()) {
            stack.extend(view.children(&path));
        }
    }
    let active_paths: Vec<_> = paths
        .iter()
        .filter(|path| view.is_active(path) == Ok(true))
        .cloned()
        .collect();
    let mut replacement =
        consumed_asset_identifiers_for_active_prims(&view, active_paths.iter().cloned());
    for (prim, identifiers) in
        composed_layer_identifiers_for_active_prims(view.stage(), active_paths)
    {
        replacement.entry(prim).or_default().extend(identifiers);
    }
    let Some(plan) = world
        .get_resource::<Assets<UsdStageAsset>>()
        .and_then(|s| s.get(id))
        .map(|s| s.projection_plan.clone())
    else {
        return;
    };
    let Some(mut inputs) = world.get_resource_mut::<Inputs>() else {
        return;
    };
    let live = inputs
        .live
        .get_or_insert_with(|| plan.asset_identifiers().clone());
    live.retain(|prim, _| {
        !paths.iter().any(|p| p.as_str() == prim)
            && !subtrees.iter().any(|p| {
                p == "/" || prim == p || prim.strip_prefix(p).is_some_and(|s| s.starts_with('/'))
            })
    });
    live.extend(replacement);
}

pub(crate) fn install(app: &mut App) {
    app.init_scene_resource::<Inputs>()
        .init_scene_resource::<SceneDatasetRequirements>()
        .add_systems(PreUpdate, update
            .after(crate::live_consume::project_stage_changes)
            .in_set(lunco_core_runtime::SimulationProgressAdmissionSet)
            .run_if(bevy::ecs::schedule::common_conditions::resource_exists::<Assets<UsdStageAsset>>))
        .add_systems(lunco_core::SceneTeardown, |mut progress: ResMut<SimulationProgress>| {
            progress.release(SimulationProgressKey::scene_datasets());
        });
}
