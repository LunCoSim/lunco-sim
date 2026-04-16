//! Primary entry point for the LunCo simulation client.
//!
//! This crate assembles all simulation plugins (Celestial, FSW, Hardware,
//! Robotics, etc.) into a cohesive application. It handles the high-level
//! Bevy app configuration, including asset sourcing, plugin initialization,
//! and global coordinate synchronization.
//!
//! ## Transform Propagation
//!
//! We rely entirely on big_space's built-in propagation systems
//! (`propagate_high_precision` for Grid entities, `propagate_low_precision`
//! for children). The custom `global_transform_propagation_system` that
//! previously ran here has been removed — it was fighting with big_space's
//! propagation and corrupting `GlobalTransform` on all entities, which was
//! the root cause of camera roll in surface mode.

use bevy::{prelude::*, asset::io::AssetSourceBuilder};
use avian3d::prelude::PhysicsPlugins;

use lunco_materials::BlueprintMaterial;
use lunco_ui::LuncoUiPlugin;
use lunco_workbench::WorkbenchAppExt;
use lunco_assets::textures_dir;
use bevy_egui::{EguiPrimaryContextPass, EguiContexts};

/// Collects egui scroll input and feeds it to the camera zoom system.
/// Runs in EguiPrimaryContextPass so egui context is available.
/// Always passes scroll through to the camera — egui panels don't consume scroll.
fn collect_scroll_input(
    mut egui_contexts: EguiContexts,
    mut scroll_res: ResMut<lunco_avatar::CameraScroll>,
) {
    if let Ok(ctx) = egui_contexts.ctx_mut() {
        scroll_res.delta += ctx.input(|i: &bevy_egui::egui::InputState| i.raw_scroll_delta.y);
    }
}

/// Main entry point for the simulation.
fn main() {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(ClearColor(Color::BLACK))
        .register_asset_source(
            "cached_textures",
            AssetSourceBuilder::platform_default(&textures_dir().to_string_lossy(), None),
        )
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(big_space::prelude::BigSpaceDefaultPlugins.build().disable::<big_space::validation::BigSpaceValidationPlugin>())
        .add_plugins(lunco_core::LunCoCorePlugin)
        .insert_resource(lunco_core::DragModeActive { active: false })
        .add_plugins(lunco_workbench::WorkbenchPlugin)
        .add_systems(EguiPrimaryContextPass, collect_scroll_input);

    // Register UI panels — Mission Control as the right inspector. The
    // central region stays empty so the 3D world shows through.
    app.register_panel(lunco_ui::MissionControl);

    #[cfg(not(feature = "sandbox"))]
    {
        app.add_plugins(lunco_celestial::CelestialPlugin)
            .add_plugins(lunco_environment::EnvironmentPlugin)
            .insert_resource(ClearColor(Color::BLACK));
    }

    app.add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(LuncoUiPlugin)
        .add_plugins(lunco_avatar::ui::AvatarUiPlugin)
        .add_plugins(lunco_fsw::LunCoFswPlugin)
        .add_plugins(lunco_hardware::LunCoHardwarePlugin)
        .add_plugins(lunco_mobility::LunCoMobilityPlugin)
        .add_plugins(lunco_robotics::LunCoRoboticsPlugin)
        .add_plugins(lunco_controller::LunCoControllerPlugin)
        .add_plugins(lunco_avatar::LunCoAvatarPlugin)
        .add_plugins(lunco_api::LunCoApiPlugin::default())
        .add_systems(Update, (toggle_slow_motion, auto_focus_earth_once))
        .run();
}

/// Toggles time dilation for debugging physics and high-speed maneuvers.
fn toggle_slow_motion(keyboard: Res<ButtonInput<KeyCode>>, mut time: ResMut<Time<Virtual>>) {
    if keyboard.just_pressed(KeyCode::KeyT) {
        if time.relative_speed() < 1.0 { time.set_relative_speed(1.0); } else { time.set_relative_speed(0.01); }
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
    *did_focus = true;

    let Some((camera_entity, cam_tf)) = q_cameras.iter().next() else { return };
    let Some((earth_entity, earth_body)) = q_bodies.iter().find(|(_, body)| body.ephemeris_id == 399) else { return };

    // Preserve current camera orientation.
    let (yaw, pitch, _) = cam_tf.rotation.to_euler(bevy::prelude::EulerRot::YXZ);

    commands.entity(camera_entity)
        .remove::<lunco_avatar::FreeFlightCamera>()
        .remove::<lunco_avatar::SpringArmCamera>()
        .remove::<lunco_avatar::OrbitCamera>()
        .remove::<lunco_avatar::FrameBlend>()
        .insert(lunco_avatar::OrbitCamera {
            target: earth_entity,
            distance: earth_body.radius_m * 3.0,
            yaw,
            pitch,
            damping: None,
            vertical_offset: 0.0,
        });
    info!("Auto-focused Earth at startup → OrbitCamera");
}
