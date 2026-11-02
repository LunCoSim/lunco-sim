//! Admit only datasets consumed by the current composed scene. USD owns the
//! dependency snapshot; the dataset registry owns identity and availability.

use bevy::prelude::*;
use lunco_assets_datasets::{DatasetRegistry, DatasetScope, SceneDatasetRequirements};
use lunco_core::{AppSceneExt, SceneResource};
use lunco_core_runtime::{SimulationProgress, SimulationProgressKey};
use lunco_usd_bevy_scene::{UsdPrimPath, UsdSceneRoot};
use lunco_usd_bevy_stage::{UsdStageAsset, UsdStageProjectionPlan};
use std::sync::Arc;

#[derive(Resource, Default)]
struct Inputs {
    root: Option<Entity>,
    plan: Option<Arc<UsdStageProjectionPlan>>,
    mounts_revision: Option<u64>,
    live: Option<std::collections::BTreeMap<String, std::collections::BTreeSet<String>>>,
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
    if root == inputs.root
        && revision == inputs.mounts_revision
        && match (&inputs.plan, plan) {
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        }
        && !registry
            .as_ref()
            .is_some_and(|registry| registry.is_changed())
        && !requirements.is_changed()
        && !inputs.is_changed()
        && !runtime.as_ref().is_some_and(|runtime| runtime.is_changed())
    {
        return;
    }
    if root != inputs.root
        || !match (&inputs.plan, plan) {
            (Some(old), Some(new)) => Arc::ptr_eq(old, new),
            (None, None) => true,
            _ => false,
        }
    {
        inputs.live = None;
    }
    inputs.root = root;
    inputs.plan = plan.cloned();
    inputs.mounts_revision = revision;
    let mut next = SceneDatasetRequirements::default();
    if let (Some(plan), Some(registry)) = (plan, registry.as_deref()) {
        next.scene = root;
        for identifier in inputs
            .live
            .as_ref()
            .unwrap_or_else(|| plan.asset_identifiers())
            .values()
            .flatten()
        {
            let addressed = if let Some(relative) = lunco_assets_core::parse_lunco_uri(identifier) {
                Some((DatasetScope::Engine, relative))
            } else if let Some((name, relative)) = lunco_assets_core::parse_twin_uri(identifier) {
                match roots
                    .as_ref()
                    .and_then(|roots| roots.root_of(name).ok().flatten())
                {
                    Some(root) => Some((
                        DatasetScope::Twin {
                            name: name.to_owned(),
                            root,
                        },
                        relative,
                    )),
                    None => {
                        next.pending_scopes.insert(format!("twin:{name}"));
                        None
                    }
                }
            } else {
                None
            };
            if let Some((scope, relative)) = addressed {
                if !registry.is_scope_scanned(&scope) {
                    next.pending_scopes.insert(scope.id());
                }
                if let Some(entry) =
                    registry.declared_artifact(&scope, std::path::Path::new(relative))
                {
                    next.ids.insert(entry.id.clone());
                }
            }
        }
    }
    if let (Some(runtime), Some(registry)) = (runtime.as_deref(), registry.as_deref()) {
        if runtime.scene == root {
            for uri in &runtime.uris {
                match registry.declared_uri(uri, roots.as_deref()) {
                    Ok(Some(entry)) => {
                        next.ids.insert(entry.id.clone());
                    }
                    Ok(None) => {
                        if registry.uri_scope_scanned(uri, roots.as_deref()) == Ok(true) {
                            faults.raise("required-asset-undeclared",root,"Runtime asset dependency",format!("No artifact is declared for {uri}. Correct the URI or declare it in Assets.toml."));
                        }
                    }
                    Err(error) => {
                        faults.raise(
                            "required-asset-invalid",
                            root,
                            "Runtime asset dependency",
                            error,
                        );
                    }
                }
                match registry.uri_scope_scanned(uri, roots.as_deref()) {
                    Ok(false) => {
                        next.pending_scopes.insert(format!("Discovering {uri}"));
                    }
                    Err(error) => {
                        faults.raise(
                            "required-asset-invalid",
                            root,
                            "Runtime asset dependency",
                            error,
                        );
                    }
                    Ok(true) => {}
                }
            }
        }
    }
    let missing: Vec<_> = next
        .ids
        .iter()
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
    let mut replacement = lunco_usd_bevy_stage::native_paths::consumed_asset_identifiers_for_prims(
        &view,
        paths.iter().cloned(),
    );
    for (prim, identifiers) in
        lunco_usd_bevy_stage::native_paths::composed_layer_identifiers_for_prims(
            view.stage(),
            paths.iter().cloned(),
        )
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
