//! Generic physical sensors.
//!
//! Vehicle control laws are authored in Modelica/Rhai and terminate at the
//! generic cosimulation/physics ports. This crate therefore contains only
//! reusable sensor projections; it has no vehicle steering policy.

use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::architecture::Port;

/// Plugin for generic hardware sensors.
pub struct LunCoHardwarePlugin;

impl Plugin for LunCoHardwarePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<AngularVelocitySensor>().add_systems(
            FixedUpdate,
            sensor_velocity_system
                .run_if(|t: Res<Time<Virtual>>| !t.is_paused() && t.relative_speed_f64() > 0.0),
        );
    }
}

/// A sensor that measures angular velocity along a specific body-local axis.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct AngularVelocitySensor {
    /// Entity of the [`Port`] receiving the sampled velocity.
    pub port_entity: Entity,
    /// Local axis to measure rotation about.
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
    q_sensors: Query<(
        &AngularVelocitySensor,
        &avian3d::prelude::AngularVelocity,
        &avian3d::prelude::Rotation,
    )>,
    mut q_ports: Query<&mut Port>,
) {
    for (sensor, velocity, rotation) in q_sensors.iter() {
        if let Ok(mut port) = q_ports.get_mut(sensor.port_entity) {
            let world_axis = rotation.0 * sensor.axis;
            port.value = velocity.0.dot(world_axis);
        }
    }
}
