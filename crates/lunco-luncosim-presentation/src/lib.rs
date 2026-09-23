//! Application-owned presentation projections.
//!
//! The shared simulator does not own status mirroring, camera-path recording
//! coordination, or streamed-terrain shadow wiring. The GUI application adds
//! this plugin after its headless-safe composition is ready.

mod presentation_bridge;
mod terrain_horizon;

/// Register all GUI-facing simulator projections.
pub fn register(app: &mut bevy::prelude::App) {
    app.add_plugins(lunco_terrain_surface::TerrainSurfaceVisualizationPlugin);
    presentation_bridge::register(app);
}
