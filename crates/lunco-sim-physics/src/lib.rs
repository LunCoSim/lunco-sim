use bevy::prelude::*;
use bevy::math::{DVec3, DQuat};
use avian3d::prelude::*;
use lunco_sim_core::architecture::{PhysicalPort, DigitalPort, Wire};
use lunco_sim_core::{Vessel, RoverVessel};
use lunco_sim_fsw::FlightSoftware;
use std::collections::HashMap;

pub struct LunCoSimPhysicsPlugin;

impl Plugin for LunCoSimPhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, (
            apply_motor_torques, 
            apply_brakes, 
            update_physics_sensors
        ).chain().before(PhysicsSystems::Prepare));
    }
}

#[derive(Component)]
pub struct MotorActuator {
    pub port_entity: Entity,
    pub axis: Vec3,
}

fn apply_motor_torques(
    q_ports: Query<&PhysicalPort>,
    mut q_motors: Query<(&MotorActuator, Forces)>,
) {
    for (motor, mut forces) in q_motors.iter_mut() {
        if let Ok(port) = q_ports.get(motor.port_entity) {
            let torque_mag = port.value as f64;
            forces.apply_local_torque(motor.axis.as_dvec3() * torque_mag);
        }
    }
}

#[derive(Component)]
pub struct BrakeActuator {
    pub port_entity: Entity,
    pub max_force: f32,
}

fn apply_brakes(
    q_ports: Query<&PhysicalPort>,
    mut q_brakes: Query<(&BrakeActuator, &mut AngularVelocity, &mut LinearVelocity)>,
) {
    for (brake, mut ang_vel, mut lin_vel) in q_brakes.iter_mut() {
        if let Ok(port) = q_ports.get(brake.port_entity) {
            let brake_factor = (1.0 - (port.value / brake.max_force).clamp(0.0, 1.0)).powf(2.0) as f64;
            ang_vel.0 *= brake_factor;
            lin_vel.0 *= brake_factor;
        }
    }
}

#[derive(Component)]
pub struct AngularVelocitySensor {
    pub port_entity: Entity,
    pub axis: Vec3,
}

fn update_physics_sensors(
    q_sensors: Query<(&AngularVelocitySensor, &AngularVelocity)>,
    mut q_ports: Query<&mut PhysicalPort>,
) {
    for (sensor, velocity) in q_sensors.iter() {
        if let Ok(mut port) = q_ports.get_mut(sensor.port_entity) {
            port.value = velocity.0.dot(sensor.axis.as_dvec3()) as f32;
        }
    }
}

#[derive(PhysicsLayer, Default)]
pub enum Layer {
    #[default]
    Default,
    RoverChassis,
    RoverWheel,
}

fn spawn_joint_rover_internal(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
    steering_type: SteeringType,
) -> Entity {
    let chassis_width = 1.8;
    let chassis_height = 0.5;
    let chassis_length = 3.0;
    let wheel_radius = 0.5;
    let wheel_width = 0.4;

    let red_material = materials.add(StandardMaterial { base_color: Color::from(Srgba::RED), perceptual_roughness: 0.5, ..default() });
    let blue_material = materials.add(StandardMaterial { base_color: Color::from(Srgba::BLUE), perceptual_roughness: 0.5, ..default() });

    let rover_entity = commands.spawn((
        Name::new(name.to_string()),
        RoverVessel,
        Vessel,
        Mesh3d(meshes.add(Cuboid::new(chassis_width, chassis_height, chassis_length))),
        MeshMaterial3d(materials.add(color)),
        Transform::from_translation(spawn_pos),
        RigidBody::Dynamic,
        Collider::cuboid(chassis_width as f64, chassis_height as f64, chassis_length as f64),
        CollisionLayers::new(Layer::RoverChassis, [Layer::Default]),
        Friction::new(0.5),
        Mass(1000.0), 
        CenterOfMass(Vec3::new(0.0, -0.2, 0.0)),
        LinearDamping(0.2), 
        AngularDamping(0.5),
    )).id();

    let drive_l_digital = commands.spawn((Name::new(format!("{}_drive_l_reg", name)), DigitalPort::default())).id();
    let drive_r_digital = commands.spawn((Name::new(format!("{}_drive_r_reg", name)), DigitalPort::default())).id();
    let steer_digital = commands.spawn((Name::new(format!("{}_steer_reg", name)), DigitalPort::default())).id();
    let brake_digital = commands.spawn((Name::new(format!("{}_brake_reg", name)), DigitalPort::default())).id();

    let mut port_map = HashMap::new();
    port_map.insert("drive_left".to_string(), drive_l_digital);
    port_map.insert("drive_right".to_string(), drive_r_digital);
    port_map.insert("steering".to_string(), steer_digital);
    port_map.insert("brake".to_string(), brake_digital);
    commands.entity(rover_entity).insert(FlightSoftware { port_map, brake_active: false });

    let wheel_offset_y = -0.1;
    let wheel_configs = [
        ("fl", Vec3::new(-1.2, wheel_offset_y, 1.2), true, true), 
        ("rl", Vec3::new(-1.2, wheel_offset_y, -1.2), true, false), 
        ("fr", Vec3::new(1.2, wheel_offset_y, 1.2), false, true),
        ("rr", Vec3::new(1.2, wheel_offset_y, -1.2), false, false),
    ];

    let steer_port = commands.spawn((Name::new(format!("{}_port_steer", name)), PhysicalPort::default())).id();
    commands.spawn(Wire { source: steer_digital, target: steer_port, scale: 0.1 });

    let wheel_tilt = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let wheel_tilt_d = DQuat::from_xyzw(
        wheel_tilt.x as f64,
        wheel_tilt.y as f64,
        wheel_tilt.z as f64,
        wheel_tilt.w as f64,
    );

    for (label, rel_pos, is_left, is_front) in wheel_configs {
        let digital_source = if is_left { drive_l_digital } else { drive_r_digital };
        
        let motor_port = commands.spawn((Name::new(format!("{}_port_{}_drive", name, label)), PhysicalPort::default())).id();
        let brake_port = commands.spawn((Name::new(format!("{}_port_{}_brake", name, label)), PhysicalPort::default())).id();
        commands.spawn(Wire { source: digital_source, target: motor_port, scale: 0.2 });
        commands.spawn(Wire { source: brake_digital, target: brake_port, scale: 1.0 });

        let wheel_material = if is_front { red_material.clone() } else { blue_material.clone() };

        let wheel_entity = commands.spawn((
            Name::new(format!("{}_wheel_{}", name, label)),
            Mesh3d(wheel_mesh.clone()),
            MeshMaterial3d(wheel_material),
            Transform::from_translation(spawn_pos + rel_pos).with_rotation(wheel_tilt),
            RigidBody::Dynamic,
            Collider::cylinder(wheel_radius as f64, wheel_width as f64),
            CollisionLayers::new(Layer::RoverWheel, [Layer::Default]),
            Friction::new(0.8), 
            Mass(40.0), 
            LinearDamping(0.5), 
            AngularDamping(0.5),
            MotorActuator { port_entity: motor_port, axis: Vec3::Y },
            BrakeActuator { port_entity: brake_port, max_force: 32767.0 },
        )).id();

        if is_front && steering_type == SteeringType::Ackermann {
            let hub_entity = commands.spawn((
                Name::new(format!("{}_hub_{}", name, label)),
                RigidBody::Dynamic, 
                Mass(10.0), 
                Collider::sphere(0.05),
                Transform::from_translation(spawn_pos + rel_pos),
                MotorActuator { port_entity: steer_port, axis: Vec3::Y },
            )).id();

            commands.spawn(RevoluteJoint::new(rover_entity, hub_entity)
                .with_local_anchor1(rel_pos.as_dvec3())
                .with_local_anchor2(DVec3::ZERO)
                .with_hinge_axis(DVec3::Y)
                .with_angle_limits(-0.6, 0.6));
                
            commands.spawn(RevoluteJoint::new(hub_entity, wheel_entity)
                .with_local_anchor1(DVec3::ZERO)
                .with_local_anchor2(DVec3::ZERO)
                .with_hinge_axis(DVec3::X)
                .with_local_basis2(wheel_tilt_d.inverse()));
        } else {
            commands.spawn(RevoluteJoint::new(rover_entity, wheel_entity)
                .with_local_anchor1(rel_pos.as_dvec3())
                .with_local_anchor2(DVec3::ZERO)
                .with_hinge_axis(DVec3::X)
                .with_local_basis2(wheel_tilt_d.inverse()));
        }
    }
    rover_entity
}

#[derive(PartialEq, Eq)]
pub enum SteeringType { Skid, Ackermann }

pub fn spawn_joint_skid_rover(commands: &mut Commands, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, wheel_mesh: Handle<Mesh>, spawn_pos: Vec3, name: &str, color: Color) -> Entity {
    spawn_joint_rover_internal(commands, meshes, materials, wheel_mesh, spawn_pos, name, color, SteeringType::Skid)
}

pub fn spawn_joint_ackermann_rover(commands: &mut Commands, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, wheel_mesh: Handle<Mesh>, spawn_pos: Vec3, name: &str, color: Color) -> Entity {
    spawn_joint_rover_internal(commands, meshes, materials, wheel_mesh, spawn_pos, name, color, SteeringType::Ackermann)
}
