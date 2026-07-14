//! LunCoSim — the full lunar-mission simulator.
//!
//! The flagship windowed app: celestial bodies + ephemeris, solar-system-scale
//! big_space, an orbital camera (auto-focus Earth on boot), and the whole
//! FSW / Hardware / Mobility / Robotics / Avatar stack under the workbench.
//! (Cf. the `sandbox` bin = ground-physics test bed, `lunica` = Modelica
//! workbench.) Assembles all simulation plugins into one cohesive Bevy app —
//! asset sourcing, plugin init, and big_space global coordinate propagation.
//!
//! ## Transform Propagation
//!
//! We rely entirely on big_space's built-in propagation systems
//! (`propagate_high_precision` for Grid entities, `propagate_low_precision`
//! for children). The custom `global_transform_propagation_system` that
//! previously ran here has been removed — it was fighting with big_space's
//! propagation and corrupting `GlobalTransform` on all entities, which was
//! the root cause of camera roll in surface mode.

use bevy::prelude::*;
use avian3d::prelude::PhysicsPlugins;

use lunco_ui::LuncoUiPlugin;
use lunco_workbench::WorkbenchAppExt;
/// Main entry point for the simulation. Single source for desktop AND web —
/// `#[cfg(target_arch = "wasm32")]` blocks handle platform differences, so
/// `build_web.sh build luncosim` compiles this same `fn main` for `wasm32`.
fn main() {
    // wasm: route panics to the browser console.
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    let mut app = App::new();
    // Register every LunCo asset source (cached_textures://, lunco-lib://,
    // lunco://, twin://) + the shared `TwinRoots` resource in ONE shared place
    // (`lunco-assets`), identical across all binaries — no per-`main()` drift.
    // Must run before `DefaultPlugins`/`AssetPlugin` snapshots the registry.
    // (`lunco://` is the engine asset library; an external collaborative
    // protocol, if added later, should take a distinct scheme like `lunco-net://`.)
    lunco_assets::register_lunco_asset_sources(&mut app);

    // Primary window: native gets the merged-titlebar workbench chrome; the
    // browser attaches to the `<canvas id="bevy">` and mirrors its CSS size.
    #[cfg(not(target_arch = "wasm32"))]
    let primary_window = lunco_workbench::merged_titlebar_window("LunCo");
    #[cfg(target_arch = "wasm32")]
    let primary_window = Window {
        title: "LunCoSim".into(),
        canvas: Some("#bevy".into()),
        fit_canvas_to_parent: true,
        prevent_default_event_handling: true,
        ..default()
    };

    app.insert_resource(Time::<Fixed>::from_hz(lunco_core::FIXED_HZ))
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(primary_window),
            ..default()
        }).set(bevy::render::RenderPlugin {
            // DX12 on Windows avoids the Vulkan window-resize panics (depth/color
            // size mismatch + SurfaceAcquireSemaphores); other platforms (incl.
            // wasm/WebGL2) keep wgpu defaults. See lunco_workbench::render_robustness.
            render_creation: lunco_workbench::preferred_wgpu_settings().into(),
            ..default()
        }).build().disable::<TransformPlugin>())
        .add_plugins({
            // big_space only registers `BigSpaceValidationPlugin` under
            // `debug_assertions`; disabling it in a release build (incl. the wasm
            // release the web build ships) panics, so gate the `.disable()`.
            let group = big_space::prelude::BigSpaceDefaultPlugins.build();
            #[cfg(debug_assertions)]
            let group = group.disable::<big_space::validation::BigSpaceValidationPlugin>();
            group
        })
        .add_plugins(lunco_core::LunCoCorePlugin)
        .insert_resource(lunco_core::DragModeActive { active: false })
        .add_plugins(lunco_workbench::WorkbenchPlugin)
        // Dismiss the HTML loading screen once the first frame paints (wasm-only;
        // no-op native). Pairs with `crates/lunco-web/web/index.html`.
        .add_plugins(lunco_web::WebReadyPlugin);

    // Register UI panels. The workbench's `ViewportPanel` holds the centre —
    // it paints nothing (transparent) but records its screen rect so the 3D
    // camera is confined to it and `apply_workbench_viewport` keeps that
    // camera active. Without it the centre is an opaque dock area that paints
    // over the full-window 3D camera (the "empty viewport" bug). Mission
    // Control docks into the right inspector via its `default_slot`.
    app.register_panel(lunco_workbench::ViewportPanel);
    app.register_panel(lunco_ui::MissionControl);

    // Avatar-/USD-spawned `Camera3d` entities land async (long after Startup);
    // tag each with `WorkbenchViewportCamera` as it appears so the workbench
    // confines it to the viewport rect instead of letting it bleed full-window
    // (and so it satisfies the camera-invariant sentinel). Mirrors sandbox.
    app.add_systems(Update, lunco_workbench::auto_tag_workbench_3d_cameras);

    // The celestial/orbital layer is what this binary IS — bodies, ephemeris,
    // solar-system-scale big_space. (It used to be switchable off by a `sandbox`
    // feature, back when a flat-ground rover world was a MODE of this app; that
    // role is now the separate `lunco-sandbox` binary, and this app carries no
    // terrain/USD stack to fall back on, so the toggle only produced an empty
    // scene. Unconditional now.)
    app.add_plugins(lunco_celestial::CelestialPlugin)
        .add_plugins(lunco_environment::EnvironmentPlugin)
        .insert_resource(ClearColor(Color::BLACK));

    #[cfg(not(target_arch = "wasm32"))]
    app.add_plugins(lunco_celestial_ephemeris::EphemerisPlugin);

    // THE RENDER GATE. Domain crates state appearance as INTENT (`PbrLook`,
    // `ShaderLook`, `SceneCamera`) and never name a material or a render
    // pipeline; this plugin — the one `bevy_pbr` consumer in the graph — binds
    // it. It also owns the dynamic `ShaderMaterial` pipeline itself
    // (`ShaderMaterialPlugin` + schema reflection; celestial Earth/Moon tiles
    // render with blueprint.wgsl), so that must NOT be added separately — Bevy
    // panics on a duplicate plugin. It also hosts the render-only code that has no
    // intent form: the horizon uniform feed, `SetEnvironmentLight`'s bloom arm, and
    // the `CaptureScreenshot` readback. This is a windowed binary, so it always adds
    // it (the `--no-ui` server does not, which is the entire gate).
    // See docs/architecture/render-decoupling.md.
    app.add_plugins(lunco_render_bevy::LuncoRenderPlugin)
        .add_plugins(PhysicsPlugins::default())
        // 12 solver substeps (avian default 6): the rigid joint-rover wheel
        // hinge leaks wheel-contact + drive impulses into the chassis as
        // "jitter when riding" at 6 substeps; 12 resolves it (drops still
        // settle perfectly). See `project_physical_rover_suspension`.
        .insert_resource(avian3d::prelude::SubstepCount(12))
        .add_plugins(LuncoUiPlugin)
        .add_plugins(lunco_avatar::ui::AvatarUiPlugin)
        .add_plugins(lunco_fsw::LunCoFswPlugin)
        .add_plugins(lunco_hardware::LunCoHardwarePlugin)
        .add_plugins(lunco_mobility::LunCoMobilityPlugin)
        .add_plugins(lunco_robotics::LunCoRoboticsPlugin)
        .add_plugins(lunco_controller::LunCoControllerPlugin)
        // Autopilot = a headless AiAgent actor that possesses + drives a vessel
        // (spec 034); on the control path, independent of the avatar/UI.
        .add_plugins(lunco_autopilot::AutopilotPlugin)
        .add_plugins(lunco_avatar::LunCoAvatarPlugin)
        .add_plugins(lunco_api::LunCoApiPlugin::default())
        .add_systems(Update, (toggle_slow_motion, auto_focus_earth_once))
        .run();
}

/// Toggles time dilation for debugging physics and high-speed maneuvers.
///
/// Drives the **unified** speed knob (`TimeTransport.rate`, which the `lunco-time`
/// spine maps onto `Time<Virtual>.relative_speed`) rather than writing
/// `relative_speed` directly — the spine reasserts `relative_speed = rate` every
/// frame, so a direct write here would be overwritten (doc 19 — T1 knob
/// unification).
fn toggle_slow_motion(
    keyboard: Res<ButtonInput<KeyCode>>,
    transport: Option<ResMut<lunco_time::TimeTransport>>,
) {
    let Some(mut transport) = transport else { return };
    if keyboard.just_pressed(KeyCode::KeyT) {
        transport.rate = if transport.rate < 1.0 { 1.0 } else { 0.01 };
    }
}

/// Directly inserts OrbitCamera targeting Earth on the first Update frame.
///
/// **Why**: Triggering FOCUS via command observer adds unnecessary indirection
/// and a 1.5s transition. We just insert OrbitCamera directly so the camera
/// is immediately usable in orbital mode.
fn auto_focus_earth_once(
    q_cameras: Query<(Entity, &Transform), With<lunco_core::Avatar>>,
    q_bodies: Query<(Entity, &lunco_celestial::CelestialBody)>,
    mut commands: Commands,
    mut did_focus: Local<bool>,
) {
    if *did_focus { return; }

    let Some((camera_entity, cam_tf)) = q_cameras.iter().next() else { return };
    let Some((earth_entity, earth_body)) = q_bodies.iter().find(|(_, body)| body.ephemeris_id == 399) else { return };
    // Arm the run-once latch only once both entities exist and we're
    // committed to inserting the camera (CQ-506): setting it on frame 1
    // before the spawn check meant auto-focus never ran.
    *did_focus = true;

    // Preserve current camera orientation.
    let (yaw, pitch, _) = cam_tf.rotation.to_euler(bevy::prelude::EulerRot::YXZ);

    commands.entity(camera_entity)
        .remove::<lunco_avatar::FreeFlightCamera>()
        .remove::<lunco_avatar::SpringArmCamera>()
        .remove::<lunco_avatar::OrbitCamera>()
        .remove::<lunco_avatar::FrameBlend>()
        .try_insert(lunco_avatar::OrbitCamera {
            target: earth_entity,
            distance: earth_body.radius_m * 3.0,
            yaw,
            pitch,
            damping: None,
            vertical_offset: 0.0,
        });
    info!("Auto-focused Earth at startup → OrbitCamera");
}
