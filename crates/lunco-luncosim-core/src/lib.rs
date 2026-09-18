//! Headless-safe LunCoSim application runtime.
//!
//! This package owns the simulation composition shared by the GUI shell,
//! headless server, and authored scene-test runner. It has no renderer, egui,
//! workbench, picking, or tutorial policy. Those capabilities are layered by
//! the application/UI packages.

use avian3d::prelude::PhysicsPlugins;
use bevy::asset::{AssetMetaCheck, AssetPlugin};
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
            .filter(|entry| entry.asset_path.ends_with(".json"))
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
                catalog.entries().iter().filter(|entry| entry.asset_path.ends_with(".json")).count(),
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

/// Asset registration needed by USD authoring in a headless world. These are
/// data stores only; no render plugin is installed here.
struct HeadlessAssetTypePlugin;

impl Plugin for HeadlessAssetTypePlugin {
    fn build(&self, app: &mut App) {
        // Avian's collider cache consumes AssetEvent<Mesh> even in a
        // render-free world. Register the asset type here so its message
        // channel exists before the first schedule update; visual material
        // stores remain intentionally limited to the data-only types below.
        app.init_asset::<bevy::mesh::Mesh>();
        app.init_asset::<bevy::shader::Shader>();
        app.init_asset::<bevy::image::Image>();
    }
}

/// Exit status returned by the production runner.
pub use bevy::app::AppExit;

/// SemVer2 product version stamped into this build. Release builds may carry a
/// CI-derived nightly version while Cargo.toml keeps the stable package base.
pub const PRODUCT_VERSION: &str = env!("LUNCO_RELEASE_VERSION");
/// Short source revision stamped into this build for diagnostics.
pub const GIT_SHA: &str = env!("LUNCO_GIT_SHA");
/// Public GitHub repository containing the stamped source revision.
pub const REPOSITORY_URL: &str = env!("LUNCO_REPOSITORY_URL");

/// Print the build identity shared by every production LunCoSim host.
pub fn log_build_identity(mode: &str) {
    println!(
        "[lunco] luncosim {ver} ({sha}) {profile} {mode} {os}/{arch}",
        ver = PRODUCT_VERSION,
        sha = GIT_SHA,
        profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    );
}

/// Collapse repeated WARN/ERROR lines into one line plus a count.
pub mod log_dedup;

#[cfg(test)]
mod headless_composition_tests {
    use super::*;

    #[test]
    fn headless_plugins_install_only_data_asset_stores() {
        let mut app = App::new();
        app.add_plugins(default_plugins());

        assert!(app.is_plugin_added::<AssetPlugin>());
        assert!(app.world().get_resource::<AssetServer>().is_some());
        assert!(app
            .world()
            .get_resource::<Assets<bevy::shader::Shader>>()
            .is_some());
        assert!(app
            .world()
            .get_resource::<Assets<bevy::image::Image>>()
            .is_some());
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

/// Build the headless Bevy plugin group shared by the server and scene tests.
///
/// This is intentionally a hand-selected substrate rather than
/// `DefaultPlugins` with render plugins disabled. Cargo feature unification can
/// make a render plugin available in a downstream GUI build even when the core
/// package did not request it; composing the headless group from `MinimalPlugins`
/// makes that boundary structural and fail-closed.
pub fn default_plugins() -> bevy::app::PluginGroupBuilder {
    // The host owns scheduling.  MinimalPlugins includes Bevy's default
    // ScheduleRunnerPlugin; leaving it enabled makes every headless host
    // that installs its explicit cadence fail with a duplicate-plugin panic.
    // Keep the substrate inert so the server and scene-test runner can each
    // install exactly one policy-owned runner.
    let group = MinimalPlugins
        .build()
        .disable::<bevy::app::ScheduleRunnerPlugin>()
        .add(bevy::app::PanicHandlerPlugin)
        .add(bevy::log::LogPlugin {
            filter: "wgpu=error,naga=warn,cranelift=warn,cranelift_jit=warn,cranelift_codegen=warn,diffsol=warn,info".into(),
            fmt_layer: |_app| {
                use bevy::log::tracing_subscriber::Layer;
                use std::io::IsTerminal;
                let ansi =
                    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
                Some(Box::new(
                    bevy::log::tracing_subscriber::fmt::Layer::default()
                        .with_ansi(ansi)
                        .with_writer(std::io::stderr)
                        .with_filter(log_dedup::DedupFilter),
                ))
            },
            ..default()
        })
        .add(bevy::diagnostic::DiagnosticsPlugin)
        .add(bevy::input::InputPlugin)
        .add(bevy::input_focus::InputFocusPlugin)
        .add(bevy::input_focus::InputDispatchPlugin)
        .add(bevy::state::app::StatesPlugin)
        .add(AssetPlugin {
            file_path: lunco_assets_core::assets_dir_abs().to_string_lossy().to_string(),
            watch_for_changes_override: Some(false),
            meta_check: AssetMetaCheck::Never,
            ..default()
        })
        .add_after::<AssetPlugin>(HeadlessAssetTypePlugin);

    // BigSpace owns the transform propagation chain for the simulation world;
    // the ordinary Bevy transform plugin is deliberately not added here.
    group.build()
}

/// Build the generic headless simulation substrate with an optional fixed
/// compute-pool size. Application services and startup-scene policy are layered
/// by `lunco-luncosim-runtime`.
///
/// This function installs only the core simulation plugin. Application
/// integrations such as Rhai policies and the production schedule runner are
/// owned by `lunco-luncosim-runtime`.
pub fn build_core_app(compute_threads: Option<usize>) -> App {
    let mut app = App::new();
    lunco_assets_runtime::register_lunco_asset_sources(&mut app);

    let mut plugins = default_plugins();
    let compute = if let Some(threads) = compute_threads {
        assert!(threads > 0, "compute_threads must be positive");
        bevy::app::TaskPoolThreadAssignmentPolicy {
            min_threads: threads,
            max_threads: threads,
            percent: 1.0,
            on_thread_spawn: None,
            on_thread_destroy: None,
        }
    } else {
        bevy::app::TaskPoolThreadAssignmentPolicy {
            min_threads: 1,
            max_threads: 4,
            percent: 1.0,
            on_thread_spawn: None,
            on_thread_destroy: None,
        }
    };
    plugins = plugins.set(bevy::app::TaskPoolPlugin {
        task_pool_options: bevy::app::TaskPoolOptions {
            compute,
            ..default()
        },
    });
    app.add_plugins(plugins);
    lunco_assets_runtime::register_lunco_asset_types(&mut app);
    app.insert_resource(lunco_physics::PhysicsDeterminism::from_compute_threads(
        compute_threads,
    ));
    app.add_plugins(log_dedup::LogDedupPlugin);
    app.add_plugins(LunCoSimCorePlugin);
    app
}

/// The shared, headless-safe simulation substrate: the persistent world shell,
/// physics, cosim, USD scene load, mobility, hardware, controller, avatar,
/// environment, and renderer-independent scene services.
///
/// The render plugins are configured in the GUI shell; this package's
/// [`default_plugins`] is headless-only. Every plugin here is pure-CPU
/// simulation/state. USD visual sync only writes the mesh/material asset stores
/// (never touches a GPU device), so it is safe in headless mode.
pub struct LunCoSimCorePlugin;

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

    fn count_gate_run(mut runs: ResMut<GateRuns>) {
        runs.0 += 1;
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
}

impl Plugin for LunCoSimCorePlugin {
    fn build(&self, app: &mut App) {
        let args: Vec<String> = std::env::args().collect();

        // Dataset state is part of the shared simulation composition. The USD
        // terrain projection consumes this registry in GUI and headless hosts;
        // installing it only in the headless constructor leaves the windowed
        // production app with a missing-resource panic during its first update.
        app.add_plugins(lunco_assets_datasets::DatasetRegistryPlugin);
        app.init_resource::<InputBindingsDefaults>()
            .add_systems(Update, load_input_bindings_defaults);

        // Asset and loaded-stage validation is a shared headless/UI service;
        // install it once with the simulator core rather than coupling it to
        // the scene mutation command crate.
        app.add_plugins(lunco_scene_validation::SceneValidationPlugin);

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
            // Streamed, dynamically-LOD'd terrain (DEM tiles + heightfield
            // colliders). Inert at M0 (config only); see lunco-terrain-surface
            // and docs/architecture/terrain-substrate.md.
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
            // Keep the application-owned invalidation boundary around
            // BigSpace's propagation sets. BigSpace remains the sole owner of
            // propagation and moving physics inputs are allowed to reopen the
            // sets; the boundary only rejects known non-input output changes.
            .configure_sets(
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
            )
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
/// over every [`lunco_usd_bevy_scene::UsdPrimPath`] is incorrect: most USD prims are not terrain and
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
        let wait = match self.execution_mode {
            lunco_core_runtime::SimulationExecutionMode::Realtime => {
                std::time::Duration::from_secs_f64(1.0 / lunco_core_runtime::FIXED_HZ)
            }
            lunco_core_runtime::SimulationExecutionMode::MaxSpeed => {
                app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                    std::time::Duration::from_secs_f64(lunco_core_runtime::SECS_PER_TICK),
                ));
                std::time::Duration::ZERO
            }
        };
        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(wait));

        info!(
            "[luncosim] running HEADLESS (--no-ui), execution={:?}: no window/GPU/egui; local simulation only",
            self.execution_mode
        );
    }
}

/// Open the high-precision propagation set only when its output can change.
///
/// BigSpace's propagation system already prunes clean subtrees, but its
/// channeled implementation still creates a compute scope and walks every grid
/// on each PostUpdate before it can discover that all subtrees are clean. The
/// application owns the schedule boundary, while BigSpace remains the sole
/// owner of propagation and its exact per-entity rules. This condition mirrors
/// only the authoritative inputs that can invalidate a high-precision global
/// transform; a false positive costs one ordinary propagation pass, while a
/// false negative would leave rendered GlobalTransforms stale.
fn high_precision_propagation_due(
    grids: Query<&Grid>,
    changed_spatial: Query<
        (),
        (
            With<CellCoord>,
            Without<Stationary>,
            Or<(Changed<Transform>, Changed<CellCoord>, Changed<ChildOf>)>,
        ),
    >,
    changed_grid_children: Query<(), (With<Grid>, Changed<Children>)>,
    uninitialized_stationary: Query<(), (With<Stationary>, Without<StationaryInitialized>)>,
) -> bool {
    grids
        .iter()
        .any(|grid| !grid.local_floating_origin().is_local_origin_unchanged())
        || !changed_spatial.is_empty()
        || !changed_grid_children.is_empty()
        || !uninitialized_stationary.is_empty()
}

/// Open BigSpace's local-floating-origin walk only when its reference-frame
/// inputs can change.
fn local_origin_propagation_due(
    changed_origin_cell: Query<(), (With<FloatingOrigin>, Changed<CellCoord>)>,
    changed_hierarchy: Query<(), Or<(Changed<ChildOf>, Changed<Children>)>>,
    added_origin: Query<(), Added<FloatingOrigin>>,
    added_grid: Query<(), Added<Grid>>,
    added_big_space: Query<(), Added<BigSpace>>,
) -> bool {
    !changed_origin_cell.is_empty()
        || !changed_hierarchy.is_empty()
        || !added_origin.is_empty()
        || !added_grid.is_empty()
        || !added_big_space.is_empty()
}

/// Open BigSpace's low-precision walk only when a local transform hierarchy or
/// an upstream high-precision root changed.
fn low_precision_propagation_due(
    changed_transforms: Query<(), Or<(Changed<Transform>, Added<Transform>)>>,
    changed_hierarchy: Query<(), Or<(Changed<ChildOf>, Changed<Children>)>>,
    // This is deliberately the same root predicate BigSpace uses for its low
    // precision walk. A descendant `GlobalTransform` is an OUTPUT of that walk;
    // treating it as an input makes the application reopen the walk because of
    // BigSpace's own previous-frame writes.
    changed_global_roots: Query<
        (),
        (
            Or<(With<Grid>, With<CellCoord>)>,
            Or<(Changed<GlobalTransform>, Added<GlobalTransform>)>,
        ),
    >,
    mut removed_transforms: RemovedComponents<Transform>,
    mut removed_hierarchy: RemovedComponents<ChildOf>,
    mut removed_global_transforms: RemovedComponents<GlobalTransform>,
) -> bool {
    !changed_transforms.is_empty()
        || !changed_hierarchy.is_empty()
        || !changed_global_roots.is_empty()
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
