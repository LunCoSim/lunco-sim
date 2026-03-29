use bevy::prelude::*;
use avian3d::prelude::*;
use leafwing_input_manager::prelude::*;

use lunco_sim_physics::{LunCoSimPhysicsPlugin, MotorActuator};
use lunco_sim_controller::{LunCoSimControllerPlugin, SpaceSystemAction, ControllerLink};
use lunco_sim_core::architecture::{PhysicalPort, DigitalPort, Wire};
use lunco_sim_obc::LunCoSimObcPlugin;
use lunco_sim_fsw::LunCoSimFswPlugin;
use lunco_sim_attributes::LunCoSimAttributesPlugin;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(PhysicsPlugins::default())
        .add_plugins(PhysicsDebugPlugin::default())
        .add_plugins(LunCoSimObcPlugin)
        .add_plugins(LunCoSimFswPlugin)
        .add_plugins(LunCoSimPhysicsPlugin)
        .add_plugins(LunCoSimControllerPlugin)
        .add_plugins(LunCoSimAttributesPlugin)
        .add_systems(Startup, setup_scene)
        .run();
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // 1. Camera & Light
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 5.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_4)),
    ));

    // 2. Static Ground
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(50.0, 50.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.5, 0.3))),
        RigidBody::Static,
        Collider::half_space(Vec3::Y),
    ));

    // 3. Rover Body
    let rover_mesh = meshes.add(Cuboid::new(2.0, 1.0, 4.0));
    let rover_mat = materials.add(Color::srgb(0.8, 0.7, 0.6));
    
    let motor_port = commands.spawn(PhysicalPort { value: 0.0 }).id();
    let steer_port = commands.spawn(PhysicalPort { value: 0.0 }).id();
    let digital_drive = commands.spawn(DigitalPort::default()).id();
    let digital_steer = commands.spawn(DigitalPort::default()).id();

    // Wires linking OBC logic registers down to physical arrays
    commands.spawn(Wire { source: digital_drive, target: motor_port, scale: 50.0 });
    commands.spawn(Wire { source: digital_steer, target: steer_port, scale: 15.0 });

    let mut input_map = InputMap::default();
    input_map.insert(SpaceSystemAction::DriveForward, KeyCode::KeyW);
    input_map.insert(SpaceSystemAction::DriveReverse, KeyCode::KeyS);
    input_map.insert(SpaceSystemAction::SteerLeft, KeyCode::KeyA);
    input_map.insert(SpaceSystemAction::SteerRight, KeyCode::KeyD);
    input_map.insert(SpaceSystemAction::Brake, KeyCode::Space);

    let rover = commands.spawn((
        Mesh3d(rover_mesh),
        MeshMaterial3d(rover_mat),
        Transform::from_xyz(0.0, 2.0, 0.0),
        RigidBody::Dynamic,
        Collider::cuboid(2.0, 1.0, 4.0),
        ConstantTorque(Vec3::ZERO),
        MotorActuator {
            port_entity: motor_port,
            axis: Vec3::Z, // Drive forward along Z
        },
        ActionState::<SpaceSystemAction>::default(),
        input_map,
    )).id();

    // Link the Controller logic to the vessel logic
    commands.entity(rover).insert(ControllerLink { vessel_entity: rover });
}
