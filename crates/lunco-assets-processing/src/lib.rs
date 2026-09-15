//! Native offline asset-processing pipelines.
//!
//! The public surface is deliberately small: Rust owns decoding, raster math,
//! cancellation, staging, and atomic commit. Rhai
//! selects and composes authored processing policy through the dataset command
//! surface; it does not perform this heavy work itself.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![allow(clippy::disallowed_methods)]

#[cfg(not(target_arch = "wasm32"))]
pub mod pds_img;
#[cfg(not(target_arch = "wasm32"))]
pub mod process;
