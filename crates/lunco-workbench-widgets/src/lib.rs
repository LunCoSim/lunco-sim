//! Reusable egui widgets shared by LunCoSim domain UI crates.
//!
//! These controls deliberately do not depend on the concrete workbench shell.
//! They can therefore be used by lightweight panels, headless composition
//! code, and alternate hosts without pulling in docking, viewport, or window
//! management.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod icons;
pub mod text_editor;
pub mod tree;

pub use icons::{icon_button, icon_button_sized, icon_text_button, paint_icon, UiIcon};
