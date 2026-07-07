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
// Gravity *types* now live in lunco-environment; celestial owns only the
// gravity systems + `PointMassGravity` model (see `gravity.rs`).
use lunco_environment::{Gravity, GravityBody};

pub mod ephemeris;
pub mod registry;
pub mod geo;
pub mod kepler;
pub mod comms;
pub mod placement;
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
pub use geo::*;
pub use kepler::*;
pub use comms::*;
pub use placement::*;
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

/// Host-app policy for the celestial stack (doc 43). Defaults preserve the
/// full-client (`luncosim`) behavior: hierarchy + Observer Camera at startup.
/// The sandbox starts with both off and flips `spawn_hierarchy` when a loaded
/// scene authors a site anchor (`lunco:anchor:*` on its root prim) — the
/// solar system then appears around the georeferenced scene; the avatar
/// camera keeps the `FloatingOrigin`.
#[derive(Resource, Debug, Clone, Copy)]
pub struct CelestialConfig {
    /// Spawn the Sun/Earth/Moon big_space hierarchy (idempotent; may be
    /// enabled at runtime).
    pub spawn_hierarchy: bool,
    /// Spawn the celestial Observer Camera and let it claim the single
    /// `FloatingOrigin`. Leave off in apps that own their camera (sandbox).
    pub spawn_observer_camera: bool,
}

impl Default for CelestialConfig {
    fn default() -> Self {
        Self { spawn_hierarchy: true, spawn_observer_camera: true }
    }
}

/// `PreUpdate` set containing the celestial epoch chain (ephemeris → body
/// rotation → site anchor → bound placement). Systems that READ celestial
/// `Transform`/`CellCoord` state in `PreUpdate` (e.g. the gravity field)
/// must order `.after(CelestialEpochSet)` or they can interleave mid-chain
/// and observe half-updated grids.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct CelestialEpochSet;

pub struct CelestialPlugin;

impl Plugin for CelestialPlugin {
    fn build(&self, app: &mut App) {
        // EmbeddedAssetsPlugin embeds shaders/textures/missions on wasm32, no-op on desktop
        app.add_plugins(embedded_assets::EmbeddedAssetsPlugin);

        // Trajectory shader is desktop-only (wasm32 embeds it via EmbeddedAssetsPlugin).
        #[cfg(not(target_arch = "wasm32"))]
        app.add_plugins(trajectories::TrajectoryShaderPlugin);

        // Terrain is now in lunco-terrain crate — register it here (guarded:
        // the sandbox adds it directly as well).
        if !app.is_plugin_added::<lunco_terrain_globe::TerrainPlugin>() {
            app.add_plugins(lunco_terrain_globe::TerrainPlugin);
        }

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
        app.init_resource::<CelestialConfig>();
        // Keep a host-app gravity choice (e.g. the sandbox's flat gravity);
        // default to surface gravity for the full client.
        if app.world().get_resource::<Gravity>().is_none() {
            app.insert_resource(Gravity::surface());
        }
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

        if !app.is_plugin_added::<GravityPlugin>() {
            app.add_plugins(GravityPlugin);
        }

        // Hierarchy spawn is gated by `CelestialConfig.spawn_hierarchy` and
        // idempotent (skipped while a `SolarSystemRoot` exists), so a host can
        // enable it at runtime (sandbox: when a site-anchored scene loads). In
        // `Update` rather than `Startup` so the world shell root exists and
        // runtime enablement needs no second registration.
        app.add_systems(
            Update,
            big_space_setup::setup_big_space_hierarchy.run_if(
                |config: Res<CelestialConfig>, q: Query<(), With<SolarSystemRoot>>| {
                    config.spawn_hierarchy && q.is_empty()
                },
            ),
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
            // Doc 43: site-anchored solar frame + geodetic/orbit placement.
            // `ephemeris_update_system` never touches the Solar Grid (id 10),
            // so the pin persists between anchor runs — no mid-chain window
            // where the hierarchy sits un-anchored.
            placement::anchor_solar_frame_to_site,
            placement::place_celestial_bound_entities,
            // Defeat stale-GT caching for the celestial subtree (see the
            // system doc: frozen heliocentric tile/body poses = blinking
            // Earth + focus teleporting into empty space).
            touch_celestial_transforms,
            tile_rotation_sync_system
                .after(bevy::transform::TransformSystems::Propagate)
                .after(big_space::prelude::BigSpaceSystems::PropagateHighPrecision),
            soi_transition_system,
        ).chain().in_set(CelestialEpochSet).after(lunco_time::TimeSpineSet));

        app.add_systems(Update, (
            celestial_telemetry_system,
            celestial_visuals_system,
        ).chain());

        // Camera-driven cube-sphere LOD: streams each body's tiles (replaces the
        // old fixed 24-tile shell). See `crate::globe_lod`.
        app.add_systems(Update, globe_lod::update_globe_lod);

        // Terrain spawning is now handled by lunco-terrain plugin
        // Systems like terrain_spawn_system run in that crate

        // Ephemeris-driven sun direction (doc 19 — T2). The system returns
        // early when the ephemeris is degenerate (`NoOpEphemerisProvider`
        // returns ZERO), so manual `SetEnvironmentLight` (yaw/pitch) control
        // stays authoritative in sandbox/web contexts without a real
        // ephemeris — the single-writer rule that resolved the old web-build
        // clobbering. With a real ephemeris the sun tracks the sim clock:
        // required since the celestial sun light is a TOP-LEVEL entity (it
        // must not ride the Solar Grid — heliocentric-magnitude translations
        // corrupt the f32 cascade-shadow matrices) and therefore inherits no
        // orientation from the site-anchored hierarchy.
        app.add_systems(Update, update_sun_light_system);
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
        // AFTER the celestial epoch chain: this system reads celestial
        // Transform/CellCoords via `world_position_seeded`; unordered it could
        // interleave mid-chain and compute gravity from half-updated grids
        // (measured: alternating ~1e11 m body offsets → randomly flipping
        // gravity vector → the "surface jitter" in site-anchored scenes).
        app.add_systems(PreUpdate, update_local_gravity_field.after(CelestialEpochSet));
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
    let Some(_solar_grid) = solar_grid_q.iter().next() else {
        // No SolarSystemRoot found, so we're not running in a celestial/heliocentric context
        return;
    };

    let sun = lunco_render::LunarSunShadow::default();
    let ls = lunco_environment::LunarSun::default();
    commands.insert_resource(sun.shadow_map());
    // Top-level, NOT under the Solar Grid — see the matching spawn in
    // `big_space_setup`: a heliocentric-magnitude translation corrupts the
    // f32 cascade-shadow matrices (whole-ground lit/black strobe).
    commands.spawn((
        sun.directional_light(Color::WHITE, ls.illuminance_lux),
        sun.cascade_config(),
        lunco_core::SunAngularDiameter(ls.angular_diameter_deg),
        Transform::default(),
        GlobalTransform::default(),
        Name::new("Sun Light"),
        lunco_core::FallbackSceneLight,
    ));
    info!("[restore-fallback-lights] restored celestial fallback light");
}

