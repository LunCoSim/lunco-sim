//! BigSpace projection and runtime integration for celestial mechanics.
//!
//! The sibling `lunco-celestial` package owns headless astronomy and semantic
//! frame values. This package owns the scene/runtime boundary: BigSpace grids,
//! ECS placement, gravity projection, terrain/visual integration, links, and
//! runtime scheduling.

use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_celestial::{CelestialBodyRegistry, ReferenceFrame};
use lunco_celestial_spatial_core::{
    AuthoredBodyAlbedo, CelestialBodyDecl, LocalGravityField, OrbitalViewPin, ReferenceFrameIndex,
    SolarSystemRoot, update_reference_frame_index,
};
// Gravity *types* now live in lunco-environment; celestial owns only the
// gravity systems + `PointMassGravity` model (see `gravity.rs`).
use lunco_environment::{Gravity, GravityBody};

mod big_space_setup;
pub mod cadence;
mod globe_lod;
mod gravity;
mod imagery;
pub mod link;
pub mod placement;
pub mod pose;
pub mod queries;
mod soi;
mod systems;
pub mod wifi;

pub mod commands;
pub use commands::*;

pub use big_space_setup::*;
pub use globe_lod::{GlobeLod, GlobeLodBudget};
pub use gravity::*;
pub use link::*;
pub use placement::*;
pub use pose::*;
pub use soi::*;
pub use systems::*;
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

/// Host configuration for optional observer-camera spawning.
#[derive(Resource, Debug, Clone, Copy)]
pub struct CelestialConfig {
    /// Spawn the celestial Observer Camera. BigSpace origin ownership remains
    /// with the persistent `OriginAnchor`; leave this off in apps that own
    /// their viewport camera (sandbox).
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

/// Update phase that publishes the body-curvature input consumed by DEM
/// construction. It is separate from authored celestial projection because
/// terrain georeferencing must exist before the curvature owner can resolve its
/// body, while the DEM build must not capture the previous (flat) value.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CelestialTerrainSet {
    Curvature,
}

/// Give the persistent BigSpace root an explicit semantic frame as soon as it
/// exists. This lets generic camera/network state use the same framed-pose
/// path in non-celestial scenes instead of falling back to an unnamed parent
/// grid convention.
fn tag_world_reference_frame(trigger: On<Add, lunco_spatial::WorldRoot>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(ReferenceFrame::World);
}

/// Backfill the semantic tag when a host installed its world shell before the
/// celestial plugin. Normal production startup is handled by the observer;
/// this keeps plugin composition order from changing frame semantics.
fn tag_existing_world_reference_frame(
    mut commands: Commands,
    roots: Query<Entity, (With<lunco_spatial::WorldRoot>, Without<ReferenceFrame>)>,
) {
    for root in &roots {
        commands.entity(root).try_insert(ReferenceFrame::World);
    }
}

impl Plugin for CelestialPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_embodiment_core::roles::EmbodimentCorePlugin>() {
            app.add_plugins(lunco_embodiment_core::roles::EmbodimentCorePlugin);
        }
        // Terrain is now in lunco-terrain crate — register it here (guarded:
        // the sandbox adds it directly as well).
        if !app.is_plugin_added::<lunco_terrain_globe::TerrainPlugin>() {
            app.add_plugins(lunco_terrain_globe::TerrainPlugin);
        }

        // Shared simulation time and its derived `WorldTime` view. Guarded so a
        // context that also adds it through another plugin can compose safely.
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        app.init_resource::<CelestialConfig>();
        app.init_resource::<lunco_port_core::ports::PortTopologyRevision>()
            .init_resource::<lunco_port_core::ports::PortTopologyState>();
        // Globe LOD consumes the shared presentation binding, not Bevy's
        // render activation flag. Keep the binding substrate available in
        // standalone celestial hosts as well as the full USD application.
        app.init_resource::<lunco_viewport_core::SceneViewport>();
        // Celestial shell geometry uses the same authoritative graphics
        // settings as USD projection. Initialise the documented default here
        // so setup does not substitute a private Balanced profile.
        app.init_resource::<lunco_render::RenderingQualitySettings>();
        app.init_resource::<globe_lod::GlobeLodBudget>();
        // Celestial content always lives in the canonical persistent BigSpace
        // shell. Installing the shell here when a host has not already done so
        // keeps headless/test apps on the same hierarchy as production.
        if !app.is_plugin_added::<lunco_spatial::WorldShellPlugin>() {
            app.add_plugins(lunco_spatial::WorldShellPlugin);
        }
        // Generic celestial geometry queries (Occultation / BodyPosition /
        // SolarPose) — the domain-free substrate authored subsystems compose
        // over (docs 10/12) — plus the solar-pose tracking system that feeds
        // the sole `SolarFramePose` reader path, including scene-local prims.
        queries::register_celestial_queries(app);
        app.register_type::<lunco_celestial_spatial_core::SolarTracked>();
        app.register_type::<lunco_celestial::CelestialBody>();
        // Generic connectivity kernel: cadence-gated pairwise link solving in
        // Rust, verdict via the language-neutral `link.connected` hook, cadence
        // tunable live via `SetLinkCadence` (docs 10/12). Domain-free.
        app.init_resource::<link::LinkConfig>();
        app.init_resource::<link::LinkSolverState>();
        app.init_resource::<link::LinkClassCatalog>();
        app.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        app.init_resource::<lunco_port_core::ports::PortTopologyState>();
        app.register_type::<link::LinkConfig>();
        app.register_type::<lunco_celestial_spatial_core::LinkNode>();
        app.register_type::<lunco_celestial_spatial_core::LinkOccluder>();
        app.register_type::<lunco_celestial_spatial_core::LinkState>();
        app.register_type::<lunco_celestial_spatial_core::LinkGeometryState>();
        app.register_type::<lunco_celestial_spatial_core::WifiNode>();
        app.register_type::<lunco_celestial_spatial_core::WifiState>();
        link::register_all_commands(app);
        app.add_observer(
            lunco_port_core::ports::bump_port_topology_on_add::<
                lunco_celestial_spatial_core::LinkNode,
            >,
        )
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_remove::<
                lunco_celestial_spatial_core::LinkNode,
            >,
        )
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_add::<
                lunco_celestial_spatial_core::LinkState,
            >,
        )
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_remove::<
                lunco_celestial_spatial_core::LinkState,
            >,
        );
        app.add_systems(PreUpdate, link::refresh_link_class_catalog);
        app.add_systems(PostUpdate, link::check_link_state_structure);
        // `update_links` is a REGULAR (non-exclusive) system — it writes through
        // Commands and adds no extra command-flush sync point. (An earlier
        // exclusive version, needed to call the TerrainRaycast provider with
        // `&mut World`, inserted a sync point that interleaved with the
        // twin/terrain despawns and tripped avian's island bookkeeping.)
        app.add_systems(Update, link::update_links.run_if(link::link_solve_due));
        app.add_systems(
            Update,
            wifi::update_wifi_links
                .run_if(wifi::wifi_links_due)
                .after(link::update_links),
        );
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
            .get_resource_or_init::<lunco_port_core::ports::PortRegistry>()
            .register(link::LINK_PORT_BACKEND);
        // Keep a host-app gravity choice (e.g. the sandbox's flat gravity);
        // default to surface gravity for the full client.
        if app.world().get_resource::<Gravity>().is_none() {
            app.insert_resource(Gravity::surface());
        }
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

        // big_space::prelude::BigSpaceDefaultPlugins should be added by the application entry point
        // after disabling TransformPlugin.

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
        // A USD PhysicsScene is projected through deferred commands while the
        // scene prims are materialised.  Those commands may land after the
        // ordinary Update systems and restore stage-frame Gravity::Flat.  A
        // site scene has a different live contract: its root is migrated onto
        // a body-fixed tangent frame, so gravity must be derived from the
        // body's frame, not from the pre-migration USD stage axis.  Run this
        // finalisation in PostUpdate, after the USD projection has flushed,
        // before the next FixedUpdate computes LocalGravity and applies force.
        app.add_systems(PostUpdate, big_space_setup::sync_site_gravity);

        // Teardown is the first phase of the scene replacement transaction.
        // It cannot be inferred from a frame where declarations happen to be
        // absent: a restart may project the replacement declarations in the
        // same frame, so that state is never observable. Retire the old sky and
        // restore the persistent physics frame exactly once at the lifecycle
        // boundary, before any replacement prim is projected.
        app.add_systems(
            lunco_core::SceneTeardown,
            (teardown_celestial_scene, link::reset_link_solver_state),
        );

        // --- LEAD-PHASE SYNCHRONIZATION ---
        // Core celestial updates in PreUpdate for Coordinate Stability
        // for Gizmos (Update) and Physics (FixedUpdate).
        // Gravity is handled by GravityPlugin (see above).
        //
        // System ordering is critical:
        // 1. big_space propagation runs first (default PreUpdate ordering)
        // 2. Our systems run AFTER to override GlobalTransform with body rotation
        // CelestialTime is the scaled child of WorldTime. Body transforms, solar
        // model inputs, geometry queries, and rendered lighting all use that
        // same resolved epoch; physics retains its ordinary fixed-step cadence.
        // Orbital view MODE state (scene-hide, gravity hold, camera
        // park/restore) — the camera itself flies to the focused body; the
        // world is never re-posed for viewing (see `OrbitalViewPin`).
        app.init_resource::<OrbitalViewPin>();
        app.init_resource::<lunco_environment::SunState>();
        if !app.is_plugin_added::<lunco_input_core::InputBindingsPlugin>() {
            app.add_plugins(lunco_input_core::InputBindingsPlugin);
        }

        // Celestial cadence: the tree is re-solved on an ANGULAR ERROR BUDGET,
        // not at a fixed wall-clock Hz (the transport rate changes the epoch
        // step, so a fixed rate is wrong at different settings — see `cadence`).
        // When the geometric budget is exceeded, the complete celestial state
        // transaction re-solves from the shared CelestialTime sample.
        //
        // The cadence gate belongs on the systems that solve celestial state.
        // BigSpace propagation is driven by actual CellCoord/Transform and
        // hierarchy changes; no per-frame dirtying is allowed to manufacture a
        // change signal or hide an invalid low-precision subtree.
        app.init_resource::<cadence::CelestialSolvedEpoch>();
        app.init_resource::<cadence::CelestialMotionBound>();
        lunco_settings::AppSettingsExt::register_settings_section::<
            cadence::CelestialCadenceSettings,
        >(app);
        // One writer, in `Last`, commits the CelestialTime sample after every
        // gated consumer has processed it. One epoch/revision pair keeps the
        // complete celestial state transaction together.
        // The structural half of the cluster gate: bumped in `First`, so an edge
        // (scene load, site edit, hierarchy rebuild) is visible to every gated
        // member in the same frame, and committed in `Last` with the epoch.
        app.init_resource::<cadence::CelestialInputsRevision>();
        app.add_systems(
            First,
            (
                cadence::bump_celestial_inputs_revision,
                cadence::refresh_motion_bound.run_if(
                    resource_changed::<cadence::CelestialInputsRevision>
                        .or_else(resource_changed::<CelestialBodyRegistry>)
                        .or_else(resource_changed::<lunco_celestial::EphemerisResource>),
                ),
            )
                .chain(),
        );
        app.add_systems(
            Last,
            cadence::commit_celestial_epoch
                .run_if(cadence::tracked_needs_solve())
                .run_if(lunco_time::scene_time_ready),
        );
        app.add_systems(
            PreUpdate,
            (
                ephemeris_update_system.run_if(cadence::tracked_needs_solve()),
                body_rotation_system.run_if(cadence::tracked_needs_solve()),
                // The solar hierarchy stays inertial. Site content is mounted
                // once beneath its body's rotating surface grid; no ancestor is
                // re-posed to make a site coincide with the world origin.
                placement::attach_site_scene_to_surface_grid.run_if(cadence::tracked_needs_solve()),
                placement::place_celestial_bound_entities.run_if(cadence::tracked_needs_solve()),
                pose::update_solar_poses.run_if(cadence::tracked_needs_solve()),
                update_sun_light_system.run_if(cadence::tracked_needs_solve()),
                soi_transition_system,
            )
                .chain()
                .in_set(CelestialEpochSet)
                .run_if(lunco_time::scene_time_ready)
                .after(lunco_time::TimeSpineSet)
                .after(lunco_time::CelestialTimeSet),
        );

        app.add_systems(
            Update,
            celestial_visuals_system.run_if(lunco_time::scene_time_ready),
        );
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
                globe_lod::update_globe_lod.run_if(globe_lod::globe_lod_update_due),
            )
                .chain(),
        );

        // Site-anchored scenes: hand the DEM terrain the body radius so it
        // curves onto the globe sphere (see `placement::sync_terrain_body_curvature`).
        app.add_systems(
            Update,
            placement::sync_terrain_body_curvature
                .in_set(CelestialTerrainSet::Curvature)
                .run_if(lunco_time::scene_time_ready),
        );

        // Terrain spawning is now handled by lunco-terrain plugin
        // Systems like terrain_spawn_system run in that crate

        // The environment projects this semantic SunState to the render light
        // in Update and samples it for cosim on its normal FixedUpdate cadence.
    }
}

/// Tear down everything the celestial subsystem spawned at the scene boundary.
///
/// This is the *architecture* that prevents the reload bugs by design, not by a
/// maintained despawn list:
///
/// * **Ownership marker.** Every celestial-owned root carries
///   [`CelestialDerived`](big_space_setup::CelestialDerived) — the solar hierarchy
///   and authored celestial bodies. Despawning those roots recursively removes
///   their grids, bodies, terrain tiles, labels, and other structural descendants.
///   A new ownership root is covered the moment it carries the marker; the invariant
///   lives in one line on the marker's doc.
/// * **Idempotent re-spawn.** The spawners gate on current outputs or stamp the
///   scene's body declarations. Those scene-owned prims and markers are removed
///   together, so a replacement scene can project its own bodies.
/// * **Resource state reset.** Terrain-curvature coupling is a resource rather
///   than an entity, so it is reset here explicitly.
///
/// The clock tree is reset separately and universally by `lunco_time::ResetTime`, fired
/// from the scene-clear choke point. `ResetTime` restores the celestial
/// `TimeDomain` as an identity child of `WorldTime`, then resets the mission
/// epoch before celestial consumers resume.
fn teardown_celestial_scene(
    mut commands: Commands,
    mut active_physics_frame: ResMut<lunco_spatial::ActivePhysicsFrame>,
    q_derived: Query<Entity, With<big_space_setup::CelestialDerived>>,
    q_world_grid: Query<Entity, With<lunco_spatial::WorldGrid>>,
    mut orbital_pin: ResMut<OrbitalViewPin>,
    curvature: Option<Res<lunco_terrain_surface::TerrainBodyCurvature>>,
) {
    // `attach_site_scene_to_surface_grid` selects a celestial surface Grid as
    // Avian's active frame. That Grid is part of the scene-owned hierarchy and
    // is about to be despawned, so restore the persistent shell frame in the
    // same lifecycle operation. Leaving an Entity id to a dead Grid makes the
    // physics bridge's next frame conversion structurally impossible.
    let world_grid = q_world_grid
        .single()
        .expect("WorldShellPlugin must provide exactly one persistent WorldGrid");
    // This assignment is intentionally immediate. `clear_scene_entities` queues
    // this teardown from inside a larger scene-replacement command buffer, and
    // the replacement mount is queued after it. Inserting the resource through
    // `Commands` would append the reset behind the replacement mount and restore
    // the outgoing WorldRoot AFTER the new body-fixed frame had been selected.
    // That made the bridge transport the live bodies into the wrong Avian frame.
    active_physics_frame.0 = world_grid;

    let mut n = 0;
    for e in &q_derived {
        // `try_despawn` (not `despawn`): a marked entity already parented under the
        // hierarchy is removed by its ancestor's recursive despawn, so by the time this
        // command applies it may be gone — `try_` makes that a no-op, not a warning.
        commands.entity(e).try_despawn();
        n += 1;
    }

    // The pin is a scene-scoped presentation fact. The avatar that owned the
    // orbit transaction is about to be retired with the outgoing scene, so
    // retaining the pin would make the replacement scene look orbital without
    // a valid `OrbitViewReturn` to restore its authored surface camera.
    *orbital_pin = OrbitalViewPin::default();
    if curvature.is_some() {
        commands.remove_resource::<lunco_terrain_surface::TerrainBodyCurvature>();
    }

    info!("[celestial] scene teardown retired {n} derived entities");
}

/// Standalone gravity plugin — registers gravity configuration types.
///
/// Provides:
/// - [`Gravity`] resource (Flat or Surface mode)
/// - [`lunco_environment::GravityProvider`] / [`GravityBody`] components
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
        app.init_resource::<OrbitalViewPin>();
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

#[cfg(test)]
mod scene_teardown_tests {
    use super::*;

    fn queue_teardown_then_select_replacement(mut commands: Commands, replacement: Entity) {
        commands.queue(lunco_core::run_scene_teardown);
        commands.queue(move |world: &mut World| {
            world.insert_resource(lunco_spatial::ActivePhysicsFrame(replacement));
        });
    }

    #[test]
    fn replacement_declarations_cannot_suppress_celestial_teardown() {
        let mut app = App::new();
        app.init_resource::<OrbitalViewPin>();
        app.add_systems(lunco_core::SceneTeardown, teardown_celestial_scene);

        let world_grid = app.world_mut().spawn(lunco_spatial::WorldGrid).id();
        let outgoing_frame = app.world_mut().spawn_empty().id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(outgoing_frame));
        let outgoing = app
            .world_mut()
            .spawn(big_space_setup::CelestialDerived)
            .id();
        // This is the restart shape that defeated the former Update/run_if:
        // replacement declarations already exist before the next frame.
        let replacement_decl = app
            .world_mut()
            .spawn(CelestialBodyDecl {
                naif: lunco_celestial::ephemeris_id::MOON,
            })
            .id();

        lunco_core::run_scene_teardown(app.world_mut());

        assert!(app.world().get_entity(outgoing).is_err());
        assert!(app.world().get_entity(replacement_decl).is_ok());
        assert_eq!(
            app.world()
                .resource::<lunco_spatial::ActivePhysicsFrame>()
                .0,
            world_grid
        );
    }

    #[test]
    fn teardown_frame_reset_cannot_overwrite_replacement_frame() {
        let mut app = App::new();
        app.init_resource::<OrbitalViewPin>();
        app.add_systems(lunco_core::SceneTeardown, teardown_celestial_scene);

        let world_grid = app.world_mut().spawn(lunco_spatial::WorldGrid).id();
        let outgoing_frame = app.world_mut().spawn_empty().id();
        let replacement_frame = app.world_mut().spawn_empty().id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(outgoing_frame));
        app.world_mut().spawn(big_space_setup::CelestialDerived);

        app.add_systems(Update, move |commands: Commands| {
            queue_teardown_then_select_replacement(commands, replacement_frame);
        });
        app.update();

        assert_eq!(
            app.world()
                .resource::<lunco_spatial::ActivePhysicsFrame>()
                .0,
            replacement_frame,
            "deferred celestial teardown reset overwrote the replacement physics frame"
        );
        assert_ne!(
            app.world()
                .resource::<lunco_spatial::ActivePhysicsFrame>()
                .0,
            world_grid
        );
    }

    #[test]
    fn scene_teardown_clears_the_outgoing_orbital_presentation_pin() {
        let mut app = App::new();
        app.insert_resource(OrbitalViewPin {
            active: true,
            body: lunco_celestial::ephemeris_id::EARTH,
            dir: DVec3::X,
            distance: 42.0,
        });
        app.add_systems(lunco_core::SceneTeardown, teardown_celestial_scene);

        let world_grid = app.world_mut().spawn(lunco_spatial::WorldGrid).id();
        app.insert_resource(lunco_spatial::ActivePhysicsFrame(world_grid));

        lunco_core::run_scene_teardown(app.world_mut());

        assert_eq!(
            *app.world().resource::<OrbitalViewPin>(),
            OrbitalViewPin::default()
        );
    }
}
