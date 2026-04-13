//! Solar system simulation and celestial mechanics.
//!
//! This crate implements the core of the LunCo digital twin, including:
//! - **Ephemeris**: High-precision planetary positioning and rotation data.
//! - **Gravity**: Per-entity surface gravity using body-fixed coordinates.
//! - **SOI (Sphere of Influence)**: Automatic coordinate frame transitions.
//! - **Terrain**: Dynamic procedural terrain generation for planetary surfaces.
//! - **Trajectories**: Rendering of orbital paths and mission predictions.

use bevy::prelude::*;
use bevy::math::DVec3;
#[cfg(not(target_arch = "wasm32"))]
use bevy::asset::load_internal_asset;
use bevy_shader::Shader;

mod clock;
mod ephemeris;
mod registry;
mod big_space_setup;
mod systems;
mod coords;
mod gravity;
mod soi;
mod terrain;
mod trajectories;
mod missions;
mod embedded_assets;

/// UI panels for celestial time control and body browser.
pub mod ui;

pub use clock::*;
pub use ephemeris::*;
pub use registry::*;
pub use big_space_setup::*;
pub use systems::*;
pub use gravity::*;
pub use soi::*;
pub use terrain::*;
pub use trajectories::*;
pub use missions::*;
pub use embedded_assets::*;

// Re-export blueprint material types from lunco-materials (the canonical source).
pub use lunco_materials::{BlueprintExtension, BlueprintMaterial, BlueprintMaterialPlugin, BLUEPRINT_SHADER_HANDLE};

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
        // EmbeddedAssetsPlugin embeds shaders/textures/missions on wasm32, no-op on desktop
        app.add_plugins(embedded_assets::EmbeddedAssetsPlugin);

        // Register blueprint shader only on desktop (wasm32 handled by EmbeddedAssetsPlugin).
        // MaterialPlugin is added separately by each binary — this avoids requiring full
        // render resources in headless/integration tests.
        #[cfg(not(target_arch = "wasm32"))]
        {
            load_internal_asset!(
                app,
                BLUEPRINT_SHADER_HANDLE,
                "../../../assets/shaders/blueprint_extension.wgsl",
                Shader::from_wgsl
            );
            app.add_plugins(trajectories::TrajectoryShaderPlugin);
        }

        app.insert_resource(get_default_celestial_clock());
        app.init_resource::<TimeWarpState>();
        app.init_resource::<TerrainTileConfig>();
        app.init_resource::<TerrainMapRegistry>();
        app.insert_resource(Gravity::surface());
        app.insert_resource(terrain::TerrainSpawnCooldown::default());
        app.register_type::<TerrainTileConfig>();
        app.register_type::<TileCoord>();
        app.register_type::<TrajectoryView>();
        app.register_type::<TrajectoryFrame>();
        app.register_type::<TrajectoryPath>();
        app.insert_resource(CelestialBodyRegistry::default_system());

        // On wasm32, EphemerisResource is set by EmbeddedAssetsPlugin using embedded CSV data.
        // On desktop, it's initialized here with filesystem access.
        #[cfg(not(target_arch = "wasm32"))]
        app.insert_resource(ephemeris::EphemerisResource {
            provider: std::sync::Arc::new(ephemeris::CelestialEphemerisProvider::new()),
        });

        // big_space::prelude::BigSpaceDefaultPlugins should be added by the application entry point
        // after disabling TransformPlugin.
        app.add_plugins(trajectories::TrajectoryPlugin);
        app.add_plugins(missions::MissionPlugin);

        app.add_plugins(GravityPlugin);

        app.add_systems(Startup, big_space_setup::setup_big_space_hierarchy);
        app.add_systems(PostStartup, setup_terrain_overrides);

        // --- LEAD-PHASE SYNCHRONIZATION ---
        // Core celestial updates in PreUpdatefor Coordinate Stability
        // for Gizmos (Update) and Physics (FixedUpdate).
        // Gravity is handled by GravityPlugin (see above).
        //
        // System ordering is critical:
        // 1. big_space propagation runs first (default PreUpdate ordering)
        // 2. Our systems run AFTER to override GlobalTransform with body rotation
        app.add_systems(PreUpdate, (
            celestial_clock_tick_system,
            ephemeris_update_system,
            body_rotation_system,
            tile_rotation_sync_system
                .after(bevy::transform::TransformSystems::Propagate)
                .after(big_space::prelude::BigSpaceSystems::PropagateHighPrecision),
            soi_transition_system,
        ).chain());

        app.add_systems(Update, (
            celestial_telemetry_system,
            celestial_visuals_system,
        ).chain());

        app.add_systems(Update, (
            terrain::terrain_spawn_system.run_if(resource_exists::<terrain::TerrainTileConfig>),
            terrain::finalize_terrain_tiles,
        ).chain());

        // Sun light runs in PostUpdate AFTER big_space propagates GlobalTransform,
        // so the camera world position is correct for light direction.
        app.add_systems(
            PostUpdate,
            update_sun_light_system
                .after(bevy::transform::TransformSystems::Propagate)
                .after(big_space::prelude::BigSpaceSystems::PropagateHighPrecision),
        );
    }
}

/// Standalone gravity plugin — for sandbox, tests, and headless sims.
///
/// Registers `gravity_system` (FixedUpdate), `update_local_gravity_field` (PreUpdate),
/// and initializes `LocalGravityField`. Does NOT require ephemeris, terrain, or SOI.
///
/// Use this when you only need gravity without the full `CelestialPlugin`.
/// The full client should use `CelestialPlugin` which includes this.
pub struct GravityPlugin;

impl Plugin for GravityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalGravityField>();
        app.register_type::<GravityBody>();
        app.add_systems(PreUpdate, update_local_gravity_field);
        app.add_systems(FixedUpdate, gravity_system);
    }
}
