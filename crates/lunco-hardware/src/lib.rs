//! Physical actuator and sensor implementations.
//!
//! This crate provides concrete implementations of the hardware described in
//! the SysML models, bridging the gap between [PhysicalPort] values and 
//! the [avian3d] physics engine.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::*;
use lunco_core::architecture::PhysicalPort;

/// Plugin for managing physical hardware components (motors, sensors, etc.).
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
           ).chain().run_if(|tw: Res<lunco_core::TimeWarpState>| tw.is_running()));
    }
}

/// A wheel-hub motor that drives a rover via a **contact force**, not a torque
/// couple.
///
/// The naive approach — `apply_torque` about the axle — works for a free wheel,
/// but on a rigid-axle joint rover the revolute transmits the motor's reaction
/// couple straight into the chassis as a nose-up pitch. At speed that compounds
/// into a wheelie/launch (see `project_physical_rover_suspension`). Instead we
/// apply a forward force at the wheel's ground-contact point: its moment about
/// the (free) axle spins the wheel, and the linear part propels the chassis,
/// leaving only the *real* traction pitch — no reaction couple. This mirrors the
/// raycast wheel (`lunco_mobility::apply_wheel_drive`) and was validated in the
/// headless `rover_jitter --drivemode=force` probe (5+ m/s with no launch).
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct MotorActuator {
    /// Entity of the [PhysicalPort] providing the throttle command (−1..=1).
    pub port_entity: Entity,
    /// Wheel spin axis (the axle). Retained for reference/diagnostics; the drive
    /// direction is derived from the chassis heading, not this axis.
    pub axis: DVec3,
    /// Peak drive authority, authored as `physxVehicleEngine:peakTorque` (N·m).
    /// Converted to an equivalent traction force `F = peak_torque / radius`
    /// delivered when the port reads ±1.0.
    pub peak_torque: f64,
    /// Wheel radius (m) — converts the authored peak torque into a traction
    /// force and sets the contact point below the hub.
    pub radius: f64,
    /// Wheel mount offset in the **chassis** local frame. The drive force is
    /// applied to the chassis at this hub (reconstructed from the chassis pose),
    /// not to the wheel body — a force on the `ChildOf` wheel is fought by Bevy
    /// transform propagation re-slaving the child each frame.
    pub mount_local: DVec3,
}

impl Default for MotorActuator {
    fn default() -> Self {
        Self {
            port_entity: Entity::PLACEHOLDER,
            axis: DVec3::Y,
            peak_torque: 1.0,
            radius: 0.4,
            mount_local: DVec3::ZERO,
        }
    }
}

/// Drives rovers from [MotorActuator] wheels via a contact force applied to the
/// **chassis** (the wheel's `ChildOf` parent). Mirrors
/// `lunco_mobility::apply_wheel_drive`: forward force at the wheel hub, derived
/// from the chassis heading, so the motor reaction couple never loads the
/// chassis as pitch (no wheelie). `q_motors` reads the wheels; `q_chassis`
/// applies to their parents (disjoint via `Without<MotorActuator>`).
fn motor_actuator_system(
    q_ports: Query<&PhysicalPort>,
    q_motors: Query<(&MotorActuator, &ChildOf)>,
    mut q_chassis: Query<Forces, Without<MotorActuator>>,
) {
    for (motor, child_of) in q_motors.iter() {
        if motor.radius <= 0.0 {
            continue;
        }
        let Ok(port) = q_ports.get(motor.port_entity) else { continue };
        let Ok(mut forces) = q_chassis.get_mut(child_of.parent()) else { continue };
        // Reconstruct the hub in avian's frame from the chassis Position/Rotation
        // + the wheel's chassis-local offset (NOT GlobalTransform, which would mix
        // in the big_space floating-origin rebasing — see apply_wheel_drive).
        let chassis_rot = forces.rotation().0;
        let hub_world = forces.position().0 + chassis_rot * motor.mount_local;
        // Rover forward = chassis heading projected onto the horizontal plane, so
        // a pitched chassis can't feed the drive force back into more pitch.
        let mut forward = chassis_rot * DVec3::NEG_Z;
        forward.y = 0.0;
        let forward = forward.normalize_or_zero();
        // `port.value` is the normalized throttle; F = throttle · peak_torque / r.
        let force_mag = port.value as f64 * motor.peak_torque / motor.radius;
        // Apply at the HUB (axle height), not the ground contact: the contact is
        // the lowest point, so it has the longest lever to the chassis CG and
        // maximises the nose-up traction pitch ("front goes up"). The hub is ~2×
        // closer to the CG → roughly half the pitch, while still propelling the
        // chassis forward. Left/right hubs differ, so a steering differential in
        // the port values yaws the rover. (Mirrors apply_wheel_drive, which also
        // applies at the hub, not the contact.)
        forces.apply_force_at_point(forward * force_mag, hub_world);
    }
}

/// A braking system that applies damping to reduce velocity.
///
/// This emulates a frictional brake by scaling down the entity's 
/// linear and angular velocity based on a [PhysicalPort] value.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct BrakeActuator {
    /// Entity of the [PhysicalPort] providing the brake command.
    pub port_entity: Entity,
    /// Maximum force limit for normalization.
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

/// System that applies damping from [BrakeActuator] components.
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

/// A sensor that measures angular velocity along a specific axis.
///
/// Writes the sampled velocity into a [PhysicalPort] for software consumption (ADC).
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct AngularVelocitySensor {
    /// Entity of the [PhysicalPort] to write the sensor output into.
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

/// System that samples angular velocity for [AngularVelocitySensor] components.
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

