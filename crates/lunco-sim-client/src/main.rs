use bevy::prelude::*;
use avian3d::prelude::*;
use leafwing_input_manager::prelude::*;

use lunco_sim_core::architecture::{DigitalPort, PhysicalPort, Wire};
use lunco_sim_physics::{LunCoSimPhysicsPlugin, MotorActuator};
use lunco_sim_fsw::LunCoSimFswPlugin;
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
        .run();
}

#[derive(Component)]
pub struct RoverVessel;

fn setup_scenario(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // 1. Ground Plane
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(100.0, 100.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.2, 0.2, 0.2))),
        RigidBody::Static,
        Collider::half_space(Vec3::Y),
    ));

    // 2. The Rover (Space System)
    let rover_entity = commands.spawn((
        Name::new("rover_v1"),
        RoverVessel,
        Vessel, // Marker for possession
        Mesh3d(meshes.add(Cuboid::new(2.0, 1.0, 3.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.8, 0.4, 0.0))),
        Transform::from_xyz(0.0, 2.0, 0.0),
        RigidBody::Dynamic,
        Collider::cuboid(2.0, 1.0, 3.0),
    )).id();

    // 3. Ports & Wires
    let motor_port = commands.spawn(PhysicalPort { value: 0.0 }).id();
    let digital_drive = commands.spawn(DigitalPort::default()).id();

    commands.spawn(Wire { 
        source: digital_drive, 
        target: motor_port, 
        scale: 50.0 
    });

    // Link Actuator to Port
    commands.entity(rover_entity).insert(MotorActuator {
        port_entity: motor_port,
        axis: Vec3::Z,
    });

    // 4. Light
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(10.0, 20.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // 5. Avatar (The Human Pilot)
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
