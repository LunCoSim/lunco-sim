//! Runtime asset integrations built on the platform-neutral asset core.
//!
//! [`lunco_assets_core`] owns canonical paths, cache roots, and scheme
//! resolution. This package owns the Bevy-facing source registration,
//! discovery, library/model catalogs, text/script assets, and web fetch
//! adapters. The split keeps path-only consumers from compiling the source and
//! catalog machinery.

#![allow(clippy::disallowed_methods)]

pub mod asset_read;
pub mod asset_sources;
pub mod dataset_artifact;
pub mod discovery;
pub mod font;
pub mod library;
pub mod models;
pub mod script_source;
pub mod scripting;
pub mod text_asset;

#[cfg(not(target_arch = "wasm32"))]
pub use asset_read::read_asset_text;
pub use asset_sources::{
    TwinAssetMounted, TwinRootsPlugin, register_lunco_asset_sources, register_lunco_asset_types,
};
pub use dataset_artifact::{
    DatasetArtifactPlugin, DatasetTextArtifactReady, ReadDatasetTextArtifact,
};
#[cfg(not(target_arch = "wasm32"))]
pub use lunco_assets_core::closure::{transitive_file_closure, transitive_file_closure_with};
pub use text_asset::{
    JsonAssetRecord, JsonAssetScopeChanged, JsonAssetScopeLoading, TextAsset, TextAssetCatalog,
    TextAssetEntry, TextAssetLoader, TextAssetPlugin,
};
