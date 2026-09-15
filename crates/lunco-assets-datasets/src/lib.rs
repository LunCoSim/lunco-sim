//! Lightweight contracts for declared, downloadable datasets.
//!
//! This package owns the data contract shared by dataset consumers: manifest
//! declarations, scoped identity, lifecycle state, artifact paths, and the
//! Bevy registry/events. The native provisioning implementation remains in
//! [`lunco-assets`], which adds HTTP, archive, image, GeoTIFF, and processing
//! dependencies only at the application boundary that explicitly downloads or
//! processes data.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod manifest;
mod plugin;
mod registry;

pub use manifest::{
    archive_extension, bake_key, bake_stamp_path, default_dem_pixel_scale_m, entry_artifact_path,
    entry_dest_path, install_marker_path, installed_destination_present, process_output_path,
    processed_output_present, source_pool_path, version_marker_path, AssetEntry, AssetManifest,
    ProcessConfig, PROCESS_PIPELINE_VERSION,
};
pub use plugin::{DatasetProvisioningActive, DatasetRegistryPlugin};
pub use registry::{
    dataset_failed, dataset_id, CancelDataset, DatasetEntry, DatasetInstalled, DatasetRegistry,
    DatasetScope, DatasetScopeReady, DatasetScopeRemoved, DatasetState, RequestDataset,
    DATASET_FAILED,
};
