//! Presentation-only celestial systems.
//!
//! Semantic astronomy and BigSpace placement remain in their owning runtime
//! packages. This crate owns the expensive, optional presentation layer:
//! trajectory sampling, mesh construction, alignment, and visibility.

mod trajectories;

pub use trajectories::{TrajectoryMeshMarker, TrajectoryPlugin, mission_visibility_system};

/// Installs trajectory presentation systems without changing the semantic
/// celestial or spatial runtime composition.
pub struct CelestialPresentationPlugin;

impl bevy::prelude::Plugin for CelestialPresentationPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.add_plugins(TrajectoryPlugin);
    }
}
