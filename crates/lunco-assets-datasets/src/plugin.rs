//! Bevy integration for dataset discovery and the read-side registry.
//!
//! This plugin is intentionally independent of native provisioning. It keeps
//! manifest discovery, lifecycle state, failure reporting, and asset reloads
//! available to headless and browser-safe hosts without compiling HTTP,
//! archive, image, or GeoTIFF processing. The explicit provisioning plugin in
//! `lunco-assets` owns workers and must be installed by an application that
//! wants `RequestDataset` downloads or `ProcessDataset` bakes to perform
//! native I/O.

use bevy::asset::AssetPath;
use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::VecDeque;
use std::collections::{BTreeSet, HashMap};

use crate::{
    DatasetInstalled, DatasetRegistry, DatasetScopeRemoved, SceneDatasetRequirements,
    dataset_failed,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::{DatasetScope, DatasetScopeReady};

use crate::RequireAsset;
use lunco_core::{AppSceneExt, on_command, register_commands};

#[on_command(RequireAsset)]
fn on_require_asset(
    trigger: On<RequireAsset>,
    mount: Res<lunco_core::SceneMountState>,
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    registry: Res<DatasetRegistry>,
    mut commands: Commands,
) -> Result<lunco_command_contracts::Ack, String> {
    if cfg!(target_arch = "wasm32") {
        return Err("Downloadable artifact provisioning requires the desktop application".into());
    }
    let scene = mount
        .active_root()
        .ok_or("RequireAsset needs an admitted scene")?;
    let reference = &trigger.event().uri;
    let path = lunco_assets_core::asset_path::load_source_uri(reference, roots.as_deref())
        .map_err(|error| error.to_string())?;
    let uri = lunco_assets_core::engine_asset_uri(&lunco_assets_core::asset_path::anchor_of(&path));
    let address = crate::DatasetScope::for_asset_uri(&uri, roots.as_deref())?;
    if address.is_none_or(|(scope, relative)| {
        registry.is_scope_scanned(&scope) && registry.declared_artifact(&scope, &relative).is_none()
    }) {
        return Err(format!(
            "No downloadable artifact is declared for {reference}; correct the URI or declare it in Assets.toml"
        ));
    }
    commands.trigger(crate::AssetRequired { scene, uri });
    Ok(lunco_command_contracts::Ack::new(
        lunco_command_contracts::OpId::new(),
    ))
}
register_commands!(on_require_asset);

fn on_asset_required(
    event: On<crate::AssetRequired>,
    mount: Option<Res<lunco_core::SceneMountState>>,
    mut required: ResMut<crate::RuntimeAssetRequirements>,
) {
    if mount.as_ref().and_then(|mount| mount.active_root()) != Some(event.scene) {
        return;
    }
    if required.scene != Some(event.scene) {
        required.scene = Some(event.scene);
        required.uris.clear();
    }
    if !required.uris.contains(&event.uri) {
        required.uris.insert(event.uri.clone());
    }
}

/// Marker installed by the native provisioning plugin while it owns dataset
/// workers. The registry plugin uses it to leave Twin-scope retirement to the
/// worker owner, preserving the close/write barrier.
#[derive(Resource, Default)]
pub struct DatasetProvisioningActive;

/// Installs dataset declarations, scoped registry state, and discovery.
pub struct DatasetRegistryPlugin;

#[derive(Resource, Default)]
struct PendingTwinManifestScans {
    #[cfg(not(target_arch = "wasm32"))]
    scans: VecDeque<PendingTwinManifestScan>,
}

#[cfg(not(target_arch = "wasm32"))]
struct PendingTwinManifestScan {
    scope: DatasetScope,
    task: Option<Task<DatasetRegistry>>,
    result: Option<DatasetRegistry>,
}

impl PendingTwinManifestScans {
    fn cancel_root(&mut self, root: &std::path::Path) {
        #[cfg(not(target_arch = "wasm32"))]
        self.scans.retain(|scan| {
            !matches!(&scan.scope, DatasetScope::Twin { root: scan_root, .. } if scan_root == root)
        });
        #[cfg(target_arch = "wasm32")]
        let _ = root;
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn contains(&self, scope: &DatasetScope) -> bool {
        self.scans.iter().any(|scan| &scan.scope == scope)
    }
}

fn drain_registry_failures(mut registry: ResMut<DatasetRegistry>, mut commands: Commands) {
    // Draining the diagnostic outbox does not change dataset catalog facts.
    for detail in registry.bypass_change_detection().take_pending_failures() {
        commands.trigger(dataset_failed(detail));
    }
}

#[cfg(test)]
mod failure_drain_tests {
    use super::*;

    #[derive(Resource, Default)]
    struct DeliveredFailures(Vec<String>);

    #[test]
    fn dataset_projection_input_ticks_and_failure_delivery() {
        let mut app = App::new();
        app.init_resource::<DatasetRegistry>()
            .init_resource::<DeliveredFailures>()
            .add_observer(
                |event: On<lunco_telemetry_core::TelemetryEvent>,
                 mut delivered: ResMut<DeliveredFailures>| {
                    assert_eq!(event.name, crate::DATASET_FAILED);
                    let lunco_telemetry_core::TelemetryValue::String(detail) = &event.data else {
                        panic!("dataset failure must retain its detail");
                    };
                    delivered.0.push(detail.clone());
                },
            )
            .add_systems(Update, drain_registry_failures);
        app.update();
        let initial_tick = app
            .world()
            .get_resource_ref::<DatasetRegistry>()
            .unwrap()
            .last_changed();
        app.update();
        assert_eq!(
            app.world()
                .get_resource_ref::<DatasetRegistry>()
                .unwrap()
                .last_changed(),
            initial_tick
        );
        app.world_mut()
            .resource_mut::<DatasetRegistry>()
            .record_failure("invalid declaration");
        let failure_tick = app
            .world()
            .get_resource_ref::<DatasetRegistry>()
            .unwrap()
            .last_changed();
        app.update();
        assert_eq!(
            app.world().resource::<DeliveredFailures>().0,
            ["invalid declaration"]
        );
        assert_eq!(
            app.world()
                .get_resource_ref::<DatasetRegistry>()
                .unwrap()
                .last_changed(),
            failure_tick
        );
        app.update();
        assert_eq!(
            app.world().resource::<DeliveredFailures>().0.len(),
            1,
            "the diagnostic is delivered once"
        );
    }
}

/// A publication that overlaps a read of the previous file revision waits for
/// that read to settle before refreshing the path. Only outstanding work lives here.
#[derive(Resource, Default)]
struct PendingDatasetRefreshes(HashMap<AssetPath<'static>, DatasetInstalled>);

fn reload_installed_asset(
    trigger: On<DatasetInstalled>,
    asset_server: Option<Res<AssetServer>>,
    registry: Res<DatasetRegistry>,
    scene: Option<Res<SceneDatasetRequirements>>,
    runtime: Res<crate::RuntimeAssetRequirements>,
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    mut pending: ResMut<PendingDatasetRefreshes>,
    mut commands: Commands,
) {
    let Some(server) = asset_server else { return };
    let event = trigger.event();
    let Some(entry) = registry
        .installed(&event.id)
        .filter(|entry| entry.scope == event.scope)
    else {
        return;
    };
    let base = match entry.mounted_artifact_uri(roots.as_deref()) {
        Ok(base) => base,
        Err(error) => {
            commands.trigger(dataset_failed(error));
            return;
        }
    };
    let mut uris = BTreeSet::from([base.clone()]);
    if let Some(pipeline) = entry
        .spec
        .process
        .as_ref()
        .and_then(|p| p.pipeline.as_ref())
    {
        uris.extend(
            pipeline
                .artifacts
                .iter()
                .map(|member| format!("{base}/{}", member.path)),
        );
    }
    if let Some(consumed) = scene
        .as_ref()
        .and_then(|scene| scene.artifacts.get(&event.id))
    {
        uris.extend(consumed.iter().cloned());
    }
    // A newly admitted dynamic consumer can precede the next composed scene
    // requirements publication. Its concrete request already belongs to the registry.
    uris.extend(runtime.uris.iter().cloned());
    for uri in uris {
        if !registry
            .declared_uri(&uri, roots.as_deref())
            .ok()
            .flatten()
            .is_some_and(|declared| declared.id == event.id)
        {
            continue;
        }
        match lunco_assets_core::asset_path::load_asset_path(&uri, None, None, None) {
            Ok(path) if !refresh_asset_path(&server, &path) => {
                pending.0.insert(path, event.clone());
            }
            Ok(_) => {}
            Err(error) => commands.trigger(dataset_failed(format!(
                "Invalid installed artifact address: {error}"
            ))),
        }
    }
}

/// Bevy's force reload is path-wide. Serialize it with existing reads so an
/// older worker result cannot overwrite the result of this publication.
fn refresh_asset_path(server: &AssetServer, path: &AssetPath<'static>) -> bool {
    let handles = server.get_handles_untyped(path.clone());
    if handles
        .iter()
        .any(|handle| server.load_state(handle.id()).is_loading())
    {
        return false;
    }
    if handles
        .iter()
        .any(|handle| server.load_state(handle.id()).is_loaded())
    {
        server.reload(path.clone());
    } else {
        // A request for a failed handle synchronously marks it Loading and
        // retains its original typed loader configuration.
        for handle in handles {
            drop(
                server
                    .load_builder()
                    .load_erased(handle.type_id(), path.clone()),
            );
        }
    }
    true
}

fn refresh_settled_dataset_paths(
    server: Res<AssetServer>,
    registry: Res<DatasetRegistry>,
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    mut pending: ResMut<PendingDatasetRefreshes>,
) {
    pending.0.retain(|path, publication| {
        let uri = lunco_assets_core::asset_path::anchor_of(path);
        let current = registry
            .declared_uri(&uri, roots.as_deref())
            .ok()
            .flatten()
            .is_some_and(|entry| entry.id == publication.id && entry.scope == publication.scope);
        current
            && publication.scope.is_live(roots.as_deref()) == Ok(true)
            && !refresh_asset_path(&server, path)
    });
}

fn on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut registry: ResMut<DatasetRegistry>,
    mut pending_twin_scans: ResMut<PendingTwinManifestScans>,
    provisioning: Option<Res<DatasetProvisioningActive>>,
    mut commands: Commands,
) {
    pending_twin_scans.cancel_root(&trigger.event().root);
    // Native provisioning owns the worker retirement and acquires its commit
    // gate before this scope becomes invisible. A registry-only host has no
    // worker owner, so it can remove the declarations directly.
    if provisioning.is_some() {
        return;
    }
    let scopes = registry.scopes_for_root(&trigger.event().root);
    for scope in scopes {
        registry.forget_scope(&scope);
        commands.trigger(DatasetScopeRemoved { scope });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn live_dataset_scopes(
    roots: &lunco_assets_core::TwinRoots,
    workspace: Option<&lunco_workspace::WorkspaceResource>,
) -> Result<Vec<DatasetScope>, String> {
    // Preview documents can mount the same admitted root through several
    // authorities. A real open Twin owns its store; otherwise one deterministic
    // mounted authority owns standalone-source preparation.
    let mut owners = std::collections::BTreeMap::new();
    for authority in roots.authorities().map_err(|e| e.to_string())? {
        if let Some(root) = roots.root_for(&authority).map_err(|e| e.to_string())? {
            let source_identity = roots
                .logical_identity_for_authority(&authority)
                .map_err(|e| e.to_string())?;
            let display_name = roots
                .display_name_for_authority(&authority)
                .map_err(|e| e.to_string())?;
            owners
                .entry(root)
                .or_insert((source_identity, Some(authority), display_name));
        }
    }
    if let Some(workspace) = workspace {
        for (_, twin) in workspace.twins() {
            if let Some(authority) = roots
                .mounted_authority_for_logical(&lunco_assets_core::logical_twin_source_identity(
                    twin,
                ))
                .map_err(|e| e.to_string())?
            {
                if let Some(root) = roots.root_for(&authority).map_err(|e| e.to_string())? {
                    let source_identity = roots
                        .logical_identity_for_authority(&authority)
                        .map_err(|e| e.to_string())?;
                    owners.insert(
                        root,
                        (source_identity, Some(authority), twin.display_name()),
                    );
                }
            }
        }
    }
    Ok(owners
        .into_iter()
        .map(
            |(root, (source_identity, authority, display_name))| DatasetScope::Twin {
                source_identity,
                authority,
                display_name,
                root,
            },
        )
        .collect())
}

fn retire_registry_scope(
    event: On<crate::RetireDatasetScope>,
    provisioning: Option<Res<DatasetProvisioningActive>>,
    mut registry: ResMut<DatasetRegistry>,
    mut commands: Commands,
) {
    if provisioning.is_some() {
        return;
    }
    registry.forget_scope(&event.scope);
    commands.trigger(DatasetScopeRemoved {
        scope: event.scope.clone(),
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn retire_unmounted_dataset_scopes(
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    registry: Res<DatasetRegistry>,
    mut pending: ResMut<PendingTwinManifestScans>,
    mut commands: Commands,
) {
    let Some(roots) = roots else {
        return;
    };
    let live = match live_dataset_scopes(&roots, workspace.as_deref()) {
        Ok(live) => live,
        Err(error) => {
            lunco_core::trigger_runtime_error(
                &mut commands,
                "twin-dataset-registry-unavailable",
                error,
            );
            return;
        }
    };
    pending.scans.retain(|scan| live.contains(&scan.scope));
    for scope in registry
        .scanned_scopes()
        .iter()
        .filter(|scope| matches!(scope, DatasetScope::Twin { .. }) && !live.contains(scope))
    {
        commands.trigger(crate::RetireDatasetScope {
            scope: scope.clone(),
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn dataset_scopes_changed(
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    pending: Res<PendingTwinManifestScans>,
    mut revision: Local<Option<u64>>,
) -> bool {
    let current = roots.as_ref().and_then(|roots| roots.revision().ok());
    let changed = *revision != current
        || workspace
            .as_ref()
            .is_some_and(|workspace| workspace.is_changed())
        || !pending.scans.is_empty();
    *revision = current;
    changed
}

#[cfg(not(target_arch = "wasm32"))]
fn scan_open_twins_for_datasets(
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    registry: Res<DatasetRegistry>,
    mut pending: ResMut<PendingTwinManifestScans>,
    mut commands: Commands,
) {
    let Some(roots) = roots else {
        return;
    };
    let open = match live_dataset_scopes(&roots, workspace.as_deref()) {
        Ok(open) => open,
        Err(error) => {
            lunco_core::trigger_runtime_error(
                &mut commands,
                "twin-dataset-registry-unavailable",
                error,
            );
            return;
        }
    };
    for scope in open {
        if registry.is_scope_scanned(&scope) || pending.contains(&scope) {
            continue;
        }
        let DatasetScope::Twin {
            source_identity,
            authority,
            display_name,
            root,
        } = &scope
        else {
            continue;
        };
        let scan_source_identity = source_identity.clone();
        let scan_authority = authority.clone();
        let scan_display_name = display_name.clone();
        let scan_root = root.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let _span = bevy::log::info_span!("dataset_twin_manifest_scan",twin=%scan_display_name)
                .entered();
            let mut prepared = DatasetRegistry::default();
            prepared.scan_twin(
                &scan_source_identity,
                scan_authority.as_deref(),
                &scan_display_name,
                &scan_root,
            );
            prepared
        });
        pending.scans.push_back(PendingTwinManifestScan {
            scope,
            task: Some(task),
            result: None,
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn commit_pending_twin_manifest_scans(
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut pending: ResMut<PendingTwinManifestScans>,
    mut registry: ResMut<DatasetRegistry>,
    mut commands: Commands,
) {
    for scan in &mut pending.scans {
        if scan.result.is_some() {
            continue;
        }
        if let Some(result) = scan
            .task
            .as_mut()
            .and_then(|task| future::block_on(future::poll_once(task)))
        {
            scan.task = None;
            scan.result = Some(result);
        }
    }

    loop {
        let ready = pending.scans.front_mut().and_then(|scan| {
            scan.result
                .take()
                .map(|result| (scan.scope.clone(), result))
        });
        let Some((scope, prepared)) = ready else {
            break;
        };
        pending.scans.pop_front();

        let still_open = match roots
            .as_ref()
            .map(|roots| live_dataset_scopes(roots, workspace.as_deref()))
        {
            Some(Ok(live)) => live.contains(&scope),
            Some(Err(error)) => {
                lunco_core::trigger_runtime_error(
                    &mut commands,
                    "twin-dataset-registry-unavailable",
                    error,
                );
                false
            }
            None => false,
        };
        if still_open {
            let routes = match prepared.twin_asset_routes(&scope) {
                Ok(routes) => routes,
                Err(error) => {
                    lunco_core::trigger_runtime_error(
                        &mut commands,
                        "twin-dataset-asset-routes-invalid",
                        error,
                    );
                    continue;
                }
            };
            let route_result = match (&roots, &scope) {
                (
                    Some(roots),
                    DatasetScope::Twin {
                        authority: Some(authority),
                        ..
                    },
                ) => roots.replace_asset_routes(authority, routes),
                _ => Err(lunco_assets_core::TwinRootsError::RegistryUnavailable),
            };
            if let Err(error) = route_result {
                lunco_core::trigger_runtime_error(
                    &mut commands,
                    "twin-dataset-asset-routes-unavailable",
                    error.to_string(),
                );
                continue;
            }
            registry.merge_prepared(prepared);
            commands.trigger(DatasetScopeReady { scope });
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
struct PendingEngineManifestScan(Option<Task<Result<DatasetRegistry, String>>>);

#[cfg(not(target_arch = "wasm32"))]
fn start_engine_manifest_scan(mut pending: ResMut<PendingEngineManifestScan>) {
    pending.0 = Some(AsyncComputeTaskPool::get().spawn(async move {
        let _scan_span = bevy::log::info_span!("dataset_engine_manifest_scan").entered();
        let manifests = lunco_assets_core::engine_manifests()
            .map_err(|error| format!("cannot enumerate engine manifests: {error}"))?;
        let mut prepared = DatasetRegistry::default();
        for (group, path) in manifests {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    prepared.register(&text, &group);
                }
                Err(error) => {
                    prepared.record_failure(format!("cannot read {}: {error}", path.display()))
                }
            }
        }
        prepared.mark_scope_scanned(DatasetScope::Engine);
        Ok(prepared)
    }));
}

#[cfg(not(target_arch = "wasm32"))]
fn commit_engine_manifest_scan(
    mut pending: ResMut<PendingEngineManifestScan>,
    mut registry: ResMut<DatasetRegistry>,
    mut commands: Commands,
) {
    let result = pending
        .0
        .as_mut()
        .and_then(|task| future::block_on(future::poll_once(task)));
    let Some(result) = result else {
        return;
    };
    pending.0 = None;

    match result {
        Ok(prepared) => {
            let total = registry.merge_prepared(prepared);
            info!("[datasets] {total} declared dataset(s) from assets/manifests");
        }
        Err(error) => registry.record_failure(error),
    }
    commands.trigger(DatasetScopeReady {
        scope: DatasetScope::Engine,
    });
}

impl Plugin for DatasetRegistryPlugin {
    fn build(&self, app: &mut App) {
        lunco_settings::ensure_download_settings(app);
        app.init_resource::<DatasetRegistry>();
        app.init_scene_resource::<crate::RuntimeAssetRequirements>();
        app.add_observer(on_asset_required);
        register_all_commands(app);
        app.init_resource::<PendingTwinManifestScans>();
        app.init_resource::<PendingDatasetRefreshes>()
            .add_observer(reload_installed_asset)
            .add_systems(
                PreUpdate,
                refresh_settled_dataset_paths
                    .after(bevy::asset::AssetTrackingSystems)
                    .run_if(|pending: Res<PendingDatasetRefreshes>| !pending.0.is_empty()),
            );
        app.add_observer(on_twin_closed);
        app.add_observer(retire_registry_scope);
        app.add_systems(Update, drain_registry_failures);
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<PendingEngineManifestScan>()
            .add_systems(Startup, start_engine_manifest_scan)
            .add_systems(Update, commit_engine_manifest_scan);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            (
                retire_unmounted_dataset_scopes,
                ApplyDeferred,
                commit_pending_twin_manifest_scans,
                scan_open_twins_for_datasets,
            )
                .chain()
                .run_if(dataset_scopes_changed),
        );
    }
}
