//! Joint backend — a revolute joint exposed as a co-simulation model.
//!
//! A physics joint is just another model with named ports: a revolute joint
//! has one rotational degree of freedom, so it exposes a single port `angle`
//! in **both** directions —
//!
//! | Port    | Direction | Meaning                                          |
//! |---------|-----------|--------------------------------------------------|
//! | `angle` | `In`      | commanded target angle (rad) — drives the motor  |
//! | `angle` | `Out`     | measured current angle (rad) — read back by wires|
//!
//! This mirrors [`crate::AvianSim`]: the joint is an engine-native backend, so
//! it auto-exposes its ports with **no USD authoring** (an observer attaches
//! [`JointSim`] to every [`avian3d::prelude::RevoluteJoint`], exactly like
//! [`crate::on_add_rigid_body`] attaches `AvianSim` to every `RigidBody`). A
//! USD/Modelica connection simply wires *to* `</Joint>.angle` — the FMI/SSP
//! contract — and the backend realizes it.
//!
//! ## Realization
//!
//! The joint is the connector: [`apply_joint_drives`] feeds the commanded
//! `angle` into the joint's own [`avian3d::prelude::AngularMotor`]
//! (`target_position`, position control), so Avian's solver rotates the bodies
//! about the hinge. This is **not** a `Transform` write — the joint drives
//! itself, and it works for dynamic bodies (a kinematic-pose hack is not
//! needed because avian 0.6.1 has a real angular position motor).
//!
//! [`read_joint_outputs`] measures the current relative angle about the hinge
//! axis after the physics step and publishes it to the `Out` port, so the
//! measured DOF flows back through wires like any other model output.

use std::collections::HashMap;

use avian3d::prelude::{MotorModel, Rotation, RevoluteJoint};
use bevy::prelude::*;

/// The port name a revolute joint exposes in both directions.
pub const JOINT_ANGLE_PORT: &str = "angle";

/// Maximum torque (N·m) the auto-exposed joint motor may apply to reach the
/// commanded angle. Generous so the joint holds its target against gravity for
/// the structures we drive (masts, panels); tune per-joint later if needed.
const JOINT_MOTOR_MAX_TORQUE: f64 = 1.0e8;

/// Motor model for the auto-exposed joint drive.
///
/// `SpringDamper`, slightly **overdamped** (`damping_ratio > 1.0`). avian's
/// `MotorModel::DEFAULT` (5 Hz, ζ=1.0) overshoots ~40% on a hard step under
/// XPBD substepping (effective damping drops below nominal — measured live), so
/// we overdamp to track without overshoot. The frequency sets how fast the
/// joint chases its setpoint; ~3 Hz settles in well under a second while
/// staying smooth for the slow setpoints our Modelica controllers emit.
const JOINT_MOTOR_MODEL: MotorModel = MotorModel::SpringDamper {
    frequency: 3.0,
    damping_ratio: 2.0,
};

/// Co-simulation marker for a revolute joint: the joint's `angle` DOF as named
/// `In`/`Out` ports.
///
/// Auto-attached by [`on_add_revolute_joint`]. The maps *are* the live port
/// table (see [`crate::ports`]): `inputs["angle"]` is the commanded target,
/// `outputs["angle"]` is the measured current angle.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct JointSim {
    /// Input ports keyed by name (`angle` = commanded target, rad).
    pub inputs: HashMap<String, f64>,
    /// Output ports keyed by name (`angle` = measured current, rad).
    pub outputs: HashMap<String, f64>,
}

impl Default for JointSim {
    fn default() -> Self {
        let mut inputs = HashMap::default();
        inputs.insert(JOINT_ANGLE_PORT.to_string(), 0.0);
        let mut outputs = HashMap::default();
        outputs.insert(JOINT_ANGLE_PORT.to_string(), 0.0);
        Self { inputs, outputs }
    }
}

/// Observer: auto-adds [`JointSim`] to any entity that gets a
/// [`avian3d::prelude::RevoluteJoint`].
///
/// This makes every revolute joint a first-class co-simulation model whose
/// `angle` can be wired to/from any other model, with no USD port declaration —
/// the engine-exposed-var precedent set by [`crate::on_add_rigid_body`].
pub fn on_add_revolute_joint(trigger: On<Add, RevoluteJoint>, mut commands: Commands) {
    commands.entity(trigger.entity).try_insert(JointSim::default());
}

/// Per-tick consumer: drives each joint's angular motor to the commanded
/// `inputs["angle"]` — position control through the joint's own [`AngularMotor`].
/// The joint is the connector, so the solver realizes the angle by rotating the
/// bodies about the hinge. No `Transform` write, works for dynamic bodies. This
/// covers both wire-commanded joints (a Modelica controller posing a mast) and
/// hand-commanded ones (the Inspector setpoint slider / `SetPort` on an un-wired
/// joint, which "holds" — see `lunco_cosim::write_port`).
///
/// ## Actuator-owned joints are excluded
///
/// Every [`RevoluteJoint`] auto-gets a [`JointSim`], but a rover's **wheel
/// joints** are not posed by an `angle` setpoint — they're spun by a velocity
/// motor ([`lunco_hardware::MotorActuator`] writes `motor.target_velocity`) and
/// steered by a frame rotation (`SteeringActuator`). If this position-hold
/// (`target_velocity = 0`, [`JOINT_MOTOR_MODEL`] spring-damper toward
/// `target_position`) also ran on them, it would zero the velocity command every
/// tick and pin each wheel at its setpoint with enormous torque — the rover
/// wouldn't move. The actuator is the single owner of such a joint's `motor`, so
/// it's tagged [`lunco_core::ActuatorDrivenJoint`] (by `lunco_hardware` when the
/// actuator is added) and excluded here via the query filter.
pub fn apply_joint_drives(
    mut q: Query<(&JointSim, &mut RevoluteJoint), Without<lunco_core::ActuatorDrivenJoint>>,
) {
    for (sim, mut joint) in &mut q {
        let Some(&angle) = sim.inputs.get(JOINT_ANGLE_PORT) else {
            continue;
        };
        if !angle.is_finite() {
            continue;
        }
        joint.motor.enabled = true;
        joint.motor.target_position = angle;
        joint.motor.target_velocity = 0.0;
        joint.motor.motor_model = JOINT_MOTOR_MODEL;
        if joint.motor.max_torque <= 0.0 {
            joint.motor.max_torque = JOINT_MOTOR_MAX_TORQUE;
        }
    }
}

/// After the physics step: publish each joint's measured current angle to its
/// `Out` port, so wires can read the realized DOF.
///
/// The measured angle is the twist of `body2`'s orientation relative to
/// `body1` about the hinge axis (frame anchors are identity in our authored
/// joints, so the local hinge axis is the body-local axis). Reads avian's
/// authoritative [`Rotation`] — populated by `Writeback` this same
/// `FixedPostUpdate` — rather than `GlobalTransform`, which Bevy only
/// propagates later in `PostUpdate` and would be stale here.
pub fn read_joint_outputs(
    mut q_joint: Query<(&mut JointSim, &RevoluteJoint)>,
    q_rot: Query<&Rotation>,
) {
    for (mut sim, joint) in &mut q_joint {
        let (Ok(r1), Ok(r2)) = (q_rot.get(joint.body1), q_rot.get(joint.body2)) else {
            continue;
        };
        let axis = joint.hinge_axis.as_vec3();
        let angle = twist_angle(dquat_to_quat(r1.0), dquat_to_quat(r2.0), axis);
        sim.outputs
            .insert(JOINT_ANGLE_PORT.to_string(), angle as f64);
    }
}

/// Avian's `Rotation` wraps a `DQuat` (f64 build); narrow to a glam `Quat`
/// component-wise for the twist computation. Field-wise conversion avoids
/// depending on a specific glam helper name across versions.
#[inline]
fn dquat_to_quat(q: bevy::math::DQuat) -> Quat {
    Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32)
}

/// Signed twist angle (rad, normalized to `(-π, π]`) of `q2` relative to `q1`
/// about `axis` (swing-twist decomposition).
fn twist_angle(q1: Quat, q2: Quat, axis: Vec3) -> f32 {
    let axis = axis.normalize_or_zero();
    if axis == Vec3::ZERO {
        return 0.0;
    }
    // body2 orientation expressed in body1's frame.
    let q_rel = q1.inverse() * q2;
    let r = Vec3::new(q_rel.x, q_rel.y, q_rel.z);
    let proj = axis * r.dot(axis);
    let twist = Quat::from_xyzw(proj.x, proj.y, proj.z, q_rel.w);
    let twist = if twist.length_squared() < 1e-12 {
        Quat::IDENTITY
    } else {
        twist.normalize()
    };
    let mut angle = 2.0 * twist.w.clamp(-1.0, 1.0).acos();
    if r.dot(axis) < 0.0 {
        angle = -angle;
    }
    // Normalize to (-π, π].
    while angle > std::f32::consts::PI {
        angle -= std::f32::consts::TAU;
    }
    while angle <= -std::f32::consts::PI {
        angle += std::f32::consts::TAU;
    }
    angle
}
