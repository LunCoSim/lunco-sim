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
//! A prismatic joint additionally exposes two `Out`-only measurements: `velocity`
//! (m/s), the rate it is sliding at, and `force` (N), the axial force its own
//! motor is developing. A physical part reads the latter to know how hard it is
//! working — a landing-leg strut's load is that number, and a shader takes its
//! glow straight off it.
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
//! joint (e.g. a rover wheel driven by `lunco_hardware::MotorActuator`'s velocity
//! motor) is left entirely alone, so the two never fight over `joint.motor`.

use avian3d::prelude::{
    AngularVelocity, LinearVelocity, MotorModel, Position, PrismaticJoint, RevoluteJoint, Rotation,
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

/// The port name a prismatic joint exposes for the axial force (N) its own motor
/// is developing. `Out` only — the force is a *result* of the motor's law and the
/// joint's realized state, so there is nothing to command.
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
            read: Some(read_measured_displacement),
            write: None,
        },
        AvianPort {
            name: JOINT_DISPLACEMENT_PORT,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PrismaticJoint>(e).map(|j| j.motor.target_position)),
            write: Some(write_motor_displacement),
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
            read: Some(joint_motor_force),
            write: None,
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
    let axis_local = j.local_slider_axis1().unwrap_or(j.slider_axis);
    Some(r1.0 * axis_local)
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
    // Velocity of a point rigidly attached to a body: v + ω × r, with `r` the
    // anchor offset carried into world by the body's orientation. A static body
    // carries no velocity components, so a missing one reads as zero rather than
    // failing the whole port — a strut hung off the world frame still has a rate.
    let anchor_vel = |body: Entity, local_anchor: DVec3| -> DVec3 {
        let lin = world.get::<LinearVelocity>(body).map_or(DVec3::ZERO, |v| v.0);
        let ang = world
            .get::<AngularVelocity>(body)
            .map_or(DVec3::ZERO, |v| v.0);
        let rot = world.get::<Rotation>(body).map_or(DQuat::IDENTITY, |r| r.0);
        lin + ang.cross(rot * local_anchor)
    };
    let v1 = anchor_vel(j.body1, j.local_anchor1().unwrap_or(DVec3::ZERO));
    let v2 = anchor_vel(j.body2, j.local_anchor2().unwrap_or(DVec3::ZERO));
    Some((v2 - v1).dot(axis))
}

/// Axial force (N) the prismatic joint's **own motor** is developing.
///
/// This is the physical RESULT the strut publishes — the spring's own reaction,
/// zero until the joint actually leaves its rest offset — not a driving term
/// pressed onto it from elsewhere.
///
/// This is the motor LAW evaluated on the solver's own state, not a solver
/// reading. avian's [`PrismaticJoint`] exposes no accumulated impulse — unlike a
/// contact, where `warm_start_normal_impulse` is a real measured impulse — so
/// there is no ground truth to read instead, and the law is only reported where
/// it is exact:
///
/// - [`MotorModel::ForceBased`] — the law IS `stiffness * (targetPosition -
///   position) + damping * (targetVelocity - velocity)`, in newtons, on the
///   solver's own state. Exact, and the only case that yields a number.
/// - [`MotorModel::AccelerationBased`] — the solver scales this by the EFFECTIVE
///   mass at the joint, which depends on both bodies' inverse masses and on the
///   anchor geometry. That is not any one body's mass, so there is no honest
///   conversion to newtons here.
/// - [`MotorModel::SpringDamper`] — parameterised by frequency and damping ratio,
///   so its force is not this expression at all.
///
/// The last two return `None`. Coefficients that are not in newtons yield no
/// newton reading: a plausible-looking wrong number on a wire is worse than an
/// absent one. Authoring `physics:type = "force"` is what makes a drive readable.
///
/// One computation, both consumers: the `force` port and anything else that wants
/// the strut's load call this, so they cannot drift apart.
pub fn joint_motor_force(world: &World, entity: Entity) -> Option<f64> {
    let j = world.get::<PrismaticJoint>(entity)?;
    let MotorModel::ForceBased { stiffness, damping } = j.motor.motor_model else {
        return None;
    };
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
    use avian3d::prelude::{Gravity, Mass, PhysicsPlugins, RigidBody};
    use bevy::asset::AssetPlugin;
    use bevy::MinimalPlugins;

    /// Gravity a landing leg is tuned against (m/s²).
    const MOON_G: f64 = 1.62;
    /// Vehicle share one leg carries — a quarter of a 2 t lander.
    const SPRUNG_MASS: f64 = 500.0;
    /// The leg spring, as `descent_lander.usda` authors it.
    const SPRING_K: f64 = 4000.0;
    const SPRING_C: f64 = 2200.0;

    /// Static deflection the spring must settle to: `m*g/k`.
    const STATIC_DEFLECTION: f64 = SPRUNG_MASS * MOON_G / SPRING_K;

    /// A leg hung under a fixed hull by a prismatic joint whose drive is the
    /// authored spring, stepped `steps` fixed ticks under `gravity`.
    ///
    /// Anchors are coincident in the start pose, so the joint's `displacement`
    /// begins at exactly 0 and the spring begins at exactly zero force — the same
    /// contract the USD legs get from their derived anchors.
    ///
    /// The slider axis runs hull→foot, as the USD legs' axis does once their
    /// `physics:localRot0` is applied to `physics:axis`. Compression therefore
    /// travels along −axis and `displacement` reads NEGATIVE under load, which is
    /// what makes `force = stiffness * (target − displacement)` POSITIVE while the
    /// strut is compressed, and what makes `-0.8..0.0` the compression stroke.
    ///
    /// The `AssetPlugin` + `Mesh` asset are not optional decoration: avian's
    /// collider cache runs a system reading `AssetEvent<Mesh>`, and a headless app
    /// that never initialised that message panics on the first step with an error
    /// that reads like a physics failure.
    fn sprung_leg(gravity: f64, steps: usize) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            TransformPlugin,
            PhysicsPlugins::default(),
        ));
        app.init_asset::<Mesh>();
        app.insert_resource(Gravity(DVec3::new(0.0, -gravity, 0.0)));
        app.insert_resource(Time::<Fixed>::from_hz(60.0));
        // `app.update()` advances `Time<Virtual>` from the REAL clock, and a tight
        // loop of updates takes microseconds — so `FixedPostUpdate`, where avian
        // steps, would never run and every body would sit exactly where it was
        // spawned. Pin one fixed tick per update; every other physics test in the
        // workspace drives the solver the same way.
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            core::time::Duration::from_secs_f64(1.0 / 60.0),
        ));

        let world = app.world_mut();
        let hull = world
            .spawn((RigidBody::Static, Transform::from_xyz(0.0, 0.0, 0.0)))
            .id();
        let leg = world
            .spawn((
                RigidBody::Dynamic,
                Mass(SPRUNG_MASS as f32),
                Transform::from_xyz(0.0, -1.0, 0.0),
            ))
            .id();
        let joint = world
            .spawn({
                let mut j = PrismaticJoint::new(hull, leg)
                    .with_local_anchor1(DVec3::ZERO)
                    .with_local_anchor2(DVec3::new(0.0, 1.0, 0.0))
                    .with_slider_axis(DVec3::Y)
                    .with_limits(-0.8, 0.0);
                j.motor.enabled = true;
                j.motor.target_position = 0.0;
                j.motor.target_velocity = 0.0;
                j.motor.max_force = 20_000.0;
                j.motor.motor_model = MotorModel::ForceBased {
                    stiffness: SPRING_K,
                    damping: SPRING_C,
                };
                j
            })
            .id();

        // Driving `update()` by hand still needs the plugins' deferred setup —
        // avian registers its types and messages in `finish`/`cleanup`.
        app.finish();
        app.cleanup();
        for _ in 0..steps {
            app.update();
        }
        (app, joint)
    }

    /// Under load the joint must actually travel: a spring that reports a force
    /// while the geometry never moves is a rigid strut with a decorative number.
    #[test]
    fn sprung_leg_compresses_under_load() {
        let (app, joint) = sprung_leg(MOON_G, 60);
        let x = read_measured_displacement(app.world(), joint).expect("displacement port");
        assert!(
            x < -0.02,
            "leg should be COMPRESSING after 1 s under load — negative displacement \
             is compression on a hull→foot axis — got {x} m"
        );
    }

    /// The spring must SETTLE at `m*g/k` — not diverge (ForceBased with a stiff
    /// gain and a heavy body is exactly where XPBD blows up) and not ring forever
    /// (ζ = c/(2√(km)) = 0.78, one overshoot then still).
    #[test]
    fn sprung_leg_settles_at_static_deflection() {
        let (app, joint) = sprung_leg(MOON_G, 600);
        let x = read_measured_displacement(app.world(), joint).expect("displacement port");
        let v = read_measured_slide_rate(app.world(), joint).expect("velocity port");
        assert!(
            x.is_finite() && v.is_finite(),
            "solver diverged: x = {x}, v = {v}"
        );
        assert!(
            (x + STATIC_DEFLECTION).abs() < 0.05,
            "expected settle near -{STATIC_DEFLECTION} m, got {x} m"
        );
        assert!(
            v.abs() < 0.02,
            "still oscillating after 10 s: {v} m/s — the damping is not taking"
        );
    }

    /// The `force` port is the spring's own reaction: it must read the settled
    /// load, and it must read zero when the joint sits at its rest offset — the
    /// property that keeps a strut's glow dark until touchdown.
    #[test]
    fn force_port_reads_the_spring_reaction() {
        let (loaded, joint) = sprung_leg(MOON_G, 600);
        let f = joint_motor_force(loaded.world(), joint).expect("force port");
        // At rest under gravity the spring carries the whole hung weight.
        let expected = SPRUNG_MASS * MOON_G;
        assert!(
            f > 0.0,
            "a compressed strut pushes back: reaction must be POSITIVE, got {f} N"
        );
        assert!(
            (f - expected).abs() < 0.15 * expected,
            "expected ~{expected} N of reaction, got {f} N"
        );

        // Same joint, no gravity: nothing displaces it, so there is nothing to
        // report. A non-zero reading here is a spring publishing an input.
        let (unloaded, joint) = sprung_leg(0.0, 60);
        let f0 = joint_motor_force(unloaded.world(), joint).expect("force port");
        assert!(f0.abs() < 1.0, "unloaded strut should read ~0 N, got {f0} N");
    }

    /// A `SpringDamper` motor is parameterised by frequency and damping ratio, so
    /// the drive-law expression is not its force. The port says so instead of
    /// reporting a number in the wrong units.
    #[test]
    fn force_port_declines_a_spring_damper_motor() {
        let (mut app, joint) = sprung_leg(MOON_G, 1);
        app.world_mut()
            .get_mut::<PrismaticJoint>(joint)
            .unwrap()
            .motor
            .motor_model = JOINT_MOTOR_MODEL;
        assert!(joint_motor_force(app.world(), joint).is_none());
    }

    /// An `AccelerationBased` drive's coefficients are mass-normalised, and the
    /// solver scales them by the EFFECTIVE mass at the joint — a function of both
    /// bodies' inverse masses and the anchor geometry, not of either body's mass.
    /// There is no honest newton reading here, so the port declines rather than
    /// multiplying by a mass it picked.
    #[test]
    fn force_port_declines_an_acceleration_based_motor() {
        let (mut app, joint) = sprung_leg(MOON_G, 1);
        app.world_mut()
            .get_mut::<PrismaticJoint>(joint)
            .unwrap()
            .motor
            .motor_model = MotorModel::AccelerationBased {
            stiffness: SPRING_K,
            damping: SPRING_C,
        };
        assert!(joint_motor_force(app.world(), joint).is_none());
    }

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
