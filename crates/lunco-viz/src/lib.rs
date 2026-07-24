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
//!
//! # Feature `ui`
//!
//! Everything that renders — viz kinds, `VizPanel`, the registry
//! plumbing, [`LuncoVizPlugin`] — sits behind the `ui` feature (off by
//! default), which is what links bevy_egui/egui_plot/workbench. A plain
//! dependency gets only the [`signal`] re-export of `lunco-signal`, so
//! it stays render-free.

#[cfg(feature = "ui")]
pub mod kinds;
#[cfg(feature = "ui")]
pub mod panel;
#[cfg(feature = "ui")]
pub mod plot_fmt;
#[cfg(feature = "ui")]
pub mod registry;
pub mod signal;
#[cfg(feature = "ui")]
pub mod view;
#[cfg(feature = "ui")]
pub mod viz;

#[cfg(feature = "ui")]
pub use kinds::line_plot::{LinePlot, LINE_PLOT_KIND};
#[cfg(feature = "ui")]
pub use panel::{VizPanel, VIZ_PANEL_KIND};
#[cfg(feature = "ui")]
pub use registry::{AppVizExt, VisualizationRegistry, VizFitRequests, VizKindCatalog};
pub use signal::{ScalarHistory, ScalarSample, SignalMeta, SignalRef, SignalRegistry, SignalType};
#[cfg(feature = "ui")]
pub use view::{Panel2DCtx, ViewKind, ViewTarget};
#[cfg(feature = "ui")]
pub use viz::{RoleSpec, SignalBinding, Visualization, VisualizationConfig, VizId, VizKindId};

#[cfg(feature = "ui")]
use bevy::prelude::*;
#[cfg(feature = "ui")]
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
#[cfg(feature = "ui")]
pub struct LuncoVizPlugin;

#[cfg(feature = "ui")]
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
