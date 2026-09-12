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

/// Which sub-part of [`lunco_scene_commands::SelectedEntities`] the Inspector
/// edits. `None` means the complete selected object.
#[derive(bevy::prelude::Resource, Default)]
pub struct InspectorTarget {
    /// The targeted sub-part entity, or `None` for the whole object.
    pub part: Option<bevy::prelude::Entity>,
}
