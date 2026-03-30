use avian3d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

use lunco_sim_attributes::LunCoSimAttributesPlugin;
use lunco_sim_avatar::{Avatar, LunCoSimAvatarPlugin};
use lunco_sim_controller::{LunCoSimControllerPlugin, SpaceSystemAction};
use lunco_sim_fsw::LunCoSimFswPlugin;
use lunco_sim_obc::LunCoSimObcPlugin;
use lunco_sim_physics::{spawn_joint_rover, LunCoSimPhysicsPlugin, MotorActuator};
use lunco_sim_rover_raycast::{spawn_raycast_rover, LunCoSimRoverRaycastPlugin};

fn main() {
    App::new()
        .insert_resource(Time::<Fixed>::from_hz(256.0))
        // .insert_resource(Substeps(8))
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
        let world_hinge = tf.rotation() * joint.hinge_axis;
        let pos = tf.translation();
        gizmos.line(pos, pos + world_hinge * 2.5, Color::WHITE);
    }
}

fn setup_scenario(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // 1. Ground Plane
    commands.spawn((
        Name::new("ground"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(100.0, 100.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.2, 0.2, 0.2))),
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Transform::from_xyz(0.0, -0.05, 0.0),
    ));

    let wheel_radius = 0.5;
    let wheel_width = 0.4;
    let wheel_mesh = meshes.add(Cylinder::new(wheel_radius, wheel_width));

    // 2. Spawn New Raycast Rover
    spawn_raycast_rover(
        &mut commands,
        &mut *meshes,
        &mut *materials,
        wheel_mesh.clone(),
        Vec3::new(3.0, 1.0, 0.0),
        "Raycast Rover (New)",
        Color::srgb(0.0, 0.8, 0.4),
    );

    // 3. Spawn Old Joint-based Rover
    spawn_joint_rover(
        &mut commands,
        &mut *meshes,
        &mut *materials,
        wheel_mesh,
        Vec3::new(-3.0, 1.0, 0.0),
        "Joint Rover (Old)",
        Color::srgb(0.8, 0.4, 0.0),
    );

    // 4. Environment
    commands.spawn((
        DirectionalLight::default(),
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
        Transform::from_xyz(10.0, 10.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
        ActionState::<SpaceSystemAction>::default(),
        input_map,
    ));
}
