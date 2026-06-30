//! Sandbox UI layer — everything that draws pixels, opens egui panels, or
//! drives an interactive camera.
//!
//! This whole module is `#[cfg(feature = "ui")]` (declared in `lib.rs`), so a
//! headless `--no-ui` / `lunco-sandbox-server` build never compiles it. The
//! entry point is [`SandboxUiPlugin`]: the app shell adds it only when running
//! windowed (`ui` feature present AND not `--no-ui`). The shared sim/physics/
//! cosim/networking core (`SandboxCorePlugin`) and the headless runner
//! (`SandboxHeadlessPlugin`) live in `lib.rs` and carry no UI.
//!
//! Mirrors the `ui/` + `*UiPlugin` convention every library crate already uses
//! (`SandboxEditUiPlugin`, `UsdUiPlugin`, `ModelicaUiPlugin`, …) — the app crate
//! is now structurally identical to them.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use leafwing_input_manager::prelude::*;
use big_space::prelude::*;

use lunco_workbench::auto_tag_workbench_3d_cameras;
use lunco_avatar::{IntentAnalogState, FreeFlightCamera, AdaptiveNearPlane, ProvisionalAvatarCamera};
use lunco_core::{Avatar, LocalAvatar};
use lunco_modelica::{ModelicaWorkbenchPlugin, ModelicaUiConfig};

mod code_panel;
mod models_palette;

/// The sandbox's interactive layer: egui workbench, bevy_picking, the USD Twin
/// browser + RTT viewport, the in-scene editor, materials, rover panels, and
/// the fallback free-flight camera.
///
/// Added by the app shell only for a windowed run. A headless server runs the
/// sim, physics, scene, cosim, and networking host (all in `SandboxCorePlugin`)
/// *without* any of this — the render plugins still load in `backends: None`
/// mode so the asset stores exist, but nothing here (GPU / window / pointer)
/// is wired.
pub(crate) struct SandboxUiPlugin;

impl Plugin for SandboxUiPlugin {
    fn build(&self, app: &mut App) {
        // Winit frame pacing. Continuous while focused lets vsync (Fifo present /
        // requestAnimationFrame on web) act as the frame timer; ReactiveLowPower
        // keeps fans quiet when backgrounded. Networked windows stay Continuous
        // unfocused so lightyear keepalives keep flowing (one of two side-by-side
        // windows is always unfocused; the default ~1 FPS throttle starves the
        // link past timeout). `--no-throttle` forces Continuous for automated
        // tests whose window sits unfocused. WinitSettings is read by the runner
        // each frame, so inserting it from this plugin (after DefaultPlugins) is
        // fine.
        let args: Vec<String> = std::env::args().collect();
        let networked = args.iter().any(|a| a == "--host" || a == "--connect");
        let no_throttle = args.iter().any(|a| a == "--no-throttle");
        {
            use bevy::winit::{UpdateMode, WinitSettings};
            app.insert_resource(WinitSettings {
                focused_mode: UpdateMode::Continuous,
                unfocused_mode: if networked || no_throttle {
                    UpdateMode::Continuous
                } else {
                    UpdateMode::reactive_low_power(std::time::Duration::from_secs(1))
                },
            });
        }

        app.add_plugins(bevy::pbr::wireframe::WireframePlugin::default())
            // bevy_picking's mesh backend: makes visible Mesh3d entities pickable,
            // so scene selection / possession / spawn-placement run as click observers.
            .add_plugins(bevy::picking::mesh_picking::MeshPickingPlugin)
            .add_plugins(lunco_workbench::WorkbenchPlugin)
            // USD Twin browser. NOTE: the USD *viewport preview*
            // (`UsdViewportPlugin`) is intentionally NOT added here. It is an
            // editor tool that OWNS its own scene — it parses the active USD doc
            // into a second `UsdStageAsset` and mounts a private `scene_root`. The
            // sandbox is a sim app: its single scene is the live `LoadScene` world,
            // viewed by the window camera. Adding the preview built the scene a
            // SECOND time (doubled crater meshes / rocks). A view must not own a
            // scene — see `docs/usd-source-of-truth-ecs-projection-design.md`.
            .add_plugins(lunco_usd::ui::UsdUiPlugin)
            .add_plugins(lunco_sandbox_edit::SandboxEditPlugin)
            .add_plugins(lunco_sandbox_edit::ui::SandboxEditUiPlugin)
            .add_plugins(lunco_materials::ShaderMaterialPlugin)
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

        // Embed the FULL lunica workbench as the "Design" workspace via the
        // shared bundle — same clipboard bridge, autosave, worker, and panels
        // as standalone lunica, so the Design tab can't drift from the real
        // IDE. We pass only the one intentional embed knob: suppress the
        // first-run help overlay (lunica's onboarding coach-marks, out of
        // place inside a 3D physics demo). Welcome panel stays ON — it's the
        // same landing page lunica uses for the Design tab.
        app.add_plugins(ModelicaWorkbenchPlugin {
            config: ModelicaUiConfig {
                include_help_overlay: false,
                include_welcome_panel: true,
            },
        });

        // Forced window placement (`--window-pos`). Parses the flag and (when
        // present) inserts the resource, suppresses geometry persistence, and
        // registers the placer system — all in `lunco-workbench` so any binary
        // gets the same behaviour.
        lunco_workbench::wire_window_placement(app, &args);

        // URL-driven boot (wasm). Lets headless test harnesses drive the
        // workbench without firing canvas pointer events. See
        // [`sandbox_boot_from_url`].
        #[cfg(target_arch = "wasm32")]
        app.add_systems(bevy::prelude::Update, sandbox_boot_from_url);
    }
}

/// Bridge egui scroll input into `lunco_avatar::CameraScroll` so the
/// avatar zoom systems (`SpringArm`, `Orbit`, `Chase`) react to mouse
/// wheel events.
///
/// Gate scroll-zoom on egui's own `wants_pointer_input()` — true over any
/// interactive widget, false over the bare 3D — read here in the egui pass so
/// it's same-frame. Note: NOT `is_pointer_over_area`/`is_using_pointer`; the
/// former is true over the full-window transparent egui area (would block the
/// scene), the latter is true for the whole duration of a scroll (would block
/// the scroll itself after the first notch).
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

/// Grace period before [`spawn_fallback_avatar`] steps in (USD load is async).
const FALLBACK_AVATAR_GRACE_SECS: f32 = 2.0;

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
        // Provisional: the authored USD Avatar camera (if the scene has one)
        // takes over and despawns this in the same flush it spawns — see
        // `ProvisionalAvatarCamera`. Without the marker, a slow (web/HTTP) scene
        // load that finishes *after* this stand-in appears leaves two order-0
        // window cameras → camera-order ambiguity + duplicate GizmoCamera.
        ProvisionalAvatarCamera,
        Transform::from_translation(Vec3::new(-30.0, 15.0, -20.0)),
        GlobalTransform::default(),
        FloatingOrigin,
        CellCoord::default(),
        Avatar,
        LocalAvatar,
        IntentAnalogState::default(),
        ActionState::<lunco_core::UserIntent>::default(),
        lunco_controller::get_avatar_input_map(),
        ChildOf(grid),
    ));
    *done = true;
}

// ── wasm URL-driven boot ──────────────────────────────────────────────────────

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
