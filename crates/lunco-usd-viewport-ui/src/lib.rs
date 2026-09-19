//! Workbench UI for the interactive USD preview viewport.
//!
//! This package owns only the egui panels over the preview runtime. Session
//! state, offscreen render targets, viewport camera interaction, preview
//! commands, and query providers live in `lunco-usd-viewport-runtime`.
//! Document and Twin-browser lifecycle presentation remains in `lunco-usd-ui`.

use bevy::prelude::{App, Plugin};
use lunco_workbench_core::WorkbenchPanelAppExt;

mod query;
mod viewport;

/// Install the USD preview workbench panels.
///
/// Add [`lunco_usd_viewport_runtime::UsdViewportPlugin`] separately to install
/// the session and render runtime that these panels display.
pub struct UsdViewportUiPlugin;

impl Plugin for UsdViewportUiPlugin {
    fn build(&self, app: &mut App) {
        query::register_api_queries(app);
        app.register_panel(viewport::UsdViewportPanel)
            .register_instance_panel(viewport::UsdPreviewViewPanel);
    }
}
