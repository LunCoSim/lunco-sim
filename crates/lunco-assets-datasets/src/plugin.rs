//! Bevy integration for dataset discovery and the read-side registry.
//!
//! This plugin is intentionally independent of native provisioning. It keeps
//! manifest discovery, lifecycle state, failure reporting, and asset reloads
//! available to headless and browser-safe hosts without compiling HTTP,
//! archive, image, or GeoTIFF processing. The explicit provisioning plugin in
//! `lunco-assets` owns workers and must be installed by an application that
//! wants `RequestDataset` downloads or `ProcessDataset` bakes to perform
//! native I/O.

use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::VecDeque;

use crate::{DatasetRegistry, DatasetScopeRemoved, dataset_failed};
#[cfg(not(target_arch = "wasm32"))]
use crate::{DatasetScope, DatasetScopeReady};

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
    for detail in registry.take_pending_failures() {
        commands.trigger(dataset_failed(detail));
    }
}

fn reload_installed_asset(
    trigger: On<crate::DatasetInstalled>,
    asset_server: Option<Res<AssetServer>>,
) {
    if let Some(asset_server) = asset_server {
        asset_server.reload(trigger.event().artifact_uri.clone());
    }
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
fn scan_open_twins_for_datasets(
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
    registry: Res<DatasetRegistry>,
    mut pending: ResMut<PendingTwinManifestScans>,
    mut commands: Commands,
) {
    let Some(roots) = roots else {
        return;
    };
    let open = match roots.names() {
        Ok(open) => open,
        Err(error) => {
            lunco_core::trigger_runtime_error(
                &mut commands,
                "twin-dataset-registry-unavailable",
                format!("could not enumerate open Twins for dataset discovery: {error}"),
            );
            return;
        }
    };
    for name in open {
        match roots.root_for(&name) {
            Ok(Some(root)) => {
                let scope = DatasetScope::Twin {
                    name: name.clone(),
                    root: root.clone(),
                };
                if registry.is_scope_scanned(&scope) || pending.contains(&scope) {
                    continue;
                }
                let scan_name = name.clone();
                let scan_root = root.clone();
                let task = AsyncComputeTaskPool::get().spawn(async move {
                    let _scan_span = bevy::log::info_span!(
                        "dataset_twin_manifest_scan",
                        twin = %scan_name
                    )
                    .entered();
                    let mut prepared = DatasetRegistry::default();
                    prepared.scan_twin(&scan_name, &scan_root);
                    prepared
                });
                pending.scans.push_back(PendingTwinManifestScan {
                    scope,
                    task: Some(task),
                    result: None,
                });
            }
            Ok(None) => {}
            Err(error) => {
                lunco_core::trigger_runtime_error(
                    &mut commands,
                    "twin-dataset-registry-unavailable",
                    format!("could not resolve Twin {name} for dataset discovery: {error}"),
                );
                return;
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn commit_pending_twin_manifest_scans(
    roots: Option<Res<lunco_assets_core::TwinRoots>>,
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

        let DatasetScope::Twin { name, root } = &scope else {
            continue;
        };
        let still_open = match roots.as_deref() {
            Some(roots) => match roots.root_for(name) {
                Ok(Some(current)) => current.as_path() == root.as_path(),
                Ok(None) => false,
                Err(error) => {
                    lunco_core::trigger_runtime_error(
                        &mut commands,
                        "twin-dataset-registry-unavailable",
                        format!(
                            "could not resolve Twin {name} while completing dataset discovery: {error}"
                        ),
                    );
                    false
                }
            },
            None => false,
        };
        if still_open {
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
        let manifests = lunco_assets_core::engine_manifests().map_err(|error| {
            format!(
                "cannot enumerate engine manifests in {}: {error}",
                lunco_assets_core::manifests_dir().display()
            )
        })?;
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
        app.init_resource::<PendingTwinManifestScans>();
        app.add_observer(reload_installed_asset);
        app.add_observer(on_twin_closed);
        app.add_systems(Update, drain_registry_failures);
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<PendingEngineManifestScan>()
            .add_systems(Startup, start_engine_manifest_scan)
            .add_systems(Update, commit_engine_manifest_scan);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, scan_open_twins_for_datasets);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(
            Update,
            commit_pending_twin_manifest_scans.before(scan_open_twins_for_datasets),
        );
    }
}
