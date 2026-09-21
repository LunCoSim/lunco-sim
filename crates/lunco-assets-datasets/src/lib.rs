//! Lightweight contracts for declared, downloadable datasets.
//!
//! This package owns the data contract shared by dataset consumers: manifest
//! declarations, scoped identity, lifecycle state, artifact paths, and the
//! Bevy registry/events. The native provisioning implementation remains in
//! [`lunco-assets`], which adds HTTP, archive, image, GeoTIFF, and processing
//! dependencies only at the application boundary that explicitly downloads or
//! processes data.

mod manifest;
mod plugin;
mod registry;

pub use manifest::{
    archive_extension, bake_stamp_path, default_dem_pixel_scale_m, entry_artifact_path,
    entry_dest_path, install_marker_path, process_output_path, source_pool_path, AssetEntry,
    AssetManifest, ProcessConfig, PROCESS_PIPELINE_VERSION,
};
#[cfg(not(target_arch = "wasm32"))]
pub use manifest::{
    bake_key, installed_destination_present, processed_output_present, version_marker_path,
};
pub use plugin::{DatasetProvisioningActive, DatasetRegistryPlugin};
pub use registry::{
    dataset_failed, dataset_id, CancelDataset, DatasetEntry, DatasetInstalled, DatasetRegistry,
    DatasetScope, DatasetScopeReady, DatasetScopeRemoved, DatasetState, ProcessDataset,
    RequestDataset, DATASET_FAILED,
};
