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
// `bevy::camera::*` exists on both native and `--no-default-features`
// wasm; `bevy::render::camera::*` only when `bevy_render` is enabled.
use bevy::camera::RenderTarget;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use lunco_workbench::WorkbenchViewportCamera;
use lunco_assets::cache_dir;
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
    // Networking present? (`--host`/`--connect`). When networked, the window
    // must keep ticking even when unfocused: lightyear's netcode link sends
    // keepalives on the update loop, and the default unfocused throttle
    // (~1 FPS) starves them past the timeout, dropping the connection a few
    // seconds after the window loses focus. Two side-by-side windows means one
    // is always unfocused — so we keep it Continuous while networked.
    let networked = args.iter().any(|a| a == "--host" || a == "--connect");
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
            unfocused_mode: if networked {
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
    app.insert_resource(ScenePath(scene_path))
        .insert_resource(virtual_time)
        // Match the workbench theme's backdrop so the window's first-
        // frame clear lines up with egui's panel fill. Without this
        // the inherited Bevy default (mid-gray) shows through the 1px
        // gaps that non-integer DPRs and egui panel-edge rounding can
        // leave at panel boundaries — visible as a "left hairline"
        // against a dark theme. Same idea as the ClearColor in `lunica.rs`.
        .insert_resource(ClearColor(Color::srgb_u8(0x1a, 0x1a, 0x1a)))
        // `lunco-lib://` shipped-fixture asset source — must be
        // registered *before* `DefaultPlugins`/`AssetPlugin` builds the
        // server. Mirrors the registration in `lunco-client`'s main
        // binary; without it, `def Cube` placeholders with
        // `payload = @lunco-lib://...@` only render their Cube fallback.
        .register_asset_source(
            "lunco-lib",
            AssetSourceBuilder::platform_default(&cache_dir().to_string_lossy(), None),
        )
        .insert_resource(Time::<Fixed>::from_hz(lunco_core::FIXED_HZ))
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
                    // Centralized merged-titlebar chrome + persisted-geometry
                    // restore (size/position). Ship defaults live as named
                    // constants in `lunco-workbench`, not here.
                    ..lunco_workbench::restored_window(window_title)
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
            .set(bevy::render::RenderPlugin {
                // DX12 on Windows avoids the Vulkan window-resize panics
                // (depth/color size mismatch + SurfaceAcquireSemaphores). Other
                // platforms keep wgpu defaults. See lunco_workbench::render_robustness.
                render_creation: lunco_workbench::preferred_wgpu_settings().into(),
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
        .add_plugins(ShaderMaterialPlugin)
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
        // Auto-tag every new window-targeting Camera3d with
        // WorkbenchViewportCamera so the workbench's PostUpdate viewport
        // sync confines it to the ViewportPanel's rect (preventing the
        // full-window 3D bleed-on-pass-skip class of bug). USD- and
        // Avatar-spawned cameras land async over many frames; the
        // `Added<Camera3d>` filter catches each as it arrives. RTT
        // cameras (USD preview, vello diagrams) target an Image and
        // are skipped — they should *not* be confined to the panel.
        .add_systems(Update, auto_tag_workbench_3d_cameras)
        // Mirror of `lunco-client/src/main.rs::collect_scroll_input`,
        // gated to "egui doesn't want the scroll" so scrolling inside a
        // dock panel (Twin browser, Console, etc.) goes to that
        // widget, while scrolling over the viewport rect (or any
        // passive area where egui has no interactive use for it) goes
        // to the avatar's `CameraScroll` resource. Without this
        // system, the avatar zoom systems (`SpringArm`, `Orbit`,
        // `Chase`) never see scroll deltas — sandbox was the one
        // binary missing this bridge (memory id 5109).
        .add_systems(EguiPrimaryContextPass, collect_scroll_input_gated)
        // Transform/visibility propagation is owned entirely by big_space
        // (`propagate_high_precision` for CellCoord/grid entities,
        // `propagate_low_precision` for ordinary children). USD prims are
        // spawned grid-anchored with visibility components already inserted
        // (lunco-usd-bevy), so they fall under low-precision propagation —
        // no custom fallback needed. The previous
        // `global_transform_propagation_system` was removed (2026-05-29): it
        // fought big_space and corrupted GlobalTransform on every entity
        // (same bug lunica's main.rs documents as the surface-camera-roll
        // root cause). Profiling showed it cost ~0 ms; this is a correctness
        // fix, not an FPS fix.
        .add_systems(PostUpdate,
            spawn_fallback_avatar.after(avian3d::prelude::PhysicsSystems::Writeback));

    // Embed the FULL lunica workbench as the "Design" workspace via the
    // shared bundle — same clipboard bridge, autosave, worker, and panels
    // as standalone lunica, so the Design tab can't drift from the real
    // IDE. We pass only the one intentional embed knob: suppress the
    // first-run help overlay (lunica's onboarding coach-marks, out of
    // place inside a 3D physics demo). Welcome panel stays ON — it's the
    // same landing page lunica uses for the Design tab; disabling it left
    // the centre empty and the 3D sandbox viewport bled through.
    app.add_plugins(ModelicaWorkbenchPlugin {
        config: ModelicaUiConfig {
            include_help_overlay: false,
            include_welcome_panel: true,
        },
    });

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

    // Multiplayer (opt-in): `--host [port]` runs the listen-server,
    // `--connect <addr>` joins one over WebTransport. Absent ⇒ single-player
    // (the networking substrate stays inert).
    #[cfg(feature = "networking")]
    if let Some(mode) = lunco_networking::NetworkMode::from_args() {
        info!("[net] networking mode: {mode:?}");
        app.add_plugins(lunco_networking::LunCoNetworkingPlugin { mode });
    }

    if log_diag {
        app.add_plugins(bevy::diagnostic::LogDiagnosticsPlugin::default());
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

/// Bridge egui scroll input into `lunco_avatar::CameraScroll` so the
/// avatar zoom systems (`SpringArm`, `Orbit`, `Chase`) react to mouse
/// wheel events.
///
/// Gated on `!ctx.wants_pointer_input()` — egui sets that to `true`
/// when the cursor is over an interactive widget that consumes scroll
/// (scrollarea, slider, combo box, …). When it's `false`, the cursor
/// is over a passive region (the `ViewportPanel` placeholder, an empty
/// dock area, the menu bar background) and the scroll naturally
/// belongs to the 3D scene. This is the same idiom `lunco-client`'s
/// non-sandbox binary uses, plus the hover gate so dock-panel
/// scrolling no longer also zooms the camera.
fn collect_scroll_input_gated(
    mut egui_contexts: EguiContexts,
    mut scroll_res: ResMut<lunco_avatar::CameraScroll>,
) {
    let Ok(ctx) = egui_contexts.ctx_mut() else { return };
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

fn setup_sandbox(
    mut commands: Commands,
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

    // --- Load scene from USD ---
    // Routed through the typed-command bus so startup and runtime
    // (API/MCP `LoadScene`, future File→Open) share one code path.
    // Empty `root_prim` auto-derives `/PascalCaseFromFilename`.
    info!("Loading sandbox scene `{}` via LoadScene", scene_path);
    commands.trigger(LoadScene {
        path: scene_path.clone(),
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

