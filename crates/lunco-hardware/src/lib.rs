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
        app.register_type::<MotorActuator>()
            .register_type::<SteeringActuator>()
            .register_type::<AngularVelocitySensor>()
            // A wheel joint driven by an actuator owns its own `motor`; mark it
            // so the cosim joint backend (`apply_joint_drives`) doesn't also
            // position-hold it and freeze the wheel. See `ActuatorDrivenJoint`.
            .add_observer(mark_actuator_driven_motor)
            .add_observer(mark_actuator_driven_steer)
            .add_systems(
                FixedUpdate,
                (
                    steering_actuator_system,
                    motor_actuator_system,
                    sensor_velocity_system,
                )
                    .chain()
                    .run_if(|t: Res<Time<Virtual>>| !t.is_paused() && t.relative_speed_f64() > 0.0),
            )
            // Rollback replay: the joint-motor actuators ARE the jointed rover's
            // drive, so re-simulating an input must re-derive them. Ordered
            // `.after(ControlDacSet)` — they read the actuator `Port`, which
            // propagation writes from this tick's command. `sensor_velocity_system`
            // is excluded: it publishes telemetry, not force, and replay must not
            // emit sensor readings for ticks that already happened.
            .add_systems(
                lunco_core::RollbackReplay,
                (steering_actuator_system, motor_actuator_system)
                    .chain()
                    .after(lunco_core::ControlDacSet),
            );
    }
}

/// Stamp [`lunco_core::ActuatorDrivenJoint`] on any joint that gains a
/// [`MotorActuator`] — the velocity motor is now the sole owner of `motor`.
fn mark_actuator_driven_motor(trigger: On<Add, MotorActuator>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(lunco_core::ActuatorDrivenJoint);
}

/// Stamp [`lunco_core::ActuatorDrivenJoint`] on any joint that gains a
/// [`SteeringActuator`] — the frame-steer owns `motor`/frame, not the cosim
/// position-hold. (Front wheels carry both actuators; `try_insert` is idempotent.)
fn mark_actuator_driven_steer(trigger: On<Add, SteeringActuator>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(lunco_core::ActuatorDrivenJoint);
}

/// A wheel-hub motor that drives a rover the **physically correct** way: it
/// commands the wheel's axle [RevoluteJoint] toward a target **spin velocity**
/// (a velocity-controlled motor, capped at `max_torque`), and the wheel-ground
/// friction propels the rover. Nothing is pushed on the chassis - the engine
/// moves the body entirely through the contact, exactly like a real vehicle.
/// This component lives on the **joint** entity (not the wheel), alongside the
/// [RevoluteJoint] it drives.
///
/// # Why velocity control, and what was measured against it
///
/// A constant axle torque sits in avian's low-slip friction dead-zone at small
/// magnitudes (the wheel barely grips and the rover hardly moves) and breaks
/// traction wildly at large ones. A velocity motor commands the spin rate; the
/// joint applies up to `max_torque` to reach it, the tyre friction does the rest.
///
/// The obvious objection is that this has no torque-speed characteristic at all:
/// with `max_torque = peakTorque x stallTorqueGain` (6x) the motor reaches its
/// target against any load, and `drivetrain_parity` confirms it - the physical
/// wheel holds w = 12 rad/s (w.r = 4.8 m/s) while the chassis does 2.2 m/s, a
/// permanent 2.6 m/s contact slip, where the raycast wheel rolls with centimetres
/// per second of it. It is also why `lunco:wheel:driveDamping` measures as INERT:
/// the motor is torque-saturated at every sample, so its regime is never entered.
///
/// **Replacing it with the authored curve was tried and MEASURED WORSE.** Setting
/// `max_torque = tau_stall.(1 - w/w_noload).|throttle|` every tick - the same law
/// `lunco_mobility::update_wheel_spin` integrates on the raycast side - makes the
/// joint a torque source, and the physical rover's terminal speed FELL from
/// 2.21 m/s to 1.46 m/s against a raycast 2.31 (an 8% gap became 37%), with the
/// skid heading collapsing from 51 deg to 7 deg. Two follow-ups ruled out the
/// obvious explanations: it is NOT the contact friction (doubling the tyre's
/// authored `physics:dynamicFriction` moved the result by less than 0.01 m/s) and
/// NOT swept-CCD interference (`SweptCcd::LINEAR` was byte-identical). Under the
/// curve the axle settled at w = 9.5 rad/s with the motor capped at ~53 N.m while
/// the chassis did 1.46 m/s - i.e. still sliding, but no longer able to buy back
/// the torque, and NOT limited by the cone it was sliding against. The torque
/// source is right in principle; something between the joint motor and the
/// contact is eating the difference and has not been identified. Until it is,
/// this is what parity is measured against. See `WheelParams::drive_motor`.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct MotorActuator {
    /// Entity of the [Port] providing the throttle command (-1..=1).
    /// For a skid rover this already carries the per-side differential.
    pub port_entity: Entity,
    /// No-load axle speed (rad/s) commanded at full throttle -
    /// `lunco:motor:noLoadSpeed` divided by the gearbox ratio. With wheel radius
    /// `r` the free-rolling top speed is about `max_omega * r`.
    pub max_omega: f64,
    /// Sign mapping throttle to spin so a positive (forward) command rolls the
    /// rover along its chassis -Z. Depends on the joint's `hinge_axis`
    /// orientation; `-1` for the canonical `axle = rotation * Y` hinge.
    pub drive_sign: f64,
}

impl Default for MotorActuator {
    fn default() -> Self {
        Self {
            port_entity: Entity::PLACEHOLDER,
            max_omega: 0.0,
            drive_sign: -1.0,
        }
    }
}

/// Drives each wheel by writing its axle joint's velocity-motor target from the
/// throttle port. The joint applies up to its `max_torque` to reach the spin;
/// the tyre-ground friction moves the rover - pure physics-engine propulsion,
/// and a steered front wheel is driven about its steered axle for free (the
/// hinge axis yaws with the wheel).
fn motor_actuator_system(
    q_ports: Query<&Port>,
    mut q_joints: Query<(&MotorActuator, &mut RevoluteJoint)>,
) {
    for (motor, mut joint) in q_joints.iter_mut() {
        let Ok(port) = q_ports.get(motor.port_entity) else {
            continue;
        };
        // Saturate to full scale before scaling by `max_omega`. The raycast path
        // clamps its throttle identically (`update_wheel_spin`), and it is that
        // agreement `drivetrain_parity` measures: unclamped, an over-range command
        // gives the raycast rover full torque but the physical rover proportionally
        // more, and the two paths diverge.
        let throttle = port.value.clamp(-1.0, 1.0);
        joint.motor.target_velocity = motor.drive_sign * throttle * motor.max_omega;
    }
}

/// Steers an Ackermann front wheel by rotating its axle [RevoluteJoint]'s
/// chassis-side reference **frame** about the vertical (Y) axis. The revolute's
/// alignment constraint then yaws the wheel to match, so the front wheel
/// physically points into the steered heading and its rolling + lateral grip
/// redirect the rover into an arc — real geometric Ackermann through one stable
/// constraint (no floating knuckle body, which diverges in avian 0.6.1).
/// Lives on the same joint entity as the wheel's [MotorActuator].
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct SteeringActuator {
    /// Entity of the [Port] providing the steering command (−1..=1).
    pub port_entity: Entity,
    /// Steering lock (rad) at the centreline (bicycle-model reference angle)
    /// reached at full steering input.
    pub max_steer_angle: f64,
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
/// same δ at the same rate and reach their different final angles together),
/// then computes this wheel's **Ackermann** angle — turn radius `R = L/tan δ`, a
/// wheel at lateral offset `y` steers `atan(L / (R − y))` so the inner wheel
/// turns more than the outer — and stores it in `output_angle`.
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
        let angle = if steer.current_ref.abs() < 1e-4 {
            0.0
        } else {
            let r = steer.wheelbase / steer.current_ref.tan(); // signed turn radius
            (steer.wheelbase / (r - steer.lateral)).atan()
        };
        steer.output_angle = angle;
        // Physical wheel: apply to the joint frame here. (Raycast wheel: no joint,
        // its transform is rotated by apply_wheel_steering from output_angle.)
        if let Some(mut joint) = joint {
            joint.frame1.basis = JointBasis::Local(DQuat::from_rotation_y(-angle).into());
        }
    }
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
