use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_sim_core::architecture::{PhysicalPort, DigitalPort, Wire};
use lunco_sim_core::{Vessel, RoverVessel};
use lunco_sim_fsw::FlightSoftware;
use std::collections::HashMap;

pub struct LunCoSimPhysicsPlugin;

impl Plugin for LunCoSimPhysicsPlugin {
    fn build(&self, app: &mut App) {
        // Avian physics pipeline integration
        app.add_systems(Update, (apply_motor_torques, apply_brakes, update_physics_sensors));
    }
}

/// Link an abstract `PhysicalPort` into a concrete `RigidBody` torque actuator.
/// Maps the f32 scalar to a specific 3D axis.
#[derive(Component)]
pub struct MotorActuator {
    /// The physical port entity supplying the torque value.
    pub port_entity: Entity,
    /// The unit axis (in local space) the torque is applied upon.
    pub axis: Vec3,
}

/// Applies Tier 1 Plant hardware scaled forces directly into our physics loop.
/// Important: We use ConstantLocalTorque to ensure the torque is relative 
/// to the wheel's orientation, not world-space.
fn apply_motor_torques(
    q_ports: Query<&PhysicalPort>,
    mut q_motors: Query<(&MotorActuator, &mut ConstantLocalTorque)>,
) {
    for (motor, mut external_torque) in q_motors.iter_mut() {
        if let Ok(port) = q_ports.get(motor.port_entity) {
            // Apply torque along the local axis defined in the component
            external_torque.0 = motor.axis * port.value;
        }
    }
}

/// Applies physical braking torque/damping to a wheel or body.
#[derive(Component)]
pub struct BrakeActuator {
    pub port_entity: Entity,
    pub max_force: f32,
}

/// Applies resistance torque proportional to the brake port value.
fn apply_brakes(
    q_ports: Query<&PhysicalPort>,
    mut q_brakes: Query<(&BrakeActuator, &mut AngularVelocity, &mut LinearVelocity)>,
) {
    for (brake, mut ang_vel, mut lin_vel) in q_brakes.iter_mut() {
        if let Ok(port) = q_ports.get(brake.port_entity) {
            // Apply braking as a significant reduction in velocity (damping)
            // Ideally this should apply a torque, but for stability we scale velocity decay
            let brake_factor = (1.0 - (port.value / brake.max_force).clamp(0.0, 1.0)).powf(2.0);
            
            // Apply brake to both linear and angular (for wheel stop and slide stop)
            ang_vel.0 *= brake_factor;
            lin_vel.0 *= brake_factor;
        }
    }
}

/// Generic sensor that copies RigidBody state to a generic sensor `PhysicalPort`
#[derive(Component)]
pub struct AngularVelocitySensor {
    pub port_entity: Entity,
    pub axis: Vec3,
}

/// Feeds back actual simulation state (L1) back into the physical pipeline for the OBC to sense (ADC path)
fn update_physics_sensors(
    q_sensors: Query<(&AngularVelocitySensor, &AngularVelocity)>,
    mut q_ports: Query<&mut PhysicalPort>,
) {
    for (sensor, velocity) in q_sensors.iter() {
        if let Ok(mut port) = q_ports.get_mut(sensor.port_entity) {
            // Project the 3D angular velocity onto our 1D measurement axis
            port.value = velocity.0.dot(sensor.axis) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_motor_actuator_integration() {
        let mut app = App::new();
        app.add_plugins(LunCoSimPhysicsPlugin);

        let port_id = app.world_mut().spawn(PhysicalPort { value: 15.0 }).id();
        let body_id = app.world_mut().spawn((
            RigidBody::Dynamic,
            ConstantLocalTorque(Vec3::ZERO),
            MotorActuator {
                port_entity: port_id,
                axis: Vec3::Y,
            }
        )).id();

        app.update();

        // 15 Nm applied down the LOCAL Y axis
        let torque = app.world().get::<ConstantLocalTorque>(body_id).unwrap();
        assert_eq!(torque.0.y, 15.0);
    }
}

#[derive(PhysicsLayer, Default)]
pub enum Layer {
    #[default]
    Default,
    RoverChassis,
    RoverWheel,
}

/// Internal shared logic for Joint-based Rover variants.
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

    let rover_entity = commands.spawn((
        Name::new(name.to_string()),
        RoverVessel,
        Vessel,
        Mesh3d(meshes.add(Cuboid::new(chassis_width, chassis_height, chassis_length))),
        MeshMaterial3d(materials.add(color)),
        Transform::from_translation(spawn_pos),
        RigidBody::Dynamic,
        Collider::cuboid(chassis_width, chassis_height, chassis_length),
        CollisionLayers::new(Layer::RoverChassis, [Layer::Default]),
        Friction::new(0.5),
        Mass(1000.0), 
        CenterOfMass(Vec3::new(0.0, -0.8, 0.0)), // Extremely low CoM
        LinearDamping(0.5),
        AngularDamping(1.0),
        AngularInertia::new(Vec3::new(5000.0, 5000.0, 2000.0)), 
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

    let f_mat = materials.add(Color::srgb(0.9, 0.1, 0.4)); 
    let r_mat = materials.add(Color::srgb(0.1, 0.4, 0.8)); 

    let wheel_configs = [
        ("fl", Vec3::new(-1.2, -0.4, -1.2), true, true), // -Z is Front
        ("rl", Vec3::new(-1.2, -0.4, 1.2), true, false), // +Z is Rear
        ("fr", Vec3::new(1.2, -0.4, -1.2), false, true),
        ("rr", Vec3::new(1.2, -0.4, 1.2), false, false),
    ];

    let steer_port = commands.spawn((Name::new(format!("{}_port_steer", name)), PhysicalPort::default())).id();
    // Increase scale to 6k to handle friction, and flip sign (-1.0 for Right-turn torque)
    commands.spawn(Wire { source: steer_digital, target: steer_port, scale: -6000.0 });

    let wheel_tilt = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);

    for (label, rel_pos, is_left, is_front) in wheel_configs {
        let mat = if is_front { f_mat.clone() } else { r_mat.clone() };
        let digital_source = if is_left { drive_l_digital } else { drive_r_digital };
        
        let motor_port = commands.spawn((Name::new(format!("{}_port_{}_drive", name, label)), PhysicalPort::default())).id();
        let brake_port = commands.spawn((Name::new(format!("{}_port_{}_brake", name, label)), PhysicalPort::default())).id();
        commands.spawn(Wire { source: digital_source, target: motor_port, scale: 6000.0 });
        commands.spawn(Wire { source: brake_digital, target: brake_port, scale: 32767.0 });

        let wheel_entity = commands.spawn((
            Name::new(format!("{}_wheel_{}", name, label)),
            Mesh3d(wheel_mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(spawn_pos + rel_pos).with_rotation(wheel_tilt),
            RigidBody::Dynamic,
            Collider::cylinder(wheel_radius, wheel_width),
            CollisionLayers::new(Layer::RoverWheel, [Layer::Default]),
            Friction::new(0.8), Mass(40.0), LinearDamping(0.5), AngularDamping(0.5),
            ConstantLocalTorque(Vec3::ZERO),
            MotorActuator { port_entity: motor_port, axis: Vec3::Y },
            BrakeActuator { port_entity: brake_port, max_force: 32767.0 },
        )).id();

        if is_front && steering_type == SteeringType::Ackermann {
            let hub_entity = commands.spawn((
                Name::new(format!("{}_hub_{}", name, label)),
                RigidBody::Dynamic, Mass(10.0), Transform::from_translation(spawn_pos + rel_pos),
                ConstantLocalTorque(Vec3::ZERO),
                MotorActuator { port_entity: steer_port, axis: Vec3::Y },
            )).id();

            commands.spawn(RevoluteJoint::new(rover_entity, hub_entity).with_local_anchor1(rel_pos).with_local_anchor2(Vec3::ZERO).with_hinge_axis(Vec3::Y).with_angle_limits(-0.6, 0.6));
            commands.spawn(RevoluteJoint::new(hub_entity, wheel_entity).with_local_anchor1(Vec3::ZERO).with_local_anchor2(Vec3::ZERO).with_hinge_axis(Vec3::X).with_local_basis2(wheel_tilt.inverse()));
        } else {
            commands.spawn(RevoluteJoint::new(rover_entity, wheel_entity).with_local_anchor1(rel_pos).with_local_anchor2(Vec3::ZERO).with_hinge_axis(Vec3::X).with_local_basis2(wheel_tilt.inverse()));
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
