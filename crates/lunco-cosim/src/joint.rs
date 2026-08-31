//! Avian revolute + prismatic joints exposed as co-simulation ports (the
//! **joint** half of the avian backend; [`crate::avian`] is the body half).
//!
//! Each single-DOF joint exposes one port in **both** directions, named for its
//! DOF — `angle` (revolute, rad) or `displacement` (prismatic, m).
//! Commandable prismatic joints are realized by Avian's native joint and their
//! authored `PhysicsDriveAPI:linear` drive. The same native path is used for
//! physical landing members: USD owns the joint geometry and drive law, while
//! Avian owns the constraint solve.
//!
//! | Joint     | Port           | `In` (commanded → motor) | `Out` (measured)        |
//! |-----------|----------------|--------------------------|-------------------------|
//! | revolute  | `angle`        | target angle (rad)       | current twist (rad)     |
//! | prismatic | `displacement` | target offset (m)        | current slider offset(m)|
//!
//! A prismatic joint additionally exposes two `Out`-only measurements: `velocity`
//! (m/s), the rate it is sliding at, and `force` (N), the axial reaction reported
//! by the authored drive. A physical part reads the latter to know its load — a
//! landing-leg strut's load is that number, and a shader takes its glow straight
//! off it.
//!
//! ## USD / Omniverse mapping
//!
//! These ports are the runtime face of the standard UsdPhysics joint-drive and
//! joint-state schemas, so an Omniverse-authored mechanism round-trips:
//! - **`In`** ⇔ `UsdPhysicsDriveAPI:{angular,linear}` `physics:targetPosition`
//!   (and the drive's `physics:maxForce` saturation, read at load — see
//!   `lunco-usd-avian`).
//! - **`Out`** ⇔ `PhysxJointStateAPI:{angular,linear}` `physics:position` and,
//!   for the prismatic rate, `physics:velocity`.
//! - `force` has no standard spelling — see [`JOINT_FORCE_PORT`].
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
//! joint (e.g. a rover wheel driven by the solved mechanical torque boundary)
//! is left entirely alone, so the two never fight over `joint.motor`.

use avian3d::prelude::{
    AngularVelocity, ComputedCenterOfMass, LinearVelocity, Mass, MotorModel, Position,
    PrismaticJoint, RevoluteJoint, Rotation,
};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;

use crate::connection::PortDirection;
use crate::ports::{AvianGroup, AvianPort};

/// The port name a revolute joint exposes in both directions.
pub const JOINT_ANGLE_PORT: &str = "angle";

/// The port name a prismatic joint exposes in both directions.
pub const JOINT_DISPLACEMENT_PORT: &str = "displacement";

/// The port name a prismatic joint exposes for its slide RATE (m/s). `Out` only —
/// a rate is measured, and the way to command one is the `displacement` setpoint.
/// Mirrors `PhysxJointStateAPI:linear physics:velocity`.
pub const JOINT_VELOCITY_PORT: &str = "velocity";

/// The port name a prismatic joint exposes for its axial reaction force (N).
/// `Out` only — the force is a *result* of the authored load law and the joint's
/// realized state, so there is nothing to command.
///
/// Unlike `displacement`/`velocity` this is a LunCo name, not a standard one:
/// `PhysxJointStateAPI` stops at position and velocity, and no UsdPhysics schema
/// spells joint-force readback. Ports are not USD schemas, so a plain name is
/// right here — inventing a `lunco:*` USD attribute to match would not be.
pub const JOINT_FORCE_PORT: &str = "force";

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
            read: Some(read_measured_angle),
            write: None,
        },
        AvianPort {
            name: JOINT_ANGLE_PORT,
            dir: PortDirection::In,
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
    let axis = j.local_hinge_axis1()?.as_vec3();
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

const PRISMATIC_STATE_PORTS: &[AvianPort] = &[
    AvianPort {
        name: JOINT_DISPLACEMENT_PORT,
        dir: PortDirection::Out,
        read: Some(read_measured_displacement),
        write: None,
    },
    AvianPort {
        name: JOINT_VELOCITY_PORT,
        dir: PortDirection::Out,
        read: Some(read_measured_slide_rate),
        write: None,
    },
    AvianPort {
        name: JOINT_FORCE_PORT,
        dir: PortDirection::Out,
        read: Some(joint_reaction_force),
        write: None,
    },
];

/// The standard prismatic-joint port group. The native joint and its authored
/// `PhysicsDriveAPI:linear` drive are the only owners of this degree of freedom.
pub const PRISMATIC_JOINT_GROUP: AvianGroup = AvianGroup {
    present: |w, e| w.get::<PrismaticJoint>(e).is_some(),
    ports: &[
        AvianPort {
            name: JOINT_DISPLACEMENT_PORT,
            dir: PortDirection::Out,
            read: Some(read_measured_displacement),
            write: None,
        },
        AvianPort {
            name: JOINT_DISPLACEMENT_PORT,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PrismaticJoint>(e).map(|j| j.motor.target_position)),
            write: Some(write_motor_displacement),
        },
        PRISMATIC_STATE_PORTS[1],
        PRISMATIC_STATE_PORTS[2],
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
    Some(displacement_along_axis(
        p1.0,
        r1.0,
        j.local_anchor1().unwrap_or(DVec3::ZERO),
        p2.0,
        r2.0,
        j.local_anchor2().unwrap_or(DVec3::ZERO),
        slider_axis_world(world, j)?,
    ))
}

/// The slider axis in world space.
///
/// `slider_axis` is body1-local (its joint basis is identity for USD-built
/// joints); carry it into world by body1's current orientation. Both the
/// displacement read and the velocity projection derive their axis here so the
/// two can never disagree about which way the joint slides.
fn slider_axis_world(world: &World, j: &PrismaticJoint) -> Option<DVec3> {
    let r1 = world.get::<Rotation>(j.body1)?;
    Some(slider_axis_world_from_rotation(r1.0, j))
}

/// Convert the joint's body-1-local slider axis into world space.
///
/// Every public state port uses this conversion. In particular, a joint axis
/// must never be projected in an unrotated frame: doing so changes the measured
/// displacement as the whole vehicle yaws.
fn slider_axis_world_from_rotation(rotation1: DQuat, j: &PrismaticJoint) -> DVec3 {
    rotation1 * j.local_slider_axis1().unwrap_or(j.slider_axis)
}

/// Measured slide rate (m/s): the relative velocity of the two anchor points,
/// projected onto the world slider axis. Sign convention matches
/// [`read_measured_displacement`] — positive means the anchors are separating
/// along the axis.
fn read_measured_slide_rate(world: &World, entity: Entity) -> Option<f64> {
    let j = world.get::<PrismaticJoint>(entity)?;
    let axis = slider_axis_world(world, j)?.normalize_or_zero();
    if axis == DVec3::ZERO {
        return Some(0.0);
    }
    // Velocity of a point rigidly attached to a body: v + ω × r, where `r` is
    // measured from the body's *global centre of mass*. `Position` is the body
    // frame origin, not the COM; using `rotation * local_anchor` here silently
    // adds `ω × COM` for any body with an authored centre-of-mass offset. The
    // landing-leg case has a large COM offset, so using the body-centre rate
    // would report false slider motion and feed the wrong load into the drive.
    //
    // A static body carries no velocity components, so a missing one reads as
    // zero rather than failing the whole port — a strut hung off the world
    // frame still has a rate.
    let anchor_vel = |body: Entity, local_anchor: DVec3| -> DVec3 {
        let lin = world
            .get::<LinearVelocity>(body)
            .map_or(DVec3::ZERO, |v| v.0);
        let ang = world
            .get::<AngularVelocity>(body)
            .map_or(DVec3::ZERO, |v| v.0);
        let rot = world.get::<Rotation>(body).map_or(DQuat::IDENTITY, |r| r.0);
        let com = world
            .get::<ComputedCenterOfMass>(body)
            .map_or(DVec3::ZERO, |com| com.0);
        lin + ang.cross(rot * (local_anchor - com))
    };
    let v1 = anchor_vel(j.body1, j.local_anchor1().unwrap_or(DVec3::ZERO));
    let v2 = anchor_vel(j.body2, j.local_anchor2().unwrap_or(DVec3::ZERO));
    Some(relative_anchor_velocity_along_axis(axis, v1, v2))
}

/// Relative rate of the two joint anchors along the native slider axis.
///
/// Keeping this projection in one helper is important: the native joint and
/// its public `velocity` port must observe the same scalar DOF. The input
/// velocities already include each body's angular contribution at its anchor.
fn relative_anchor_velocity_along_axis(axis: DVec3, body1: DVec3, body2: DVec3) -> f64 {
    (body2 - body1).dot(axis.normalize_or_zero())
}

/// Axial reaction force (N) developed by the prismatic joint's authored drive.
///
/// This is a physical result, not a driving term pressed onto the joint from
/// elsewhere. The coefficient-based cases below are the standard Avian motor
/// realizations of a USD drive:
///
/// - [`MotorModel::ForceBased`] — the law IS `stiffness * (targetPosition -
///   position) + damping * (targetVelocity - velocity)`, in newtons, on the
///   solver's own state. Exact.
/// - [`MotorModel::SpringDamper`] — the stable realisation a `physics:type =
///   "force"` spring is loaded as (avian's `ForceBased` is unstable for a stiff,
///   damped, heavy drive; see `lunco_usd_avian::JointDrive::motor_model`). It
///   carries the SAME force law, expressed as `frequency`/`damping_ratio` scaled
///   by the driven body's mass: `stiffness = m*(2*pi*f)^2`, `damping =
///   m*2*zeta*(2*pi*f)`. Recovering `m` from the driven body ([`Mass`]) yields
///   exactly `k*x + c*v` again. `None` if that mass is unreadable.
/// - [`MotorModel::AccelerationBased`] — the solver scales this by the EFFECTIVE
///   mass at the joint, which depends on both bodies' inverse masses and on the
///   anchor geometry. That is not any one body's mass, so there is no honest
///   conversion to newtons here — returns `None`.
///
/// A coefficient that is not in newtons yields no newton reading: a
/// plausible-looking wrong number on a wire is worse than an absent one.
/// Authoring `physics:type = "force"` is what makes a drive readable.
///
/// One computation, both consumers: the `force` port and anything else that wants
/// the strut's load call this, so they cannot drift apart.
pub fn joint_reaction_force(world: &World, entity: Entity) -> Option<f64> {
    let j = world.get::<PrismaticJoint>(entity)?;
    if !j.motor.enabled {
        return None;
    }
    // Recover the dimensional force law from the same owner that converts USD
    // force drives to Avian's implicit representation. Acceleration-based
    // motors deliberately produce no newton reading.
    let mass = world.get::<Mass>(j.body2)?.0 as f64;
    let (stiffness, damping) =
        lunco_physics::motor_model_force_coefficients(j.motor.motor_model, mass)?;
    let x = read_measured_displacement(world, entity)?;
    let v = read_measured_slide_rate(world, entity)?;
    let f = stiffness * (j.motor.target_position - x) + damping * (j.motor.target_velocity - v);
    // The motor cannot pull harder than its saturation, so neither may the number
    // the strut reports about itself.
    let max = j.motor.max_force;
    Some(if max.is_finite() && max > 0.0 {
        f.clamp(-max, max)
    } else {
        f
    })
}

/// Commanded displacement (`In`): drive the joint's linear motor to `value` (m)
/// via position control — same enable-on-write, finite-guard, and default-fill
/// contract as [`write_motor_angle`].
fn write_motor_displacement(world: &mut World, entity: Entity, value: f64) -> bool {
    {
        let Some(mut j) = world.get_mut::<PrismaticJoint>(entity) else {
            return false;
        };
        if !value.is_finite() {
            return true;
        }
        j.motor.enabled = true;
        j.motor.target_position = value;
        j.motor.target_velocity = 0.0;
    }
    let Some(mut j) = world.get_mut::<PrismaticJoint>(entity) else {
        return false;
    };
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
    fn displacement_is_invariant_under_a_rigid_vehicle_rotation() {
        let joint = PrismaticJoint::new(Entity::PLACEHOLDER, Entity::PLACEHOLDER)
            .with_slider_axis(DVec3::X);
        let local_offset = DVec3::new(-0.30, 0.0, 0.0);
        let unrotated = displacement_along_axis(
            DVec3::ZERO,
            DQuat::IDENTITY,
            DVec3::ZERO,
            local_offset,
            DQuat::IDENTITY,
            DVec3::ZERO,
            slider_axis_world_from_rotation(DQuat::IDENTITY, &joint),
        );

        let vehicle_rotation = DQuat::from_rotation_z(0.73);
        let world_offset = DVec3::new(8.0, 12.0, -4.0);
        let rotated = displacement_along_axis(
            world_offset,
            vehicle_rotation,
            DVec3::ZERO,
            world_offset + vehicle_rotation * local_offset,
            vehicle_rotation,
            DVec3::ZERO,
            slider_axis_world_from_rotation(vehicle_rotation, &joint),
        );

        assert!((unrotated + 0.30).abs() < 1.0e-12);
        assert!(
            (rotated - unrotated).abs() < 1.0e-12,
            "a rigid vehicle rotation changed joint displacement from \
             {unrotated} m to {rotated} m"
        );
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

    #[test]
    fn rate_uses_anchor_velocity_not_body_center_velocity() {
        // A rotating leg can have zero centre velocity while its joint anchor
        // moves. This is the contribution the native joint state must see.
        let axis = DVec3::Y;
        let body1_anchor_velocity = DVec3::ZERO;
        let body2_anchor_velocity = DVec3::new(0.0, 0.35, 0.0);
        let rate =
            relative_anchor_velocity_along_axis(axis, body1_anchor_velocity, body2_anchor_velocity);
        assert!(
            (rate - 0.35).abs() < 1e-12,
            "expected anchor rate 0.35, got {rate}"
        );
    }
}
