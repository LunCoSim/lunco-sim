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

/// THE motor torque–speed law, on the axle. Signed, four-quadrant, one definition.
///
/// This is the authored curve the USD schema states — `τ = k·(V − k·ω)/R`
/// (`LunCoMotorAPI` doc) — with demand `d` scaling the applied voltage:
///
/// ```text
/// i = (d·V − k_e·ω)/R
/// τ = k_t·i = τ_stall·d − (k_t·k_e/R)·ω = τ_stall·(d − ω/ω_nl)
/// ```
///
/// **Both wheel realizations call this.** It lives here rather than in
/// `lunco-mobility` because `lunco-mobility` already depends on this crate, and
/// here it sits beside [`MotorActuator`], the machine it describes. It is a pure
/// function of authored numbers — no solver state, no servo gain — so sharing it
/// does not transcribe avian's solver math into our crates (the gain stays each
/// actuator's own business, and is unobservable because this curve saturates first).
///
/// Two properties matter and both fall out of the linear law rather than being
/// bolted on:
///
/// * **Correct at partial demand.** Torque reaches zero at `d·ω_nl`, the speed the
///   servo targets. The previous form, `τ_stall·d·(1 − ω/ω_nl)`, kept authority all
///   the way to `ω_nl` — at `d = 0.4, ω = 2.4` it delivered 1.6× the authored torque.
/// * **Four-quadrant.** Above `d·ω_nl` the result goes negative on its own: that is
///   back-EMF braking, and it is why the old `.clamp(0.0, 1.0)` — not a missing
///   feature — was what removed the motor's ability to resist being back-driven.
///
/// `omega` must be the axle speed **in the demand-positive sense** (each realization
/// applies its own `drive_sign`/convention before calling).
///
/// Magnitude is clamped to `peak_torque`: plugging (`d` opposing `ω`) is genuinely
/// ~2× stall current in a real machine, but no authored number sanctions torque
/// beyond the stall figure, and a real controller current-limits there.
#[inline]
pub fn axle_torque(peak_torque: f64, max_omega: f64, demand: f64, omega: f64) -> f64 {
    if max_omega <= 0.0 {
        // No authored no-load speed: the motor is the plain torque source its
        // stall figure describes.
        return demand * peak_torque;
    }
    (peak_torque * (demand - omega / max_omega)).clamp(-peak_torque, peak_torque)
}

/// A wheel-hub motor drives a rover through its axle [RevoluteJoint]. Its target
/// velocity establishes direction and no-load speed; its torque ceiling is the
/// same live, command-scaled DC curve used by raycast wheels. Wheel-ground
/// contact supplies propulsion—no force is applied directly to the chassis.
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
    /// Stall torque at the axle (N m), after the gearbox ratio, efficiency, and
    /// output limit. This is the physical wheel's sole torque authority.
    pub peak_torque: f64,
    /// Peak braking torque at the axle (N m) — `physxVehicleWheel:maxBrakeTorque`,
    /// the SAME authored number `lunco_mobility::WheelRaycast::brake_torque_max`
    /// obeys. A brake is a property of the wheel, not of the drivetrain that
    /// realises it, so both kinds stop with the same authority.
    pub brake_torque: f64,
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
            peak_torque: 0.0,
            brake_torque: 0.0,
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
    q_bodies: Query<(&AngularVelocity, &Rotation)>,
    q_inputs: Query<&lunco_core::InputPorts>,
    q_child_of: Query<&ChildOf>,
    mut q_joints: Query<(&MotorActuator, &mut RevoluteJoint)>,
) {
    for (motor, mut joint) in q_joints.iter_mut() {
        let Ok(port) = q_ports.get(motor.port_entity) else {
            continue;
        };
        // THE BRAKE, which this realization did not have at all. The wheel
        // authors `physxVehicleWheel:maxBrakeTorque` and the raycast wheel has
        // always applied it; here the motor simply switched off at zero throttle,
        // so a joint rover had no way to stop and coasted on a frictionless
        // hinge. `ackermann_parity`'s pivot phase brakes to rest before asserting
        // that a steered rover does not rotate without drive — the raycast rover
        // stopped and the joint rover was still rolling through the measurement,
        // turning on its steered wheels, and the test read that as a steering
        // term leaking onto the drive ports.
        //
        // A velocity motor expresses a brake exactly: target zero, capped at the
        // authored brake torque. It overrides drive rather than summing with it —
        // a wheel cannot both be driven and held — and the raycast wheel resolves
        // the same precedence from the same flag, so the two kinds stop alike.
        //
        // Resolved by walking to the VESSEL, not by reading `body1`. The joint's
        // carrier is whatever the wheel hinges to, which on a rocker-bogie is a
        // suspension link with no command surface: reading `body1` there answers
        // "not braking" for a rover that is braking, and says nothing about it.
        let braking =
            lunco_core::architecture::owning_input_ports(joint.body2, &q_child_of, &q_inputs)
                .map(|c| c.brake_active)
                .unwrap_or(false);
        if braking && motor.brake_torque > 0.0 {
            joint.motor.enabled = true;
            joint.motor.target_velocity = 0.0;
            joint.motor.max_torque = motor.brake_torque;
            continue;
        }
        // Saturate to full scale before scaling by `max_omega`. The raycast path
        // clamps its throttle identically (`update_wheel_spin`), and it is that
        // agreement `drivetrain_parity` measures: unclamped, an over-range command
        // gives the raycast rover full torque but the physical rover proportionally
        // more, and the two paths diverge.
        let throttle = port.value.clamp(-1.0, 1.0);
        if throttle.abs() <= f64::EPSILON || motor.max_omega <= 0.0 || motor.peak_torque <= 0.0 {
            // Zero demand disconnects the motor. It is neither a hidden brake nor
            // a source of airborne wheel spin; bearing drag remains the wheel's
            // own physical loss.
            joint.motor.enabled = false;
            continue;
        }

        let Ok((carrier_omega, carrier_rot)) = q_bodies.get(joint.body1) else {
            continue;
        };
        let Ok((wheel_omega, _)) = q_bodies.get(joint.body2) else {
            continue;
        };

        // The joint axis is in the carrier-body frame. The relative rate about
        // its world-space form is the axle speed seen by the motor curve.
        let axle = carrier_rot.0 * joint.hinge_axis;
        let relative_omega = (wheel_omega.0 - carrier_omega.0).dot(axle);
        // Axle speed in the motor's own positive sense, which is what the curve
        // is written in.
        let omega_signed = relative_omega * motor.drive_sign;
        let available_torque =
            axle_torque(motor.peak_torque, motor.max_omega, throttle, omega_signed);

        // Avian uses zero as the *unlimited* torque sentinel, and `max_torque` is a
        // MAGNITUDE — the servo supplies the sign by driving toward its target from
        // whichever side it is on, which is what makes the negative (braking) branch
        // of the curve reach the joint at all. At the curve's zero crossing the motor
        // is genuinely at equilibrium, so releasing it is correct rather than writing
        // the sentinel and injecting an unbounded impulse.
        if available_torque.abs() <= f64::EPSILON {
            joint.motor.enabled = false;
            continue;
        }

        joint.motor.enabled = true;
        joint.motor.target_velocity = motor.drive_sign * throttle * motor.max_omega;
        joint.motor.max_torque = available_torque.abs();
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

#[cfg(test)]
mod motor_curve_tests {
    use super::axle_torque;

    const PEAK: f64 = 255.0;
    const W_NL: f64 = 12.0;

    /// The authored endpoints: stall torque at rest, zero at no-load.
    #[test]
    fn the_curve_hits_its_two_authored_endpoints() {
        assert!((axle_torque(PEAK, W_NL, 1.0, 0.0) - PEAK).abs() < 1e-9);
        assert!(axle_torque(PEAK, W_NL, 1.0, W_NL).abs() < 1e-9);
    }

    /// Torque must vanish at `d·ω_nl` — the speed the servo targets — not at the
    /// full no-load speed. The previous form kept authority to `ω_nl`, delivering
    /// 1.6x the authored torque at `d = 0.4, ω = 2.4`.
    #[test]
    fn torque_vanishes_at_the_partial_demand_no_load_speed() {
        let d = 0.4;
        assert!(axle_torque(PEAK, W_NL, d, d * W_NL).abs() < 1e-9);

        let mid = axle_torque(PEAK, W_NL, d, 2.4);
        let previous_form = PEAK * d * (1.0 - 2.4 / W_NL);
        assert!((mid - 0.2 * PEAK).abs() < 1e-9, "authored law: {mid}");
        assert!(
            previous_form > mid * 1.5,
            "regression guard: {previous_form}"
        );
    }

    /// Above the demand's no-load speed the motor RESISTS — this is the quadrant a
    /// `.clamp(0.0, 1.0)` used to remove, leaving a rover that freewheels downhill.
    #[test]
    fn back_driving_past_no_load_produces_braking_torque() {
        let tau = axle_torque(PEAK, W_NL, 0.5, W_NL);
        assert!(tau < 0.0, "expected braking torque, got {tau}");
    }

    /// Plugging is real but bounded: no authored number sanctions torque beyond stall.
    #[test]
    fn magnitude_never_exceeds_the_authored_stall_torque() {
        for &(d, w) in &[(-1.0, W_NL), (1.0, -W_NL), (1.0, 0.0), (-1.0, -W_NL)] {
            assert!(axle_torque(PEAK, W_NL, d, w).abs() <= PEAK + 1e-9);
        }
    }

    /// An unauthored no-load speed degenerates to a plain torque source, which is
    /// what the raycast fallback branch relies on.
    #[test]
    fn no_authored_no_load_speed_degenerates_to_a_torque_source() {
        assert!((axle_torque(PEAK, 0.0, 0.5, 99.0) - 0.5 * PEAK).abs() < 1e-9);
    }
}
