use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
use lunco_core::architecture::PhysicalPort;

pub struct LunCoHardwarePlugin;

impl Plugin for LunCoHardwarePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<MotorActuator>()
           .register_type::<BrakeActuator>()
           .register_type::<AngularVelocitySensor>()
           .add_systems(FixedUpdate, (
               motor_actuator_system,
               brake_actuator_system,
               sensor_velocity_system,
           ).chain().run_if(|tw: Res<lunco_core::TimeWarpState>| tw.physics_enabled));
    }
}

/// Digital-to-Physical: Applies torque to a rigid body based on a PhysicalPort value.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct MotorActuator {
    pub port_entity: Entity,
    pub axis: DVec3,
}

impl Default for MotorActuator {
    fn default() -> Self {
        Self {
            port_entity: Entity::PLACEHOLDER,
            axis: DVec3::Y,
        }
    }
}

fn motor_actuator_system(
    q_ports: Query<&PhysicalPort>,
    mut q_motors: Query<(&MotorActuator, Forces)>,
) {
    for (motor, mut forces) in q_motors.iter_mut() {
        if let Ok(port) = q_ports.get(motor.port_entity) {
            let torque_mag = port.value as f64;
            forces.apply_local_torque(motor.axis * torque_mag);
        }
    }
}

/// Digital-to-Physical: Applies damping to a joint/body based on a PhysicalPort value.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct BrakeActuator {
    pub port_entity: Entity,
    pub max_force: f64,
}

impl Default for BrakeActuator {
    fn default() -> Self {
        Self {
            port_entity: Entity::PLACEHOLDER,
            max_force: 32767.0,
        }
    }
}

fn brake_actuator_system(
    q_ports: Query<&PhysicalPort>,
    mut q_brakes: Query<(&BrakeActuator, &mut AngularVelocity, &mut LinearVelocity)>,
) {
    for (brake, mut ang_vel, mut lin_vel) in q_brakes.iter_mut() {
        if let Ok(port) = q_ports.get(brake.port_entity) {
            let brake_factor = (1.0 - (port.value as f64 / brake.max_force).clamp(0.0, 1.0)).powf(2.0);
            ang_vel.0 *= brake_factor;
            lin_vel.0 *= brake_factor;
        }
    }
}

/// Physical-to-Digital: Samples angular velocity along an axis and writes to a PhysicalPort.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct AngularVelocitySensor {
    pub port_entity: Entity,
    pub axis: DVec3,
}

impl Default for AngularVelocitySensor {
    fn default() -> Self {
        Self {
            port_entity: Entity::PLACEHOLDER,
            axis: DVec3::Y,
        }
    }
}

fn sensor_velocity_system(
    q_sensors: Query<(&AngularVelocitySensor, &AngularVelocity)>,
    mut q_ports: Query<&mut PhysicalPort>,
) {
    for (sensor, velocity) in q_sensors.iter() {
        if let Ok(mut port) = q_ports.get_mut(sensor.port_entity) {
            port.value = velocity.0.dot(sensor.axis) as f32;
        }
    }
}
