//! Lightweight contracts for declared, downloadable datasets.
//!
//! This package owns the data contract shared by dataset consumers: manifest
//! declarations, scoped identity, lifecycle state, artifact paths, and the
//! Bevy registry/events. The native provisioning implementation remains in
//! [`lunco-assets`], which adds HTTP, archive, image, GeoTIFF, and processing
//! dependencies only at the application boundary that explicitly downloads or
//! processes data.

mod manifest;
pub mod pipeline;
mod plugin;
mod registry;

pub use manifest::{
    AssetEntry, AssetManifest, PROCESS_PIPELINE_VERSION, ProcessConfig, archive_extension,
    bake_stamp_path, dataset_cache_root, default_dem_pixel_scale_m, entry_artifact_path,
    entry_dest_path, install_marker_path, process_output_path, source_pool_path,
};
#[cfg(not(target_arch = "wasm32"))]
pub use manifest::{
    bake_key, installed_destination_present, pds_detached_label, processed_output_present,
    source_read_path, version_marker_path,
};
pub use plugin::{DatasetProvisioningActive, DatasetRegistryPlugin};
pub use registry::{
    AssetRequired, CancelDataset, DATASET_FAILED, DatasetEntry, DatasetInput, DatasetInputState,
    DatasetInstalled, DatasetRegistry, DatasetScope, DatasetScopeReady, DatasetScopeRemoved,
    DatasetState, ProcessDataset, RequestDataset, RequireAsset, RetireDatasetScope,
    RuntimeAssetRequirements, dataset_failed, dataset_id, dataset_state_changed,
};

/// Current scene's declared data dependencies. The USD owner supplies composed
/// identities; consumers share these facts without reading files or scanning USD.
#[derive(bevy::prelude::Resource, Default, Clone, Debug, PartialEq, Eq)]
pub struct SceneDatasetRequirements {
    /// Exact mounted scene lease whose dependency discovery has completed.
    pub scene: Option<bevy::prelude::Entity>,
    /// Declared datasets referenced by the selected scene, regardless of recommendation.
    pub ids: std::collections::BTreeSet<String>,
    /// Discovery must finish before absence of a declaration can be established.
    pub pending_scopes: std::collections::BTreeSet<String>,
}
impl lunco_core::SceneResource for SceneDatasetRequirements {}

/// Admit a handle used by a runtime consumer, rather than by catalog discovery.
/// The same typed asset handle remains owned by the caller while data is missing.
pub fn required_asset_data_pending(
    id: impl Into<bevy::asset::UntypedAssetId>,
    server: &bevy::asset::AssetServer,
    registry: Option<&DatasetRegistry>,
    roots: Option<&lunco_assets_core::TwinRoots>,
    scene: Option<bevy::prelude::Entity>,
    commands: &mut bevy::prelude::Commands,
) -> bool {
    if cfg!(target_arch = "wasm32") {
        return false;
    }
    let (Some(registry), Some(scene)) = (registry, scene) else {
        return false;
    };
    let id = id.into();
    let Some(path) = server.get_path(id) else {
        return false;
    };
    let uri = lunco_assets_core::engine_asset_uri(&lunco_assets_core::asset_path::anchor_of(&path));
    let mut pending = registry.uri_scope_scanned(&uri, roots) == Ok(false);
    if let Ok(Some(entry)) = registry.declared_uri(&uri, roots) {
        commands.trigger(AssetRequired { scene, uri });
        pending |= !entry.state.is_installed();
    }
    // Publication restarts failed handles synchronously. A parent can still
    // carry an earlier recursive failure while its child loads this revision.
    pending || server.load_state(id).is_loading()
}
