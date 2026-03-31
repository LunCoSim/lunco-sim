use avian3d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

use lunco_sim_attributes::LunCoSimAttributesPlugin;
use lunco_sim_avatar::{Avatar, LunCoSimAvatarPlugin};
use lunco_sim_controller::{LunCoSimControllerPlugin, SpaceSystemAction};
use lunco_sim_fsw::LunCoSimFswPlugin;
use lunco_sim_obc::LunCoSimObcPlugin;
use lunco_sim_physics::{
    spawn_joint_ackermann_rover, spawn_joint_skid_rover, LunCoSimPhysicsPlugin, MotorActuator,
};
use lunco_sim_rover_raycast::{
    spawn_raycast_ackermann_rover, spawn_raycast_skid_rover, LunCoSimRoverRaycastPlugin,
};
mod blueprint_extension;
mod rover_counter;
use blueprint_extension::{BlueprintExtension, BlueprintMaterial};
use rover_counter::RoverCountPlugin;

fn main() {
    App::new()
        .insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(SubstepCount(8))
        .add_plugins(DefaultPlugins)
        .add_plugins(PhysicsPlugins::default()) // Avian3D
        // LunCo Modules
        .add_plugins(lunco_sim_core::LunCoSimCorePlugin)
        .add_plugins(LunCoSimPhysicsPlugin)
        .add_plugins(LunCoSimFswPlugin)
        .add_plugins(LunCoSimObcPlugin)
        .add_plugins(LunCoSimControllerPlugin)
        .add_plugins(LunCoSimAttributesPlugin)
        .add_plugins(LunCoSimAvatarPlugin)
        .add_plugins(LunCoSimRoverRaycastPlugin)
        .add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(RoverCountPlugin)
        .add_systems(Startup, setup_scenario)
        .add_systems(Update, draw_wheel_diagnostics)
        .run();
}

/// Visual Diagnostic
fn draw_wheel_diagnostics(
    mut gizmos: Gizmos,
    q_wheels: Query<(&GlobalTransform, &MotorActuator)>,
    q_joints: Query<(&RevoluteJoint, &GlobalTransform)>,
) {
    for (tf, _) in q_wheels.iter() {
        let pos = tf.translation();
        gizmos.line(
            pos,
            pos + tf.right().as_vec3() * 2.0,
            Color::srgb(1.0, 0.0, 0.0),
        );
        gizmos.line(
            pos,
            pos + tf.up().as_vec3() * 2.5,
            Color::srgb(0.0, 1.0, 0.0),
        );
        gizmos.line(
            pos,
            pos + tf.forward().as_vec3() * 2.0,
            Color::srgb(0.0, 0.0, 1.0),
        );
    }

    for (joint, tf) in q_joints.iter() {
        let world_hinge = tf.rotation() * joint.hinge_axis.as_vec3();
        let pos = tf.translation();
        gizmos.line(pos, pos + world_hinge * 2.5, Color::WHITE);
    }
}

fn setup_scenario(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut blueprint_materials: ResMut<Assets<BlueprintMaterial>>,
) {
    // 1. Massive Ground Plane (1000m)
    let ground_size = 1000.0;
    commands.spawn((
        Name::new("ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(ground_size, ground_size))),
        MeshMaterial3d(blueprint_materials.add(BlueprintMaterial {
            base: StandardMaterial {
                base_color: Color::srgb(0.02, 0.1, 0.3), // Blueprint Blue
                perceptual_roughness: 0.9,
                ..default()
            },
            extension: BlueprintExtension {
                line_color: LinearRgba::new(0.4, 0.8, 1.0, 1.0),
                grid_scale: 10.0,
                line_width: 2.0,
                _padding: Vec2::ZERO,
            },
        })),
        RigidBody::Static,
        Collider::cuboid(ground_size as f64, 0.1_f64, ground_size as f64),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    // 2. Training Ramp (Using exact same blueprint look)
    let ramp_size = Vec3::new(12.0, 1.0, 6.0);
    commands.spawn((
        Name::new("ramp"),
        Mesh3d(meshes.add(Cuboid::from_size(ramp_size))),
        MeshMaterial3d(blueprint_materials.add(BlueprintMaterial {
            base: StandardMaterial {
                base_color: Color::srgb(0.1, 0.3, 0.6), // Lighter Blue
                perceptual_roughness: 0.9,
                ..default()
            },
            extension: BlueprintExtension {
                line_color: LinearRgba::new(0.6, 0.9, 1.0, 1.0),
                grid_scale: 2.0,
                line_width: 1.0,
                _padding: Vec2::ZERO,
            },
        })),
        RigidBody::Static,
        Collider::cuboid(ramp_size.x as f64, ramp_size.y as f64, ramp_size.z as f64),
        Transform::from_xyz(0.0, 0.3, 40.0)
            .with_rotation(Quat::from_rotation_x(-15.0_f32.to_radians())),
    ));

    let wheel_radius = 0.5;
    let wheel_width = 0.4;
    let wheel_mesh = meshes.add(Cylinder::new(wheel_radius, wheel_width));

    // 2. Raycast Variants
    spawn_raycast_skid_rover(
        &mut commands,
        &mut *meshes,
        &mut *materials,
        wheel_mesh.clone(),
        Vec3::new(4.0, 1.0, 0.0),
        "Raycast Skid (R-S)",
        Color::srgb(0.0, 0.8, 0.4),
    );
    spawn_raycast_ackermann_rover(
        &mut commands,
        &mut *meshes,
        &mut *materials,
        wheel_mesh.clone(),
        Vec3::new(10.0, 1.0, 0.0),
        "Raycast Ackermann (R-A)",
        Color::srgb(0.0, 0.4, 0.8),
    );

    // 3. Joint Variants
    spawn_joint_skid_rover(
        &mut commands,
        &mut *meshes,
        &mut *materials,
        wheel_mesh.clone(),
        Vec3::new(-4.0, 1.0, 0.0),
        "Joint Skid (J-S)",
        Color::srgb(0.8, 0.4, 0.0),
    );
    spawn_joint_ackermann_rover(
        &mut commands,
        &mut *meshes,
        &mut *materials,
        wheel_mesh.clone(),
        Vec3::new(-10.0, 1.0, 0.0),
        "Joint Ackermann (J-A)",
        Color::srgb(0.9, 0.1, 0.4),
    );

    // 4. Environment
    commands.spawn((
        DirectionalLight {
            illuminance: 10000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(10.0, 20.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // 5. Avatar
    let mut input_map = InputMap::default();
    input_map.insert(SpaceSystemAction::DriveForward, KeyCode::KeyW);
    input_map.insert(SpaceSystemAction::DriveReverse, KeyCode::KeyS);
    input_map.insert(SpaceSystemAction::SteerLeft, KeyCode::KeyA);
    input_map.insert(SpaceSystemAction::SteerRight, KeyCode::KeyD);
    input_map.insert(SpaceSystemAction::Brake, KeyCode::Space);
    commands.spawn((
        Avatar,
        Camera3d::default(),
        AmbientLight {
            brightness: 100.0,
            ..default()
        },
        Transform::from_xyz(5.0, 10.0, -25.0).looking_at(Vec3::new(0.0, 0.0, 10.0), Vec3::Y),
        ActionState::<SpaceSystemAction>::default(),
        input_map,
    ));
}
