//! Application-level composition of the USD runtime projections.
//!
//! The individual visual, diagnostics, physics, simulation, and document
//! command crates remain independently usable. This package owns only the
//! convenience bundle used by a complete application, keeping that aggregate
//! dependency closure out of headless USD document consumers. Vehicle and
//! simulation projection is enabled by the default `simulation` feature and
//! can be omitted by lean runtime consumers; the Modelica/Rhai co-simulation
//! projection is opt-in through the `cosim` feature, which also enables
//! `simulation`.

use bevy::prelude::{App, IntoScheduleConfigs, Plugin};

/// Install the standard USD runtime projection stack.
///
/// Enable the package's `simulation` feature when standard vehicle/simulation
/// projection is part of the host. Enable `cosim` when authored Modelica/Rhai
/// participants and USD co-simulation wiring are part of the host; `cosim`
/// includes `simulation`.
pub struct UsdPlugins;

impl Plugin for UsdPlugins {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            lunco_usd_commands::UsdCommandsPlugin,
            lunco_usd_queries::UsdQueriesPlugin,
            lunco_usd_bevy_runtime_core::UsdSceneRuntimePlugin,
            lunco_usd_bevy::UsdVisualPlugin,
            lunco_usd_bevy_animation::UsdAnimationPlugin,
            lunco_usd_bevy_diagnostics::UsdDiagnosticsPlugin,
            lunco_usd_avian::UsdAvianPlugin,
        ));
        #[cfg(feature = "simulation")]
        app.add_plugins((
            lunco_usd_sim_shader::UsdShaderPlugin,
            lunco_usd_sim_celestial::CelestialProjectionPlugin,
            lunco_usd_sim::UsdSimPlugin,
            lunco_usd_sim_telemetry::PhysicsTelemetryPlugin,
        ));
        #[cfg(feature = "simulation")]
        app.configure_sets(
            bevy::prelude::Update,
            lunco_usd_sim_celestial::CelestialProjectionSet::Projection
                .before(lunco_usd_sim_core::UsdSimSet::Projection),
        );
        #[cfg(feature = "cosim")]
        app.add_plugins(lunco_usd_sim_cosim::UsdSimCosimPlugin);
        #[cfg(feature = "api")]
        app.add_plugins(lunco_usd_sim_cosim_api::UsdSimCosimApiPlugin);
        #[cfg(feature = "api")]
        app.add_plugins(lunco_usd_sim_domain_api::UsdSimDomainApiPlugin);
    }
}
