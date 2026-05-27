//! A standalone sandbox for rapid testing of ground mobility and physics.
//!
//! Loads the entire scene from USD **synchronously** during Startup,
//! so all entities (rover chassis + wheels) exist before physics runs.
//! This matches the original sandbox behavior exactly.

// glibc's allocator serialises cross-thread allocations through a
// shared arena lock; with avian's contact graph allocating heavily on
// a parallel task pool every fixed tick, the main render thread paid
// a tail-latency penalty on every alloc. mimalloc uses per-thread
// heaps and a lock-free fast path, removing the contention. Native
// only — wasm has its own allocator pipeline.
#[cfg(not(target_arch = "wasm32"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use bevy::prelude::*;
use bevy::asset::{AssetPlugin, io::AssetSourceBuilder};
use lunco_assets::cache_dir;
use bevy::pbr::wireframe::WireframePlugin;
use big_space::prelude::*;
use avian3d::prelude::PhysicsPlugins;
use leafwing_input_manager::prelude::*;

use lunco_mobility::LunCoMobilityPlugin;
use lunco_hardware::LunCoHardwarePlugin;
use lunco_usd::{ui::{UsdUiPlugin, UsdViewportPlugin}, UsdPlugins, UsdPrimPath};
use lunco_terrain::TerrainPlugin;
use lunco_sandbox_edit::SandboxEditPlugin;
use lunco_controller::LunCoControllerPlugin;
use lunco_avatar::{LunCoAvatarPlugin, IntentAnalogState, FreeFlightCamera, AdaptiveNearPlane};
use lunco_celestial::GravityPlugin;
use lunco_environment::EnvironmentPlugin;
use lunco_core::Avatar;
use lunco_cosim::CoSimPlugin;
use lunco_cosim::systems::propagate::CosimSet as PropagateCosimSet;
use lunco_cosim::systems::apply_forces::CosimSet as ApplyForcesCosimSet;
use lunco_modelica::{ModelicaCorePlugin, ModelicaSet};
use big_space::prelude::Grid;
use lunco_materials::{BlueprintMaterialPlugin, SolarPanelMaterialPlugin};

#[path = "../code_panel.rs"]
mod code_panel;
#[path = "../models_palette.rs"]
mod models_palette;

/// Parse API port from CLI args.
/// 
/// Supports:
fn main() {
    // Match lunica's pattern: scan argv for `--api <port>`
    // so the window title can advertise the listening port. Saves
    // confusion when several instances run side-by-side.
    //
    // Also parse `--no-vsync`. By default the window uses Bevy's
    // `PresentMode::Fifo` (VSync on), which caps FPS to the display
    // refresh — typically 60 Hz on laptops. Useful for measuring real
    // CPU/GPU headroom without the display cap.
    let args: Vec<String> = std::env::args().collect();
    let api_port: Option<u16> = {
        let mut port = None;
        for i in 0..args.len() {
            if args[i] == "--api" {
                port = Some(3000);
                if i + 1 < args.len() {
                    if let Ok(p) = args[i + 1].parse::<u16>() {
                        port = Some(p);
                    }
                }
                break;
            }
        }
        port
    };
    let no_vsync = args.iter().any(|a| a == "--no-vsync");
    // `--scene <path>` overrides the default sandbox_scene.usda load.
    // Path is relative to the asset source root (`assets/`). Used by
    // automated joint/physics tests that need an isolated minimal
    // scene rather than the full sandbox.
    let scene_path: String = {
        let mut s = "scenes/sandbox/sandbox_scene.usda".to_string();
        for i in 0..args.len() {
            if args[i] == "--scene" && i + 1 < args.len() {
                s = args[i + 1].clone();
                break;
            }
        }
        s
    };
    // `--log-diag` toggles Bevy's `LogDiagnosticsPlugin`, which prints
    // FPS / FrameTime / EntityCount and — when `bevy_diagnostic` is on
    // for avian — the physics step time, every second. Off by default
    // because the lines are noisy; flip it on while hunting perf.
    let log_diag = args.iter().any(|a| a == "--log-diag");
    let present_mode = if no_vsync {
        bevy::window::PresentMode::Mailbox
    } else {
        bevy::window::PresentMode::Fifo
    };
    let window_title = match api_port {
        Some(p) => format!("sandbox — Listening on {p}"),
        None => "sandbox".to_string(),
    };

    let mut app = App::new();
    // Cap how much catchup `FixedUpdate` does after a slow frame.
    // Default Bevy behaviour: if a frame took 50ms, the next frame
    // runs 3 fixed ticks (16.67ms each) to catch up — *which makes
    // that frame slow too*, breeding the next slow frame. The cap
    // lives on `Time<Virtual>` (clamps how much delta accumulates
    // per real-time tick); `Time<Fixed>` reads delta from Virtual,
    // so capping Virtual transitively caps fixed catchup. 33ms ≈ 2
    // fixed ticks — residual real time is *dropped* instead of
    // compounded, breaking the jitter cascade.
    let mut virtual_time = Time::<Virtual>::default();
    virtual_time.set_max_delta(std::time::Duration::from_millis(33));
    app.insert_resource(ScenePath(scene_path))
        .insert_resource(virtual_time)
        // `lunco-lib://` shipped-fixture asset source — must be
        // registered *before* `DefaultPlugins`/`AssetPlugin` builds the
        // server. Mirrors the registration in `lunco-client`'s main
        // binary; without it, `def Cube` placeholders with
        // `payload = @lunco-lib://...@` only render their Cube fallback.
        .register_asset_source(
            "lunco-lib",
            AssetSourceBuilder::platform_default(&cache_dir().to_string_lossy(), None),
        )
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(lunco_core::TimeWarpState { physics_enabled: true, ..default() })
        .insert_resource(avian3d::prelude::Gravity::ZERO)
        .insert_resource(lunco_celestial::Gravity::flat(9.81, bevy::math::DVec3::NEG_Y))
        .add_plugins(DefaultPlugins
            .set(AssetPlugin {
                file_path: std::env::current_dir().unwrap_or_default().join("assets").to_string_lossy().to_string(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    #[cfg(not(target_arch = "wasm32"))]
                    resolution: bevy::window::WindowResolution::new(1600, 1000),
                    #[cfg(not(target_arch = "wasm32"))]
                    position: WindowPosition::Centered(MonitorSelection::Primary),
                    // On wasm, attach to the `#bevy` canvas in index.html and
                    // mirror its parent's CSS size to the bevy window. Without
                    // this winit never sees DOM resize events and the canvas
                    // stays at its default 1x1 logical size — menu bars and
                    // panels render outside the visible area.
                    #[cfg(target_arch = "wasm32")]
                    canvas: Some("#bevy".to_string()),
                    #[cfg(target_arch = "wasm32")]
                    fit_canvas_to_parent: true,
                    present_mode,
                    ..lunco_workbench::merged_titlebar_window(window_title)
                }),
                ..default()
            })
            .set(bevy::log::LogPlugin {
                // Quieten third-party noise: rumoca's JIT + diffsol's solver
                // both emit per-function and per-step info that floods the
                // log during balloon stepping. Override via RUST_LOG when
                // diagnosing one of them.
                filter: "wgpu=error,naga=warn,cranelift=warn,cranelift_jit=warn,cranelift_codegen=warn,diffsol=warn,info".into(),
                ..default()
            })
            .build()
            .disable::<TransformPlugin>())
        .add_plugins({
            // big_space only registers `BigSpaceValidationPlugin` when
            // `debug_assertions` is on (or the `debug` feature). Calling
            // `.disable::<...>()` panics in release because the plugin
            // isn't in the group, so we gate the disable on the same cfg.
            let group = BigSpaceDefaultPlugins.build();
            #[cfg(debug_assertions)]
            let group = group.disable::<big_space::validation::BigSpaceValidationPlugin>();
            group
        })
        .add_plugins(WireframePlugin::default())
        // EntityCount is cheap and useful any time we look at perf; add
        // unconditionally. LogDiagnosticsPlugin is loud — it prints a
        // multi-line summary every second — so gate it on `--log-diag`.
        .add_plugins(bevy::diagnostic::EntityCountDiagnosticsPlugin::default())
        .add_plugins(PhysicsPlugins::default().set(avian3d::prelude::PhysicsInterpolationPlugin::interpolate_all()))
        .add_plugins(CoSimPlugin)
        .add_plugins(lunco_workbench::WorkbenchPlugin)
        .add_plugins(lunco_core::LunCoCorePlugin)
        .add_plugins(GravityPlugin)
        .add_plugins(EnvironmentPlugin)
        .add_plugins(TerrainPlugin)
        .add_plugins(LunCoHardwarePlugin)
        .add_plugins(LunCoMobilityPlugin)
        .add_plugins(UsdPlugins)
        // Phase 3+: surface USD documents in the Twin browser, plus
        // the singleton render-to-texture viewport so clicking a
        // `.usda` row in the file browser previews it in a workbench
        // tab (Blender-style orbit: left-drag rotates, scroll zooms).
        // The viewport draws to its own offscreen `Image`, so the
        // primary 3D scene camera is unaffected.
        .add_plugins(UsdUiPlugin)
        .add_plugins(UsdViewportPlugin)
        .add_plugins(SandboxEditPlugin)
        .add_plugins(lunco_sandbox_edit::ui::SandboxEditUiPlugin)
        .add_plugins(LunCoControllerPlugin)
        .add_plugins(LunCoAvatarPlugin)
        .add_plugins(BlueprintMaterialPlugin)
        .add_plugins(SolarPanelMaterialPlugin)
        .add_plugins(lunco_scripting::LunCoScriptingPlugin)
        // Rover-specific panels and the attach-a-model click flow.
        .add_plugins(|app: &mut App| {
            use lunco_workbench::WorkbenchAppExt;
            app.register_panel(code_panel::CodePanel);
            app.register_panel(models_palette::ModelsPalette);
            app.init_resource::<models_palette::AttachState>();
            // Attach click runs BEFORE shift-click selection so a ball
            // click in attach-mode lands as an attach, not a select.
            app.add_systems(
                Update,
                models_palette::handle_attach_click
                    .before(lunco_sandbox_edit::selection::handle_entity_selection),
            );
        })
        .init_resource::<SandboxSettings>()
        .add_systems(Startup, setup_sandbox)
        // ModelicaPlugin's AnalyzePerspective is registered before SandboxEditUiPlugin's
        // workspaces, so without this nudge we'd boot into the Modelica layout.
        // Activate the 3D-only View workspace by default — full-screen scene,
        // no panels. User can switch to Build via the workspace tabs.
        .add_systems(Startup, |mut layout: ResMut<lunco_workbench::WorkbenchLayout>| {
            layout.activate_perspective(lunco_workbench::PerspectiveId("sandbox_view"));
        })
        .add_systems(Update, apply_sandbox_settings)
        // Cosim pipeline ordering inside FixedUpdate:
        //   HandleResponses → Propagate → ApplyForces → SpawnRequests.
        // All USD-driven cosim wiring (compile dispatch, SimComponent
        // setup, per-tick sync) is registered by
        // `lunco_usd_sim::cosim::install` from `UsdPlugins`.
        .configure_sets(FixedUpdate, (
            ModelicaSet::HandleResponses,
            PropagateCosimSet::Propagate,
            ApplyForcesCosimSet::ApplyForces,
            ModelicaSet::SpawnRequests,
        ).chain())
        // Selection must run before avatar possession so DragModeActive flag is set
        .add_systems(Update, lunco_sandbox_edit::selection::handle_entity_selection.before(lunco_avatar::avatar_raycast_possession))
        // Manual transform/visibility propagation runs ONCE per frame
        // after physics writeback. The earlier triple-call (PreUpdate
        // + two in PostUpdate) ate ~80% of frame time on sandbox
        // because each call rebuilds a HashMap of every spatial entity
        // four times. Big_space's own BigSpacePropagationPlugin handles
        // the CellCoord-rooted hierarchy; this fallback exists only to
        // cover USD-spawned children that lack CellCoord.
        .add_systems(PostUpdate, (
            global_transform_propagation_system,
            spawn_fallback_avatar,
        ).chain().after(avian3d::prelude::PhysicsSystems::Writeback));

    // ModelicaCorePlugin owns `ModelicaChannels` and `ModelicaSet` system
    // sets that many sandbox systems hard-depend on. On wasm we suppress
    // the MSL auto-fetch (no manifest shipped with sandbox_web — sandbox
    // cosim doesn't compile against Modelica.Library classes).
    #[cfg(target_arch = "wasm32")]
    app.insert_resource(lunco_modelica::msl_remote::SkipMslAutoLoad);
    app.add_plugins(ModelicaCorePlugin);

    #[cfg(feature = "lunco-api")]
    app.add_plugins(lunco_api::LunCoApiPlugin::default());

    if log_diag {
        app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
    }

    app.run();
}

#[derive(Resource, Reflect)]
struct SandboxSettings {
    sun_yaw: f32,
    sun_pitch: f32,
    ambient_brightness: f32,
    ambient_color: LinearRgba,
    wireframe: bool,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            sun_yaw: 0.5,
            sun_pitch: -0.8,
            ambient_brightness: 400.0,
            ambient_color: LinearRgba::WHITE,
            wireframe: false,
        }
    }
}

/// Resource that holds the asset-source-relative path of the scene
/// to load on Startup. Initialised from the `--scene` CLI arg.
#[derive(Resource)]
struct ScenePath(String);

fn setup_sandbox(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    scene_path: Res<ScenePath>,
) {
    let scene_path: String = scene_path.0.clone();
    let big_space_root = commands.spawn(BigSpace::default()).id();
    let grid = commands.spawn((
        Grid::new(2000.0, 1.0e10),
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Sandbox_Grid"),
    )).set_parent_in_place(big_space_root).id();

    // --- Sun (directional light) ---
    commands.spawn((
        DirectionalLight {
            illuminance: 10000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(10.0, 20.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
        GlobalTransform::default(),
        CellCoord::default(),
        Name::new("Sun"),
    )).set_parent_in_place(grid);

    // --- Load scene from USD (ground + ramp + ALL rovers) ---
    // The scene file references rover definitions from external .usda files
    // with position overrides. The UsdComposer flattens everything into
    // a single stage, then sync_usd_visuals spawns entities for all prims.
    let scene_handle = asset_server.load(scene_path.as_str().to_string());
    info!("Loading sandbox scene from USD");
    commands.spawn((
        Name::new("SandboxScene"),
        UsdPrimPath {
            stage_handle: scene_handle,
            path: "/SandboxScene".to_string(),
        },
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
        Transform::default(),
        CellCoord::default(),
    )).set_parent_in_place(grid);

    // Balloons live in `sandbox_scene.usda` now (Red/GreenBalloon prims
    // reference `vessels/balloons/{modelica,python}_balloon.usda`).
    // The cosim translator reads `lunco:modelicaModel` / `lunco:scriptModel`
    // and `lunco:simWires` to wire up Modelica/Python and SimConnections.
}

/// Spawns a default avatar if no USD-defined Avatar was loaded.
///
/// This acts as a fallback when the scene file doesn't contain an Avatar
/// prim, ensuring the user always has a controllable camera.
///
/// USD asset loading is async — checking for `Camera3d` on frame 1 is too
/// eager and would spawn a fallback even when the scene *will* provide
/// one a few frames later, leaving the world with two cameras + two
/// `FloatingOrigin`s (which big_space resets every frame, killing perf
/// and breaking propagation). Wait a grace period; if the scene didn't
/// publish a camera by then, spawn the fallback exactly once.
const FALLBACK_AVATAR_GRACE_SECS: f32 = 2.0;

fn spawn_fallback_avatar(
    time: Res<Time>,
    q_cameras: Query<Entity, With<Camera3d>>,
    q_grids: Query<Entity, With<Grid>>,
    mut commands: Commands,
    mut done: Local<bool>,
) {
    if *done { return; }
    // A USD-spawned camera ends the wait immediately.
    if q_cameras.iter().next().is_some() {
        *done = true;
        return;
    }
    // Otherwise let USD have its grace window before we step in.
    if time.elapsed_secs() < FALLBACK_AVATAR_GRACE_SECS {
        return;
    }
    let Some(grid) = q_grids.iter().next() else { return; };

    info!("No USD camera after {FALLBACK_AVATAR_GRACE_SECS}s, spawning fallback FreeFlightCamera");
    commands.spawn((
        Camera3d::default(),
        FreeFlightCamera {
            yaw: -2.245559,
            pitch: -0.303039,
            damping: None,
        },
        AdaptiveNearPlane,
        Transform::from_translation(Vec3::new(-30.0, 15.0, -20.0)),
        GlobalTransform::default(),
        FloatingOrigin,
        CellCoord::default(),
        Avatar,
        IntentAnalogState::default(),
        ActionState::<lunco_core::UserIntent>::default(),
        lunco_controller::get_avatar_input_map(),
        ChildOf(grid),
    ));
    *done = true;
}


fn apply_sandbox_settings(
    settings: Res<SandboxSettings>,
    mut q_sun: Query<&mut Transform, With<DirectionalLight>>,
    mut q_ambient: Query<&mut AmbientLight>,
) {
    if settings.is_changed() {
        for mut tf in q_sun.iter_mut() {
            tf.rotation = Quat::from_euler(EulerRot::YXZ, settings.sun_yaw, settings.sun_pitch, 0.0);
        }
        for mut ambient in q_ambient.iter_mut() {
            ambient.brightness = settings.ambient_brightness;
            ambient.color = Color::Srgba(settings.ambient_color.into());
        }
    }
}

fn global_transform_propagation_system(
    mut commands: Commands,
    // Only newly-spawned entities need the missing-component patch.
    // Without `Added<Transform>` this query iterates the entire scene
    // every frame even though almost all entities already have the
    // components inserted on a previous tick.
    q_needs: Query<Entity, (Added<Transform>, Or<(With<Visibility>, With<Mesh3d>, With<Text2d>, With<Transform>)>, Without<InheritedVisibility>, Without<CellCoord>)>,
    mut q_spatial: Query<(Entity, &mut GlobalTransform, &Transform, Option<&ChildOf>)>,
    mut q_visibility: Query<(Entity, &mut InheritedVisibility, &mut ViewVisibility, &Visibility, Option<&ChildOf>)>,
) {
    for ent in q_needs.iter() {
        commands.entity(ent).insert((InheritedVisibility::default(), ViewVisibility::default(), GlobalTransform::default()));
    }
    for _ in 0..4 {
        let mut gtf_cache = std::collections::HashMap::new();
        for (ent, gtf, _, _) in q_spatial.iter() { gtf_cache.insert(ent, *gtf); }
        for (_ent, mut gtf, local_tf, child_of_opt) in q_spatial.iter_mut() {
            let parent_gtf = if let Some(child_of) = child_of_opt { gtf_cache.get(&child_of.parent()).cloned().unwrap_or_default() } else { GlobalTransform::default() };
            *gtf = parent_gtf.mul_transform(*local_tf);
        }
    }
    for _ in 0..4 {
        let mut vis_cache = std::collections::HashMap::new();
        for (ent, inherited, _, _, _) in q_visibility.iter() { vis_cache.insert(ent, inherited.get()); }
        for (_, mut inherited, _view, visibility, child_of_opt) in q_visibility.iter_mut() {
            // If entity is explicitly Visible, it's always visible regardless of parent
            if *visibility == Visibility::Visible {
                *inherited = InheritedVisibility::VISIBLE;
                continue;
            }
            // If entity is explicitly Hidden, it's always hidden
            if *visibility == Visibility::Hidden {
                *inherited = InheritedVisibility::HIDDEN;
                continue;
            }
            // Otherwise inherit from parent
            let parent_visible = if let Some(child_of) = child_of_opt { *vis_cache.get(&child_of.parent()).unwrap_or(&true) } else { true };
            *inherited = if parent_visible { InheritedVisibility::VISIBLE } else { InheritedVisibility::HIDDEN };
        }
    }
}
