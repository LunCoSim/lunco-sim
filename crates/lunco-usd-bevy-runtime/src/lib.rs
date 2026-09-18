//! Application-level composition of the USD runtime projections.
//!
//! The individual visual, diagnostics, physics, simulation, and document
//! command crates remain independently usable. This package owns only the
//! convenience bundle used by a complete application, keeping that aggregate
//! dependency closure out of headless USD document consumers. The Modelica/Rhai
//! co-simulation projection is opt-in through the `cosim` feature.

use bevy::prelude::{App, Plugin};

/// Install the standard USD runtime projection stack.
///
/// Enable the package's `cosim` feature when authored Modelica/Rhai
/// participants and USD co-simulation wiring are part of the host.
pub struct UsdPlugins;

impl Plugin for UsdPlugins {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            lunco_usd_commands::UsdCommandsPlugin,
            lunco_usd_bevy_runtime_core::UsdSceneRuntimePlugin,
            lunco_usd_bevy::UsdVisualPlugin,
            lunco_usd_bevy_animation::UsdAnimationPlugin,
            lunco_usd_bevy_diagnostics::UsdDiagnosticsPlugin,
            lunco_usd_avian::UsdAvianPlugin,
            lunco_usd_sim::UsdSimPlugin,
        ));
        #[cfg(feature = "cosim")]
        app.add_plugins(lunco_usd_sim_cosim::UsdSimCosimPlugin);
        #[cfg(feature = "api")]
        app.add_plugins(lunco_usd_sim_cosim_api::UsdSimCosimApiPlugin);
        #[cfg(feature = "api")]
        app.add_plugins(lunco_usd_sim_domain_api::UsdSimDomainApiPlugin);
    }
}
