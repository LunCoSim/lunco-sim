//! Egui/workbench presentation for LunCoSim's in-scene editor.
//!
//! The editor mechanisms live in `lunco-luncosim-edit-core`. This package
//! owns the interactive selection/gizmo adapters and the dockable panels that
//! observe core ECS state and emit the shared typed commands.

pub mod diagnostic_visuals;
pub mod gizmo;
pub mod joint_viz;
pub mod perf_bridge;
pub mod physics_gizmo;
pub mod physics_viz;
pub mod script_tools;
pub mod selection;
pub mod ui;
