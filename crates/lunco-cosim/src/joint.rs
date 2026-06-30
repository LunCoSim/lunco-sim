//! Avian revolute + prismatic joints exposed as co-simulation ports (the
//! **joint** half of the avian backend; [`crate::avian`] is the body half).
//!
//! Each single-DOF joint exposes one port in **both** directions, named for its
//! DOF — `angle` (revolute, rad) or `displacement` (prismatic, m):
//!
//! | Joint     | Port           | `In` (commanded → motor) | `Out` (measured)        |
//! |-----------|----------------|--------------------------|-------------------------|
//! | revolute  | `angle`        | target angle (rad)       | current twist (rad)     |
//! | prismatic | `displacement` | target offset (m)        | current slider offset(m)|
//!
//! ## USD / Omniverse mapping
//!
//! These ports are the runtime face of the standard UsdPhysics joint-drive and
//! joint-state schemas, so an Omniverse-authored mechanism round-trips:
//! - **`In`** ⇔ `UsdPhysicsDriveAPI:{angular,linear}` `physics:targetPosition`
//!   (and the drive's `physics:maxForce` saturation, read at load — see
//!   `lunco-usd-avian`).
//! - **`Out`** ⇔ `PhysxJointStateAPI:{angular,linear}` `physics:position`.
//!
//! ## Realization
//!
//! The joint is the connector. The `In` port's write **drives the joint's own
//! [`avian3d::prelude::AngularMotor`]** (`target_position`, position control), so
//! avian's solver rotates the bodies about the hinge — not a `Transform` write,
//! and it works for dynamic bodies. The `Out` port **measures** the current
//! relative angle about the hinge axis on demand (the twist of `body2` relative
//! to `body1`), so the realized DOF flows back through wires like any other
//! output.
//!
//! ## Driven only when wired
//!
//! Crucially, the motor is touched **only when a wire targets `angle`** — the
//! write closure runs solely from the propagation master. An un-wired revolute
//! joint (e.g. a rover wheel driven by `lunco_hardware::MotorActuator`'s velocity
//! motor) is left entirely alone, so the two never fight over `joint.motor`.

use avian3d::prelude::{MotorModel, Position, PrismaticJoint, RevoluteJoint, Rotation};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;

use crate::connection::{PortDirection, PortType};
use crate::ports::{AvianGroup, AvianPort};

/// The port name a revolute joint exposes in both directions.
pub const JOINT_ANGLE_PORT: &str = "angle";

/// The port name a prismatic joint exposes in both directions.
pub const JOINT_DISPLACEMENT_PORT: &str = "displacement";

/// Maximum torque (N·m) the joint motor may apply to reach the commanded angle.
/// Generous so the joint holds its target against gravity for the structures we
/// drive (masts, panels); tune per-joint later if needed. A USD-authored
/// `UsdPhysicsDriveAPI:angular physics:maxForce` overrides this at load.
const JOINT_MOTOR_MAX_TORQUE: f64 = 1.0e8;

/// Maximum force (N) a prismatic joint motor may apply — the linear analog of
/// [`JOINT_MOTOR_MAX_TORQUE`]. Overridden by a USD `UsdPhysicsDriveAPI:linear
/// physics:maxForce` at load.
const JOINT_MOTOR_MAX_FORCE: f64 = 1.0e8;

/// Motor model for the joint drive.
///
/// `SpringDamper`, slightly **overdamped** (`damping_ratio > 1.0`). avian's
/// `MotorModel::DEFAULT` (5 Hz, ζ=1.0) overshoots ~40% on a hard step under
/// XPBD substepping (effective damping drops below nominal — measured live), so
/// we overdamp to track without overshoot. The frequency sets how fast the joint
/// chases its setpoint; ~3 Hz settles in well under a second while staying smooth
/// for the slow setpoints our Modelica controllers emit.
const JOINT_MOTOR_MODEL: MotorModel = MotorModel::SpringDamper {
    frequency: 3.0,
    damping_ratio: 2.0,
};

/// The revolute-joint port group: measured `angle` out, commanded `angle` in.
///
/// Gated on [`RevoluteJoint`] presence. The `Out` port reads the measured twist;
/// the `In` port reads the current motor setpoint and writes drive the motor.
pub const REVOLUTE_JOINT_GROUP: AvianGroup = AvianGroup {
    present: |w, e| w.get::<RevoluteJoint>(e).is_some(),
    ports: &[
        AvianPort {
            name: JOINT_ANGLE_PORT,
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(read_measured_angle),
            write: None,
        },
        AvianPort {
            name: JOINT_ANGLE_PORT,
            dir: PortDirection::In,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<RevoluteJoint>(e).map(|j| j.motor.target_position)),
            write: Some(write_motor_angle),
        },
    ],
};

/// Measured angle (`Out`): the twist of `body2`'s orientation relative to
/// `body1` about the hinge axis. Reads avian's authoritative [`Rotation`]
/// (populated by `Writeback`), so during the next tick's propagation it reflects
/// the physics step that just completed.
fn read_measured_angle(world: &World, entity: Entity) -> Option<f64> {
    let j = world.get::<RevoluteJoint>(entity)?;
    let r1 = world.get::<Rotation>(j.body1)?;
    let r2 = world.get::<Rotation>(j.body2)?;
    let axis = j.hinge_axis.as_vec3();
    Some(twist_angle(dquat_to_quat(r1.0), dquat_to_quat(r2.0), axis) as f64)
}

/// Commanded angle (`In`): drive the joint's angular motor to `value` via
/// position control. Returns `true` (the port exists) even for a non-finite
/// command, which is ignored as a transient rather than written.
fn write_motor_angle(world: &mut World, entity: Entity, value: f64) -> bool {
    let Some(mut j) = world.get_mut::<RevoluteJoint>(entity) else {
        return false;
    };
    if !value.is_finite() {
        return true;
    }
    j.motor.enabled = true;
    j.motor.target_position = value;
    j.motor.target_velocity = 0.0;
    j.motor.motor_model = JOINT_MOTOR_MODEL;
    if j.motor.max_torque <= 0.0 {
        j.motor.max_torque = JOINT_MOTOR_MAX_TORQUE;
    }
    true
}

/// The prismatic-joint port group: measured `displacement` out, commanded
/// `displacement` in. Gated on [`PrismaticJoint`] presence. Mirror of
/// [`REVOLUTE_JOINT_GROUP`] for the one translational DOF — drives a landing-gear
/// strut, an elevator/piston, or any USD `PhysicsPrismaticJoint` from a wire,
/// the API, rhai, or Modelica.
pub const PRISMATIC_JOINT_GROUP: AvianGroup = AvianGroup {
    present: |w, e| w.get::<PrismaticJoint>(e).is_some(),
    ports: &[
        AvianPort {
            name: JOINT_DISPLACEMENT_PORT,
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(read_measured_displacement),
            write: None,
        },
        AvianPort {
            name: JOINT_DISPLACEMENT_PORT,
            dir: PortDirection::In,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<PrismaticJoint>(e).map(|j| j.motor.target_position)),
            write: Some(write_motor_displacement),
        },
    ],
};

/// Measured displacement (`Out`): the signed offset (m) of `body2` relative to
/// `body1` along the slider axis, projecting both anchors' world positions onto
/// the world-space axis (`PhysxJointStateAPI:linear physics:position`).
fn read_measured_displacement(world: &World, entity: Entity) -> Option<f64> {
    let j = world.get::<PrismaticJoint>(entity)?;
    let p1 = world.get::<Position>(j.body1)?;
    let p2 = world.get::<Position>(j.body2)?;
    let r1 = world.get::<Rotation>(j.body1)?;
    let r2 = world.get::<Rotation>(j.body2)?;
    // `slider_axis` is body1-local (its joint basis is identity for USD-built
    // joints); carry it into world by body1's current orientation.
    let axis_local = j.local_slider_axis1().unwrap_or(j.slider_axis);
    Some(displacement_along_axis(
        p1.0,
        r1.0,
        j.local_anchor1().unwrap_or(DVec3::ZERO),
        p2.0,
        r2.0,
        j.local_anchor2().unwrap_or(DVec3::ZERO),
        r1.0 * axis_local,
    ))
}

/// Commanded displacement (`In`): drive the joint's linear motor to `value` (m)
/// via position control — same enable-on-write, finite-guard, and default-fill
/// contract as [`write_motor_angle`].
fn write_motor_displacement(world: &mut World, entity: Entity, value: f64) -> bool {
    let Some(mut j) = world.get_mut::<PrismaticJoint>(entity) else {
        return false;
    };
    if !value.is_finite() {
        return true;
    }
    j.motor.enabled = true;
    j.motor.target_position = value;
    j.motor.target_velocity = 0.0;
    j.motor.motor_model = JOINT_MOTOR_MODEL;
    if j.motor.max_force <= 0.0 {
        j.motor.max_force = JOINT_MOTOR_MAX_FORCE;
    }
    true
}

/// Signed displacement (m) of `body2` relative to `body1` along `axis_world`,
/// from the two anchors' world positions. Pure (no `World`) so it is
/// unit-testable; shared convention with the motor's `target_position` (zero
/// when the anchors coincide along the axis).
fn displacement_along_axis(
    p1: DVec3,
    r1: DQuat,
    anchor1: DVec3,
    p2: DVec3,
    r2: DQuat,
    anchor2: DVec3,
    axis_world: DVec3,
) -> f64 {
    let axis = axis_world.normalize_or_zero();
    if axis == DVec3::ZERO {
        return 0.0;
    }
    let a1 = p1 + r1 * anchor1;
    let a2 = p2 + r2 * anchor2;
    (a2 - a1).dot(axis)
}

/// First entity in `root`'s subtree carrying a [`RevoluteJoint`] (the joint that
/// exposes the `angle` port). Selection targets the logical root, but the joint
/// prim is usually nested (e.g. `/SolarTower/Hinge`), so the inspector resolves
/// it through here. Keeps the avian-type coupling inside this crate.
pub fn joint_angle_holder(world: &mut World, root: Entity) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        if world.get::<RevoluteJoint>(e).is_some() {
            return Some(e);
        }
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.iter());
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displacement_projects_onto_slider_axis() {
        // body2 sits 0.30 m along +Y from body1; a Y slider reads +0.30 m.
        let d = displacement_along_axis(
            DVec3::ZERO,
            DQuat::IDENTITY,
            DVec3::ZERO,
            DVec3::new(0.0, 0.30, 0.0),
            DQuat::IDENTITY,
            DVec3::ZERO,
            DVec3::Y,
        );
        assert!((d - 0.30).abs() < 1e-9, "expected 0.30, got {d}");
    }

    #[test]
    fn displacement_ignores_off_axis_translation() {
        // An X offset does not register on a Y slider (orthogonal projection).
        let d = displacement_along_axis(
            DVec3::ZERO,
            DQuat::IDENTITY,
            DVec3::ZERO,
            DVec3::new(0.5, 0.0, 0.0),
            DQuat::IDENTITY,
            DVec3::ZERO,
            DVec3::Y,
        );
        assert!(d.abs() < 1e-9, "expected 0, got {d}");
    }

    #[test]
    fn displacement_uses_anchor_offsets() {
        // Coincident body centres, but body2's anchor is +0.2 m along Y → +0.2.
        let d = displacement_along_axis(
            DVec3::ZERO,
            DQuat::IDENTITY,
            DVec3::ZERO,
            DVec3::ZERO,
            DQuat::IDENTITY,
            DVec3::new(0.0, 0.2, 0.0),
            DVec3::Y,
        );
        assert!((d - 0.2).abs() < 1e-9, "expected 0.2, got {d}");
    }
}
