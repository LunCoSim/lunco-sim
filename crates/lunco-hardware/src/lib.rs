//! Physical actuator and sensor implementations.
//!
//! This crate provides concrete implementations of the hardware described in
//! the SysML models, bridging the gap between [Port] values and
//! the [avian3d] physics engine.

use avian3d::prelude::*;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use lunco_core::architecture::Port;
use lunco_core::ports::{PortBackend, PortDirection, PortRef, PortRegistry};

/// What the drivetrain is ACTUALLY delivering at a motor's driven axle, exposed
/// as native output ports so a plot, a model, or a wire can read it.
///
/// # Why this exists
///
/// The torque a motor delivers is computed here, per tick, from the authored
/// torque–speed curve and the measured axle speed ([`axle_torque`]) — and it went
/// straight into the joint's `max_torque` without ever being readable. The USD
/// side has an `outputs:torque` attribute on every `Motor_*` prim, but NOTHING
/// WRITES IT: those prims' data is folded into `WheelParams` at parse time (see
/// `lunco_cosim`'s propagation notes on structural endpoints), so a telemetry
/// channel or a wire pointed at it read an authored zero forever and looked like
/// a dead drivetrain.
///
/// # Why it lands on the MOTOR, not on the synthesized joint or wheel
///
/// The joint is synthesized and therefore has no USD identity. The wheel is a
/// physical contact part, but the value is the operating point of the motor that
/// drives it. `MotorReadbackTarget` is projected once from the composed
/// `lunco:motor:drivenWheel` relationship, so both Avian and raycast realizations
/// update the authored motor entity without a name convention or telemetry relay.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component, Default, Debug)]
pub struct MotorReadback {
    /// Delivered axle torque, **N·m**, signed in the wheel's drive-positive
    /// sense. Negative while the motor is regenerating (back-EMF braking) or
    /// while the brake is holding.
    pub torque: f64,
    /// Measured axle speed, **rad/s**, in the same drive-positive sense.
    pub axle_speed: f64,
}

/// The authored motor entity that presents a wheel/joint's native drivetrain
/// readback. This is a topology binding, resolved once during USD projection;
/// fixed-step physics only follows the entity handle.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component, Debug)]
pub struct MotorReadbackTarget(pub Entity);

/// The port names [`MotorReadback`] publishes, in `list` order.
const READBACK_PORTS: [&str; 2] = ["torque", "axle_speed"];

fn read_readback(world: &World, entity: Entity, name: &str) -> Option<f64> {
    let r = world.get::<MotorReadback>(entity)?;
    match name {
        "torque" => Some(r.torque),
        "axle_speed" => Some(r.axle_speed),
        _ => None,
    }
}

/// Drivetrain readback as **outputs only**. A measurement is not a setpoint: a
/// wire into `torque` would be a request the physics never honours, so there is
/// no `write_input` that pretends otherwise.
const MOTOR_READBACK_BACKEND: PortBackend = PortBackend {
    list: |w, e, out| {
        let Some(r) = w.get::<MotorReadback>(e) else {
            return;
        };
        for name in READBACK_PORTS {
            out.push(PortRef {
                name: name.to_string(),
                direction: PortDirection::Out,
                value: match name {
                    "torque" => r.torque,
                    _ => r.axle_speed,
                },
            });
        }
    },
    read_output: read_readback,
    read_input: |_, _, _| None,
    write_input: |_, _, _, _| false,
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// Plugin for managing physical hardware components (motors, sensors, etc.).
pub struct LunCoHardwarePlugin;

impl Plugin for LunCoHardwarePlugin {
    fn build(&self, app: &mut App) {
        // The drivetrain's own instrumentation: delivered torque + axle speed as
        // ports on the authored motor. Registered here because the backend owns
        // the value — the same rule that moved the sun's angles into
        // `lunco-environment`.
        app.init_resource::<PortRegistry>();
        app.world_mut()
            .resource_mut::<PortRegistry>()
            .register(MOTOR_READBACK_BACKEND);
        app.register_type::<MotorActuator>()
            .register_type::<MotorReadback>()
            .register_type::<MotorReadbackTarget>()
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
    q_sleeping: Query<(), With<Sleeping>>,
    q_inputs: Query<&lunco_core::InputPorts>,
    q_child_of: Query<&ChildOf>,
    mut q_joints: Query<(Entity, &MotorActuator, &mut RevoluteJoint)>,
    // The authored motor target is projected once from USD. It keeps native Avian
    // readback on the motor users inspect, while the joint remains implementation
    // detail.
    q_targets: Query<&MotorReadbackTarget>,
    mut q_readback: Query<&mut MotorReadback>,
    mut commands: Commands,
) {
    // Update the motor operating point. Every early-return branch below
    // reports too — a released motor delivering 0 N·m is a MEASUREMENT, and
    // leaving the last non-zero value standing would read as a wheel still
    // pulling after the throttle came off.
    let publish = |q: &mut Query<&mut MotorReadback>,
                   targets: &Query<&MotorReadbackTarget>,
                   joint: Entity,
                   torque: f64,
                   omega: f64| {
        let Ok(target) = targets.get(joint) else {
            return;
        };
        if let Ok(mut r) = q.get_mut(target.0) {
            r.torque = torque;
            r.axle_speed = omega;
        }
    };
    for (joint_entity, motor, mut joint) in q_joints.iter_mut() {
        // Measured axle speed, in the motor's own drive-positive sense — the same
        // number the curve is evaluated at below, so the published pair is always
        // one consistent operating point on the torque–speed line.
        let measured_omega = match (q_bodies.get(joint.body1), q_bodies.get(joint.body2)) {
            (Ok((carrier_omega, carrier_rot)), Ok((wheel_omega, _))) => {
                let axle = carrier_rot.0 * joint.hinge_axis;
                (wheel_omega.0 - carrier_omega.0).dot(axle) * motor.drive_sign
            }
            _ => 0.0,
        };
        let Ok(port) = q_ports.get(motor.port_entity) else {
            publish(
                &mut q_readback,
                &q_targets,
                joint_entity,
                0.0,
                measured_omega,
            );
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
            // A brake is torque too, and it OPPOSES the spin — reporting the bare
            // magnitude would show a wheel being driven while it is being stopped.
            let sign = if measured_omega > 0.0 { -1.0 } else { 1.0 };
            publish(
                &mut q_readback,
                &q_targets,
                joint_entity,
                sign * motor.brake_torque,
                measured_omega,
            );
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
            publish(
                &mut q_readback,
                &q_targets,
                joint_entity,
                0.0,
                measured_omega,
            );
            continue;
        }

        // Changing an Avian joint motor does not itself wake a sleeping island.
        // A jointed vehicle commonly reaches that state while held by its brake
        // during startup; without an explicit wake, the new non-zero command is
        // written onto a joint the solver has excluded, and the whole drivetrain
        // remains asleep indefinitely. `WakeBody` is Avian's authoritative island
        // transition: waking either endpoint wakes every body connected through
        // this joint/contact island. Queue it only on the sleeping edge so steady
        // driving adds no per-tick command work.
        if q_sleeping.contains(joint.body1) || q_sleeping.contains(joint.body2) {
            commands.queue(avian3d::dynamics::solver::islands::WakeBody(joint.body1));
        }

        // The bodies were read once above for `measured_omega`; a missing body
        // means the joint is half-built this tick and the curve has no operating
        // point to evaluate at.
        if q_bodies.get(joint.body1).is_err() || q_bodies.get(joint.body2).is_err() {
            publish(
                &mut q_readback,
                &q_targets,
                joint_entity,
                0.0,
                measured_omega,
            );
            continue;
        }
        let omega_signed = measured_omega;
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
            publish(
                &mut q_readback,
                &q_targets,
                joint_entity,
                0.0,
                measured_omega,
            );
            continue;
        }

        joint.motor.enabled = true;
        joint.motor.target_velocity = motor.drive_sign * throttle * motor.max_omega;
        joint.motor.max_torque = available_torque.abs();
        // SIGNED, unlike the joint's `max_torque`: the solver wants a magnitude
        // and supplies the sign by servoing, but a reader wants to know which way
        // the machine is pushing — the negative branch is the motor regenerating.
        publish(
            &mut q_readback,
            &q_targets,
            joint_entity,
            available_torque,
            measured_omega,
        );
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
mod readback_tests {
    use super::*;

    /// The whole point of the component: the delivered torque is READABLE, by
    /// name, through the port registry. A component without a readback is not
    /// claimed (the membership test is the gate), so this backend can never
    /// answer for some unrelated prim that happens to be asked for "torque".
    #[test]
    fn delivered_torque_and_axle_speed_read_through_the_registry() {
        let mut app = App::new();
        app.init_resource::<PortRegistry>();
        app.world_mut()
            .resource_mut::<PortRegistry>()
            .register(MOTOR_READBACK_BACKEND);
        let reg = app.world().resource::<PortRegistry>().clone();

        let motor = app
            .world_mut()
            .spawn(MotorReadback {
                torque: -12.5,
                axle_speed: 3.25,
            })
            .id();
        let bare = app.world_mut().spawn_empty().id();

        assert_eq!(
            reg.read_output_port(app.world(), motor, "torque"),
            Some(-12.5),
            "signed: negative is the motor regenerating, not an error"
        );
        assert_eq!(
            reg.read_output_port(app.world(), motor, "axle_speed"),
            Some(3.25)
        );
        assert_eq!(
            reg.read_output_port(app.world(), bare, "torque"),
            None,
            "an entity with no readback is not a drivetrain"
        );
    }

    /// A measurement is not a setpoint. A wire INTO `torque` must be refused
    /// rather than silently swallowed — propagation reports a dangling target,
    /// which is the truthful answer.
    #[test]
    fn torque_is_not_writable() {
        let mut app = App::new();
        app.init_resource::<PortRegistry>();
        app.world_mut()
            .resource_mut::<PortRegistry>()
            .register(MOTOR_READBACK_BACKEND);
        let reg = app.world().resource::<PortRegistry>().clone();
        let wheel = app.world_mut().spawn(MotorReadback::default()).id();

        assert!(!reg.write_port(app.world_mut(), wheel, "torque", 99.0));
        assert_eq!(
            app.world().get::<MotorReadback>(wheel).unwrap().torque,
            0.0,
            "the write must not have landed"
        );
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
