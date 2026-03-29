use bevy::prelude::*;
use avian3d::prelude::*;
use lunco_sim_core::architecture::PhysicalPort;

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
fn apply_motor_torques(
    q_ports: Query<&PhysicalPort>,
    mut q_motors: Query<(&MotorActuator, &mut ConstantTorque)>,
) {
    for (motor, mut external_torque) in q_motors.iter_mut() {
        if let Ok(port) = q_ports.get(motor.port_entity) {
            // port.value is Nm (Newton-meters). We multiply by the directional axis.
            // Using standard f32 Vectors since cross-domain resolution maintains standard Bevy types 
            // natively before the solver uses 'big_space' floating origins.
            let torque_vec = motor.axis * port.value;
            // Avian3d handles forces cumulatively, so we set or apply it based on configuration
            external_torque.0 = torque_vec;
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
        // Include Avian Minimal logic if possible, or just mock the ConstantTorque component logic
        app.add_plugins(LunCoSimPhysicsPlugin);

        let port_id = app.world_mut().spawn(PhysicalPort { value: 15.0 }).id();
        let body_id = app.world_mut().spawn((
            RigidBody::Dynamic,
            ConstantTorque(Vec3::ZERO),
            MotorActuator {
                port_entity: port_id,
                axis: Vec3::Y,
            }
        )).id();

        app.update();

        // 15 Nm applied down the Y axis
        let torque = app.world().get::<ConstantTorque>(body_id).unwrap();
        // Testing that logic modified the component correctly:
        assert_eq!(torque.0.y, 15.0);
    }
}
