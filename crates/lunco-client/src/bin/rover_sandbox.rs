//! A standalone sandbox for rapid testing of ground mobility and physics.
//!
//! This binary bypasses the full celestial ephemeris system to provide a 
//! stable, flat-ground environment for debugging rovers, actuators, and FSW.
//! It serves as the primary development playground for mechanical engineering 
//! and control logic.

use bevy::prelude::*;
use bevy::asset::io::AssetSourceBuilder;
use bevy::math::DVec3;
use avian3d::prelude::*;
use big_space::prelude::*;
use leafwing_input_manager::prelude::*;

use lunco_core::{Avatar, TimeWarpState, IntentState};
use lunco_hardware::LunCoHardwarePlugin;
use lunco_mobility::LunCoMobilityPlugin;
use lunco_robotics::{
    LunCoRoboticsPlugin,
    rover::{spawn_joint_rover, spawn_raycast_rover, SteeringType},
};
use lunco_fsw::LunCoFswPlugin; 
use lunco_controller::LunCoControllerPlugin; 
use lunco_avatar::{LunCoAvatarPlugin, IntentAnalogState};
use lunco_celestial::{BlueprintMaterial, BlueprintExtension, CelestialClock, CelestialBody};
use lunco_camera::{ObserverCamera, ObserverMode, ActiveCamera};
use bevy_egui::{egui, EguiContexts, EguiPlugin, EguiPrimaryContextPass};

/// Tunable settings for the sandbox environment lighting.
///
/// **Tunability Mandate**: All magic numbers for visuals must be exposed 
/// for real-time adjustment.
#[derive(Resource, Reflect)]
#[reflect(Resource)]
struct SandboxLightSettings {
    /// Intensity of the directional sun light (lux).
    pub sun_illuminance: f32,
    /// Color of the primary sun source.
    pub sun_color: Srgba,
    /// Pitch angle of the sun in radians (-PI/2 is directly overhead).
    pub sun_pitch: f32,
    /// Yaw angle of the sun in radians.
    pub sun_yaw: f32,
    /// Brightness of the global ambient light.
    pub ambient_brightness: f32,
    /// Tint of the ambient light.
    pub ambient_color: Srgba,
}

impl Default for SandboxLightSettings {
    fn default() -> Self {
        Self {
            sun_illuminance: 10_000.0,
            sun_color: Srgba::WHITE,
            sun_pitch: -1.0,
            sun_yaw: 0.0,
            ambient_brightness: 1_000.0,
            ambient_color: Srgba::WHITE,
        }
    }
}


/// Sandbox application entry point.
fn main() {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(ClearColor(Color::Srgba(Srgba::new(0.05, 0.05, 0.15, 1.0))))
        .insert_resource(CelestialClock::default())
        .insert_resource(TimeWarpState { physics_enabled: true, ..default() })
        .register_asset_source(
            "cached_textures",
            AssetSourceBuilder::platform_default("../../.cache/textures", None),
        )
        // BigSpace is used even in the sandbox to ensure architectural parity 
        // with the main simulation client.
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(BigSpaceDefaultPlugins.build().disable::<big_space::validation::BigSpaceValidationPlugin>())
        
        .add_plugins(lunco_core::LunCoCorePlugin)
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(LunCoFswPlugin) 
        .add_plugins(LunCoHardwarePlugin)
        .add_plugins(LunCoMobilityPlugin)
        .add_plugins(LunCoRoboticsPlugin)
        .add_plugins(LunCoAvatarPlugin)
        .add_plugins(LunCoControllerPlugin)
        .add_plugins(EguiPlugin::default())
        .init_resource::<SandboxLightSettings>();

    // THE UNIVERSAL SYNC BRIDGE
    // Required since TransformPlugin is disabled for BigSpace support.
    app.add_systems(PreUpdate, global_transform_propagation_system);
    app.add_systems(PostUpdate, global_transform_propagation_system.after(avian3d::prelude::PhysicsSystems::Writeback));

    app.add_systems(Startup, setup_sandbox);
    app.add_systems(Update, apply_sandbox_light_settings);
    app.add_systems(EguiPrimaryContextPass, sandbox_light_ui_system);
    
    app.run();
}

/// A robust multi-pass system to propagate [GlobalTransform] and [Visibility] across grids.
fn global_transform_propagation_system(
    mut commands: Commands,
    q_needs: Query<Entity, (Or<(With<Visibility>, With<Mesh3d>, With<Text>, With<Transform>)>, Without<InheritedVisibility>, Without<CellCoord>)>,
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

/// Initializes the sandbox scene, including lighting, a flat grounded grid, 
/// and several rover prototypes for testing.
fn setup_sandbox(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut blueprint_materials: ResMut<Assets<BlueprintMaterial>>,
) {
    // 0. BigSpace Root
    let big_space_root = commands.spawn((
        BigSpace::default(),
        InheritedVisibility::default(),
        GlobalTransform::default(),
        Name::new("BigSpace_Root"),
    )).id();

    let grid_entity = commands.spawn((
        Grid::new(2000.0, 1.0e10), 
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Visibility::default(),
        InheritedVisibility::default(),
        Name::new("Sandbox_Grid"),
    )).set_parent_in_place(big_space_root).id();

    // 1. Clip Plane Anchor: Provides a reference for the camera's dynamic 
    // clip plane adjustment system.
    commands.spawn((
        CelestialBody {
            name: "Sandbox_Focus".to_string(),
            ephemeris_id: 0,
            radius_m: 1.0,
        },
        Transform::from_xyz(0.0, 0.0, 0.0),
        CellCoord::default(),
        Visibility::Visible,
    )).set_parent_in_place(grid_entity);

    // 2. Top-down Lighting (Sun directly overhead)
    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            illuminance: 10_000.0, 
            ..default()
        },
        Transform::from_rotation(Quat::from_rotation_x(-1.0)),
        CellCoord::default(),
        Name::new("Sandbox_Sun"),
    )).set_parent_in_place(grid_entity);

    // Ambient light for general visibility.
    commands.spawn((
        AmbientLight {
            color: Color::WHITE,
            brightness: 1_000.0,
            ..default()
        },
        Name::new("Sandbox_AmbientLight"),
    )).set_parent_in_place(grid_entity);

    // 3. Ground with Blueprint Grid Material.
    let blueprint_mat = blueprint_materials.add(BlueprintMaterial {
        base: StandardMaterial {
            base_color: Color::srgb(0.2, 0.2, 0.2), 
            perceptual_roughness: 0.9,
            ..default()
        },
        extension: BlueprintExtension {
            high_color: LinearRgba::new(0.5, 0.5, 0.5, 1.0),
            low_color: LinearRgba::new(0.1, 0.1, 0.1, 1.0),
            grid_scale: 1.0,
            line_width: 2.0,
            subdivisions: Vec2::new(10.0, 10.0),
            transition: 0.0, 
            ..default()
        },
    });

    commands.spawn((
        Name::new("Blueprint_Ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(2000.0, 2000.0))),
        MeshMaterial3d(blueprint_mat),
        RigidBody::Static,
        Collider::half_space(DVec3::Y),
        CellCoord::default(),
    )).set_parent_in_place(grid_entity);

    // 4. Testing Ramp for checking suspension and traction logic.
    let ramp_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.7, 0.7, 0.7),
        ..default()
    });
    commands.spawn((
        Name::new("Ramp"),
        Mesh3d(meshes.add(Cuboid::new(30.0, 1.0, 40.0))),
        MeshMaterial3d(ramp_mat),
        Transform::from_xyz(25.0, 4.0, 0.0).with_rotation(Quat::from_rotation_z(0.3)),
        RigidBody::Static,
        Collider::cuboid(30.0, 1.0, 40.0),
        Friction::new(1.0),
        CellCoord::default(),
    )).set_parent_in_place(grid_entity);

    // 5. Spawn prototype rovers of different steering and wheel types.
    let rovers_root = commands.spawn((
        Name::new("Rovers_Root"), 
        Transform::from_xyz(0.0, 0.0, 0.0), 
        Visibility::default(),
        CellCoord::default(),
    )).set_parent_in_place(grid_entity).id();
    
    // Joint-Based Rovers (Complex physics)
    spawn_joint_rover(
        &mut commands, 
        &mut meshes,
        &mut materials,
        rovers_root, 
        Vec3::new(-15.0, 5.0, -10.0), 
        "Joint_Skid", 
        Color::srgb(0.8, 0.2, 0.2),
        SteeringType::Skid,
    );

    spawn_joint_rover(
        &mut commands, 
        &mut meshes,
        &mut materials,
        rovers_root, 
        Vec3::new(-15.0, 5.0, 10.0), 
        "Joint_Ackermann", 
        Color::srgb(0.2, 0.8, 0.2),
        SteeringType::Ackermann,
    );

    // Raycast-Based Rovers (High-performance simulation)
    let r_skid = spawn_raycast_rover(
        &mut commands, 
        &mut meshes,
        &mut materials,
        Vec3::new(15.0, 5.0, -10.0), 
        "Raycast_Skid", 
        Color::srgb(0.2, 0.2, 0.8),
        SteeringType::Skid,
    );
    commands.entity(r_skid).set_parent_in_place(grid_entity);

    let r_ack = spawn_raycast_rover(
        &mut commands, 
        &mut meshes,
        &mut materials,
        Vec3::new(15.0, 5.0, 10.0), 
        "Raycast_Ackermann", 
        Color::srgb(0.8, 0.8, 0.2),
        SteeringType::Ackermann,
    );
    commands.entity(r_ack).set_parent_in_place(grid_entity);

    // 6. Avatar & Camera
    commands.spawn((
        Name::new("Sandbox_Avatar"),
        Avatar,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            near: 0.1,
            far: 10000.0,
            ..default()
        }),
        bevy::core_pipeline::tonemapping::Tonemapping::TonyMcMapface,
        bevy::post_process::bloom::Bloom::NATURAL,
        Transform::default(), 
        ObserverCamera { 
            mode: ObserverMode::Orbital,
            focus_target: Some(rovers_root),
            altitude: 20.0,
            distance: 20.0,
            ..default()
        },
        FloatingOrigin,
        CellCoord::default(),
        IntentState::default(),
        lunco_controller::get_avatar_input_map(),
        IntentAnalogState::default(),
        ActiveCamera,
    )).set_parent_in_place(grid_entity);
}

/// Renders the egui control panel for real-time light tuning.
fn sandbox_light_ui_system(
    mut contexts: EguiContexts,
    mut settings: ResMut<SandboxLightSettings>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return; };
    
    egui::Window::new("Environment Controls").show(ctx, |ui| {
        ui.heading("Primary Sun (Directional)");
        ui.add(egui::Slider::new(&mut settings.sun_illuminance, 0.0..=200_000.0).text("Illuminance (lux)"));
        
        ui.horizontal(|ui| {
            ui.label("Sun Color");
            let mut color = [settings.sun_color.red, settings.sun_color.green, settings.sun_color.blue];
            if ui.color_edit_button_rgb(&mut color).changed() {
                settings.sun_color.red = color[0];
                settings.sun_color.green = color[1];
                settings.sun_color.blue = color[2];
            }
        });

        ui.add(egui::Slider::new(&mut settings.sun_pitch, -std::f32::consts::PI..=0.0).text("Pitch (Angle)"));
        ui.add(egui::Slider::new(&mut settings.sun_yaw, -std::f32::consts::PI..=std::f32::consts::PI).text("Yaw (Angle)"));

        ui.separator();
        ui.heading("Ambient Light");
        ui.add(egui::Slider::new(&mut settings.ambient_brightness, 0.0..=5_000.0).text("Brightness"));

        ui.horizontal(|ui| {
            ui.label("Ambient Color");
            let mut color = [settings.ambient_color.red, settings.ambient_color.green, settings.ambient_color.blue];
            if ui.color_edit_button_rgb(&mut color).changed() {
                settings.ambient_color.red = color[0];
                settings.ambient_color.green = color[1];
                settings.ambient_color.blue = color[2];
            }
        });

        if ui.button("Reset Defaults").clicked() {
            *settings = SandboxLightSettings::default();
        }
    });
}

/// Syncs the [SandboxLightSettings] resource to the light entities in the scene.
fn apply_sandbox_light_settings(
    settings: Res<SandboxLightSettings>,
    mut q_sun: Query<(&mut DirectionalLight, &mut Transform), With<Name>>,
    mut q_ambient: Query<&mut AmbientLight, With<Name>>,
) {
    if settings.is_changed() {
        for (mut sun, mut tf) in q_sun.iter_mut() {
            // Find by name to ensure we only touch sandbox lights
            // (Though in this binary there's likely only one).
            // A more robust way would be a marker component.
            sun.illuminance = settings.sun_illuminance;
            sun.color = Color::Srgba(settings.sun_color);
            tf.rotation = Quat::from_euler(EulerRot::YXZ, settings.sun_yaw, settings.sun_pitch, 0.0);
        }

        for mut ambient in q_ambient.iter_mut() {
            ambient.brightness = settings.ambient_brightness;
            ambient.color = Color::Srgba(settings.ambient_color);
        }
    }
}

