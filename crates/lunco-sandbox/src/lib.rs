//! The LunCo sandbox application — ground mobility + physics, loaded from USD.
//!
//! [`run`] builds and runs the app. It is the single shared entry point for BOTH
//! binaries:
//!   - `sandbox` (this crate, default `ui` feature) — the windowed GUI;
//!   - `sandbox-server` (the `lunco-sandbox-server` crate, no `ui`) — headless.
//!
//! ## Architecture: composition root, not a UI host
//!
//! The app is three named plugins, composed by a tiny shell — mirroring how the
//! library crates split into core modules + a `*UiPlugin`:
//!   - [`SandboxCorePlugin`] — sim / physics / cosim / USD / networking / API.
//!     Headless-safe, added unconditionally.
//!   - [`ui::SandboxUiPlugin`] (`ui` feature) — egui workbench, picking, the
//!     in-scene editor, materials, panels, fallback camera. Added only when
//!     running windowed.
//!   - [`SandboxHeadlessPlugin`] — the `ScheduleRunner` + the Modelica/spawn
//!     cores a server needs in the UI plugin's place. Added only when headless.
//!
//! GUI = `SandboxCorePlugin + SandboxUiPlugin`; headless =
//! `SandboxCorePlugin + SandboxHeadlessPlugin`. Both bins compose the SAME
//! `SandboxCorePlugin`, so they can never drift. The only place the GUI/headless
//! decision touches plugin *configuration* is [`default_plugins`] (the window /
//! render / winit backend must be chosen at `PluginGroup` build time) — that is
//! inherently a shell concern.

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
use bevy::asset::{AssetMetaCheck, AssetPlugin};
use big_space::prelude::*;
use avian3d::prelude::PhysicsPlugins;

use lunco_mobility::LunCoMobilityPlugin;
use lunco_hardware::LunCoHardwarePlugin;
// USD core (scene load + collider build) is always needed; the Twin browser /
// RTT viewport UI plugins are `ui`-only (added by `SandboxUiPlugin`).
use lunco_usd::{LoadScene, UsdPlugins};
use lunco_terrain::TerrainPlugin;
use lunco_controller::LunCoControllerPlugin;
use lunco_avatar::LunCoAvatarPlugin;
use lunco_celestial::GravityPlugin;
use lunco_environment::EnvironmentPlugin;
use lunco_cosim::CoSimPlugin;
use lunco_cosim::systems::propagate::CosimSet as PropagateCosimSet;
use lunco_cosim::systems::apply_forces::CosimSet as ApplyForcesCosimSet;
// `ModelicaSet` orders the cosim pipeline (always). The egui workbench plugin is
// added by `SandboxUiPlugin`; headless adds `ModelicaCorePlugin` instead.
use lunco_modelica::ModelicaSet;

#[cfg(feature = "ui")]
mod ui;

/// Run the sandbox, choosing GUI vs. headless from the build + flags: headless
/// when the `ui` feature is absent, or `--no-ui` / `LUNCO_NO_UI` is set;
/// otherwise the windowed GUI. This is the `sandbox` (GUI) bin's entry point.
pub fn run() {
    let headless = !cfg!(feature = "ui")
        || std::env::args().any(|a| a == "--no-ui")
        || std::env::var("LUNCO_NO_UI").is_ok_and(|v| v != "0" && !v.is_empty());
    run_with_mode(headless);
}

/// Run the sandbox HEADLESS, unconditionally — the `sandbox-server` bin's entry
/// point. Forcing the mode here (rather than inferring it from the absent `ui`
/// feature) makes the server stay windowless **even if `ui` gets unified on** by
/// a `cargo build --workspace` (which compiles the GUI `sandbox` bin alongside
/// it). So the server never tries to open a window; in a lean `-p
/// lunco-sandbox-server` build the GUI stack isn't linked at all.
pub fn run_headless() {
    run_with_mode(true);
}

/// Composition root. Builds the shared core, then conditionally layers on the UI
/// or the headless runner. Nothing UI-specific lives here beyond selecting the
/// windowing backend in [`default_plugins`].
fn run_with_mode(headless: bool) {
    let mut app = App::new();

    // Register every LunCo asset source (lunco://, lunco-lib://, twin://,
    // cached_textures://) + the shared `TwinRoots` resource in ONE shared place
    // (`lunco-assets`), so all binaries get identical schemes. MUST run before
    // `DefaultPlugins`/`AssetPlugin` snapshots the source registry.
    lunco_assets::register_lunco_asset_sources(&mut app);

    app.add_plugins(default_plugins(headless));
    app.add_plugins(SandboxCorePlugin { headless });

    #[cfg(feature = "ui")]
    if !headless {
        app.add_plugins(ui::SandboxUiPlugin);
    }

    if headless {
        app.add_plugins(SandboxHeadlessPlugin);
    }

    app.run();
}

/// Build the base [`DefaultPlugins`] group for the chosen mode.
///
/// This is the one place the GUI/headless split touches plugin *configuration*,
/// because the render backend and the window must be decided at `PluginGroup`
/// build time — a plugin added later cannot reconfigure `RenderPlugin`/
/// `WindowPlugin`. Headless (and every `--no-ui`-feature build) uses `backends:
/// None`: the render world + asset stores initialise (so USD visual sync can
/// populate the meshes avian colliders read), but no GPU device is created and
/// nothing is drawn — `ScheduleRunnerPlugin` (added by [`SandboxHeadlessPlugin`])
/// ticks the app in winit's place.
fn default_plugins(headless: bool) -> bevy::app::PluginGroupBuilder {
    use bevy::render::settings::WgpuSettings;
    // `headless` only selects render/window config in `ui` builds; a no-`ui`
    // build is always windowless, so the param is unused there.
    #[cfg(not(feature = "ui"))]
    let _ = headless;

    // Window title (advertises the `--api` port so side-by-side instances are
    // distinguishable) + present mode are windowed-only and must be known at
    // window-build time, so they're computed here rather than in the UI plugin.
    #[cfg(feature = "ui")]
    let (window_title, present_mode) = {
        let args: Vec<String> = std::env::args().collect();
        let no_vsync = args.iter().any(|a| a == "--no-vsync");
        // Networked side-by-side windows: one is ALWAYS unfocused, and an
        // unfocused window under `Fifo` (vsync) can block on present when the
        // compositor stops servicing it — which stalls the WHOLE update loop
        // (sim + netcode + the 20 Hz snapshot send), not just rendering. Use
        // non-blocking `Mailbox` while networked so the background window keeps
        // ticking at full rate.
        let networked = args.iter().any(|a| a == "--host" || a == "--connect");
        let mut api_port: Option<u16> = None;
        for i in 0..args.len() {
            if args[i] == "--api" {
                api_port = Some(lunco_core::session::DEFAULT_API_PORT);
                if i + 1 < args.len() {
                    if let Ok(p) = args[i + 1].parse::<u16>() {
                        api_port = Some(p);
                    }
                }
                break;
            }
        }
        let title = match api_port {
            Some(p) => format!("sandbox — Listening on {p}"),
            None => "sandbox".to_string(),
        };
        let present = if no_vsync || networked {
            bevy::window::PresentMode::Mailbox
        } else {
            bevy::window::PresentMode::Fifo
        };
        (title, present)
    };

    #[cfg(feature = "ui")]
    let render_creation = if headless {
        WgpuSettings { backends: None, ..default() }.into()
    } else {
        lunco_workbench::preferred_wgpu_settings().into()
    };
    #[cfg(not(feature = "ui"))]
    let render_creation = WgpuSettings { backends: None, ..default() }.into();

    let group = DefaultPlugins
        .set(AssetPlugin {
            file_path: std::env::current_dir().unwrap_or_default().join("assets").to_string_lossy().to_string(),
            // Don't probe for `.meta` sidecars: we ship none, so every asset
            // load would otherwise fire a failed `<asset>.meta` fetch.
            meta_check: AssetMetaCheck::Never,
            ..default()
        })
        .set(bevy::log::LogPlugin {
            // Quieten third-party noise (rumoca JIT + diffsol per-step).
            filter: "wgpu=error,naga=warn,cranelift=warn,cranelift_jit=warn,cranelift_codegen=warn,diffsol=warn,info".into(),
            ..default()
        })
        .set(bevy::render::RenderPlugin { render_creation, ..default() });

    // Window/winit setup. With the `ui` feature the runtime `headless` flag still
    // picks the windowless variant (no primary window, WinitPlugin disabled).
    // Without `ui` there's no winit crate to disable, so just declare a
    // windowless `WindowPlugin`.
    #[cfg(feature = "ui")]
    let group = if headless {
        group
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                close_when_requested: false,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>()
    } else {
        group.set(WindowPlugin {
            primary_window: Some(Window {
                // On wasm, attach to the `#bevy` canvas and mirror its CSS size.
                #[cfg(target_arch = "wasm32")]
                canvas: Some("#bevy".to_string()),
                #[cfg(target_arch = "wasm32")]
                fit_canvas_to_parent: true,
                present_mode,
                // Centralized merged-titlebar chrome + persisted geometry.
                ..lunco_workbench::restored_window(window_title)
            }),
            ..default()
        })
    };
    #[cfg(not(feature = "ui"))]
    let group = group.set(WindowPlugin {
        primary_window: None,
        exit_condition: bevy::window::ExitCondition::DontExit,
        close_when_requested: false,
        ..default()
    });

    group.build().disable::<TransformPlugin>()
}

/// The shared, headless-safe core: the persistent world shell, physics, cosim,
/// USD scene load, mobility/hardware/controller/avatar, environment, the HTTP
/// API, and networking. Added unconditionally by both the GUI and the server, so
/// the two binaries can never drift.
///
/// The render plugins are configured in [`default_plugins`] (added before this);
/// here every plugin is pure-CPU sim/state. USD visual sync only writes the
/// mesh/material asset stores (never touches a GPU device), so it's safe under
/// `backends: None`.
pub struct SandboxCorePlugin {
    pub headless: bool,
}

impl Plugin for SandboxCorePlugin {
    fn build(&self, app: &mut App) {
        let args: Vec<String> = std::env::args().collect();

        // `--scene <path>` overrides the default sandbox_scene.usda load. Path is
        // relative to the asset source root (`assets/`). Used by automated joint/
        // physics tests that need an isolated minimal scene.
        let scene_path = {
            let mut s = "scenes/sandbox/sandbox_scene.usda".to_string();
            for i in 0..args.len() {
                if args[i] == "--scene" && i + 1 < args.len() {
                    s = args[i + 1].clone();
                    break;
                }
            }
            s
        };

        // Cap how much catchup `FixedUpdate` does after a slow frame. Default
        // Bevy behaviour: a 50ms frame breeds 3 catch-up fixed ticks next frame,
        // making that frame slow too — a self-feeding jitter cascade. The cap
        // lives on `Time<Virtual>`; `Time<Fixed>` reads its delta from Virtual,
        // so capping Virtual transitively caps fixed catchup. 33ms ≈ 2 ticks —
        // residual real time is dropped instead of compounded.
        let mut virtual_time = Time::<Virtual>::default();
        virtual_time.set_max_delta(std::time::Duration::from_millis(33));

        app.insert_resource(ScenePath(scene_path))
            .insert_resource(virtual_time)
            // Match the workbench theme's backdrop so the window's first-frame
            // clear lines up with egui's panel fill (no "left hairline" at panel
            // boundaries under non-integer DPRs). Harmless headless.
            .insert_resource(ClearColor(Color::srgb_u8(0x1a, 0x1a, 0x1a)))
            .insert_resource(Time::<Fixed>::from_hz(lunco_core::FIXED_HZ))
            // `speed: 1.0` is load-bearing: `..default()` leaves it 0.0, which
            // keeps physics running but FREEZES `SimTick` — breaking every
            // tick-keyed netcode path. Match the rover examples: both explicit.
            .insert_resource(lunco_core::TimeWarpState { physics_enabled: true, speed: 1.0 })
            .insert_resource(avian3d::prelude::Gravity::ZERO)
            .insert_resource(lunco_environment::Gravity::flat(9.81, bevy::math::DVec3::NEG_Y))
            // Studio lighting for the sandbox — a generic editor scene, NOT a
            // calibrated lunar surface (the canonical 128 klx / EV15 `LunarSun`
            // crushes the dark blueprint ground to black). Inserted BEFORE
            // `EnvironmentPlugin` so its `init_resource` keeps these. The sun
            // spawn AND every camera's exposure read this one resource, so lux
            // and EV stay matched. Tunable live via `SetEnvironmentLight`.
            .insert_resource(lunco_environment::LunarSun {
                illuminance_lux: 10_000.0,
                exposure_ev100: 9.7,
                ..Default::default()
            })
            // Persistent world shell: one BigSpace root + `WorldGrid` + one
            // `FloatingOrigin`. big_space only registers its validation plugin
            // under `debug_assertions`, so the `.disable()` is gated the same way
            // (calling it in release would panic — the plugin isn't in the group).
            .add_plugins({
                let group = BigSpaceDefaultPlugins.build();
                #[cfg(debug_assertions)]
                let group = group.disable::<big_space::validation::BigSpaceValidationPlugin>();
                group
            })
            // EntityCount is cheap and useful any time we look at perf.
            .add_plugins(bevy::diagnostic::EntityCountDiagnosticsPlugin::default())
            .add_plugins(PhysicsPlugins::default().set(avian3d::prelude::PhysicsInterpolationPlugin::interpolate_all()))
            // 12 solver substeps (avian default 6): joint-based rovers buzz the
            // chassis under drive torque at 6 substeps. Quantified in the headless
            // `rover_jitter` probe. See `project_physical_rover_suspension`.
            .insert_resource(avian3d::prelude::SubstepCount(12))
            .add_plugins(CoSimPlugin)
            .add_plugins(lunco_core::LunCoCorePlugin)
            .add_plugins(lunco_core::WorldShellPlugin)
            .add_plugins(GravityPlugin)
            .add_plugins(EnvironmentPlugin)
            .add_plugins(TerrainPlugin)
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
            .add_plugins(lunco_scripting::LunCoScriptingPlugin)
            // Default scene-wide fill for scenes that author no lighting; a
            // scene-authored UsdLux light takes ambient over.
            .insert_resource(bevy::light::GlobalAmbientLight {
                brightness: 40.0,
                ..Default::default()
            })
            .add_systems(Startup, setup_sandbox)
            // Cosim pipeline ordering inside FixedUpdate:
            //   HandleResponses → Propagate → ApplyForces → SpawnRequests.
            .configure_sets(FixedUpdate, (
                ModelicaSet::HandleResponses,
                PropagateCosimSet::Propagate,
                ApplyForcesCosimSet::ApplyForces,
                ModelicaSet::SpawnRequests,
            ).chain());

        // Dismiss the HTML loading screen once the first frame paints (wasm-only;
        // no-op on native). Pairs with `web/index.html` → `lunco-boot.js`.
        app.add_plugins(lunco_web::WebReadyPlugin);

        // HTTP automation bridge — native `--api` server / wasm JS bridge. Linked
        // in the GUI and the headless compile server alike.
        #[cfg(feature = "lunco-api")]
        app.add_plugins(lunco_api::LunCoApiPlugin::default());

        // Multiplayer. Native: `--host [port]` / `--connect <addr>`; browser:
        // `?connect=host`. With no address the plugin still loads client-capable
        // but idle (single-player) so the in-sim *Connect* button / `JoinServer`
        // command can dial a server at runtime.
        #[cfg(feature = "networking")]
        {
            let mode = lunco_networking::NetworkMode::resolve(self.headless);
            info!("[net] networking mode: {mode:?}");
            app.add_plugins(lunco_networking::LunCoNetworkingPlugin { mode });
            // Connect-menu bridge adapter (seeds connect_hint, re-dispatches the
            // NetConnect/Disconnect bridge events). Observers + a Startup seed
            // only — no egui — so it's safe headless; keep it so the host still
            // answers runtime JoinServer/LeaveServer.
            app.add_plugins(lunco_networking::ui::LunCoNetworkingUiPlugin);
        }

        // LogDiagnosticsPlugin is loud (a multi-line summary every second) — gate
        // it on `--log-diag`.
        if args.iter().any(|a| a == "--log-diag") {
            app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
        }
    }
}

/// The headless runner: the Modelica/spawn cores a windowed build gets
/// transitively from its UI plugins, plus the `ScheduleRunnerPlugin` that ticks
/// the app in winit's place. Added only when running headless.
pub struct SandboxHeadlessPlugin;

impl Plugin for SandboxHeadlessPlugin {
    fn build(&self, app: &mut App) {
        // Modelica COMPILE CORE only (channels + worker thread + `.mo` asset
        // loader + compile-dispatch systems) — NO egui/viz/workbench. Windowed
        // builds get this transitively via `ModelicaWorkbenchPlugin`; headless
        // must add it directly or the cosim `on_load_scene` observer panics on a
        // missing `Res<ModelicaChannels>`. The server runs Modelica cosim models
        // authoritatively, so it needs the real compile path, not a stub.
        app.add_plugins(lunco_modelica::ModelicaCorePlugin);

        // Spawn-command CORE (runtime spawn/move/property commands + the
        // `apply_net_replication` system that tags dynamic scene bodies with
        // `NetReplicate`). Windowed builds get this transitively via
        // `SandboxEditPlugin`; without it the headless host replicates NOTHING
        // (the connect baseline is empty) because nothing marks the rovers. The
        // gizmo/selection/physics-viz halves of `SandboxEditPlugin` stay UI-only.
        app.add_plugins(lunco_sandbox_edit::commands::SpawnCommandPlugin);

        // No winit event loop drives updates headless, so install a runner that
        // ticks the app at the sim's fixed rate. (Windowed builds are paced by
        // winit / vsync.)
        app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1.0 / lunco_core::FIXED_HZ as f64),
        ));

        info!("[net] sandbox running HEADLESS (--no-ui): no window/GPU/egui; sim + networking host only");
    }
}

/// Resource that holds the asset-source-relative path of the scene to load on
/// Startup. Initialised from the `--scene` CLI arg by [`SandboxCorePlugin`].
#[derive(Resource)]
struct ScenePath(String);

// `set_parent_in_place` is `disallowed_methods`-banned for its atomicity
// hazard (a `GridAnchor`/`RigidBody` parented after spawn can be mis-tagged
// `RigidBody::Static`). The two uses here parent the big_space root → Grid
// and a `DirectionalLight` → Grid — neither is a rigid body / GridAnchor, so
// that hazard doesn't apply. Locally allowed.
#[allow(clippy::disallowed_methods)]
fn setup_sandbox(world: &mut World) {
    let scene_path: String = world.resource::<ScenePath>().0.clone();

    // The persistent world shell (BigSpace root + `WorldGrid` + the single
    // `FloatingOrigin`) is owned by `WorldShellPlugin`. `ensure_world_root` is
    // create-or-get, so the Sun hangs off the canonical grid regardless of which
    // Startup system ran first.
    let grid = lunco_core::ensure_world_root(world);

    // --- Sun (directional light) on the world grid ---
    //
    // Real lunar shadows: hard-edged, jet-black, and long. Canonical lunar-sun
    // cascade split + 4096² atlas from the single source of truth
    // (`lunco_render::LunarSunShadow`), shared with the celestial and USD paths.
    // The biases are overridden for this binary's hard-shadow look: with
    // `Hardware2x2` filtering (see `force_hard_shadow_filtering`) the normal bias
    // must stay small or it detaches/softens the contact edge — unlike the
    // terrain-acne-tuned default (0.06/2.5) used under PCF.
    let sun = lunco_render::LunarSunShadow {
        depth_bias: 0.02,
        normal_bias: 0.8,
        ..Default::default()
    };
    // Illuminance + angular size from the active-scene `LunarSun` resource (every
    // camera's exposure reads the same resource, so sun lux and camera EV can't
    // drift apart).
    let ls = *world.resource::<lunco_environment::LunarSun>();
    world.insert_resource(sun.shadow_map());
    world.spawn((
        sun.directional_light(Color::WHITE, ls.illuminance_lux),
        sun.cascade_config(),
        lunco_core::SunAngularDiameter(ls.angular_diameter_deg),
        // Low sun (~11° above horizon, yaw 0.5 rad) for long raking lunar
        // shadows — same YXZ convention as `SetEnvironmentLight` and the
        // Inspector → Environment controls.
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.5, -0.2, 0.0)),
        GlobalTransform::default(),
        CellCoord::default(),
        Name::new("Sun"),
        // Default sun for scenes that author no lighting. A scene that authors a
        // UsdLux `DistantLight` (e.g. the moonbase Twin) replaces it: the loader
        // despawns every `FallbackSceneLight` and takes over ambient too.
        lunco_usd::FallbackSceneLight,
        ChildOf(grid),
    ));

    // --- Load scene from USD ---
    // Routed through the typed-command bus so startup and runtime (API/MCP
    // `LoadScene`) share one code path. Empty `root_prim` auto-derives
    // `/PascalCaseFromFilename`.
    //
    // An ABSOLUTE `--scene` path names an external Twin scene: register its
    // folder under the `twin://` source (keyed by the folder name) and load
    // through that source — stable, cross-platform identity. Relative paths load
    // from the default `assets/` source unchanged.
    let load_path = {
        let pb = std::path::PathBuf::from(&scene_path);
        match (
            pb.is_absolute(),
            pb.parent(),
            pb.parent().and_then(|p| p.file_name()),
            pb.file_name(),
        ) {
            (true, Some(parent), Some(key), Some(file)) => {
                let key = key.to_string_lossy().into_owned();
                world
                    .resource::<lunco_assets::twin_source::TwinRoots>()
                    .register(&key, parent);
                format!("twin://{}/{}", key, file.to_string_lossy())
            }
            _ => scene_path.clone(),
        }
    };
    info!("Loading sandbox scene `{}` via LoadScene", load_path);
    world.trigger(LoadScene {
        path: load_path,
        root_prim: String::new(),
    });
}
