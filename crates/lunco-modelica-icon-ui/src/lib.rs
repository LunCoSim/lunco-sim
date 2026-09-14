//! Reusable egui renderer for Modelica `Icon` and `Diagram` graphics.
//!
//! The renderer depends only on Modelica's parsed annotation data, shared
//! theme tokens, and the MSL asset-source boundary for bitmap primitives. It is
//! independent of the Modelica workbench panels, so diagram editors and model
//! previews can share it without coupling their crates together.

#![forbid(unsafe_code)]

pub mod icon_paint;
pub mod image_loader;

pub use icon_paint::*;
pub use image_loader::ModelicaImageLoader;

/// Install the raster decoders and the Modelica `modelica://` byte loader.
///
/// The caller owns the one-time Bevy scheduling guard; this function only
/// installs the egui loaders once its context is ready.
pub fn install_image_loaders(ctx: &bevy_egui::egui::Context) {
    egui_extras::install_image_loaders(ctx);
    ctx.add_bytes_loader(std::sync::Arc::new(ModelicaImageLoader::new()));
}
