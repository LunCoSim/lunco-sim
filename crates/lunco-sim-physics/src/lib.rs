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
        app.add_systems(Update, (apply_motor_torques, update_physics_sensors));
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

/// Blueprint for assembling a legacy Joint-based Rover.
pub fn spawn_joint_rover(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    wheel_mesh: Handle<Mesh>,
    spawn_pos: Vec3,
    name: &str,
    color: Color,
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
        CollisionLayers::new(Layer::RoverChassis, [Layer::Default]), // Only collide with world
        Friction::new(0.5),
        Mass(1000.0), 
        CenterOfMass(Vec3::new(0.0, -0.4, 0.0)), 
        LinearDamping(0.2),
        AngularDamping(0.3),
        AngularInertia::new(Vec3::new(5000.0, 5000.0, 2000.0)), 
    )).id();

    let drive_l_digital = commands.spawn((Name::new(format!("{}_drive_l_reg", name)), DigitalPort::default())).id();
    let drive_r_digital = commands.spawn((Name::new(format!("{}_drive_r_reg", name)), DigitalPort::default())).id();

    let mut port_map = HashMap::new();
    port_map.insert("drive_left".to_string(), drive_l_digital);
    port_map.insert("drive_right".to_string(), drive_r_digital);
    commands.entity(rover_entity).insert(FlightSoftware { port_map });

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
        let motor_port = commands.spawn((Name::new(format!("{}_port_{}", name, label)), PhysicalPort::default())).id();
        commands.spawn(Wire { source: digital_source, target: motor_port, scale: 5000.0 });

        let wheel_entity = commands.spawn((
            Name::new(format!("{}_wheel_{}", name, label)),
            Mesh3d(wheel_mesh.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(spawn_pos + rel_pos).with_rotation(wheel_tilt),
            RigidBody::Dynamic,
            Collider::cylinder(wheel_radius, wheel_width),
            CollisionLayers::new(Layer::RoverWheel, [Layer::Default]), // Only collide with world
            Friction::new(5.0), 
            Mass(20.0),
            ConstantLocalTorque(Vec3::ZERO),
            MotorActuator {
                port_entity: motor_port,
                axis: Vec3::Y, 
            },
        )).id();

        commands.spawn((
            Name::new(format!("{}_joint_{}", name, label)),
            RevoluteJoint::new(rover_entity, wheel_entity)
                .with_local_anchor1(rel_pos)
                .with_local_anchor2(Vec3::ZERO)
                .with_hinge_axis(Vec3::X) 
                .with_local_basis2(wheel_tilt.inverse()) 
        ));
    }

    rover_entity
}
