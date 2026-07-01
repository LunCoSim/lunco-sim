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
use big_space::prelude::CellCoord;
// Gravity *types* now live in lunco-environment; celestial owns only the
// gravity systems + `PointMassGravity` model (see `gravity.rs`).
use lunco_environment::{Gravity, GravityBody};

pub mod ephemeris;
pub mod registry;
mod big_space_setup;
mod globe_lod;
mod systems;
mod coords;
mod gravity;
mod soi;
mod trajectories;
mod missions;
mod embedded_assets;

/// Re-export terrain types from lunco-terrain for backward compatibility.
pub use lunco_terrain_globe::*;

// Re-export TerrainTileConfig explicitly since it's used by celestial code
pub use lunco_terrain_globe::TerrainTileConfig;

/// UI panels for celestial time control and body browser.
#[cfg(feature = "ui")]
pub mod ui;
pub mod commands;
pub use commands::*;

pub use ephemeris::*;
pub use registry::*;
pub use big_space_setup::*;
pub use systems::*;
pub use gravity::*;
pub use soi::*;
pub use trajectories::*;
pub use missions::*;
pub use embedded_assets::*;

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

        // Trajectory shader is desktop-only (wasm32 embeds it via EmbeddedAssetsPlugin).
        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(trajectories::TrajectoryShaderPlugin);

        // Terrain is now in lunco-terrain crate — register it here
        app.add_plugins(lunco_terrain_globe::TerrainPlugin);

        // The unified mission-time spine (doc 19 — T1): MissionClock + transport +
        // the derived `WorldTime` view. Guarded so a context that also adds it via
        // another plugin (e.g. `UsdBevyPlugin` for the animation sampler) is fine.
        // `TimePlugin` now owns the wall-clock seed itself (Startup), so every
        // spine context anchors at the real launch instant — no celestial-only
        // seed system anymore.
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        app.init_resource::<TerrainMapRegistry>();
        app.insert_resource(Gravity::surface());
        app.register_type::<TrajectoryView>();
        app.register_type::<TrajectoryFrame>();
        app.register_type::<TrajectoryPath>();
        app.insert_resource(CelestialBodyRegistry::default_system());

        // Insert a no-op `EphemerisResource` so downstream systems
        // (missions, trajectories, body positioning) can unconditionally
        // depend on `Res<EphemerisResource>`. Apps that want real
        // planetary positions add `lunco-celestial-ephemeris`'s
        // `EphemerisPlugin`, which overwrites this with the
        // VSOP2013/ELP-backed `CelestialEphemerisProvider`.
        app.insert_resource(ephemeris::EphemerisResource {
            provider: std::sync::Arc::new(ephemeris::NoOpEphemerisProvider),
        });

        // big_space::prelude::BigSpaceDefaultPlugins should be added by the application entry point
        // after disabling TransformPlugin.
        app.add_plugins(trajectories::TrajectoryPlugin);
        app.add_plugins(missions::MissionPlugin);

        app.add_plugins(GravityPlugin);

        // After the world shell so the solar grids nest under its single root
        // (and the Observer Camera claims the shell's seeded FloatingOrigin).
        // `.after` a possibly-absent set is a no-op, so standalone celestial (no
        // WorldShellPlugin) still works — it then spawns its own root via the
        // fallback in `setup_big_space_hierarchy`.
        app.add_systems(
            Startup,
            big_space_setup::setup_big_space_hierarchy.after(lunco_core::WorldShellSet),
        );

        // --- LEAD-PHASE SYNCHRONIZATION ---
        // Core celestial updates in PreUpdate for Coordinate Stability
        // for Gizmos (Update) and Physics (FixedUpdate).
        // Gravity is handled by GravityPlugin (see above).
        //
        // System ordering is critical:
        // 1. big_space propagation runs first (default PreUpdate ordering)
        // 2. Our systems run AFTER to override GlobalTransform with body rotation
        // The spine (`advance_world_clock`, in `TimeSpineSet`) runs first; then the
        // celestial chain consumes the derived `WorldTime.epoch_jd` directly — no
        // `CelestialClock` bridge anymore. Ordered `.after` the spine so the epoch
        // is fresh this frame.
        app.add_systems(PreUpdate, (
            ephemeris_update_system,
            body_rotation_system,
            tile_rotation_sync_system
                .after(bevy::transform::TransformSystems::Propagate)
                .after(big_space::prelude::BigSpaceSystems::PropagateHighPrecision),
            soi_transition_system,
        ).chain().after(lunco_time::TimeSpineSet));

        app.add_systems(Update, (
            celestial_telemetry_system,
            celestial_visuals_system,
        ).chain());

        // Camera-driven cube-sphere LOD: streams each body's tiles (replaces the
        // old fixed 24-tile shell). See `crate::globe_lod`.
        app.add_systems(Update, globe_lod::update_globe_lod);

        // Terrain spawning is now handled by lunco-terrain plugin
        // Systems like terrain_spawn_system run in that crate

        // NOTE: the sun's *direction* is owned by its spawn transform
        // (`big_space_setup`) and edited at runtime via
        // `lunco_environment::SetEnvironmentLight` (the Inspector / web sun
        // control). There is intentionally no per-frame system forcing it: the
        // old `update_sun_light_system` hardcoded `look_to(NEG_Z)` every frame,
        // which clobbered `SetEnvironmentLight` on web builds (where the lone
        // `DirectionalLight` made its `single_mut()` succeed, unlike native
        // where the earthshine fill made it a silent no-op). A future
        // ephemeris-driven sun would write the transform from the sim clock,
        // not pin it to a constant.
        app.add_observer(on_restore_fallback_lights);
    }
}

/// Standalone gravity plugin — registers gravity configuration types.
///
/// Provides:
/// - [`Gravity`] resource (Flat or Surface mode)
/// - [`GravityProvider`] / [`GravityBody`] components
/// - [`LocalGravityField`] resource + `update_local_gravity_field` for the
///   avatar's "up" direction (camera/UI use)
///
/// Does **NOT** apply gravity forces to `RigidBody` entities. For that, also
/// add [`lunco_environment::EnvironmentPlugin`](https://docs.rs/lunco-environment),
/// which computes per-entity `LocalGravity` and applies forces to Avian.
///
/// Use this when you only need gravity configuration without the full
/// `CelestialPlugin`. The full client should use `CelestialPlugin` which
/// includes this.
pub struct GravityPlugin;

impl Plugin for GravityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalGravityField>();
        app.register_type::<GravityBody>();
        app.add_systems(PreUpdate, update_local_gravity_field);
        // NOTE: `gravity_system` (force application to RigidBodies) lives in
        // `lunco-environment`'s `EnvironmentPlugin` and consumes `LocalGravity`.
        // Add EnvironmentPlugin alongside GravityPlugin for full gravity behavior.
    }
}

fn on_restore_fallback_lights(
    _trigger: On<lunco_core::RestoreFallbackLights>,
    mut commands: Commands,
    fallbacks: Query<Entity, With<lunco_core::FallbackSceneLight>>,
    solar_grid_q: Query<Entity, With<SolarSystemRoot>>,
) {
    if !fallbacks.is_empty() {
        return;
    }
    let Some(solar_grid) = solar_grid_q.iter().next() else {
        // No SolarSystemRoot found, so we're not running in a celestial/heliocentric context
        return;
    };

    let sun = lunco_render::LunarSunShadow::default();
    let ls = lunco_environment::LunarSun::default();
    commands.insert_resource(sun.shadow_map());
    commands.spawn((
        sun.directional_light(Color::WHITE, ls.illuminance_lux),
        sun.cascade_config(),
        lunco_core::SunAngularDiameter(ls.angular_diameter_deg),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Name::new("Sun Light"),
        lunco_core::FallbackSceneLight,
        ChildOf(solar_grid),
    ));
    info!("[restore-fallback-lights] restored celestial fallback light");
}

