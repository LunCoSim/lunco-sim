//! Headless-safe LunCoSim application runtime.
//!
//! This package owns the simulation composition shared by the GUI shell,
//! headless server, and authored scene-test runner. It has no renderer, egui,
//! workbench, picking, or tutorial policy. Those capabilities are layered by
//! the application/UI packages.

use avian3d::prelude::PhysicsPlugins;
use bevy::prelude::*;
use big_space::prelude::*;

use lunco_avatar::LunCoAvatarPlugin;
use lunco_controller::LunCoControllerPlugin;
use lunco_cosim::CoSimPlugin;
use lunco_cosim_core::schedule::{
    CosimApplySet as ApplyForcesCosimSet, CosimSet as PropagateCosimSet,
};
use lunco_environment::EnvironmentPlugin;
use lunco_hardware::LunCoHardwarePlugin;
use lunco_mobility::LunCoMobilityPlugin;
use lunco_modelica_runtime::ModelicaSet;
use lunco_obstacle_field::ObstacleFieldPlugin;
use lunco_terrain_globe::TerrainPlugin;
use lunco_terrain_surface::TerrainSurfacePlugin;
use lunco_usd_avian_core::BigSpacePhysicsBridgePlugin;
use lunco_usd_avian_filters::filtered_pairs::UsdCollisionFilter;
use lunco_usd_bevy_runtime::UsdPlugins;

const INPUT_BINDINGS_KIND: &str = "lunco.input-bindings.v1";

struct PendingInputBindingsAsset {
    path: String,
    handle: Handle<lunco_assets_runtime::TextAsset>,
}

/// Runtime state for the application-owned input defaults.
///
/// The input contract does not know where its defaults live. The application
/// discovers the uniquely typed JSON document from the runtime asset manifest,
/// then applies it to the persisted section through the generic input API.
#[derive(Resource, Default)]
struct InputBindingsDefaults {
    candidates: Vec<PendingInputBindingsAsset>,
    scan_started: bool,
    completed: bool,
}

fn load_input_bindings_defaults(
    mut state: ResMut<InputBindingsDefaults>,
    catalog: Option<Res<lunco_assets_runtime::TextAssetCatalog>>,
    asset_server: Option<Res<AssetServer>>,
    assets: Option<Res<Assets<lunco_assets_runtime::TextAsset>>>,
    mut settings: Option<ResMut<lunco_input_core::InputBindingsSettings>>,
    mut commands: Commands,
) {
    if state.completed {
        return;
    }
    let (Some(catalog), Some(asset_server), Some(assets), Some(settings)) =
        (catalog, asset_server, assets, settings.as_mut())
    else {
        return;
    };
    if !catalog.ready() {
        return;
    }

    if !state.scan_started {
        state.candidates = catalog
            .entries()
            .iter()
            .filter(|entry| entry.twin_id.is_none() && entry.asset_path.ends_with(".json"))
            .map(|entry| PendingInputBindingsAsset {
                path: entry.asset_path.clone(),
                handle: entry.handle.clone(),
            })
            .collect();
        state.scan_started = true;
    }

    let candidates = std::mem::take(&mut state.candidates);
    let mut pending = Vec::new();
    let mut failed_paths = Vec::new();
    let mut selected: Option<(String, String)> = None;

    for candidate in candidates {
        let Some(asset) = assets.get(&candidate.handle) else {
            let load_failed = asset_server
                .get_load_state(candidate.handle.id())
                .is_some_and(|load_state| load_state.is_failed());
            if !load_failed {
                pending.push(candidate);
            } else {
                failed_paths.push(candidate.path);
            }
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&asset.text) else {
            continue;
        };
        if value.get("kind").and_then(serde_json::Value::as_str) != Some(INPUT_BINDINGS_KIND) {
            continue;
        }
        if selected.is_some() {
            state.completed = true;
            lunco_core::trigger_runtime_error(
                &mut commands,
                "input-bindings-defaults-ambiguous",
                format!(
                    "runtime asset listing contains more than one asset marked {INPUT_BINDINGS_KIND}"
                ),
            );
            return;
        }
        selected = Some((candidate.path, asset.text.clone()));
    }

    state.candidates = pending;
    if !state.candidates.is_empty() {
        return;
    }

    state.completed = true;
    let Some((path, text)) = selected else {
        lunco_core::trigger_runtime_error(
            &mut commands,
            "input-bindings-defaults-missing",
            format!(
                "runtime asset listing contains no asset marked {INPUT_BINDINGS_KIND}; discovered {} JSON asset(s), failed: {}",
                catalog
                    .entries()
                    .iter()
                    .filter(|entry| entry.asset_path.ends_with(".json"))
                    .count(),
                if failed_paths.is_empty() {
                    "none".to_string()
                } else {
                    failed_paths.join(", ")
                }
            ),
        );
        return;
    };
    if let Err(error) = settings.apply_defaults_json(&text) {
        lunco_core::trigger_runtime_error(
            &mut commands,
            "input-bindings-defaults-invalid",
            format!("{path}: invalid authored input bindings: {error}"),
        );
    }
}

/// The luncosim's one physics configuration.
fn luncosim_physics_plugins() -> impl PluginGroup {
    PhysicsPlugins::default()
        .with_collision_hooks::<UsdCollisionFilter>()
        .set(avian3d::prelude::PhysicsInterpolationPlugin::interpolate_all())
}

/// The luncosim's gravity before any scene is loaded, and the value scene
/// teardown restores when one unloads.
pub const SANDBOX_GRAVITY: lunco_environment::Gravity = lunco_environment::Gravity::flat(
    lunco_environment::MOON_SURFACE_GRAVITY,
    bevy::math::DVec3::NEG_Y,
);

/// The shared, headless-safe simulation substrate: the persistent world shell,
/// physics, cosim, USD scene load, mobility, hardware, controller, avatar,
/// environment, and renderer-independent scene services.
///
/// GPU presentation and recovery are configured by the GUI shell. Shared
/// quality policy and USD visual sync remain device-independent, so scene
/// materialization uses the same authored profile in headless mode.
pub struct LunCoSimSimulationPlugin;

// `set_parent_in_place` is `disallowed_methods`-banned for its atomicity
// hazard (a `GridAnchor`/`RigidBody` parented after spawn can be mis-tagged
// `RigidBody::Static`). `ensure_world_root` may parent the big_space root →
// Grid internally; it is not a rigid body / GridAnchor, so that hazard doesn't
// apply. Locally allowed.
#[allow(clippy::disallowed_methods)]
fn setup_luncosim(world: &mut World) {
    // The persistent world shell (BigSpace root + `WorldGrid` + the single
    // `FloatingOrigin`) is owned by `WorldShellPlugin`. `ensure_world_root` is a
    // defensive create-or-get so the shell exists before any scene loads.
    //
    // The scene's SUN is no longer spawned here. It is a UsdLux `DistantLight`
    // authored in the scene itself (`sandbox_scene.usda`), instantiated by the
    // same USD loader as every other light — so a scene clear despawns it and a
    // scene load recreates it as ordinary scene content. There is no
    // Rust-spawned sun and no restore-on-switch machinery; see
    // `lunco_usd_bevy_light::light` for the single light path and the
    // post-load light-existence check that errors if a scene ships without one.
    let grid = lunco_spatial::ensure_world_root(world);
    // The shell owns topology; the application owns which grid Avian uses.
    // Bind the canonical WorldGrid explicitly for the empty state. Scene
    // mounts replace this binding with their authored site frame when celestial
    // placement completes.
    world.insert_resource(lunco_spatial::ActivePhysicsFrame(grid));
}

#[cfg(test)]
mod physics_configuration_tests {
    use super::*;
    use avian3d::prelude::{
        NoRotationEasing, NoTranslationEasing, RigidBody, RotationInterpolation,
        TranslationInterpolation,
    };

    #[test]
    fn physical_bodies_receive_render_interpolation() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(luncosim_physics_plugins());

        let body = app.world_mut().spawn(RigidBody::Dynamic).id();

        assert!(
            app.world().get::<TranslationInterpolation>(body).is_some(),
            "a physical body's rendered translation must be eased between solved poses"
        );
        assert!(
            app.world().get::<RotationInterpolation>(body).is_some(),
            "a physical body's rendered rotation must be eased between solved poses"
        );
        assert!(
            app.world().get::<NoTranslationEasing>(body).is_none(),
            "the bridge must not disable rendered translation easing"
        );
        assert!(
            app.world().get::<NoRotationEasing>(body).is_none(),
            "the bridge must not disable rendered rotation easing"
        );
    }
}

#[cfg(test)]
mod big_space_propagation_gate_tests {
    use super::*;

    #[derive(Resource, Default)]
    struct GateRuns(u32);

    #[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
    struct BigSpaceGateRuns {
        local_origins: u32,
        high_precision: u32,
    }

    fn count_gate_run(mut runs: ResMut<GateRuns>) {
        runs.0 += 1;
    }

    fn count_local_origin_gate(mut runs: ResMut<BigSpaceGateRuns>) {
        runs.local_origins += 1;
    }

    fn count_high_precision_gate(mut runs: ResMut<BigSpaceGateRuns>) {
        runs.high_precision += 1;
    }

    #[test]
    fn low_precision_gate_ignores_descendant_global_transform_outputs() {
        let mut app = App::new();
        app.init_resource::<GateRuns>()
            .add_systems(Update, count_gate_run.run_if(low_precision_propagation_due));

        let root = app.world_mut().spawn(CellCoord::default()).id();
        let descendant = app.world_mut().spawn(GlobalTransform::IDENTITY).id();

        // The initial root admission opens the gate once.
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 1);

        // A descendant GlobalTransform is BigSpace output, not an input to the
        // low-precision root walk. Mutating it must not reopen the gate.
        app.world_mut()
            .entity_mut(descendant)
            .insert(GlobalTransform::from_translation(Vec3::new(1.0, 0.0, 0.0)));
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 1);

        // An actual local spatial input still opens the gate.
        app.world_mut()
            .get_mut::<Transform>(root)
            .expect("CellCoord requires a local Transform")
            .translation
            .x += 1.0;
        app.update();
        assert_eq!(app.world().resource::<GateRuns>().0, 2);
    }

    #[test]
    fn origin_shift_gets_one_settle_pass_then_high_precision_gate_closes() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, BigSpaceDefaultPlugins))
            .init_resource::<BigSpaceGateRuns>()
            .add_systems(
                PostStartup,
                (
                    count_local_origin_gate.in_set(BigSpaceSystems::LocalFloatingOrigins),
                    count_high_precision_gate.in_set(BigSpaceSystems::PropagateHighPrecision),
                ),
            )
            .add_systems(
                PostUpdate,
                (
                    count_local_origin_gate.in_set(BigSpaceSystems::LocalFloatingOrigins),
                    count_high_precision_gate.in_set(BigSpaceSystems::PropagateHighPrecision),
                ),
            );
        configure_big_space_propagation_gates(&mut app);

        let root = app.world_mut().spawn(BigSpaceRootBundle::default()).id();
        let origin = app
            .world_mut()
            .spawn((
                CellCoord::default(),
                Transform::default(),
                GlobalTransform::default(),
                FloatingOrigin,
            ))
            .set_parent_in_place(root)
            .id();

        // Drain startup and first-frame additions before recording the baseline.
        app.update();
        app.update();
        let baseline = app.world().resource::<BigSpaceGateRuns>();
        let local_origins_before = baseline.local_origins;
        let high_precision_before = baseline.high_precision;

        app.world_mut()
            .get_mut::<CellCoord>(origin)
            .expect("floating origin cell")
            .x = 1;
        app.update();

        let after_shift = app.world().resource::<BigSpaceGateRuns>();
        assert!(after_shift.local_origins > local_origins_before);
        assert!(after_shift.high_precision > high_precision_before);
        assert!(app.world().resource::<BigSpaceOriginSettlePending>().0);
        assert!(
            !app.world()
                .get::<Grid>(root)
                .expect("BigSpace root grid")
                .local_floating_origin()
                .is_local_origin_unchanged(),
            "a changed origin remains unsettled after its first computation"
        );

        let high_precision_after_shift = after_shift.high_precision;
        let local_origins_after_shift = after_shift.local_origins;
        app.update();

        let after_settle = app.world().resource::<BigSpaceGateRuns>();
        assert_eq!(
            after_settle.local_origins,
            local_origins_after_shift + 1,
            "the changed local origin needs one follow-up compute to restore its unchanged flag"
        );
        assert_eq!(
            after_settle.high_precision, high_precision_after_shift,
            "settling the origin flag is not another spatial input change"
        );
        assert!(
            app.world()
                .get::<Grid>(root)
                .expect("BigSpace root grid")
                .local_floating_origin()
                .is_local_origin_unchanged(),
            "the follow-up computation settles the changed origin"
        );
        assert!(!app.world().resource::<BigSpaceOriginSettlePending>().0);

        let settled = *after_settle;
        app.update();
        assert_eq!(
            *app.world().resource::<BigSpaceGateRuns>(),
            settled,
            "stable inputs must close both BigSpace propagation gates after settling"
        );
    }
}

impl Plugin for LunCoSimSimulationPlugin {
    fn build(&self, app: &mut App) {
        if !app
            .world()
            .contains_resource::<lunco_physics::PhysicsDeterminism>()
        {
            app.insert_resource(lunco_physics::PhysicsDeterminism::from_compute_threads(
                None,
            ));
        }
        let args: Vec<String> = std::env::args().collect();

        // Dataset state is part of the shared simulation composition. The USD
        // terrain projection consumes this registry in GUI and headless hosts;
        // installing it only in the headless constructor leaves the windowed
        // production app with a missing-resource panic during its first update.
        app.add_plugins(lunco_assets_datasets::DatasetRegistryPlugin);
        app.add_plugins(lunco_assets_runtime::DatasetArtifactPlugin);
        app.init_resource::<InputBindingsDefaults>()
            .add_systems(Update, load_input_bindings_defaults);

        // Asset and loaded-stage validation is a shared headless/UI service;
        // install it once with the simulator core rather than coupling it to
        // the scene mutation command crate.
        app.add_plugins(lunco_scene_validation::SceneValidationPlugin);

        // Quality-dependent mesh and USD visual projection use the same
        // authored profile in every host. Only GPU recovery stays in the GUI.
        if !app.is_plugin_added::<lunco_render::RenderQualityPolicyPlugin>() {
            app.add_plugins(lunco_render::RenderQualityPolicyPlugin);
        }

        app.add_plugins(lunco_core_runtime::gate::GatePlugin);

        app
            // Match the workbench theme's backdrop so the window's first-frame
            // clear lines up with egui's panel fill (no "left hairline" at panel
            // boundaries under non-integer DPRs). Harmless headless.
            .insert_resource(ClearColor(Color::srgb_u8(0x1a, 0x1a, 0x1a)))
            .insert_resource(Time::<Fixed>::from_hz(lunco_core_runtime::FIXED_HZ))
            .insert_resource(avian3d::prelude::Gravity::ZERO)
            // The luncosim's gravity BEFORE any scene loads. Lunar, because every
            // vehicle in it is: the rovers' drivetrains, the lander's struts and
            // its propellant budget are all sized for 1.62. A scene overrides this
            // through its own `UsdPhysicsScene`; this is only what an empty
            // viewport uses, and the value teardown restores when a scene unloads.
            .insert_resource(SANDBOX_GRAVITY)
            // A scene SHOULD override gravity — that is what its `UsdPhysicsScene`
            // is for. What it must not do is leave that override behind: unloading
            // a lunar scene restores the luncosim's own value, so whatever loads
            // next starts from the app's baseline rather than the last scene's.
            .add_systems(lunco_core::SceneTeardown, |mut commands: Commands| {
                commands.insert_resource(SANDBOX_GRAVITY)
            })
            // Studio lighting for the luncosim — a generic editor scene, NOT a
            // calibrated lunar surface (the canonical `LunarSun` defaults
            // crush the dark blueprint ground to black). Inserted BEFORE
            // `EnvironmentPlugin` so its `init_resource` keeps this one
            // authoritative resource. The sun spawn AND every camera's
            // exposure read it, so lux and EV stay matched. Tunable live via
            // `SetEnvironmentLight`.
            .insert_resource(lunco_environment::LunarSun::default())
            // Persistent world shell: one BigSpace root + `WorldGrid` + the
            // persistent `OriginAnchor`/`FloatingOrigin`. The validation plugin (debug builds only, logs
            // errors, never panics) is ENABLED: WorldRoot is Transform-free —
            // big_space-canonical — now that the Phase 5 bridge owns BOTH
            // things the root `Transform` was load-bearing for (avian's GT
            // sync and its root-anchored ColliderTransform propagation). The
            // validator is the guard that keeps new spawn paths canonical.
            .add_plugins(BigSpaceDefaultPlugins);
        #[cfg(debug_assertions)]
        gate_big_space_hierarchy_validation(app);
        app
            // EntityCount is cheap and useful any time we look at perf.
            .add_plugins(bevy::diagnostic::EntityCountDiagnosticsPlugin::default())
            // `with_collision_hooks` installs the ONE pair filter avian allows per
            // app: authored `PhysicsFilteredPairsAPI` pairs (`lunco-usd-avian-filters`'s
            // `UsdCollisionFilter`). Anything else that must veto a contact belongs
            // in that hook rather than in a second one — there is no second slot.
            .add_plugins(luncosim_physics_plugins())
            // Whoever installs physics installs its readiness gate: terrain/obstacle
            // subsystems suspend *integration* (avian's `Time<Physics>`) while their
            // colliders bake, instead of pausing the world clock. See `lunco-physics`.
            .add_plugins(lunco_physics::PhysicsGatePlugin)
            // Phase 5: physics stops sharing GlobalTransform with the render
            // world. Disables ALL of avian's f32 transform sync — including
            // `propagate_before_physics`, the third plain-GT whole-tree writer
            // (the measured 1-in-5–9 render strobe; doc 45 addendum) — and owns
            // the Position ↔ (cell, Transform) sync in the f64 cell chain. The
            // 2026-07-09 narrow_phase island panic was the old bridge dirtying
            // every static's Position every tick (whole-world contact churn);
            // this bridge is shadow-gated: a body syncs only when an external
            // writer actually moved it. Must be added AFTER PhysicsPlugins
            // (it overrides PhysicsTransformConfig).
            .add_plugins(BigSpacePhysicsBridgePlugin)
            // `lunco_physics::PhysicsGatePlugin` owns the single solver-resolution
            // contract and installs eight Avian substeps for every host. Keeping
            // this choice at the physics owner prevents the GUI, server, and web
            // application paths from silently simulating different mechanics.
            .add_plugins(CoSimPlugin)
            .add_plugins(lunco_core_runtime::LunCoCoreRuntimePlugin)
            .add_plugins(lunco_telemetry_core::LunCoTelemetryCorePlugin)
            .add_plugins(lunco_core_session::LunCoCoreSessionPlugin)
            // Renderer-independent exposure aggregation is kept in its own
            // production crate. It remains in the shared core path so GUI and
            // headless hosts publish identical facts, while exposure edits no
            // longer recompile this application composition root.
            .add_plugins(lunco_luncosim_exposures::RuntimeExposuresPlugin)
            .add_plugins(lunco_spatial::WorldShellPlugin)
            // Parameter telemetry — the PRODUCER of `SampledParameter`. Its consumer
            // side (`lunco_api`'s `sampled_param_observer`, i.e. `SubscribeTelemetry`,
            // plus `TelemetryResponse::from_sampled` and core's logger) was already
            // shipped and wired; this plugin was the one missing link, so the API
            // advertised parameter telemetry that could never arrive. Costs nothing
            // until someone authors a `Parameter` (the sampler is `run_if`-gated on
            // one existing), and it samples on the FIXED clock, so headless runs get a
            // stable telemetry rate instead of one that tracks the frame rate.
            .add_plugins(lunco_telemetry::LunCoTelemetryPlugin)
            // Canonical Twin change-journal (op log). CORE substrate, not UI:
            // it must exist on the headless server + every client so authored
            // edits are recorded (the domain registries' `wire_*_journal_handle`
            // systems fire on `resource_added::<JournalResource>` and attach a
            // recorder to each DocumentHost). Previously added only by the
            // workbench UI plugin, so a headless networked host journaled
            // nothing — the blocker for journal-on-wire sync. Pure lifecycle
            // observers + resources; no GPU/egui. The workbench add is now
            // guarded to avoid a double-add (double observers).
            .add_plugins(lunco_doc_bevy::TwinJournalPlugin)
            // GravityPlugin now rides in via CelestialPlugin below (guarded).
            .add_plugins(EnvironmentPlugin)
            .add_plugins(TerrainPlugin)
            // Procedural crater + rock field generator. Owns the shared
            // `ObstacleFieldSpec` + `UpdateObstacleFieldSpec` only; the real ground
            // is the USD DEM terrain, which observes that command and stamps
            // craters / scatters rocks into its own grid.
            .add_plugins(ObstacleFieldPlugin)
            // Authoritative DEM terrain, analytic queries, and heightfield
            // colliders. Camera-driven visual LOD is installed by the GUI
            // presentation composition, not by server or scene-test hosts.
            .add_plugins(TerrainSurfacePlugin)
            // Celestial stack (doc 43): dormant unless the SCENE asks for it. Bodies
            // are authored in USD (`LunCoCelestialBodyAPI` — reference
            // `assets/celestial/solar_system.usda`), and every celestial subsystem
            // gates on that authored fact, so the flat luncosim arena gets no sky at
            // all. The luncosim avatar remains independent of BigSpace origin
            // ownership. The
            // generic link kernel (doc 49) is always on — it needs no hierarchy —
            // and publishes `LinkState` + `link.aos`/`link.los`, NOT `comms:*`
            // ports (there is no comms subsystem to own them).
            .insert_resource(lunco_celestial_spatial::CelestialConfig {
                spawn_observer_camera: false,
            })
            .add_plugins(lunco_celestial_spatial::CelestialPlugin)
            // Real VSOP2013/ELP body positions on ALL platforms (wasm too) —
            // this is the explicit provider required by orbital scenes.
            .add_plugins(lunco_celestial_ephemeris::EphemerisPlugin)
            // Connectivity rides on the generic link kernel the CelestialPlugin
            // registers (doc 49): geometry in Rust, verdict via the `link.connected`
            // hook, routing authored in rhai over `query("Links")`. There is no comms
            // Rust plugin. Scene-local endpoints opt into pose tracking via
            // `lunco:solarTracked`.
            .add_plugins(LunCoHardwarePlugin)
            .add_plugins(LunCoMobilityPlugin)
            // USD scene load + avian collider build + cosim wiring —
            // server-authoritative, headless-safe.
            .add_plugins(UsdPlugins)
            // Vessel input + possession command observers. Headless-safe:
            // leafwing's InputManager rides on bevy_input (no winit), so a server
            // just produces no input while the Drive/Brake/Possess command
            // observers + wire-type registrations the host needs stay live.
            .add_plugins(LunCoControllerPlugin)
            .add_plugins(LunCoAvatarPlugin)
            .add_systems(Startup, setup_luncosim)
            // Cosim pipeline ordering: worker responses land in Update; the
            // fixed loop then propagates, applies, and dispatches the next
            // Modelica communication point.
            .configure_sets(
                FixedUpdate,
                (
                    PropagateCosimSet::Propagate,
                    ApplyForcesCosimSet::ApplyForces,
                    ModelicaSet::SpawnRequests,
                )
                    .chain(),
            );
        configure_big_space_propagation_gates(app);
        #[cfg(feature = "sysml")]
        app.add_plugins(lunco_sysml::SysmlPlugin);
        // Dynamic USD bodies are first promoted in `ActivateDynamicBodies`.
        // The terrain support projection must observe that promotion before it
        // decides whether physics may resume; plugin insertion order is not a
        // valid synchronization contract for a streamed physics world.
        // USD→terrain projection (`lunco-usd-terrain`): an authored terrain prim with
        // `lunco:assetMode="dem"` gets a DEM heightfield built onto it from its child
        // layer prims, and hand edits author back onto the document's runtime layer.
        // Core (not GUI-gated): the headless server needs the collider for
        // deterministic physics, and the crate links no render code.
        app.add_plugins(lunco_usd_terrain::UsdTerrainPlugin);
        // Scene-time selection runs at the settled scene-transition boundary.
        // Until that result resets the time spine, celestial projection and
        // DEM/georeference construction stay gated; the first consumers then
        // share the selected scene epoch.
        app.configure_sets(
            Update,
            (
                lunco_usd_sim_celestial::CelestialProjectionSet::Projection,
                lunco_usd_terrain::UsdTerrainSet::Bridge,
                lunco_celestial_spatial::CelestialTerrainSet::Curvature,
                lunco_terrain_surface::TerrainSurfaceSet::Build,
            )
                .chain(),
        );
        // Curvature is a resource inserted by the celestial coupling system.
        // Apply that deferred insertion before the DEM build set so the first
        // oracle captures the final body radius instead of being restamped from
        // a provisional flat surface one frame later.
        app.add_systems(
            Update,
            ApplyDeferred
                .after(lunco_celestial_spatial::CelestialTerrainSet::Curvature)
                .before(lunco_terrain_surface::TerrainSurfaceSet::Build),
        );
        // The activation gate stays here — it is the assembly point that sees both the
        // terrain request and the USD simulation readiness contract.
        app.add_systems(
            Update,
            track_ground_collider_pending.after(lunco_usd_terrain::UsdTerrainSet::Bridge),
        );
        // LogDiagnosticsPlugin is loud (a multi-line summary every second) — gate
        // it on `--log-diag`.
        if args.iter().any(|a| a == "--log-diag") {
            app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
        }
    }
}

/// Hold dynamic-body activation while an actual DEM terrain build is in flight.
///
/// The USD terrain bridge is ordered before this system and its deferred commands
/// are flushed at that boundary, so the query sees the authoritative
/// [`DemTerrainRequest`] for a newly composed terrain in the same update. A query
/// over every loaded USD prim is incorrect: most USD prims are not terrain and
/// would keep the entire simulation kinematic until an arbitrary timeout.
///
/// The request is removed together with the finished collider/oracle by the
/// terrain-surface owner. A declared-but-uninstalled Twin DEM carries
/// [`lunco_usd_terrain::DemDatasetPending`] instead of a build request; that
/// state is equally not ready for dynamic admission. This crate only mirrors
/// those domain-owned readiness states into the USD-simulation activation
/// resource.
fn track_ground_collider_pending(
    building: Query<
        (),
        Or<(
            With<lunco_terrain_surface::DemTerrainRequest>,
            With<lunco_usd_terrain::DemDatasetPending>,
        )>,
    >,
    mut pending: ResMut<lunco_usd_sim_core::GroundColliderPending>,
) {
    pending.0 = !building.is_empty();
}

#[cfg(test)]
mod ground_collider_gate_tests {
    use super::*;
    use lunco_usd_bevy_scene::UsdPrimPath;
    use lunco_usd_bevy_stage::UsdStageAsset;

    #[test]
    fn only_an_active_dem_request_holds_dynamic_activation() {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .init_resource::<lunco_usd_sim_core::GroundColliderPending>()
            .add_systems(Update, track_ground_collider_pending);

        // A loaded USD stage contains many prims that are not terrain. They do
        // not participate in this gate.
        let stage = Handle::<UsdStageAsset>::default();
        app.world_mut().spawn(UsdPrimPath {
            stage_handle: stage,
            path: "/Rover/Chassis".into(),
        });
        app.update();
        assert!(
            !app.world()
                .resource::<lunco_usd_sim_core::GroundColliderPending>()
                .0
        );

        let terrain = app
            .world_mut()
            .spawn(lunco_terrain_surface::DemTerrainRequest {
                uri: "terrain/site".into(),
                half_window: 1.0,
                target_res: 0,
                lod_viz: false,
                collider_ring: false,
                collider: lunco_terrain_surface::TerrainColliderSettings::default(),
            })
            .id();
        app.update();
        assert!(
            app.world()
                .resource::<lunco_usd_sim_core::GroundColliderPending>()
                .0
        );

        app.world_mut()
            .entity_mut(terrain)
            .remove::<lunco_terrain_surface::DemTerrainRequest>();
        app.update();
        assert!(
            !app.world()
                .resource::<lunco_usd_sim_core::GroundColliderPending>()
                .0
        );
    }

    #[test]
    fn an_uninstalled_twin_dem_keeps_dynamic_activation_held() {
        let mut app = App::new();
        app.init_resource::<lunco_usd_sim_core::GroundColliderPending>()
            .add_systems(Update, track_ground_collider_pending);

        let pending = app
            .world_mut()
            .spawn(lunco_usd_terrain::DemDatasetPending::new(
                "summer-space-school/apollo15",
            ))
            .id();
        app.update();
        assert!(
            app.world()
                .resource::<lunco_usd_sim_core::GroundColliderPending>()
                .0
        );

        app.world_mut().entity_mut(pending).despawn();
        app.update();
        assert!(
            !app.world()
                .resource::<lunco_usd_sim_core::GroundColliderPending>()
                .0
        );
    }
}

pub struct LunCoSimHeadlessPlugin {
    /// Host execution policy. Max-speed mode uses an explicit fixed duration
    /// and a zero-wait runner; realtime mode remains wall-clock paced.
    pub execution_mode: lunco_core_runtime::SimulationExecutionMode,
}

/// Install a headless runner whose wait is read after every update.
///
/// The command surface can therefore switch `SimulationExecutionMode` without
/// restarting the process. The runner owns only wall-clock waiting; the time
/// spine and any deterministic recorder still own the duration fed to Bevy's
/// clock. In headless max-speed mode this runner also installs the fixed
/// duration that the mode promises. Realtime restores automatic wall-clock
/// sampling when a live command switches back.
fn install_dynamic_headless_runner(app: &mut App) {
    app.set_runner(|mut app| {
        use bevy::app::PluginsState;
        use std::time::{Duration, Instant};

        let plugins_state = app.plugins_state();
        if plugins_state != PluginsState::Cleaned {
            while app.plugins_state() == PluginsState::Adding {
                std::thread::yield_now();
            }
            app.finish();
            app.cleanup();
        }

        let mut previous_mode = None;
        loop {
            let started = Instant::now();
            app.update();

            if let Some(exit) = app.should_exit() {
                return exit;
            }

            let mode = app
                .world()
                .get_resource::<lunco_core_runtime::SimulationExecutionMode>()
                .copied()
                .unwrap_or_default();
            if previous_mode != Some(mode) {
                match mode {
                    lunco_core_runtime::SimulationExecutionMode::Realtime => {
                        app.world_mut()
                            .insert_resource(bevy::time::TimeUpdateStrategy::Automatic);
                    }
                    lunco_core_runtime::SimulationExecutionMode::MaxSpeed => {
                        app.world_mut().insert_resource(
                            bevy::time::TimeUpdateStrategy::ManualDuration(
                                Duration::from_secs_f64(lunco_core_runtime::SECS_PER_TICK),
                            ),
                        );
                    }
                }
                previous_mode = Some(mode);
            }

            let cadence = match mode {
                lunco_core_runtime::SimulationExecutionMode::Realtime => {
                    Duration::from_secs_f64(1.0 / lunco_core_runtime::FIXED_HZ)
                }
                lunco_core_runtime::SimulationExecutionMode::MaxSpeed => Duration::ZERO,
            };
            let elapsed = started.elapsed();
            if elapsed < cadence {
                std::thread::sleep(cadence - elapsed);
            }
        }
    });
}

impl Default for LunCoSimHeadlessPlugin {
    fn default() -> Self {
        Self {
            execution_mode: lunco_core_runtime::SimulationExecutionMode::Realtime,
        }
    }
}

impl Plugin for LunCoSimHeadlessPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.execution_mode);
        // Modelica compiler/document core plus the separate execution plugin —
        // NO egui/viz/workbench. The split keeps compiler-only consumers free
        // of solver workers while this runtime still installs the authoritative
        // Modelica compile and execution path used by cosim scenes.
        app.add_plugins(lunco_modelica_core::ModelicaCorePlugin);
        app.add_plugins(lunco_modelica_execution::ModelicaExecutionPlugin);

        // Spawn-command CORE (runtime spawn/move/property commands + the
        // `apply_net_replication` system that tags dynamic scene bodies with
        // `NetReplicate`). Windowed builds get this transitively via
        // `SceneEditPlugin`; without it the headless host replicates NOTHING
        // (the connect baseline is empty) because nothing marks the rovers. The
        // gizmo/selection/physics-viz halves of `SceneEditPlugin` stay UI-only.
        app.add_plugins(lunco_scene_commands::commands::SpawnCommandPlugin);
        app.add_plugins(lunco_scene_camera::SceneCameraCommandPlugin);
        app.add_plugins(lunco_scene_selection::SceneSelectionPlugin);

        // No winit event loop drives updates headless. Realtime mode uses the
        // fixed cadence as the server's wall-clock pacing; max-speed mode feeds
        // one fixed duration per update and removes the wait entirely. Both
        // modes still execute the same schedules and the same causal barrier.
        if matches!(
            self.execution_mode,
            lunco_core_runtime::SimulationExecutionMode::MaxSpeed
        ) {
            app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_secs_f64(lunco_core_runtime::SECS_PER_TICK),
            ));
        }
        install_dynamic_headless_runner(app);

        info!(
            "[luncosim] running HEADLESS (--no-ui), execution={:?}: no window/GPU/egui; local simulation only",
            self.execution_mode
        );
    }
}

#[derive(Resource, Default)]
struct BigSpaceOriginSettlePending(bool);

type HighPrecisionPropagationChanges = Or<(
    (With<FloatingOrigin>, Changed<CellCoord>),
    (
        With<CellCoord>,
        Without<Stationary>,
        Or<(Changed<Transform>, Changed<CellCoord>, Changed<ChildOf>)>,
    ),
    (With<Grid>, Changed<Children>),
    (With<Stationary>, Without<StationaryInitialized>),
)>;

/// Open the high-precision propagation set only when an input can change its output.
///
/// BigSpace's propagation system already prunes clean subtrees, but an active
/// channeled pass still creates a compute scope and walks grids before finding
/// clean work. The application owns the schedule boundary, while BigSpace
/// remains the sole owner of propagation and its per-entity rules. This
/// condition mirrors only authoritative inputs that can invalidate a
/// high-precision global transform; a false positive costs one ordinary pass,
/// while a false negative would leave rendered GlobalTransforms stale. The
/// per-compute local-origin unchanged flag is output, not persistent input.
fn high_precision_propagation_due(changed: Query<(), HighPrecisionPropagationChanges>) -> bool {
    !changed.is_empty()
}

type LocalOriginPropagationChanges = Or<(
    (With<FloatingOrigin>, Changed<CellCoord>),
    Or<(Changed<ChildOf>, Changed<Children>)>,
    Added<FloatingOrigin>,
    Added<Grid>,
    Added<BigSpace>,
)>;

/// Open BigSpace's local-origin walk for changed inputs or one settle pass.
fn local_origin_propagation_due(
    settle_pending: Res<BigSpaceOriginSettlePending>,
    changed: Query<(), LocalOriginPropagationChanges>,
) -> bool {
    !changed.is_empty() || settle_pending.0
}

/// Remember whether BigSpace needs one follow-up local-origin computation.
/// This scan runs only after an admitted propagation pass, not on idle frames.
fn update_big_space_origin_settle_pending(
    mut settle_pending: ResMut<BigSpaceOriginSettlePending>,
    grids: Query<&Grid>,
) {
    settle_pending.0 = grids
        .iter()
        .any(|grid| !grid.local_floating_origin().is_local_origin_unchanged());
}

/// Apply application-owned input admission without replacing BigSpace's systems.
fn configure_big_space_propagation_gates(app: &mut App) {
    app.init_resource::<BigSpaceOriginSettlePending>()
        .add_systems(
            PostStartup,
            update_big_space_origin_settle_pending
                .after(LocalFloatingOrigin::compute_all)
                .in_set(BigSpaceSystems::LocalFloatingOrigins),
        )
        .add_systems(
            PostUpdate,
            update_big_space_origin_settle_pending
                .after(LocalFloatingOrigin::compute_all)
                .in_set(BigSpaceSystems::LocalFloatingOrigins),
        );

    app.configure_sets(
        PostStartup,
        BigSpaceSystems::LocalFloatingOrigins.run_if(local_origin_propagation_due),
    )
    .configure_sets(
        PostUpdate,
        BigSpaceSystems::LocalFloatingOrigins.run_if(local_origin_propagation_due),
    )
    .configure_sets(
        PostStartup,
        BigSpaceSystems::PropagateHighPrecision.run_if(high_precision_propagation_due),
    )
    .configure_sets(
        PostUpdate,
        BigSpaceSystems::PropagateHighPrecision.run_if(high_precision_propagation_due),
    )
    .configure_sets(
        PostStartup,
        BigSpaceSystems::PropagateLowPrecision.run_if(low_precision_propagation_due),
    )
    .configure_sets(
        PostUpdate,
        BigSpaceSystems::PropagateLowPrecision.run_if(low_precision_propagation_due),
    );
}

type LowPrecisionPropagationChanges = Or<(
    Or<(Changed<Transform>, Added<Transform>)>,
    Or<(Changed<ChildOf>, Changed<Children>)>,
    (
        Or<(With<Grid>, With<CellCoord>)>,
        Or<(Changed<GlobalTransform>, Added<GlobalTransform>)>,
    ),
)>;

/// Open BigSpace's low-precision walk only when a local transform hierarchy or
/// an upstream high-precision root changed.
fn low_precision_propagation_due(
    changed: Query<(), LowPrecisionPropagationChanges>,
    // This is deliberately the same root predicate BigSpace uses for its low
    // precision walk. A descendant `GlobalTransform` is an OUTPUT of that walk;
    // treating it as an input makes the application reopen the walk because of
    // BigSpace's own previous-frame writes.
    mut removed_transforms: RemovedComponents<Transform>,
    mut removed_hierarchy: RemovedComponents<ChildOf>,
    mut removed_global_transforms: RemovedComponents<GlobalTransform>,
) -> bool {
    !changed.is_empty()
        || removed_transforms.read().next().is_some()
        || removed_hierarchy.read().next().is_some()
        || removed_global_transforms.read().next().is_some()
}

/// Keep BigSpace's debug hierarchy validator authoritative while avoiding a
/// full-tree walk on every settled frame. Hierarchy validity can change only
/// when the spatial component set or parent/child topology changes; transform
/// value changes do not alter which node kind an entity is.
#[cfg(debug_assertions)]
fn gate_big_space_hierarchy_validation(app: &mut App) {
    let removed = app
        .remove_systems_in_set(
            PostUpdate,
            big_space::validation::validate_hierarchy::<big_space::validation::SpatialHierarchyRoot>,
            bevy::ecs::schedule::ScheduleCleanupPolicy::RemoveSystemsOnly,
        )
        .expect("BigSpace hierarchy validator must be installed by BigSpaceDefaultPlugins");
    assert_eq!(
        removed, 1,
        "BigSpace hierarchy validator must be installed once"
    );
    app.add_systems(
        PostUpdate,
        big_space::validation::validate_hierarchy::<big_space::validation::SpatialHierarchyRoot>
            .run_if(big_space_hierarchy_validation_due)
            .after(TransformSystems::Propagate),
    );
}

#[cfg(debug_assertions)]
fn big_space_hierarchy_validation_due(
    changed_components: Query<
        (),
        Or<(
            Added<CellCoord>,
            Added<Transform>,
            Added<GlobalTransform>,
            Added<BigSpace>,
            Added<Grid>,
            Added<FloatingOrigin>,
            Added<ChildOf>,
            Changed<ChildOf>,
            Changed<Children>,
        )>,
    >,
    mut removed_cell: RemovedComponents<CellCoord>,
    mut removed_transform: RemovedComponents<Transform>,
    mut removed_global_transform: RemovedComponents<GlobalTransform>,
    mut removed_big_space: RemovedComponents<BigSpace>,
    mut removed_grid: RemovedComponents<Grid>,
    mut removed_floating_origin: RemovedComponents<FloatingOrigin>,
    mut removed_child: RemovedComponents<ChildOf>,
) -> bool {
    !changed_components.is_empty()
        || removed_cell.read().next().is_some()
        || removed_transform.read().next().is_some()
        || removed_global_transform.read().next().is_some()
        || removed_big_space.read().next().is_some()
        || removed_grid.read().next().is_some()
        || removed_floating_origin.read().next().is_some()
        || removed_child.read().next().is_some()
}
