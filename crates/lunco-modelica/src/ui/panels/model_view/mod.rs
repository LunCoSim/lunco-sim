//! Modelica multi-instance view — the central hub for model editing.

pub mod context;
pub mod render;

// Data types (`types`), the `ModelTabs` registry (`tabs`), the
// documentation extractor (`parsing`), and the default-simulation-class
// resolver (`context::default_simulation_class` & friends) all moved to
// egui-free core modules:
//   - `crate::model_tabs_types` — MODEL_VIEW_KIND, ModelTabState, …
//   - `crate::model_tabs`       — ModelTabs
//   - `crate::doc_extract`      — extract_documentation
//   - `crate::sim_default`      — default_simulation_class, RunTargetOverrides, …
pub use context::sync_active_tab_to_doc;
pub use render::ModelViewPanel;

use crate::model_tabs::ModelTabs;
use crate::model_tabs_types::TabRenderContext;
use bevy::prelude::*;
use lunco_workbench::WorkbenchAppExt;

pub struct ModelViewPlugin;

impl Plugin for ModelViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ModelTabs>()
            .init_resource::<TabRenderContext>()
            .init_resource::<crate::sim_default::RunTargetOverrides>()
            .register_instance_panel(ModelViewPanel::default());
    }
}
