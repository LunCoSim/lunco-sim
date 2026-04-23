//! LunCoSim visualization framework.
//!
//! See `README.md` for architecture — the three layers (signal / viz /
//! view), the dependency direction, and the roadmap for 3D / Panel3D /
//! additional viz kinds.
//!
//! This crate deliberately stays domain-agnostic: it knows how to route
//! typed samples into renderers, and nothing about Modelica, Avian, or
//! any specific producer. Domain crates depend on `lunco-viz`, not the
//! reverse.

pub mod signal;
pub mod view;
pub mod viz;
pub mod registry;
pub mod panel;
pub mod kinds;

pub use signal::{
    export_signals_to_csv, ScalarHistory, ScalarSample, SignalMeta, SignalRef, SignalRegistry,
    SignalType,
};
pub use view::{Panel2DCtx, ViewKind, ViewTarget};
pub use viz::{
    RoleSpec, SignalBinding, Visualization, VisualizationConfig, VizId, VizKindId,
};
pub use registry::{AppVizExt, VisualizationRegistry, VizFitRequests, VizKindCatalog};
pub use panel::{VizPanel, VIZ_PANEL_KIND};
pub use kinds::line_plot::{LinePlot, LINE_PLOT_KIND};

use bevy::prelude::*;
use lunco_workbench::WorkbenchAppExt;

/// Default sample capacity per scalar signal. ~20k samples covers
/// roughly 5–6 minutes of 60 Hz stepping or ~3 hours at 2 Hz — long
/// enough that scrolling back through a long-running simulation is
/// useful, short enough that the ring buffer stays under ~2 MB per
/// signal worst-case (16 B × 20 000).
pub const DEFAULT_SIGNAL_HISTORY: usize = 20_000;

/// Install the visualization framework.
///
/// Registers:
///
/// * `SignalRegistry` resource (default sample horizon
///   `DEFAULT_SIGNAL_HISTORY`).
/// * `VisualizationRegistry` + `VizKindCatalog` resources.
/// * Built-in `LinePlot` viz kind.
/// * `VizPanel` as a multi-instance workbench panel.
///
/// Domain plugins (`ModelicaPlugin`, future Avian bridge, …) are
/// expected to be added *after* this plugin so they see the registry
/// resources on app build.
pub struct LuncoVizPlugin;

impl Plugin for LuncoVizPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SignalRegistry::with_default_capacity(
            DEFAULT_SIGNAL_HISTORY,
        ))
            .init_resource::<VisualizationRegistry>()
            .init_resource::<VizKindCatalog>()
            .init_resource::<VizFitRequests>()
            .register_visualization::<LinePlot>()
            .register_instance_panel(VizPanel::default());
    }
}
