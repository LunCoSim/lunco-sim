//! Physical actuator and sensor implementations.
//!
//! This crate provides concrete implementations of the hardware described in
//! the SysML models, bridging the gap between [Port] values and
//! the [avian3d] physics engine.

use avian3d::prelude::*;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use lunco_core::architecture::Port;

/// Plugin for managing physical hardware components (motors, sensors, etc.).
pub struct LunCoHardwarePlugin;

impl Plugin for LunCoHardwarePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SteeringActuator>()
            .register_type::<AngularVelocitySensor>()
            .add_observer(mark_actuator_driven_steer)
            .add_systems(
                FixedUpdate,
                (steering_actuator_system, sensor_velocity_system)
                    .chain()
                    .run_if(|t: Res<Time<Virtual>>| !t.is_paused() && t.relative_speed_f64() > 0.0),
            )
            .add_systems(
                lunco_core::RollbackReplay,
                steering_actuator_system
                    .chain()
                    .after(lunco_core::ControlDacSet),
            );
    }
}

/// Stamp [`lunco_core::ActuatorDrivenJoint`] on any joint that gains a
/// [`SteeringActuator`] — the frame-steer owns `motor`/frame, not the cosim
/// position-hold. (Front wheels carry both actuators; `try_insert` is idempotent.)
fn mark_actuator_driven_steer(
    trigger: On<Add, SteeringActuator>,
    query: Query<&SteeringActuator>,
    mut commands: Commands,
) {
    commands
        .entity(trigger.entity)
        .try_insert(lunco_core::ActuatorDrivenJoint);
    if let Ok(steering) = query.get(trigger.entity) {
        if steering.port_entity != Entity::PLACEHOLDER {
            commands
                .entity(steering.port_entity)
                .try_insert(lunco_core::CausalStateSink);
        }
    }
}

/// Steers an Ackermann front wheel by rotating its axle [RevoluteJoint]'s
/// chassis-side reference **frame** about the vertical (Y) axis. The revolute's
/// alignment constraint then yaws the wheel to match, so the front wheel
/// physically points into the steered heading and its rolling + lateral grip
/// redirect the rover into an arc — real geometric Ackermann through one stable
/// constraint (no floating knuckle body, which diverges in avian 0.6.1).
/// Lives on the same joint entity as the wheel's solved torque boundary.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct SteeringActuator {
    /// Entity of the [Port] providing the steering command (−1..=1).
    pub port_entity: Entity,
    /// Steering lock (rad) at the centreline (bicycle-model reference angle)
    /// reached at full steering input.
    pub max_steer_angle: f64,
    /// Authored Ackermann correction strength: 0 is parallel steering, 1 is
    /// full inner/outer-wheel geometry, and intermediate values blend the two
    /// angles as defined by `PhysxVehicleAckermannSteeringAPI`.
    pub ackermann_strength: f64,
    /// Current **centreline reference** steer angle (rad), ramped toward the
    /// commanded target. Both front wheels ramp this same reference at the same
    /// rate and derive their Ackermann angle from it each tick, so they slew in
    /// lockstep and reach their (different) target angles at the *same time*.
    /// Ramping smoothly also avoids a hard `frame1.basis` jump, which would make
    /// the rigid alignment constraint fire a large impulse and hop the rover.
    /// Internal state; not authored.
    pub current_ref: f64,
    /// This wheel's lateral offset from the rover centreline (chassis-local X, m;
    /// +left). Used for the Ackermann correction so the inner wheel turns more
    /// than the outer.
    pub lateral: f64,
    /// Wheelbase (m): longitudinal distance from this (front) axle to the rear
    /// axle. Sets the turn geometry for the Ackermann correction.
    pub wheelbase: f64,
    /// The computed steer angle (rad) for THIS wheel, written every tick by
    /// [steering_actuator_system]. This is the single shared output consumed by
    /// both wheel kinds — the physical joint applies it to its frame basis, and
    /// the raycast wheel (`lunco_mobility::apply_wheel_steering`) applies it to
    /// its visual transform — so the steering model lives in exactly one place.
    pub output_angle: f64,
}

impl Default for SteeringActuator {
    fn default() -> Self {
        Self {
            port_entity: Entity::PLACEHOLDER,
            max_steer_angle: 0.5,
            ackermann_strength: 0.0,
            current_ref: 0.0,
            lateral: 0.0,
            wheelbase: 2.0,
            output_angle: 0.0,
        }
    }
}

/// Steering slew rate (rad/s). Full lock (~0.5 rad) is reached in ~0.4 s — quick
/// but smooth enough that the alignment constraint doesn't impulse-jump the rover.
const STEER_SLEW_RATE: f64 = 1.25;

/// THE single steering model, shared by physical and raycast wheels. For every
/// [SteeringActuator] it slews the *centreline reference* angle `δ` toward
/// `steer · max_steer_angle` at `STEER_SLEW_RATE` (so both front wheels ramp the
/// same δ at the same rate and reach their final angles together), computes the
/// full **Ackermann** angle, and blends it with the parallel angle according to
/// the authored `ackermann_strength`.
///
/// If the actuator's entity also carries a [RevoluteJoint] (the physical wheel),
/// the angle is applied here to the joint's body1 frame basis (the alignment
/// constraint yaws the wheel). The raycast wheel has no joint; it reads
/// `output_angle` in `lunco_mobility::apply_wheel_steering` and rotates its
/// transform. Either way the steering math exists only here — DRY.
fn steering_actuator_system(
    time: Res<Time>,
    q_ports: Query<&Port>,
    mut q: Query<(&mut SteeringActuator, Option<&mut RevoluteJoint>)>,
) {
    let dt = time.delta_secs_f64();
    let max_step = STEER_SLEW_RATE * dt;
    for (mut steer, joint) in q.iter_mut() {
        let Ok(port) = q_ports.get(steer.port_entity) else {
            continue;
        };
        // Rate-limit the SHARED centreline reference (keeps both wheels in sync).
        let target_ref = port.value.clamp(-1.0, 1.0) * steer.max_steer_angle;
        let delta = (target_ref - steer.current_ref).clamp(-max_step, max_step);
        steer.current_ref += delta;
        // Per-wheel Ackermann angle from the ramped reference. Near-zero → straight
        // (avoid the 1/tan blow-up).
        let angle = steering_wheel_angle(
            steer.current_ref,
            steer.lateral,
            steer.wheelbase,
            steer.ackermann_strength,
        );
        steer.output_angle = angle;
        // Physical wheel: apply to the joint frame here. (Raycast wheel: no joint,
        // its transform is rotated by apply_wheel_steering from output_angle.)
        if let Some(mut joint) = joint {
            joint.frame1.basis = JointBasis::Local(DQuat::from_rotation_y(-angle));
        }
    }
}

/// Convert a vehicle's centreline steering reference into one wheel's output
/// angle. The parallel angle is the centreline reference; the Ackermann angle
/// uses the wheel's authored lateral offset and wheelbase. The schema's strength
/// is already validated at the USD boundary, so this function does not clamp or
/// invent a value for malformed input.
fn steering_wheel_angle(
    centreline: f64,
    lateral: f64,
    wheelbase: f64,
    ackermann_strength: f64,
) -> f64 {
    if centreline.abs() < 1e-4 {
        return 0.0;
    }
    let radius = wheelbase / centreline.tan();
    let full_ackermann = (wheelbase / (radius - lateral)).atan();
    centreline + ackermann_strength * (full_ackermann - centreline)
}

/// A sensor that measures angular velocity along a specific axis.
///
/// Writes the sampled velocity into a [Port] for software consumption.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct AngularVelocitySensor {
    /// Entity of the [Port] to write the sensor output into.
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
    q_sensors: Query<(&AngularVelocitySensor, &AngularVelocity, &Rotation)>,
    mut q_ports: Query<&mut Port>,
) {
    for (sensor, velocity, rotation) in q_sensors.iter() {
        if let Ok(mut port) = q_ports.get_mut(sensor.port_entity) {
            // CQ-520: `AngularVelocity` is world-frame (avian), but
            // `sensor.axis` is documented body-local. Rotate the axis into
            // world before projecting, else a tilted chassis reads the
            // wrong angular-rate component.
            let world_axis = rotation.0 * sensor.axis;
            port.value = velocity.0.dot(world_axis);
        }
    }
}

#[cfg(test)]
mod steering_tests {
    use super::steering_wheel_angle;

    #[test]
    fn ackermann_strength_selects_parallel_or_full_geometry() {
        let parallel = steering_wheel_angle(0.4, 0.8, 2.0, 0.0);
        let full = steering_wheel_angle(0.4, 0.8, 2.0, 1.0);
        let expected_full = (2.0 / (2.0 / 0.4_f64.tan() - 0.8)).atan();
        assert!((parallel - 0.4).abs() < 1e-12);
        assert!((full - expected_full).abs() < 1e-12);
        assert!(full > parallel, "the inner wheel needs the larger angle");
    }

    #[test]
    fn ackermann_strength_blends_without_recomputing_the_reference() {
        let parallel = steering_wheel_angle(0.4, -0.8, 2.0, 0.0);
        let full = steering_wheel_angle(0.4, -0.8, 2.0, 1.0);
        let halfway = steering_wheel_angle(0.4, -0.8, 2.0, 0.5);
        assert!((halfway - (parallel + full) * 0.5).abs() < 1e-12);
    }
}
