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
use bevy::asset::{AssetMetaCheck, AssetPlugin};
// `bevy::camera::*` exists on both native and `--no-default-features`
// wasm; `bevy::render::camera::*` only when `bevy_render` is enabled.
use bevy::camera::RenderTarget;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use lunco_workbench::WorkbenchViewportCamera;
use bevy::pbr::wireframe::WireframePlugin;
use big_space::prelude::*;
use avian3d::prelude::PhysicsPlugins;
use leafwing_input_manager::prelude::*;

use lunco_mobility::LunCoMobilityPlugin;
use lunco_hardware::LunCoHardwarePlugin;
use lunco_usd::{ui::{UsdUiPlugin, UsdViewportPlugin}, LoadScene, UsdPlugins};
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
use lunco_modelica::{ModelicaWorkbenchPlugin, ModelicaSet, ModelicaUiConfig};
use big_space::prelude::Grid;
use lunco_materials::{BlueprintMaterialPlugin, ShaderMaterialPlugin};

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
    // `--no-throttle` forces the window to keep updating at full rate even when
    // unfocused, disabling the `reactive_low_power` background throttle (~1 FPS).
    // Single-player windows normally throttle when unfocused to keep fans quiet;
    // that masks render-loop behaviour during headless/automated tests (an
    // unfocused test window drops to 1 FPS, so motion can't be observed). This
    // flag keeps it Continuous regardless of focus. Networking already forces
    // Continuous (keepalive requirement), so this is a no-op there.
    let no_throttle = args.iter().any(|a| a == "--no-throttle");
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
    // `--window-pos <spec>` snaps the OS window to a screen region so a
    // host and a client instance can sit side by side without manual
    // dragging. Parsed + wired by `lunco_workbench::wire_window_placement`
    // below, after the plugins are added.
    //
    // Networking present? (`--host`/`--connect`). When networked, the window
    // must keep ticking even when unfocused: lightyear's netcode link sends
    // keepalives on the update loop, and the default unfocused throttle
    // (~1 FPS) starves them past the timeout, dropping the connection a few
    // seconds after the window loses focus. Two side-by-side windows means one
    // is always unfocused — so we keep it Continuous while networked.
    let networked = args.iter().any(|a| a == "--host" || a == "--connect");
    // `--no-ui` (or `LUNCO_NO_UI=1`) runs the sandbox HEADLESS: no OS window, no
    // winit, no GPU device, no egui/workbench chrome — just the server-authoritative
    // sim (USD scene + avian physics + cosim + networking host). For a headless
    // Ubuntu deploy of `sandbox.lunco.space`. The render plugins still load in
    // `backends: None` mode so the asset stores (`Assets<Mesh>`/`Assets<StandardMaterial>`)
    // exist — USD visual sync populates the meshes the avian colliders key off — but
    // nothing is ever drawn. `ScheduleRunnerPlugin` drives the loop in winit's place.
    let headless = args.iter().any(|a| a == "--no-ui")
        || std::env::var("LUNCO_NO_UI").is_ok_and(|v| v != "0" && !v.is_empty());
    // Present mode. Networked side-by-side windows: one is ALWAYS unfocused, and an
    // unfocused window under `Fifo` (vsync) can block on present when the compositor
    // stops servicing it — which stalls the WHOLE update loop (sim + netcode + the
    // 20 Hz snapshot send), not just rendering. That's the "fps collapses when not
    // in focus → clunky sync" symptom. `Continuous` update mode alone doesn't help
    // because the stall is in `present`, not the redraw request. Use non-blocking
    // `Mailbox` while networked so the background window keeps ticking at full rate.
    let present_mode = if no_vsync || networked {
        bevy::window::PresentMode::Mailbox
    } else {
        bevy::window::PresentMode::Fifo
    };
    let window_title = match api_port {
        Some(p) => format!("sandbox — Listening on {p}"),
        None => "sandbox".to_string(),
    };

    let mut app = App::new();
    // Match lunica's pacer: Continuous while focused lets vsync (Fifo
    // present / requestAnimationFrame on web) act as the frame timer;
    // ReactiveLowPower keeps fans quiet when the window is in the
    // background. Applies on wasm too — the original "UI vanishes on
    // zoom" bug surfaced under reactive mode where the egui chrome
    // could stay stale while the 3D pass kept refreshing.
    {
        use bevy::winit::{UpdateMode, WinitSettings};
        app.insert_resource(WinitSettings {
            focused_mode: UpdateMode::Continuous,
            // Networked: stay Continuous unfocused so keepalives keep flowing.
            // Single-player: low-power when backgrounded to keep fans quiet.
            unfocused_mode: if networked || no_throttle {
                UpdateMode::Continuous
            } else {
                UpdateMode::reactive_low_power(std::time::Duration::from_secs(1))
            },
        });
    }
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
    // Register every LunCo asset source (lunco://, lunco-lib://, twin://,
    // cached_textures://) + the shared `TwinRoots` resource in ONE shared place
    // (`lunco-assets`), so all binaries get identical schemes. Must run before
    // `DefaultPlugins`/`AssetPlugin` snapshots the source registry.
    lunco_assets::register_lunco_asset_sources(&mut app);

    // Headless-aware base plugin group. Windowed: normal winit window + GPU.
    // Headless (`--no-ui`): no primary window, winit disabled, GPU backend NONE.
    // The render plugins still register the mesh/material asset stores (so USD
    // visual sync can populate the meshes avian colliders read), but create no
    // device and draw nothing. `ScheduleRunnerPlugin` (added below) ticks the app
    // in winit's place.
    let default_plugins = {
        use bevy::render::settings::WgpuSettings;
        let render_creation = if headless {
            WgpuSettings { backends: None, ..default() }.into()
        } else {
            lunco_workbench::preferred_wgpu_settings().into()
        };
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
        if headless {
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
        }
        .build()
        .disable::<TransformPlugin>()
    };

    app.insert_resource(ScenePath(scene_path))
        .insert_resource(virtual_time)
        // Match the workbench theme's backdrop so the window's first-
        // frame clear lines up with egui's panel fill. Without this
        // the inherited Bevy default (mid-gray) shows through the 1px
        // gaps that non-integer DPRs and egui panel-edge rounding can
        // leave at panel boundaries — visible as a "left hairline"
        // against a dark theme. Same idea as the ClearColor in `lunica.rs`.
        .insert_resource(ClearColor(Color::srgb_u8(0x1a, 0x1a, 0x1a)))
        .insert_resource(Time::<Fixed>::from_hz(lunco_core::FIXED_HZ))
        // `speed: 1.0` is load-bearing: `..default()` leaves it 0.0, which keeps
        // physics running (wheels gate on `physics_enabled` only) but FREEZES
        // `SimTick` — `advance_sim_tick` needs `speed > 0.0`. A frozen tick breaks
        // every tick-keyed netcode path (snapshot interpolation timebase, input
        // stamping). Match the rover examples: both flags set explicitly.
        .insert_resource(lunco_core::TimeWarpState { physics_enabled: true, speed: 1.0 })
        .insert_resource(avian3d::prelude::Gravity::ZERO)
        .insert_resource(lunco_environment::Gravity::flat(9.81, bevy::math::DVec3::NEG_Y))
        // Studio lighting for the sandbox — a generic editor scene, NOT a
        // calibrated lunar surface. The canonical 128 klx + EV15 `LunarSun` is
        // tuned for 0.13-albedo regolith; the sandbox's dark blueprint-grid
        // ground crushes to near-black under it. Inserted BEFORE plugins so
        // `EnvironmentPlugin`'s `init_resource` keeps these. The sun spawn AND
        // every camera's exposure read this one resource, so lux and EV stay
        // matched. Tunable live via `SetEnvironmentLight` / the Inspector.
        .insert_resource(lunco_environment::LunarSun {
            illuminance_lux: 10_000.0,
            exposure_ev100: 9.7,
            ..Default::default()
        })
        .add_plugins(default_plugins)
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
        // EntityCount is cheap and useful any time we look at perf; add
        // unconditionally. LogDiagnosticsPlugin is loud — it prints a
        // multi-line summary every second — so gate it on `--log-diag`.
        .add_plugins(bevy::diagnostic::EntityCountDiagnosticsPlugin::default())
        .add_plugins(PhysicsPlugins::default().set(avian3d::prelude::PhysicsInterpolationPlugin::interpolate_all()))
        // 12 solver substeps (avian default is 6). The joint-based rovers buzz
        // the chassis under drive torque at 6 substeps — the rigid wheel↔chassis
        // revolute can't resolve the wheel-contact + drive impulse coupling and
        // it leaks into the chassis as "jitter when riding". At 12 substeps the
        // jitter vanishes while drops still settle perfectly. Quantified in the
        // headless `rover_jitter` probe. See `project_physical_rover_suspension`.
        .insert_resource(avian3d::prelude::SubstepCount(12))
        .add_plugins(CoSimPlugin)
        .add_plugins(lunco_core::LunCoCorePlugin)
        // Persistent world shell: one BigSpace root + `WorldGrid` + one
        // `FloatingOrigin`. Scenes mount into it via `ensure_world_root`.
        .add_plugins(lunco_core::WorldShellPlugin)
        .add_plugins(GravityPlugin)
        .add_plugins(EnvironmentPlugin)
        .add_plugins(TerrainPlugin)
        .add_plugins(LunCoHardwarePlugin)
        .add_plugins(LunCoMobilityPlugin)
        // USD scene load + avian collider build + cosim wiring — server-authoritative
        // sim, headless-safe (colliders are pure CPU; visual sync only writes the
        // mesh/material asset stores, never touches a GPU device).
        .add_plugins(UsdPlugins)
        // Vessel input + possession command observers. Headless-safe: leafwing's
        // InputManager rides on bevy_input (no winit), so the keyboard just
        // produces nothing on a server while the Drive/Brake/Possess command
        // observers + their wire type registrations the host needs stay live.
        .add_plugins(LunCoControllerPlugin)
        .add_plugins(LunCoAvatarPlugin)
        .add_plugins(lunco_scripting::LunCoScriptingPlugin)
        // Default scene-wide fill for scenes that author no lighting; a
        // scene-authored UsdLux light takes ambient over (DomeLight
        // intensity, or 0 when absent). Runtime control: Inspector →
        // Environment, or the `SetEnvironmentLight` command.
        .insert_resource(bevy::light::GlobalAmbientLight {
            brightness: 40.0,
            ..Default::default()
        })
        .add_systems(Startup, setup_sandbox)
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
        ).chain());

    // ── UI / render-only layer (skipped under `--no-ui`) ──────────────────
    // Everything here draws pixels, opens egui panels, or drives an interactive
    // camera. A headless server runs the sim, physics, scene, cosim, and
    // networking host above *without* any of it. The render plugins still loaded
    // in `backends: None` mode so the asset stores exist; these plugins are what
    // would actually need a GPU / window / pointer.
    if !headless {
        app.add_plugins(WireframePlugin::default())
            // bevy_picking's mesh backend: makes visible Mesh3d entities pickable,
            // so scene selection / possession / spawn-placement run as click observers.
            .add_plugins(bevy::picking::mesh_picking::MeshPickingPlugin)
            .add_plugins(lunco_workbench::WorkbenchPlugin)
            // USD Twin browser + the offscreen RTT preview viewport.
            .add_plugins(UsdUiPlugin)
            .add_plugins(UsdViewportPlugin)
            .add_plugins(SandboxEditPlugin)
            .add_plugins(lunco_sandbox_edit::ui::SandboxEditUiPlugin)
            .add_plugins(BlueprintMaterialPlugin)
            .add_plugins(ShaderMaterialPlugin)
            // Rover-specific panels and the attach-a-model click flow.
            .add_plugins(|app: &mut App| {
                use lunco_workbench::WorkbenchAppExt;
                app.register_panel(code_panel::CodePanel);
                app.register_panel(models_palette::ModelsPalette);
                app.init_resource::<models_palette::AttachState>();
                // Attach is bevy_picking-driven (observes the same `Pointer<Click>`
                // as selection; egui occlusion handled by the framework).
                app.add_observer(models_palette::on_scene_click_attach);
                app.add_systems(Update, models_palette::attach_escape_system);
            })
            // ModelicaPlugin's AnalyzePerspective registers before SandboxEditUiPlugin's
            // workspaces; without this nudge we'd boot into the Modelica layout.
            // Activate the 3D-only View workspace by default.
            .add_systems(Startup, |mut layout: ResMut<lunco_workbench::WorkbenchLayout>| {
                layout.activate_perspective(lunco_workbench::PerspectiveId("sandbox_view"));
            })
            // Confine window-targeting cameras to the ViewportPanel rect (prevents
            // the full-window 3D bleed-on-pass-skip bug). RTT cameras are skipped.
            .add_systems(Update, auto_tag_workbench_3d_cameras)
            // Sharpest shadow filter (hard airless-Moon terminator) on each camera.
            .add_systems(Update, force_hard_shadow_filtering)
            // egui scroll → avatar `CameraScroll` bridge (gated on the viewport rect).
            .add_systems(EguiPrimaryContextPass, collect_scroll_input_gated)
            // Fallback free-flight camera when the scene authors none — interactive
            // only; a headless server has no user to control.
            .add_systems(PostUpdate,
                spawn_fallback_avatar.after(avian3d::prelude::PhysicsSystems::Writeback));
    }

    if headless {
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
    }

    // Embed the FULL lunica workbench as the "Design" workspace via the
    // shared bundle — same clipboard bridge, autosave, worker, and panels
    // as standalone lunica, so the Design tab can't drift from the real
    // IDE. We pass only the one intentional embed knob: suppress the
    // first-run help overlay (lunica's onboarding coach-marks, out of
    // place inside a 3D physics demo). Welcome panel stays ON — it's the
    // same landing page lunica uses for the Design tab; disabling it left
    // the centre empty and the 3D sandbox viewport bled through.
    // egui IDE workspace — UI only. (A headless server that needs server-side
    // Modelica cosim would add `ModelicaCorePlugin` instead; the default sandbox
    // scene doesn't, so skip the whole thing.)
    if !headless {
        app.add_plugins(ModelicaWorkbenchPlugin {
            config: ModelicaUiConfig {
                include_help_overlay: false,
                include_welcome_panel: true,
            },
        });
    }

    // Dismiss the HTML loading screen once the first frame paints
    // (wasm-only; no-op on native). Pairs with `web/index.html` →
    // `lunco-boot.js`.
    app.add_plugins(lunco_web::WebReadyPlugin);

    // URL-driven boot. Lets headless test harnesses drive the workbench
    // without firing canvas pointer events (synthetic DOM events don't
    // flow through winit's web event handlers, so e.g.
    // `chrome-devtools-mcp` can't click into the canvas). Supported
    // query params:
    //
    //   ?workspace=<perspective_id>   Activate a registered Perspective.
    //                                 Sandbox ships: sandbox_view,
    //                                 rover_build, modelica_analyze.
    //   ?open=<qualified.class.name>  Trigger `OpenClass`. Waits for
    //                                 `MslLoadState::Ready` before
    //                                 firing — without that the
    //                                 trigger races MSL install and
    //                                 the workbench logs `could not
    //                                 locate <class>`.
    //
    // Runs on `Update` so it can poll MSL state; self-disables after
    // both knobs are applied. Failures are logged and non-fatal.
    #[cfg(target_arch = "wasm32")]
    app.add_systems(bevy::prelude::Update, sandbox_boot_from_url);

    #[cfg(feature = "lunco-api")]
    app.add_plugins(lunco_api::LunCoApiPlugin::default());

    // Multiplayer. Native: `--host [port]` runs the listen-server, `--connect
    // <addr>` auto-joins one. Browser: `?connect=host` in the page URL. With no
    // address the plugin still loads **client-capable but idle** (single-player)
    // so the in-sim *Connect* button / `JoinServer` command can dial a server at
    // runtime — connecting is no longer a launch-time-only decision.
    #[cfg(feature = "networking")]
    {
        let mode = lunco_networking::NetworkMode::resolve();
        info!("[net] networking mode: {mode:?}");
        app.add_plugins(lunco_networking::LunCoNetworkingPlugin { mode });
        // Connect-menu bridge adapter (seeds connect_hint, re-dispatches the
        // NetConnect/Disconnect bridge events). Observers + a Startup seed only —
        // no egui — so it's safe headless too; keep it so the host still answers
        // runtime JoinServer/LeaveServer.
        app.add_plugins(lunco_networking::ui::LunCoNetworkingUiPlugin);
    }

    if log_diag {
        app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
    }

    // Forced window placement (`--window-pos`). Parses the flag and (when
    // present) inserts the resource, suppresses geometry persistence, and
    // registers the placer system — all in `lunco-workbench` so any binary
    // gets the same behaviour. Winit-specific, so skip it headless.
    if !headless {
        lunco_workbench::wire_window_placement(&mut app, &args);
    }

    if headless {
        info!("[net] sandbox running HEADLESS (--no-ui): no window/GPU/egui; sim + networking host only");
    }

    app.run();
}

/// State machine for [`sandbox_boot_from_url`].
///
/// Lives in a `Local` so the boot work happens exactly once per app
/// lifetime — once `open_class` is satisfied the system runs and
/// no-ops in O(1).
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct SandboxBootState {
    parsed: bool,
    workspace: Option<String>,
    open_class: Option<String>,
    done: bool,
}

/// wasm-only `Update` system that reads `window.location.search` and:
///   - activates the perspective named by `?workspace=…` (once, on
///     first run);
///   - triggers an `OpenClass` for `?open=…` once `MslLoadState`
///     reaches `Ready`. Without that gate the trigger races MSL
///     install and the workbench can't find the class.
///
/// Self-disables after both are applied. Useful for headless test
/// harnesses (e.g. `chrome-devtools-mcp`) which can't drive the egui
/// canvas via synthetic DOM events.
#[cfg(target_arch = "wasm32")]
fn sandbox_boot_from_url(
    mut commands: bevy::prelude::Commands,
    mut layout: Option<bevy::prelude::ResMut<lunco_workbench::WorkbenchLayout>>,
    msl: Option<bevy::prelude::Res<lunco_assets::msl::MslLoadState>>,
    mut state: bevy::prelude::Local<SandboxBootState>,
) {
    if state.done { return; }

    // ── First-run: parse URL, kick the workspace switch ──────────
    if !state.parsed {
        let search = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .unwrap_or_default();
        for kv in search.trim_start_matches('?').split('&') {
            let mut parts = kv.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let val_enc = parts.next().unwrap_or("");
            let val = js_sys::decode_uri_component(val_enc)
                .map(|j| j.as_string().unwrap_or_else(|| val_enc.to_string()))
                .unwrap_or_else(|_| val_enc.to_string());
            match key {
                "workspace" => state.workspace = Some(val),
                "open" => state.open_class = Some(val),
                _ => {}
            }
        }
        if let (Some(ws), Some(layout)) = (state.workspace.as_ref(), layout.as_mut()) {
            let id: &'static str = Box::leak(ws.clone().into_boxed_str());
            layout.activate_perspective(lunco_workbench::PerspectiveId(id));
            bevy::log::info!("[sandbox_boot_from_url] activated perspective `{ws}`");
        }
        state.parsed = true;
    }

    // ── Per-frame poll: dispatch OpenClass once MSL is ready ─────
    if let Some(qual) = state.open_class.clone() {
        let ready = matches!(
            msl.as_deref(),
            Some(lunco_assets::msl::MslLoadState::Ready { .. })
        );
        if !ready {
            return;
        }
        commands.trigger(lunco_modelica::ui::commands::OpenClass {
            qualified: qual.clone(),
            ..Default::default()
        });
        bevy::log::info!("[sandbox_boot_from_url] OpenClass({qual}) triggered (MSL ready)");
    }
    state.done = true;
}

/// Resource that holds the asset-source-relative path of the scene
/// to load on Startup. Initialised from the `--scene` CLI arg.
#[derive(Resource)]
struct ScenePath(String);

/// Bridge egui scroll input into `lunco_avatar::CameraScroll` so the
/// avatar zoom systems (`SpringArm`, `Orbit`, `Chase`) react to mouse
/// wheel events.
///
/// Gate scroll-zoom on the **viewport rect** (`PanelRects`), NOT on
/// `ctx.wants_pointer_input()`.
///
/// `wants_pointer_input()` was wrong here: `egui_dock` renders the viewport
/// leaf as an egui `Area`, so egui reports it "wants" the pointer over the bare
/// 3D scene too — which swallowed wheel-zoom over the viewport entirely. That's
/// the exact reason selection + possession were migrated to `PanelRects`
/// (`selection.rs` documents it); this scroll gate was left on the broken
/// signal. Now it matches: zoom is collected only when the cursor is over a
/// recorded viewport rect (and not under a floating popup), and **fails open**
/// when no viewport rect exists (chrome-less full-window "View" perspective).
/// Over a docked panel's scrollarea the cursor is outside the viewport rect, so
/// the wheel scrolls the panel instead of zooming — the behaviour the old
/// `wants_pointer_input` hover gate was trying (and failing over the scene) to
/// get.
fn collect_scroll_input_gated(
    mut egui_contexts: EguiContexts,
    mut scroll_res: ResMut<lunco_avatar::CameraScroll>,
) {
    let Ok(ctx) = egui_contexts.ctx_mut() else { return };
    // Wheel-zoom is the only scene input not routed through bevy_picking (it's
    // not a pick). Gate it on egui's own `wants_pointer_input()` — true over any
    // interactive widget, false over the bare 3D — read here in the egui pass so
    // it's same-frame. Note: NOT `is_pointer_over_area`/`is_using_pointer`; the
    // former is true over the full-window transparent egui area (would block the
    // scene), the latter is true for the whole duration of a scroll (would block
    // the scroll itself after the first notch).
    if ctx.wants_pointer_input() {
        return;
    }
    scroll_res.delta += ctx.input(|i: &bevy_egui::egui::InputState| i.raw_scroll_delta.y);
}

/// Tag freshly-added window-targeting `Camera3d` entities with
/// `WorkbenchViewportCamera` so the workbench's PostUpdate viewport
/// sync confines them to the `ViewportPanel` rect.
///
/// RTT cameras (`RenderTarget::Image`) are skipped: they paint into
/// their own offscreen Image (USD preview, vello diagrams) and must
/// not have a window-scoped viewport written to them.
///
/// `Added<Camera3d>` fires once per entity, the same frame the
/// component is inserted. USD scene-load and async Avatar spawning
/// can both land Camera3d entities long after `Startup`; this catches
/// each as it arrives.
fn auto_tag_workbench_3d_cameras(
    mut commands: Commands,
    new_cams: Query<
        (Entity, Option<&RenderTarget>),
        (Added<Camera3d>, Without<WorkbenchViewportCamera>),
    >,
) {
    for (entity, target) in &new_cams {
        let targets_window = matches!(target, None | Some(RenderTarget::Window(_)));
        if targets_window {
            commands.entity(entity).insert(WorkbenchViewportCamera);
        }
    }
}

// `set_parent_in_place` is `disallowed_methods`-banned for its atomicity
// hazard (a `GridAnchor`/`RigidBody` parented after spawn can be mis-tagged
// `RigidBody::Static`). The two uses here parent the big_space root → Grid
// and a `DirectionalLight` → Grid — neither is a rigid body / GridAnchor, so
// that hazard doesn't apply, and this is a native-only bin. Locally allowed.
#[allow(clippy::disallowed_methods)]
fn setup_sandbox(world: &mut World) {
    let scene_path: String = world.resource::<ScenePath>().0.clone();

    // The persistent world shell (BigSpace root + `WorldGrid` + the single
    // `FloatingOrigin`) is owned by `WorldShellPlugin`. `ensure_world_root` is
    // create-or-get, so the Sun hangs off the canonical grid regardless of which
    // Startup system ran first — no eager root spawn here, no "first Grid" guess.
    let grid = lunco_core::ensure_world_root(world);

    // --- Sun (directional light) on the world grid ---
    //
    // Real lunar shadows: hard-edged, jet-black, and *long* — cast by both the
    // terrain itself (crater rims, ridges) and rovers. Three things produce
    // that look:
    //   1. Terrain casts shadows again (no `NotShadowCaster` — see lunco-usd-avian).
    //   2. A LONG cascade range (≤ ~1.5 km) so a low-sun shadow isn't clipped
    //      mid-streak — at ~6° elevation a rover throws a shadow tens of metres
    //      long and a ridge hundreds. The near cascade still sits tight (≈40 m)
    //      so rover contact shadows stay razor-sharp; far cascades cover the
    //      terrain self-shadows.
    //   3. `ShadowFilteringMethod::Hardware2x2` on the camera (see
    //      `force_hard_shadow_filtering`) — the sharpest filter, for the airless
    //      hard terminator instead of soft PCF penumbrae.
    // Cascades are camera-relative and big_space recenters the camera near the
    // render origin every frame, so the large world Y (~2462 m) costs no
    // shadow-map precision. 4096² is the max safe atlas (8192² × 4 cascades
    // would be ~1 GB VRAM); the cascade split keeps the near field dense.
    // Canonical lunar-sun cascade split + 4096² atlas from the single source of
    // truth (`lunco_render::LunarSunShadow`), shared with the celestial and USD
    // paths. The biases are overridden below for this binary's hard-shadow look:
    // with `Hardware2x2` filtering (see `force_hard_shadow_filtering`) the normal
    // bias must stay small or it detaches/softens the contact edge — unlike the
    // terrain-acne-tuned default (0.06/2.5) used under PCF.
    let sun = lunco_render::LunarSunShadow {
        depth_bias: 0.02,
        normal_bias: 0.8,
        ..Default::default()
    };
    // Illuminance + angular size from the active-scene `LunarSun` resource (the
    // sandbox inserts studio values in `main`; every camera's exposure reads the
    // same resource, so sun lux and camera EV can't drift apart).
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
        // Default sun for scenes that author no lighting. A scene that
        // authors a UsdLux `DistantLight` (e.g. the moonbase Twin) replaces
        // it: the loader despawns every `FallbackSceneLight` and takes over
        // ambient too (no authored `DomeLight` ⇒ ambient 0).
        lunco_usd::FallbackSceneLight,
        ChildOf(grid),
    ));

    // --- Load scene from USD ---
    // Routed through the typed-command bus so startup and runtime
    // (API/MCP `LoadScene`, future File→Open) share one code path.
    // Empty `root_prim` auto-derives `/PascalCaseFromFilename`.
    //
    // An ABSOLUTE `--scene` path names an external Twin scene: register its
    // folder under the `twin://` source (keyed by the folder name) and load
    // through that source — stable, cross-platform identity. Relative paths load
    // from the default `assets/` source unchanged. This mirrors what the
    // File → Open Folder / Twin-open flow does; the CLI is the headless entry to
    // the same mechanism.
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
    active_sun: Res<lunco_environment::LunarSun>,
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
        // NO SMAA on this (workbench) camera: SMAA's post-process resolve does
        // not survive the full-window-3D + egui-overlay compositing, so it
        // renders a blank/black viewport (and crashes outright without the
        // `smaa_luts` feature). MSAA (the `Camera3d` default) covers geometry
        // edges. See the matching note on the USD avatar camera in lunco-usd-sim.
        //
        // Exposure read from the active-scene `LunarSun` resource — the SAME
        // source as the sun illuminance, so they stay matched. Tunable live via
        // SetEnvironmentLight / the Inspector.
        bevy::camera::Exposure { ev100: active_sun.exposure_ev100 },
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


/// Inserts the sharpest shadow filter (`Hardware2x2`) on every 3D camera as it
/// appears. USD- and Avatar-spawned cameras land async over many frames; the
/// `Without<ShadowFilteringMethod>` filter catches each exactly once.
fn force_hard_shadow_filtering(
    mut commands: Commands,
    q: Query<Entity, (With<Camera3d>, Without<bevy::light::ShadowFilteringMethod>)>,
) {
    for e in &q {
        commands
            .entity(e)
            .insert(bevy::light::ShadowFilteringMethod::Hardware2x2);
    }
}

