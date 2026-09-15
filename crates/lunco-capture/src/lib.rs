//! Render-bound screenshot and offline recording capabilities.
//!
//! The API command and GPU readback implementation live here so the generic
//! workbench shell does not own capture-specific source or dependencies. Add
//! the `api` feature when the host exposes the screenshot command surface.

#[cfg(feature = "api")]
pub mod screenshot;
