//! Application-level composition of the USD runtime projections.
//!
//! The individual visual, diagnostics, physics, simulation, and document
//! command crates remain independently usable. This package owns only the
//! convenience bundle used by a complete application, keeping that aggregate
//! dependency closure out of headless USD document consumers.

use bevy::prelude::{App, Plugin};

/// Install the complete USD runtime projection stack.
pub struct UsdPlugins;

impl Plugin for UsdPlugins {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            lunco_usd_bevy::UsdBevyPlugin,
            lunco_usd_bevy_diagnostics::UsdDiagnosticsPlugin,
            lunco_usd_avian::UsdAvianPlugin,
            lunco_usd_sim::UsdSimPlugin,
        ));
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
    }
}
