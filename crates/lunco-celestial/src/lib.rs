//! Solar system simulation and celestial mechanics.
//!
//! This crate implements the core of the LunCo digital twin, including:
//! - **Ephemeris**: High-precision planetary positioning and rotation data.
//! - **Gravity**: Per-entity surface gravity using body-fixed coordinates.
//! - **SOI (Sphere of Influence)**: Automatic coordinate frame transitions.
//! - **Terrain**: Dynamic procedural terrain generation for planetary surfaces.
//! - **Trajectories**: Rendering of orbital paths and mission predictions.

use bevy::math::DVec3;
use bevy::prelude::*;
// Gravity *types* now live in lunco-environment; celestial owns only the
// gravity systems + `PointMassGravity` model (see `gravity.rs`).
use lunco_environment::{Gravity, GravityBody};

mod big_space_setup;
/// The frame conversions. `coords` is `pub` because `lunco-celestial-ephemeris` was
/// re-implementing `ecliptic_to_bevy` by hand for want of access — a conversion people copy
/// is a conversion that drifts.
pub mod cadence;
pub mod coords;
mod embedded_assets;
pub mod ephemeris;
/// Coordinate-frame newtypes. Zero-cost, and they make the two silent frame-mix incidents
/// this crate has already shipped (the Shackleton sun 45° below the horizon; an ecliptic sun
/// direction published as site-ENU) into COMPILE ERRORS.
pub mod frames;
pub mod geo;
mod globe_lod;
mod gravity;
pub mod iau;
mod imagery;
pub mod kepler;
pub mod link;
mod missions;
pub mod placement;
pub mod pose;
pub mod queries;
pub mod registry;
mod soi;
mod surface_pose;
mod systems;
mod trajectories;
/// Frame CONVERSIONS over [`frames`] — hub-and-spoke through `Solar`, like SPICE.
///
/// This file shipped in `acc61943` and was never declared as a module, so it has never
/// been compiled: `BodyRegistry` (a name that does not exist — the registry is
/// [`registry::CelestialBodyRegistry`]) would have failed on the first build. Which is
/// also why its `libration_in_solar` — the only L1/L2 solver in the tree — has never
/// been reachable from a scene.
pub mod transform;
pub mod wifi;

pub mod commands;
/// UI panels for celestial time control and body browser.
#[cfg(feature = "ui")]
pub mod ui;
pub use commands::*;

pub use big_space_setup::*;
pub use embedded_assets::*;
pub use ephemeris::*;
pub use geo::*;
pub use globe_lod::{GlobeLod, GlobeLodBudget};
pub use gravity::*;
pub use iau::*;
pub use kepler::*;
pub use link::*;
pub use missions::*;
pub use placement::*;
pub use pose::*;
pub use registry::*;
pub use soi::*;
pub use surface_pose::*;
pub use systems::*;
pub use trajectories::*;
pub use wifi::*;

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

/// A scene-authored declaration that a celestial body exists — the ECS projection
/// of USD's `LunCoCelestialBodyAPI` (`int lunco:body = 399`).
///
/// **This is the switch that turns the sky on**, and it is scene data, not a code
/// flag. No prim declares a body ⇒ no hierarchy, no globes, no orbit views, no
/// ephemeris. See [`celestial_declared`].
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CelestialBodyDecl {
    /// NAIF id: 10 Sun, 399 Earth, 301 Moon.
    pub naif: i32,
}

/// A body map authored ON the body prim — the ECS projection of
/// `asset lunco:body:albedoMap = @lunco://textures/earth.png@`.
///
/// Which map a body wears is scene content, and this is the one-attribute way
/// to say it: no Material network, just a texture, resolved through the
/// ordinary asset schemes. A Twin can therefore ship its own Earth
/// (`@twin://mytwin/textures/earth_1970.png@`) without touching the engine's
/// manifest, and a scene that says nothing falls back to the declared dataset
/// (see `imagery`), then to the body's own colour.
///
/// For anything richer than an albedo map — a custom shader, extra inputs —
/// bind a `UsdShade` Material to the prim instead; that path already exists
/// and wins over both.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct AuthoredBodyAlbedo {
    /// Asset reference exactly as authored, resolved by the `AssetServer`.
    pub asset: String,
}

/// Run condition: does the loaded scene actually ask for celestial content?
///
/// Replaces `CelestialConfig.spawn_hierarchy` — a boolean in Rust that decided
/// whether a solar system existed, flipped as a side effect of a scene authoring a
/// site anchor. A scene could not *say* "I want a Moon"; it could only trip a switch,
/// and any subsystem that forgot to read the switch leaked planets into scenes that
/// never asked for them (which is exactly what `TrajectoryPlugin` did — Earth/Moon
/// orbit views in the flat sandbox arena).
///
/// Now the scene declares its bodies in USD and every celestial subsystem gates on
/// the same authored fact. Re-armable by construction: load a scene with bodies at
/// runtime and the hierarchy builds then.
pub fn celestial_declared(q: Query<(), With<CelestialBodyDecl>>) -> bool {
    !q.is_empty()
}

/// Host-app policy for the celestial stack (doc 43).
///
/// Note what is NOT here any more: `spawn_hierarchy`. Whether a solar system exists
/// is the *scene's* call, authored as `LunCoCelestialBodyAPI` prims and gated by
/// [`celestial_declared`] — not a host-app boolean. This resource now carries only
/// genuine host policy: whether the app owns its own camera.
#[derive(Resource, Debug, Clone, Copy)]
pub struct CelestialConfig {
    /// Spawn the celestial Observer Camera and let it claim the single
    /// `FloatingOrigin`. Leave off in apps that own their camera (sandbox).
    pub spawn_observer_camera: bool,
}

impl Default for CelestialConfig {
    fn default() -> Self {
        Self {
            spawn_observer_camera: true,
        }
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

/// Give the persistent BigSpace root an explicit semantic frame as soon as it
/// exists. This lets generic camera/network state use the same framed-pose
/// path in non-celestial scenes instead of falling back to an unnamed parent
/// grid convention.
fn tag_world_reference_frame(trigger: On<Add, lunco_core::WorldRoot>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(ReferenceFrame::World);
}

/// Backfill the semantic tag when a host installed its world shell before the
/// celestial plugin. Normal production startup is handled by the observer;
/// this keeps plugin composition order from changing frame semantics.
fn tag_existing_world_reference_frame(
    mut commands: Commands,
    roots: Query<Entity, (With<lunco_core::WorldRoot>, Without<ReferenceFrame>)>,
) {
    for root in &roots {
        commands.entity(root).try_insert(ReferenceFrame::World);
    }
}

impl Plugin for CelestialPlugin {
    fn build(&self, app: &mut App) {
        // EmbeddedAssetsPlugin embeds mission data on wasm32, no-op on desktop.
        app.add_plugins(embedded_assets::EmbeddedAssetsPlugin);

        // (`TrajectoryShaderPlugin` was removed with the dead trajectory
        // `MaterialExtension` — see the note in `trajectories.rs`.)

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
        app.init_resource::<CelestialConfig>();
        app.init_resource::<globe_lod::GlobeLodBudget>();
        // Celestial content always lives in the canonical persistent BigSpace
        // shell. Installing the shell here when a host has not already done so
        // keeps headless/test apps on the same hierarchy as production and
        // removes the former celestial-only root fallback.
        if !app.is_plugin_added::<lunco_core::WorldShellPlugin>() {
            app.add_plugins(lunco_core::WorldShellPlugin);
        }
        // Generic celestial geometry queries (Occultation / BodyPosition /
        // SolarPose) — the domain-free substrate authored subsystems compose
        // over (docs 10/12) — plus the solar-pose tracking system that feeds
        // `SolarPose` (incl. scene-local prims a read-only query can't resolve).
        queries::register_celestial_queries(app);
        app.register_type::<pose::SolarTracked>();
        app.add_systems(
            Update,
            pose::update_solar_poses.run_if(cadence::tracked_needs_solve()),
        );

        // Generic connectivity kernel: cadence-gated pairwise link solving in
        // Rust, verdict via the language-neutral `link.connected` hook, cadence
        // tunable live via `SetLinkCadence` (docs 10/12). Domain-free.
        app.init_resource::<link::LinkConfig>();
        app.register_type::<link::LinkConfig>();
        app.register_type::<link::LinkNode>();
        app.register_type::<link::LinkOccluder>();
        app.register_type::<link::LinkState>();
        app.register_type::<link::LinkGeometryState>();
        app.register_type::<wifi::WifiNode>();
        app.register_type::<wifi::WifiState>();
        link::register_all_commands(app);
        // `update_links` is a REGULAR (non-exclusive) system — it writes through
        // Commands and adds no extra command-flush sync point. (An earlier
        // exclusive version, needed to call the TerrainRaycast provider with
        // `&mut World`, inserted a sync point that interleaved with the
        // twin/terrain despawns and tripped avian's island bookkeeping.)
        app.add_systems(Update, link::update_links.after(pose::update_solar_poses));
        app.add_systems(Update, wifi::update_wifi_links.after(link::update_links));
        // Expose the working peer's range + verdict as PORTS, so an authored RF model
        // (`assets/models/CommsLink.mo`) can turn metres into bits/s off an ordinary
        // output→input wire.
        //
        // Registered as a backend rather than pushed into `SimComponent.outputs` by a
        // system: ports are read on demand, so there is no publish tick to get wrong.
        // The previous bridge needed `FixedUpdate` + `.before(CosimSet::Propagate)` to
        // be correct at all — see `link::LINK_PORT_BACKEND` for what that cost.
        //
        // Registration order is resolution precedence, and cosim's builtins (Modelica
        // first) are already in by now, so a model that authors its own `link_*`
        // variable keeps it.
        app.world_mut()
            .get_resource_or_init::<lunco_core::ports::PortRegistry>()
            .register(link::LINK_PORT_BACKEND);
        // Keep a host-app gravity choice (e.g. the sandbox's flat gravity);
        // default to surface gravity for the full client.
        if app.world().get_resource::<Gravity>().is_none() {
            app.insert_resource(Gravity::surface());
        }
        app.register_type::<TrajectoryView>();
        app.register_type::<TrajectoryFrame>();
        app.register_type::<TrajectoryPath>();
        app.register_type::<ReferenceFrame>();
        app.init_resource::<ReferenceFrameIndex>();
        app.add_observer(tag_world_reference_frame);
        app.add_systems(
            First,
            (
                tag_existing_world_reference_frame,
                update_reference_frame_index,
            )
                .chain(),
        );
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

        // Hierarchy spawn is gated by what the SCENE declares (`LunCoCelestialBodyAPI`
        // prims → `CelestialBodyDecl`), not by a host boolean, and is idempotent
        // (skipped while a `SolarSystemRoot` exists). In `Update` rather than `Startup`
        // so it fires whenever a scene with bodies loads — including at runtime.
        app.add_systems(
            Update,
            big_space_setup::setup_big_space_hierarchy.run_if(
                |q_decl: Query<(), With<CelestialBodyDecl>>,
                 q: Query<(), With<SolarSystemRoot>>| {
                    !q_decl.is_empty() && q.is_empty()
                },
            ),
        );
        app.add_systems(Update, big_space_setup::sync_site_gravity);

        // Teardown is the first phase of the scene replacement transaction.
        // It cannot be inferred from a frame where declarations happen to be
        // absent: a restart may project the replacement declarations in the
        // same frame, so that state is never observable. Retire the old sky and
        // restore the persistent physics frame exactly once at the lifecycle
        // boundary, before any replacement prim is projected.
        app.add_systems(lunco_core::SceneTeardown, teardown_celestial_scene);

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
        // Orbital view MODE state (scene-hide, gravity hold, camera
        // park/restore) — the camera itself flies to the focused body; the
        // world is never re-posed for viewing (see `placement::OrbitalViewPin`).
        app.init_resource::<placement::OrbitalViewPin>();
        app.init_resource::<systems::SunDirectionWorld>();

        // Celestial cadence: the tree is re-solved on an ANGULAR ERROR BUDGET,
        // not every frame and not at a fixed Hz (sim time warps, so a rate is
        // wrong at every other warp factor — see `cadence`). Solving every frame
        // measured ~10 ms/frame across the five systems gated below.
        //
        // The cadence gate belongs on the systems that solve celestial state.
        // BigSpace propagation is driven by actual CellCoord/Transform and
        // hierarchy changes; no per-frame dirtying is allowed to manufacture a
        // change signal or hide an invalid low-precision subtree.
        app.init_resource::<cadence::CelestialSolvedEpoch>();
        lunco_settings::AppSettingsExt::register_settings_section::<
            cadence::CelestialCadenceSettings,
        >(app);
        // One writer, in `Last`, under the same condition as its readers: every
        // gated system in this frame saw the same `CelestialSolvedEpoch`, so the
        // cluster advances together or not at all. A half-advanced tree would put
        // the sun and the bodies at different instants.
        // The structural half of the cluster gate: bumped in `First`, so an edge
        // (scene load, site edit, hierarchy rebuild) is visible to every gated
        // member in the same frame, and committed in `Last` with the epoch.
        app.init_resource::<cadence::CelestialInputsRevision>();
        app.add_systems(First, cadence::bump_celestial_inputs_revision);
        app.add_systems(
            Last,
            cadence::commit_celestial_epoch.run_if(cadence::tracked_needs_solve()),
        );

        app.add_systems(
            PreUpdate,
            (
                ephemeris_update_system.run_if(cadence::tracked_needs_solve()),
                body_rotation_system,
                // Doc 43: site-anchored solar frame + geodetic/orbit placement.
                // `ephemeris_update_system` never touches the Solar Grid (id 10),
                // so the pin persists between anchor runs — no mid-chain window
                // where the hierarchy sits un-anchored.
                //
                // On the SAME condition as `ephemeris_update_system` above, and
                // that is load-bearing: the pin is derived FROM the body poses,
                // so solving it at a different epoch than the bodies puts the
                // site frame and the tree at different instants. Ungating it
                // (tried, 2026-07-26) made the pin fresh while the bodies stayed
                // at the last solved epoch — the sun disc and the light then
                // disagreed and the world snapped whenever the bodies caught up
                // ("I move and it jumps back"). The cluster advances together or
                // not at all; `celestial_needs_solve` also fires on structural
                // edges, so a scene loading at a standing epoch still anchors.
                // A scene's USD projection creates `CelestialBodyDecl` during
                // `Update`, and the hierarchy is then spawned in `Update` too.
                // The first `PreUpdate` that sees a newly-loaded site therefore
                // precedes the `SolarSystemRoot` command application.  Do not
                // call the anchorer in that transitional frame: it is not a
                // broken scene and reporting it as one made a correctly staged
                // package look unlit while its Earth/Moon imagery was loading.
                // `Added<SolarSystemRoot>` bumps the cadence revision on the
                // following frame, which makes this run as soon as the grid
                // actually exists.
                placement::anchor_solar_frame_to_site
                    .run_if(cadence::tracked_needs_solve())
                    .run_if(|q: Query<(), With<big_space_setup::SolarSystemRoot>>| !q.is_empty()),
                placement::attach_site_scene_to_surface_grid
                    .run_if(cadence::tracked_needs_solve())
                    .run_if(|q: Query<(), With<big_space_setup::SolarSystemRoot>>| !q.is_empty()),
                placement::place_celestial_bound_entities,
                soi_transition_system,
            )
                .chain()
                .in_set(CelestialEpochSet)
                .after(lunco_time::TimeSpineSet),
        );

        app.add_systems(Update, celestial_visuals_system);
        // Hide the local scene (its whole subtree) while the orbital world-pin
        // is active — the celestial tree is slid away, so the scene would fill
        // the foreground of the orbital view.
        app.add_systems(Update, placement::orbital_pin_scene_visibility);

        // Camera-driven cube-sphere LOD: streams each body's tiles (replaces the
        // old fixed 24-tile shell). See `crate::globe_lod`.
        //
        // A body's LOOK is content: if the scene bound a Material to the body's
        // prim, the ordinary USD → `ShaderLook` path authored it there and this
        // carries it onto the globe. Ordered before the LOD so an adopted look
        // and the tiles that carry it land in the same frame.
        //
        // Body IMAGERY is a declared dataset, not a path in this crate: the
        // manifest says which body each texture is of, and `imagery` binds
        // whatever is installed (downloaded, cached, or shipped in the
        // package). It runs BEFORE the authored-look adoption so a scene that
        // binds its own Material still wins — content overrules the default.
        app.init_resource::<imagery::BoundBodyImagery>();
        app.init_resource::<imagery::PendingBodyImagery>();
        app.add_systems(
            Update,
            (
                // Weakest first, so a stronger statement overwrites it in the
                // same frame: engine-wide dataset default → the map authored on
                // the prim → a full Material bound to the prim.
                imagery::bind_dataset_body_imagery,
                imagery::adopt_authored_body_albedo,
                big_space_setup::adopt_authored_body_look,
                globe_lod::update_globe_lod,
            )
                .chain(),
        );

        // Site-anchored scenes: hand the DEM terrain the body radius so it
        // curves onto the globe sphere (see `placement::sync_terrain_body_curvature`).
        app.add_systems(Update, placement::sync_terrain_body_curvature);

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
        app.add_systems(
            Update,
            update_sun_light_system.run_if(cadence::tracked_needs_solve()),
        );
    }
}

/// Tear down everything the celestial subsystem spawned at the scene boundary.
///
/// This is the *architecture* that prevents the reload bugs by design, not by a
/// maintained despawn list:
///
/// * **Ownership marker.** Every celestial-derived entity carries
///   [`CelestialDerived`](big_space_setup::CelestialDerived) — grids, bodies, inertial
///   anchors, orbit views, mission spacecraft. Despawning that set (recursively, to
///   catch unmarked children like globe tiles) removes the whole sky. A new celestial
///   entity is covered the moment it carries the marker; the invariant lives in one
///   line on the marker's doc.
/// * **Idempotent re-spawn.** The spawners gate on "does my output exist yet"
///   (`SolarSystemRoot`/`TrajectoryView` empty), not a `Local` latch — so once this
///   clears them, loading a scene *with* bodies rebuilds cleanly. Missions gate the
///   same way but per-declaration: `MissionSpawned` is stamped on the USD prim entity
///   that declared the mission, so it dies with the prim on teardown and a reload
///   re-declares and re-spawns without any resource needing to be consulted.
/// * **Resource state reset.** `MissionRegistry` (a diagnostic list of the ids spawned
///   into the current scene) and the terrain-curvature coupling are resources, not
///   entities, so they are reset here explicitly.
///
/// The clock tree is reset separately and universally by `lunco_time::ResetTime`, fired
/// from the scene-clear choke point — so a detached/fast sky clock is already back on
/// its `Epoch` root by the time this runs.
fn teardown_celestial_scene(
    mut commands: Commands,
    q_derived: Query<Entity, With<big_space_setup::CelestialDerived>>,
    q_world_root: Query<Entity, With<lunco_core::WorldRoot>>,
    mut registry: ResMut<MissionRegistry>,
    curvature: Option<Res<lunco_terrain_surface::TerrainBodyCurvature>>,
) {
    // `attach_site_scene_to_surface_grid` selects a celestial surface Grid as
    // Avian's active frame. That Grid is part of the scene-owned hierarchy and
    // is about to be despawned, so restore the persistent shell frame in the
    // same lifecycle operation. Leaving an Entity id to a dead Grid makes the
    // physics bridge's next frame conversion structurally impossible.
    let world_root = q_world_root
        .single()
        .expect("WorldShellPlugin must provide exactly one persistent WorldRoot");
    commands.insert_resource(lunco_core::ActivePhysicsFrame(world_root));

    let mut n = 0;
    for e in &q_derived {
        // `try_despawn` (not `despawn`): a marked entity already parented under the
        // hierarchy is removed by its ancestor's recursive despawn, so by the time this
        // command applies it may be gone — `try_` makes that a no-op, not a warning.
        commands.entity(e).try_despawn();
        n += 1;
    }

    registry.missions.clear();
    if curvature.is_some() {
        commands.remove_resource::<lunco_terrain_surface::TerrainBodyCurvature>();
    }

    info!("[celestial] scene teardown retired {n} derived entities");
}

#[cfg(test)]
mod scene_teardown_tests {
    use super::*;

    #[test]
    fn replacement_declarations_cannot_suppress_celestial_teardown() {
        let mut app = App::new();
        app.init_resource::<MissionRegistry>();
        app.add_systems(lunco_core::SceneTeardown, teardown_celestial_scene);

        let world_root = app.world_mut().spawn(lunco_core::WorldRoot).id();
        let outgoing_frame = app.world_mut().spawn_empty().id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(outgoing_frame));
        let outgoing = app
            .world_mut()
            .spawn(big_space_setup::CelestialDerived)
            .id();
        // This is the restart shape that defeated the former Update/run_if:
        // replacement declarations already exist before the next frame.
        let replacement_decl = app
            .world_mut()
            .spawn(CelestialBodyDecl {
                naif: ephemeris_id::MOON,
            })
            .id();

        lunco_core::run_scene_teardown(app.world_mut());

        assert!(app.world().get_entity(outgoing).is_err());
        assert!(app.world().get_entity(replacement_decl).is_ok());
        assert_eq!(
            app.world().resource::<lunco_core::ActivePhysicsFrame>().0,
            world_root
        );
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
        // `update_local_gravity_field` reads the pin to HOLD the field while an
        // orbital view is active. `CelestialPlugin` also inits it, but this
        // plugin is the documented "gravity without the full CelestialPlugin"
        // entry point, so it must stand alone — otherwise the system panics on
        // a missing `Res` in any app that adds only this. `init_resource` is
        // idempotent, so adding both plugins is still fine.
        app.init_resource::<placement::OrbitalViewPin>();
        app.register_type::<GravityBody>();
        // AFTER the celestial epoch chain: this system reads celestial
        // Transform/CellCoords via `world_position_seeded`; unordered it could
        // interleave mid-chain and compute gravity from half-updated grids
        // (measured: alternating ~1e11 m body offsets → randomly flipping
        // gravity vector → the "surface jitter" in site-anchored scenes).
        app.add_systems(
            PreUpdate,
            update_local_gravity_field.after(CelestialEpochSet),
        );
        // NOTE: `gravity_system` (force application to RigidBodies) lives in
        // `lunco-environment`'s `EnvironmentPlugin` and consumes `LocalGravity`.
        // Add EnvironmentPlugin alongside GravityPlugin for full gravity behavior.
    }
}
