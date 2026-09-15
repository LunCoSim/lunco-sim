//! Native Modelica source-library provisioning and indexing.
//!
//! The compiler crate owns runtime source access and parsed-bundle loading.
//! This package owns the host-only work of scanning a source library and
//! building the editor index. The workbench composes that tool through its
//! application plugin, keeping the asset tool itself independent of Bevy.

#[cfg(not(target_arch = "wasm32"))]
pub mod indexer;
