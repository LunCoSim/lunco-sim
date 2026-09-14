//! Native dataset/download and offline asset-processing runtime.
//!
//! The lightweight asset identity, source, and storage APIs live in
//! [`lunco-assets-core`]. Dataset declarations, registry state, and lifecycle
//! commands live in [`lunco-assets-datasets`]. This package owns only the
//! operations that intentionally carry archive, HTTP, image, GeoTIFF, and
//! native processing dependencies. Keep ordinary runtime consumers on the
//! lighter packages; add this package at an application or explicit
//! dataset-provisioning boundary.

#![allow(clippy::disallowed_methods)]

pub mod datasets;
pub mod download;
#[cfg(not(target_arch = "wasm32"))]
pub mod pds_img;
pub mod process;
