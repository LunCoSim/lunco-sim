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


