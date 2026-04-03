use bevy::{prelude::*, asset::io::AssetSourceBuilder, math::DVec3};
use avian3d::prelude::*;
use big_space::prelude::*;
use leafwing_input_manager::prelude::*;

use lunco_core::{Avatar, TimeWarpState};
use lunco_physics::{
    LunCoPhysicsPlugin, 
    spawn_joint_skid_rover, 
    spawn_joint_ackermann_rover,
};
use lunco_fsw::LunCoFswPlugin; 
use lunco_rover_raycast::{
    LunCoRoverRaycastPlugin, 
    spawn_raycast_skid_rover, 
    spawn_raycast_ackermann_rover
};
use lunco_controller::{LunCoControllerPlugin, SpaceSystemAction, get_default_input_map}; 
use lunco_avatar::LunCoAvatarPlugin;
use lunco_celestial::{BlueprintMaterial, BlueprintExtension, ObserverCamera, ObserverMode, CelestialClock, CelestialBody};

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
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>())
        .add_plugins(BigSpaceDefaultPlugins.build().disable::<big_space::validation::BigSpaceValidationPlugin>())
        
        .add_plugins(lunco_core::LunCoCorePlugin)
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(LunCoFswPlugin) 
        .add_plugins(LunCoPhysicsPlugin)
        .add_plugins(LunCoRoverRaycastPlugin)
        .add_plugins(LunCoAvatarPlugin)
        .add_plugins(LunCoControllerPlugin);

    app.add_systems(Startup, setup_sandbox);
    
    app.run();
}

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

    // 1. Clip Plane Anchor (to force 0.1m near plane)
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

    // 2. High Altitude Sky Light
    commands.spawn((
        DirectionalLight {
            shadows_enabled: true,
            illuminance: 140_000.0, // Blinding sun for high contrast
            ..default()
        },
        // Very steep angle from high "in the sky"
        Transform::from_rotation(Quat::from_rotation_x(90.0)),
        CellCoord::default(),
        Name::new("Sandbox_Sun"),
    )).set_parent_in_place(grid_entity);

    // 3. Ground
    let blueprint_mat = blueprint_materials.add(BlueprintMaterial {
        base: StandardMaterial {
            base_color: Color::srgb(0.25, 0.45, 0.85),
            perceptual_roughness: 0.9,
            ..default()
        },
        extension: BlueprintExtension {
            high_color: LinearRgba::new(0.7, 0.9, 1.0, 1.0),
            low_color: LinearRgba::new(0.2, 0.3, 0.7, 1.0),
            grid_scale: 1.0,
            line_width: 3.5,
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

    // 4. Testing Ramp
    let ramp_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.8, 0.8, 0.8),
        ..default()
    });
    commands.spawn((
        Name::new("Ramp"),
        Mesh3d(meshes.add(Cuboid::new(40.0, 1.0, 100.0))),
        MeshMaterial3d(ramp_mat),
        Transform::from_xyz(50.0, 8.0, 50.0).with_rotation(Quat::from_rotation_z(0.35)),
        RigidBody::Static,
        Collider::cuboid(40.0, 1.0, 100.0),
        Friction::new(1.0),
        CellCoord::default(),
    )).set_parent_in_place(grid_entity);

    // 5. ALL ROVERS RESTORED
    let rovers_root = commands.spawn((
        Name::new("Rovers_Root"), 
        Transform::from_xyz(0.0, 0.0, 0.0), 
        Visibility::default(),
        CellCoord::default(),
    )).set_parent_in_place(grid_entity).id();
    
    let wheel_mesh = meshes.add(Cylinder::new(0.5, 0.4));
    
    // Joint-Based Rovers
    spawn_joint_skid_rover(
        &mut commands, 
        rovers_root, 
        wheel_mesh.clone(), 
        Vec3::new(-15.0, 5.0, -10.0), 
        "Joint_Skid", 
        Color::srgb(0.8, 0.2, 0.2)
    );

    spawn_joint_ackermann_rover(
        &mut commands, 
        rovers_root, 
        wheel_mesh.clone(), 
        Vec3::new(-15.0, 5.0, 10.0), 
        "Joint_Ackermann", 
        Color::srgb(0.2, 0.8, 0.2)
    );

    // Raycast-Based Rovers
    spawn_raycast_skid_rover(
        &mut commands, 
        wheel_mesh.clone(), 
        Vec3::new(15.0, 5.0, -10.0), 
        "Raycast_Skid", 
        Color::srgb(0.2, 0.2, 0.8)
    );

    spawn_raycast_ackermann_rover(
        &mut commands, 
        wheel_mesh.clone(), 
        Vec3::new(15.0, 5.0, 10.0), 
        "Raycast_Ackermann", 
        Color::srgb(0.8, 0.8, 0.2)
    );

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
        Transform::from_xyz(60.0, 400.0, 600.0).looking_at(Vec3::ZERO, Vec3::Y),
        ObserverCamera { 
            mode: ObserverMode::Orbital,
            distance: 100.0,
            pitch: -0.7, 
            yaw: 0.8,    
            focus_target: Some(rovers_root), 
            ..default()
        },
        FloatingOrigin,
        CellCoord::default(),
        ActionState::<SpaceSystemAction>::default(),
        get_default_input_map(),
        lunco_celestial::ActiveCamera,
    )).set_parent_in_place(grid_entity);
}
