use bevy::prelude::*;
use avian3d::prelude::*;
use leafwing_input_manager::prelude::*;
use std::collections::HashMap;

use lunco_sim_core::architecture::{DigitalPort, PhysicalPort, Wire, CommandMessage};
use lunco_sim_physics::{LunCoSimPhysicsPlugin, MotorActuator};
use lunco_sim_fsw::{LunCoSimFswPlugin, FlightSoftware};
use lunco_sim_obc::LunCoSimObcPlugin;
use lunco_sim_controller::{LunCoSimControllerPlugin, SpaceSystemAction};
use lunco_sim_attributes::LunCoSimAttributesPlugin;
use lunco_sim_avatar::{LunCoSimAvatarPlugin, Avatar, Vessel};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(PhysicsPlugins::default()) // Avian3D
        // LunCo Modules
        .add_plugins(LunCoSimPhysicsPlugin)
        .add_plugins(LunCoSimFswPlugin)
        .add_plugins(LunCoSimObcPlugin)
        .add_plugins(LunCoSimControllerPlugin)
        .add_plugins(LunCoSimAttributesPlugin)
        .add_plugins(LunCoSimAvatarPlugin)
        .add_systems(Startup, setup_scenario)
        .add_systems(Update, draw_wheel_diagnostics)
        .run();
}

#[derive(Component)]
pub struct RoverVessel;

/// Visual Diagnostic
fn draw_wheel_diagnostics(
    mut gizmos: Gizmos,
    q_wheels: Query<(&GlobalTransform, &MotorActuator)>,
    q_joints: Query<(&RevoluteJoint, &GlobalTransform)>,
) {
    for (tf, _) in q_wheels.iter() {
        let pos = tf.translation();
        gizmos.line(pos, pos + tf.right() * 2.0, Color::srgb(1.0, 0.0, 0.0)); 
        gizmos.line(pos, pos + tf.up() * 2.5, Color::srgb(0.0, 1.0, 0.0));    
        gizmos.line(pos, pos + tf.forward() * 2.0, Color::srgb(0.0, 0.0, 1.0)); 
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

    // 2. The Rover Chassis (STABILIZED)
    let chassis_width = 1.8;
    let chassis_height = 0.5;
    let chassis_length = 3.0;
    let rover_spawn_pos = Vec3::new(0.0, 1.0, 0.0);
    
    let rover_entity = commands.spawn((
        Name::new("rover_chassis"),
        RoverVessel,
        Vessel,
        Mesh3d(meshes.add(Cuboid::new(chassis_width, chassis_height, chassis_length))),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.4, 0.0))),
        Transform::from_translation(rover_spawn_pos),
        RigidBody::Dynamic,
        Collider::cuboid(chassis_width, chassis_height, chassis_length),
        Friction::new(0.5),
        Mass(1000.0), 
        CenterOfMass(Vec3::new(0.0, -0.4, 0.0)), 
        LinearDamping(0.2),
        AngularDamping(0.3),
        AngularInertia::new(Vec3::new(5000.0, 5000.0, 2000.0)), 
    )).id();

    // 3. Digital Channels
    let drive_l_digital = commands.spawn((Name::new("drive_l_reg"), DigitalPort::default())).id();
    let drive_r_digital = commands.spawn((Name::new("drive_r_reg"), DigitalPort::default())).id();

    // 4. FSW
    let mut port_map = HashMap::new();
    port_map.insert("drive_left".to_string(), drive_l_digital);
    port_map.insert("drive_right".to_string(), drive_r_digital);
    commands.entity(rover_entity).insert(FlightSoftware { port_map });

    // 5. Wheels
    let wheel_radius = 0.5;
    let wheel_width = 0.4;
    let wheel_mesh = meshes.add(Cylinder::new(wheel_radius, wheel_width));

    let f_mat = materials.add(Color::srgb(0.9, 0.1, 0.4)); 
    let r_mat = materials.add(Color::srgb(0.1, 0.4, 0.8)); 

    let wheel_configs = [
        ("fl", Vec3::new(-1.2, -0.4, 1.2), drive_l_digital, f_mat.clone()),
        ("rl", Vec3::new(-1.2, -0.4, -1.2), drive_l_digital, r_mat.clone()),
        ("fr", Vec3::new(1.2, -0.4, 1.2), drive_r_digital, f_mat.clone()),
        ("rr", Vec3::new(1.2, -0.4, -1.2), drive_r_digital, r_mat.clone()),
    ];

    let wheel_tilt = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);

    for (label, rel_pos, digital_source, mat) in wheel_configs {
        let motor_port = commands.spawn((Name::new(format!("port_{}", label)), PhysicalPort::default())).id();
        commands.spawn(Wire { source: digital_source, target: motor_port, scale: 5000.0 });

        let wheel_entity = commands.spawn((
            Name::new(format!("wheel_{}", label)),
            Mesh3d(wheel_mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(rover_spawn_pos + rel_pos).with_rotation(wheel_tilt),
            RigidBody::Dynamic,
            Collider::cylinder(wheel_radius, wheel_width),
            Friction::new(5.0), 
            Mass(20.0),
            ConstantLocalTorque(Vec3::ZERO),
            MotorActuator {
                port_entity: motor_port,
                axis: Vec3::Y, 
            },
        )).id();

        commands.spawn((
            Name::new(format!("joint_{}", label)),
            RevoluteJoint::new(rover_entity, wheel_entity)
                .with_local_anchor1(rel_pos)
                .with_local_anchor2(Vec3::ZERO)
                .with_hinge_axis(Vec3::X) 
                .with_local_basis2(wheel_tilt.inverse()) 
        ));
    }

    // 6. Environment
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(10.0, 20.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // 7. Avatar
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
