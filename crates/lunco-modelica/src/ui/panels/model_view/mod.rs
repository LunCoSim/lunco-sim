//! Modelica multi-instance view — the central hub for model editing.

pub mod types;
pub mod tabs;
pub mod context;
pub mod render;
pub mod parsing;

pub use types::{MODEL_VIEW_KIND, ModelTabState, ModelViewMode, TabId, TabRenderContext};
pub use tabs::ModelTabs;
pub use context::{default_simulation_class, drilled_class_for_doc, set_run_target_for_doc, sync_active_tab_to_doc, RunTargetOverrides};
pub use render::ModelViewPanel;
pub use parsing::extract_documentation;

use bevy::prelude::*;
use lunco_workbench::WorkbenchAppExt;

pub struct ModelViewPlugin;

impl Plugin for ModelViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ModelTabs>()
            .init_resource::<TabRenderContext>()
            .init_resource::<context::RunTargetOverrides>()
            .register_instance_panel(ModelViewPanel::default());
    }
}
