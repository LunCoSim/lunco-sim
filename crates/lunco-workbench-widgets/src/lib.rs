//! Reusable egui widgets shared by LunCoSim domain UI crates.
//!
//! These controls deliberately do not depend on the concrete workbench shell.
//! They can therefore be used by lightweight panels, headless composition
//! code, and alternate hosts without pulling in docking, viewport, or window
//! management.
//!
//! Application text fields use [`text_editor`] builders so domain panels can
//! add their own labels, validation, and commit behavior without creating a
//! second widget setup. Panels own any containing `ScrollArea` needed to bound
//! multiline editors or long result lists.

pub mod icons;
pub mod text_editor;
pub mod tree;

pub use icons::{UiIcon, icon_button, icon_button_sized, icon_text_button, paint_icon};
