//! Reusable egui renderer for Modelica `Icon` and `Diagram` graphics.
//!
//! The renderer depends only on Modelica's parsed annotation data, shared
//! theme tokens, and egui's shared asynchronous texture loaders. It is
//! independent of the Modelica workbench panels, so diagram editors and model
//! previews can share it without coupling their crates together.

pub mod icon_paint;
pub use icon_paint::*;
