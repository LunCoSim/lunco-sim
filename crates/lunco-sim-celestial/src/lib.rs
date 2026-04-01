use bevy::prelude::*;
use bevy::math::DVec3;
use lunco_sim_core::TimeWarpState;

mod clock;
mod ephemeris;
mod registry;
mod big_space_setup;
mod systems;
mod coords;
mod camera;
mod gravity;
mod soi;
mod terrain;
mod trajectories;

pub use clock::*;
pub use ephemeris::*;
pub use registry::*;
pub use big_space_setup::*;
pub use systems::*;
pub use camera::*;
pub use gravity::*;
pub use soi::*;
pub use terrain::*;
pub use trajectories::*;

#[derive(Event, Debug, Clone, Copy)]
pub struct SurfaceClickEvent {
    pub planet: Entity,
    pub click_pos_local: DVec3, // Relative to planet center (solar/root units)
    pub surface_normal: Vec3,
}

#[derive(Event, Debug, Clone, Copy)]
pub struct RoverClickEvent {
    pub rover: Entity,
}

pub struct CelestialPlugin;

impl Plugin for CelestialPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CelestialClock>();
        app.init_resource::<TimeWarpState>();
        app.init_resource::<TerrainTileConfig>();
        app.insert_resource(CelestialBodyRegistry::default_system());
        
        app.insert_resource(ephemeris::EphemerisResource {
            provider: Box::new(ephemeris::CelestialEphemerisProvider::new()),
        });
        
        // No add_event needed in Bevy 0.18 Observer pattern
        app.add_plugins(big_space::prelude::BigSpaceDefaultPlugins);
        app.add_plugins(trajectories::TrajectoryPlugin);

        app.add_systems(Startup, big_space_setup::setup_big_space_hierarchy);
        
        // --- LEAD-PHASE SYNCHRONIZATION ---
        // We move core celestial updates to PreUpdate 
        // to ensure Coordinate Stability for Gizmos (Update) and Physics (FixedUpdate)
        app.add_systems(PreUpdate, (
            celestial_clock_tick_system,
            ephemeris_update_system,
            body_rotation_system,
            soi_transition_system,
        ).chain());

        app.add_plugins(camera::CameraMigrationPlugin);

        app.add_systems(Update, (
            update_sun_light_system,
            celestial_telemetry_system,
        ).chain());
        
        app.add_systems(Update, (
            gravity::update_global_gravity_system.run_if(resource_exists::<avian3d::prelude::Gravity>),
            terrain::terrain_spawn_system.run_if(resource_exists::<terrain::TerrainTileConfig>),
        ).chain());
    }
}
