//! Primary entry point for the LunCo simulation client.
//!
//! This crate assembles all simulation plugins (Celestial, FSW, Hardware, 
//! Robotics, etc.) into a cohesive application. It handles the high-level 
//! Bevy app configuration, including asset sourcing, plugin initialization, 
//! and global coordinate synchronization.

use bevy::{prelude::*, asset::io::AssetSourceBuilder};
use big_space::prelude::CellCoord;

mod ui;
use lunco_celestial::BlueprintMaterial;
use ui::LunCoUiPlugin;

/// Main entry point for the simulation.
///
/// Sets up the Bevy [App] with the required plugins, resources, and systems. 
/// It also initializes the [big_space] coordinate system to allow for 
/// solar-system-scale simulations.
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
        .add_plugins(big_space::prelude::BigSpaceDefaultPlugins)
        .add_plugins(lunco_core::LunCoCorePlugin);

    // THE UNIVERSAL SYNC BRIDGE
    // This system ensures that transforms and visibility are correctly propagated 
    // across different coordinate grids (cells) in the large-scale simulation.
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
        .add_systems(Update, toggle_slow_motion)
        .run();
}

/// Toggles time dilation for debugging physics and high-speed maneuvers.
fn toggle_slow_motion(keyboard: Res<ButtonInput<KeyCode>>, mut time: ResMut<Time<Virtual>>) {
    if keyboard.just_pressed(KeyCode::KeyT) {
        if time.relative_speed() < 1.0 { time.set_relative_speed(1.0); } else { time.set_relative_speed(0.01); }
    }
}

/// A robust multi-pass system to propagate [GlobalTransform] and [Visibility] across [big_space] grids.
///
/// Since [big_space] disables Bevy's default [TransformPlugin] to prevent 
/// floating-point precision loss, this system manually synchronizes 
/// spatial data across parent-child hierarchies that span multiple grid cells.
fn global_transform_propagation_system(
    mut commands: Commands,
    q_needs: Query<Entity, (Or<(With<Visibility>, With<Mesh3d>, With<Text>, With<Transform>)>, Without<InheritedVisibility>, Without<CellCoord>)>,
    mut q_spatial: Query<(Entity, &mut GlobalTransform, &Transform, Option<&ChildOf>)>,
    mut q_visibility: Query<(Entity, &mut InheritedVisibility, &mut ViewVisibility, &Visibility, Option<&ChildOf>)>,
) {
    // 1. Initial backfill: Ensure all relevant entities have the required 
    // spatial components for propagation.
    for ent in q_needs.iter() {
        commands.entity(ent).insert((
            InheritedVisibility::default(),
            ViewVisibility::default(),
            GlobalTransform::default(),
        ));
    }

    // 2. Transform propagation: Recursively calculate GlobalTransforms.
    // Performed in multiple passes to handle deep hierarchies without 
    // complex tree traversal.
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

    // 3. Visibility propagation: Sync InheritedVisibility based on hierarchy.
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

