//! Egui/workbench presentation for LunCoSim's in-scene editor.
//!
//! The editor mechanisms live in `lunco-luncosim-edit-core`. This package
//! owns the interactive selection adapter and dockable panels that observe
//! core ECS state and emit the shared typed commands. The focused transform
//! transaction adapter lives in the sibling
//! `lunco-luncosim-edit-gizmo-ui` package.

pub mod diagnostic_visuals;
pub mod joint_viz;
pub mod perf_bridge;
pub mod physics_gizmo;
pub mod physics_viz;
pub mod script_tools;
pub mod selection;
pub mod ui;
