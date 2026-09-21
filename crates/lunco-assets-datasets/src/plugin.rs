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

use crate::{
    dataset_failed, DatasetRegistry, DatasetScope, DatasetScopeReady, DatasetScopeRemoved,
};

/// Marker installed by the native provisioning plugin while it owns dataset
/// workers. The registry plugin uses it to leave Twin-scope retirement to the
/// worker owner, preserving the close/write barrier.
#[derive(Resource, Default)]
pub struct DatasetProvisioningActive;

/// Installs dataset declarations, scoped registry state, and discovery.
pub struct DatasetRegistryPlugin;

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
    provisioning: Option<Res<DatasetProvisioningActive>>,
    mut commands: Commands,
) {
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
    mut registry: ResMut<DatasetRegistry>,
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
                if registry.is_scope_scanned(&scope) {
                    continue;
                }
                registry.scan_twin(&name, &root);
                commands.trigger(DatasetScopeReady { scope });
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
fn scan_engine_manifests(mut registry: ResMut<DatasetRegistry>, mut commands: Commands) {
    let manifests = match lunco_assets_core::engine_manifests() {
        Ok(manifests) => manifests,
        Err(error) => {
            registry.record_failure(format!(
                "cannot enumerate engine manifests in {}: {error}",
                lunco_assets_core::manifests_dir().display()
            ));
            commands.trigger(DatasetScopeReady {
                scope: DatasetScope::Engine,
            });
            return;
        }
    };
    let mut total = 0;
    for (group, path) in manifests {
        match std::fs::read_to_string(&path) {
            Ok(text) => total += registry.register(&text, &group),
            Err(error) => {
                registry.record_failure(format!("cannot read {}: {error}", path.display()))
            }
        }
    }
    info!("[datasets] {total} declared dataset(s) from assets/manifests");
    commands.trigger(DatasetScopeReady {
        scope: DatasetScope::Engine,
    });
}

impl Plugin for DatasetRegistryPlugin {
    fn build(&self, app: &mut App) {
        lunco_settings::ensure_download_settings(app);
        app.init_resource::<DatasetRegistry>();
        app.add_observer(reload_installed_asset);
        app.add_observer(on_twin_closed);
        app.add_systems(Update, drain_registry_failures);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Startup, scan_engine_manifests);
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, scan_open_twins_for_datasets);
    }
}
