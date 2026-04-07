//! Primary entry point for the LunCo simulation client.
//!
//! This crate assembles all simulation plugins (Celestial, FSW, Hardware, 
//! Robotics, etc.) into a cohesive application. It handles the high-level 
//! Bevy app configuration, including asset sourcing, plugin initialization, 
//! and global coordinate synchronization.

use bevy::{prelude::*, asset::io::AssetSourceBuilder};

mod ui;
use lunco_celestial::BlueprintMaterial;
use ui::LunCoUiPlugin;

/// Main entry point for the simulation.
fn main() {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(ClearColor(Color::BLACK))
        .register_asset_source(
            "cached_textures",
            AssetSourceBuilder::platform_default("../../.cache/textures", None),
        )
        // Note: TransformPlugin is disabled because big_space uses its own propagation systems.
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>()) 
        .add_plugins(big_space::prelude::BigSpaceDefaultPlugins.build().disable::<big_space::validation::BigSpaceValidationPlugin>())
        .add_plugins(lunco_core::LunCoCorePlugin);

    // THE UNIVERSAL SYNC BRIDGE
    // Required since TransformPlugin is disabled for BigSpace support.
    app.add_systems(PreUpdate, global_transform_propagation_system);
    app.add_systems(PostUpdate, global_transform_propagation_system.after(avian3d::prelude::PhysicsSystems::Writeback));

    #[cfg(not(feature = "sandbox"))]
    {
        app.add_plugins(lunco_celestial::CelestialPlugin)
            .insert_resource(ClearColor(Color::BLACK));
    }

    app.add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(LunCoUiPlugin)
        .add_plugins(lunco_fsw::LunCoFswPlugin)
        .add_plugins(lunco_hardware::LunCoHardwarePlugin)
        .add_plugins(lunco_mobility::LunCoMobilityPlugin)
        .add_plugins(lunco_robotics::LunCoRoboticsPlugin)
        .add_plugins(lunco_controller::LunCoControllerPlugin)
        .add_plugins(lunco_avatar::LunCoAvatarPlugin)
        .add_systems(Update, (toggle_slow_motion, auto_focus_earth_once))
        .run();
}

/// A robust multi-pass system to propagate [GlobalTransform] and [Visibility] across grids.
fn global_transform_propagation_system(
    mut commands: Commands,
    q_needs: Query<Entity, (Or<(With<Visibility>, With<Mesh3d>, With<Text>, With<Transform>)>, Without<InheritedVisibility>, Without<big_space::prelude::CellCoord>)>,
    mut q_spatial: Query<(Entity, &mut GlobalTransform, &Transform, Option<&ChildOf>)>,
    mut q_visibility: Query<(Entity, &mut InheritedVisibility, &mut ViewVisibility, &Visibility, Option<&ChildOf>)>,
) {
    // 1. Initial backfill 
    for ent in q_needs.iter() {
        commands.entity(ent).insert((
            InheritedVisibility::default(),
            ViewVisibility::default(),
            GlobalTransform::default(),
        ));
    }

    // 2. Transform propagation (Manual fallback for TransformPlugin)
    for _ in 0..4 {
        let mut gtf_cache = std::collections::HashMap::new();
        for (ent, gtf, _, _) in q_spatial.iter() {
            gtf_cache.insert(ent, *gtf);
        }

        for (_ent, mut gtf, local_tf, child_of_opt) in q_spatial.iter_mut() {
            let parent_gtf = if let Some(child_of) = child_of_opt {
                gtf_cache.get(&child_of.parent()).cloned().unwrap_or_default()
            } else {
                GlobalTransform::default()
            };
            
            let new_gtf = parent_gtf.mul_transform(*local_tf);
            if gtf.to_matrix() != new_gtf.to_matrix() {
                *gtf = new_gtf;
            }
        }
    }

    // 3. Visibility propagation (Boolean sync)
    for _ in 0..4 {
        let mut vis_cache = std::collections::HashMap::new();
        for (ent, inherited, _, _, _) in q_visibility.iter() {
            vis_cache.insert(ent, inherited.get());
        }

        for (_, mut inherited, _view, visibility, child_of_opt) in q_visibility.iter_mut() {
            let parent_visible = if let Some(child_of) = child_of_opt {
                *vis_cache.get(&child_of.parent()).unwrap_or(&true)
            } else {
                true
            };
            
            let is_visible = parent_visible && visibility != Visibility::Hidden;
            if inherited.get() != is_visible {
                *inherited = if is_visible { InheritedVisibility::VISIBLE } else { InheritedVisibility::HIDDEN };
            }
        }
    }
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


