//! The physics readiness gate: "is the world safe to *integrate* right now?"
//!
//! # Why this is not a clock
//!
//! Subsystems routinely need rigid-body integration suspended for a few frames —
//! the DEM heightfield is still baking, a collider ring has not yet paged in under
//! a rover, an obstacle field was just regenerated and its colliders need a frame
//! to re-seat. Step physics during those windows and a `Dynamic` body free-falls
//! through a collider that does not exist yet, tunnels under the heightfield, and
//! is gone.
//!
//! The old code expressed that wait by writing the **user's transport**
//! (`lunco_time::TimeTransport.mode = Paused`). That is a category error with three
//! visible consequences:
//!
//! 1. The sandbox *opened paused*. The holds are up during the first frames of
//!    every scene load, so the user was handed a stopped world and had to press
//!    play to undo an engine implementation detail.
//! 2. It froze **everything**, not just physics — the tick, and with it the epoch,
//!    the ephemeris, the animation sampler, the lighting. A collider that has not
//!    finished baking is no reason for the planets to stop moving.
//! 3. Release needed a "did *we* pause it?" flag (`paused_by_us`) to avoid
//!    un-pausing a pause the user had started themselves — bookkeeping that only
//!    existed because two unrelated concepts shared one bit.
//!
//! So readiness gates **physics**, and nothing else. [`PhysicsHolds`] pauses avian's
//! `Time<Physics>` clock, which zeroes the physics delta and stops the solver while
//! `Time<Virtual>`, the `SimTick`, `WorldTime.epoch_jd`, the celestial chain and the
//! avatar all keep running. The user's transport is never touched, and the world
//! integrates again on its own the moment the last hold clears.
//!
//! Holds are keyed by a `&'static str` reason, so several subsystems can hold
//! concurrently and each releases only its own; physics runs when the set is empty.

use avian3d::dynamics::joints::EntityConstraint;
use avian3d::dynamics::solver::{
    solver_body::{SolverBody, SolverBodyInertia},
    xpbd::{joints::PrismaticJointSolverData, XpbdConstraint},
};
use avian3d::prelude::{
    AngularVelocity, ComputedCenterOfMass, ContactGraph, CustomPositionIntegration, JointDisabled,
    LinearVelocity, Physics, Position, PrismaticJoint, RigidBody, RigidBodyColliders,
    RigidBodyDisabled, Rotation, Sensor,
};
use avian3d::schedule::PhysicsTime;
use bevy::ecs::schedule::ApplyDeferred;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use std::time::Duration;

pub mod escape;
pub mod pose;
pub mod readiness;
pub mod spatial;
pub mod support;
pub use escape::{EscapeDiagnosticPlugin, WorldBounds};
pub use pose::{PhysicsPoseSeeded, SimulationPoseQuery, SimulationPoseReadState};
pub use readiness::{Integrable, ReadinessEffectPlugin};
pub use spatial::{GridSpatialQuery, GridSpatialQueryState};
pub use support::{PhysicsSupportContact, PhysicsSupportFootprint, PhysicsSupportSet};

/// Number of Avian solver substeps in one authoritative fixed physics tick.
///
/// The fixed tick remains owned by `lunco-core::FIXED_HZ`; this value controls
/// only the solver resolution inside that tick. It is deliberately one
/// cross-platform contract: changing solver resolution by target architecture
/// changes the physical result and cannot be a hidden application fallback.
///
/// Eight substeps is the production contract for articulated bodies. It keeps
/// the suspension and wheel contact constraints resolved at the same fixed
/// physics boundary; individual scenes cannot select a different resolution or
/// receive an asset-specific correction.
pub const DEFAULT_SUBSTEP_COUNT: u32 = 8;

/// Bounds for the live solver-resolution diagnostic control.
///
/// These are deliberately separate from [`DEFAULT_SUBSTEP_COUNT`]. A scenario
/// may temporarily vary the Avian resolution while diagnosing a scene, but the
/// authored/runtime contract remains the default above unless the application
/// owner changes that default in this crate.
pub const MIN_DIAGNOSTIC_SUBSTEP_COUNT: u32 = 1;
pub const MAX_DIAGNOSTIC_SUBSTEP_COUNT: u32 = 64;

/// Read the live Avian solver resolution.
pub fn solver_substeps(world: &World) -> Option<u32> {
    world
        .get_resource::<avian3d::prelude::SubstepCount>()
        .map(|count| count.0)
}

/// Change solver resolution for a live diagnostic run.
///
/// The mutation is immediate and therefore takes effect at the next solver
/// schedule. Invalid values are rejected at this owner; there is no silent
/// clamp or fallback. Callers should restore the value explicitly after a
/// comparison if they want to continue under the production contract.
pub fn set_solver_substeps(world: &mut World, count: u32) -> Result<(), String> {
    if !(MIN_DIAGNOSTIC_SUBSTEP_COUNT..=MAX_DIAGNOSTIC_SUBSTEP_COUNT).contains(&count) {
        return Err(format!(
            "solver substeps must be in {MIN_DIAGNOSTIC_SUBSTEP_COUNT}..={MAX_DIAGNOSTIC_SUBSTEP_COUNT}, got {count}"
        ));
    }
    if world
        .get_resource::<avian3d::prelude::SubstepCount>()
        .is_none()
    {
        return Err("Avian SubstepCount resource is not installed".to_string());
    };
    world.resource_mut::<avian3d::prelude::SubstepCount>().0 = count;
    Ok(())
}

/// Live contact-material coefficients for a physics entity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContactFrictionParameters {
    /// Kinetic Coulomb coefficient, used after relative sliding starts.
    pub dynamic: f64,
    /// Static Coulomb coefficient, used while relative tangential speed is zero.
    pub static_coefficient: f64,
}

/// Change one entity's contact friction for a live diagnostic run.
///
/// The component is the Avian contact-material owner used by the USD bridge;
/// no separate script-side material cache is created. The combine rule and
/// authored USD remain unchanged, so this is an explicit in-memory experiment
/// and is not a persistence path.
pub fn set_contact_friction(
    world: &mut World,
    entity: Entity,
    friction: ContactFrictionParameters,
) -> Result<(), String> {
    use avian3d::prelude::Friction;

    if !friction.dynamic.is_finite()
        || !friction.static_coefficient.is_finite()
        || friction.dynamic < 0.0
        || friction.static_coefficient < 0.0
        || friction.dynamic > avian3d::math::Scalar::MAX as f64
        || friction.static_coefficient > avian3d::math::Scalar::MAX as f64
    {
        return Err(format!(
            "invalid contact friction: coefficients must be finite, non-negative, and representable by Avian, got {friction:?}"
        ));
    }
    let Some(mut value) = world.get_mut::<Friction>(entity) else {
        return Err(format!("entity {entity:?} has no Avian Friction component"));
    };
    value.dynamic_coefficient = friction.dynamic as avian3d::math::Scalar;
    value.static_coefficient = friction.static_coefficient as avian3d::math::Scalar;
    Ok(())
}

/// Read one entity's live Avian contact friction, if authored/projected.
pub fn contact_friction_snapshot(
    world: &World,
    entity: Entity,
) -> Option<ContactFrictionParameters> {
    let value = world.get::<avian3d::prelude::Friction>(entity)?;
    Some(ContactFrictionParameters {
        dynamic: value.dynamic_coefficient as f64,
        static_coefficient: value.static_coefficient as f64,
    })
}

/// Live relative joint-damping coefficients for a physics joint.
///
/// Avian applies these as a dimensionless rate over the physics interval, so
/// both fields have units of s^-1. They are not force-drive coefficients and do
/// not replace the joint's geometric constraint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointDampingParameters {
    pub linear: f64,
    pub angular: f64,
}

fn has_supported_joint(world: &World, entity: Entity) -> bool {
    world.get::<avian3d::prelude::FixedJoint>(entity).is_some()
        || world
            .get::<avian3d::prelude::PrismaticJoint>(entity)
            .is_some()
        || world
            .get::<avian3d::prelude::RevoluteJoint>(entity)
            .is_some()
        || world
            .get::<avian3d::prelude::SphericalJoint>(entity)
            .is_some()
        || world
            .get::<avian3d::prelude::DistanceJoint>(entity)
            .is_some()
}

/// Add or replace Avian's native relative joint damping for a live diagnostic
/// run. The joint entity must already carry one of Avian's supported joint
/// components; this function never creates or replaces a constraint.
pub fn set_joint_damping(
    world: &mut World,
    entity: Entity,
    damping: JointDampingParameters,
) -> Result<(), String> {
    if !damping.linear.is_finite()
        || !damping.angular.is_finite()
        || damping.linear < 0.0
        || damping.angular < 0.0
        || damping.linear > avian3d::math::Scalar::MAX as f64
        || damping.angular > avian3d::math::Scalar::MAX as f64
    {
        return Err(format!(
            "invalid joint damping: rates must be finite, non-negative, and representable by Avian, got {damping:?}"
        ));
    }
    if !has_supported_joint(world, entity) {
        return Err(format!("entity {entity:?} has no supported Avian joint"));
    }

    let value = avian3d::prelude::JointDamping {
        linear: damping.linear as avian3d::math::Scalar,
        angular: damping.angular as avian3d::math::Scalar,
    };
    if let Some(mut existing) = world.get_mut::<avian3d::prelude::JointDamping>(entity) {
        *existing = value;
    } else {
        world.entity_mut(entity).insert(value);
    }
    Ok(())
}

/// Read Avian's native relative joint damping. An installed joint without a
/// `JointDamping` component has the documented lossless default of zero.
pub fn joint_damping_snapshot(world: &World, entity: Entity) -> Option<JointDampingParameters> {
    if !has_supported_joint(world, entity) {
        return None;
    }
    let value = world
        .get::<avian3d::prelude::JointDamping>(entity)
        .copied()
        .unwrap_or_default();
    Some(JointDampingParameters {
        linear: value.linear as f64,
        angular: value.angular as f64,
    })
}

/// Aggregate the live Avian contacts owned by one physics entity.
///
/// This is deliberately a read-only diagnostic at the physics boundary. It
/// reports the effective manifold coefficient and the impulses Avian retained
/// for the next warm start; it does not infer a force or mutate the solver.
/// `pair_count` includes overlapping AABBs, while `touching_pair_count` and the
/// impulse fields include only actual touching contacts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContactDebugSnapshot {
    pub pair_count: u32,
    pub touching_pair_count: u32,
    pub manifold_count: u32,
    pub max_friction: f64,
    pub total_normal_impulse: f64,
    pub normal_impulse_vector: [f64; 3],
    pub max_normal_speed: f64,
    pub max_tangent_speed: f64,
    pub max_penetration: f64,
    pub total_warm_start_tangent_impulse: f64,
    pub contact_normal: [f64; 3],
    pub contact_point: [f64; 3],
}

pub fn contact_debug_snapshot(world: &World, entity: Entity) -> Option<ContactDebugSnapshot> {
    use avian3d::prelude::{AngularVelocity, ContactGraph, LinearVelocity};

    let graph = world.get_resource::<ContactGraph>()?;
    let mut snapshot = ContactDebugSnapshot::default();
    for pair in graph.contact_pairs_with(entity) {
        snapshot.pair_count += 1;
        if !pair.is_touching() {
            continue;
        }
        snapshot.touching_pair_count += 1;
        for manifold in &pair.manifolds {
            snapshot.manifold_count += 1;
            snapshot.max_friction = snapshot.max_friction.max(manifold.friction as f64);
            snapshot.contact_normal = [
                manifold.normal.x as f64,
                manifold.normal.y as f64,
                manifold.normal.z as f64,
            ];
            let normal_impulse = manifold.normal * manifold.total_normal_impulse();
            snapshot.normal_impulse_vector[0] += normal_impulse.x as f64;
            snapshot.normal_impulse_vector[1] += normal_impulse.y as f64;
            snapshot.normal_impulse_vector[2] += normal_impulse.z as f64;
            for point in &manifold.points {
                snapshot.total_normal_impulse += point.normal_impulse as f64;
                snapshot.max_normal_speed = snapshot
                    .max_normal_speed
                    .max(point.normal_speed.abs() as f64);
                snapshot.max_penetration = snapshot.max_penetration.max(point.penetration as f64);
                let velocity_at = |body: Option<Entity>, anchor: avian3d::math::Vector| {
                    let Some(body) = body else {
                        return avian3d::math::Vector::ZERO;
                    };
                    let linear = world
                        .get::<LinearVelocity>(body)
                        .map_or(avian3d::math::Vector::ZERO, |velocity| velocity.0);
                    let angular = world
                        .get::<AngularVelocity>(body)
                        .map_or(avian3d::math::Vector::ZERO, |velocity| velocity.0);
                    linear + angular.cross(anchor)
                };
                let relative =
                    velocity_at(pair.body2, point.anchor2) - velocity_at(pair.body1, point.anchor1);
                let tangent = relative - manifold.normal * relative.dot(manifold.normal);
                snapshot.max_tangent_speed =
                    snapshot.max_tangent_speed.max(tangent.length() as f64);
                snapshot.total_warm_start_tangent_impulse +=
                    point.warm_start_tangent_impulse.length() as f64;
                snapshot.contact_point = [
                    point.point.x as f64,
                    point.point.y as f64,
                    point.point.z as f64,
                ];
            }
        }
    }
    Some(snapshot)
}

/// A dimensional force-drive description for a prismatic joint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrismaticDriveParameters {
    /// Linear stiffness in N/m.
    pub stiffness: f64,
    /// Linear damping in N·s/m.
    pub damping: f64,
    /// Maximum actuator force in N.
    pub max_force: f64,
}

/// Why a dimensional force drive cannot be lowered to Avian's implicit model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForceDriveMotorError {
    /// A positive spring coefficient needs a positive generalized inertia to
    /// preserve the authored force law in `SpringDamper` form.
    MissingGeneralizedInertia,
    /// A drive coefficient is not a finite, non-negative physical quantity.
    InvalidCoefficients,
}

/// Convert a dimensional force spring into Avian's stable spring-damper model.
///
/// USD's `physics:type = "force"` coefficients remain the public physical
/// contract. Avian receives the equivalent implicit representation when a
/// generalized inertia is available, preserving the authored force law while
/// avoiding an explicit stiff-force update at the fixed-step boundary. For a
/// linear drive this is a mass; for an angular drive it is a moment of inertia
/// about the driven coordinate.
pub fn force_drive_motor_model(
    stiffness: f64,
    damping: f64,
    generalized_inertia: f64,
) -> Result<avian3d::prelude::MotorModel, ForceDriveMotorError> {
    if !stiffness.is_finite() || !damping.is_finite() || stiffness < 0.0 || damping < 0.0 {
        return Err(ForceDriveMotorError::InvalidCoefficients);
    }
    if stiffness > 0.0 {
        if !generalized_inertia.is_finite() || generalized_inertia <= 0.0 {
            return Err(ForceDriveMotorError::MissingGeneralizedInertia);
        }
        let omega = (stiffness / generalized_inertia).sqrt();
        Ok(avian3d::prelude::MotorModel::SpringDamper {
            frequency: omega / std::f64::consts::TAU,
            damping_ratio: damping / (2.0 * (stiffness * generalized_inertia).sqrt()),
        })
    } else {
        // Avian has no implicit zero-stiffness damping-only motor. This is the
        // exact USD force law for a pure damper, not a spring-drive fallback.
        Ok(avian3d::prelude::MotorModel::ForceBased { stiffness, damping })
    }
}

/// Recover force-equivalent coefficients from an Avian motor model.
///
/// Acceleration-based motors intentionally return `None`: their coefficients
/// are accelerations and the effective mass depends on the live joint geometry,
/// so presenting them as newtons would be dimensionally false.
pub fn motor_model_force_coefficients(
    model: avian3d::prelude::MotorModel,
    generalized_inertia: f64,
) -> Option<(f64, f64)> {
    match model {
        avian3d::prelude::MotorModel::ForceBased { stiffness, damping } => {
            Some((stiffness, damping))
        }
        avian3d::prelude::MotorModel::SpringDamper {
            frequency,
            damping_ratio,
        } if generalized_inertia > 0.0 => {
            let omega = std::f64::consts::TAU * frequency;
            Some((
                generalized_inertia * omega * omega,
                generalized_inertia * 2.0 * damping_ratio * omega,
            ))
        }
        avian3d::prelude::MotorModel::AccelerationBased { .. }
        | avian3d::prelude::MotorModel::SpringDamper { .. } => None,
    }
}

/// A live prismatic-drive readout, in the same dimensional units used by USD.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrismaticDriveSnapshot {
    pub enabled: bool,
    /// `"force"` for a force-equivalent drive, `"acceleration"` when the
    /// authored motor coefficients are acceleration-based rather than N/m.
    pub model: &'static str,
    pub target_position: f64,
    pub target_velocity: f64,
    pub stiffness: f64,
    pub damping: f64,
    pub max_force: f64,
}

/// Replace a prismatic joint's dimensional force-drive coefficients in place.
///
/// The target position/velocity are intentionally preserved: position commands
/// remain on the joint's existing co-simulation ports, while this operation
/// changes only the physical drive law. The joint entity must already carry the
/// native Avian joint; no replacement constraint is created.
pub fn set_prismatic_drive(
    world: &mut World,
    entity: Entity,
    drive: PrismaticDriveParameters,
) -> Result<(), String> {
    use avian3d::prelude::{ComputedMass, Mass, PrismaticJoint};

    if !drive.stiffness.is_finite()
        || !drive.damping.is_finite()
        || !drive.max_force.is_finite()
        || drive.stiffness < 0.0
        || drive.damping < 0.0
        || drive.max_force <= 0.0
    {
        return Err(format!(
            "invalid prismatic drive: stiffness/damping must be non-negative and max_force positive, got {drive:?}"
        ));
    }
    let joint = world
        .get::<PrismaticJoint>(entity)
        .ok_or_else(|| format!("entity {entity:?} has no PrismaticJoint"))?;
    let body_mass = world
        .get::<Mass>(joint.body2)
        .map(|mass| mass.0 as f64)
        .or_else(|| {
            world
                .get::<ComputedMass>(joint.body2)
                .map(|mass| mass.value() as f64)
        })
        .ok_or_else(|| format!("prismatic joint {entity:?} has no driven-body mass"))?;
    let model = force_drive_motor_model(drive.stiffness, drive.damping, body_mass)
        .map_err(|error| format!("cannot lower prismatic force drive: {error:?}"))?;
    let Some(mut joint) = world.get_mut::<PrismaticJoint>(entity) else {
        return Err(format!("entity {entity:?} has no PrismaticJoint"));
    };
    joint.motor.enabled = true;
    joint.motor.motor_model = model;
    joint.motor.max_force = drive.max_force;
    Ok(())
}

/// Read a prismatic drive as dimensional stiffness/damping, regardless of
/// whether Avian stores its equivalent implicit spring-damper representation.
pub fn prismatic_drive_snapshot(world: &World, entity: Entity) -> Option<PrismaticDriveSnapshot> {
    use avian3d::prelude::{ComputedMass, Mass, MotorModel, PrismaticJoint};

    let joint = world.get::<PrismaticJoint>(entity)?;
    let mass = world
        .get::<Mass>(joint.body2)
        .map(|mass| mass.0 as f64)
        .or_else(|| {
            world
                .get::<ComputedMass>(joint.body2)
                .map(|mass| mass.value() as f64)
        })
        .unwrap_or(0.0);
    let model = match joint.motor.motor_model {
        MotorModel::AccelerationBased { .. } => "acceleration",
        _ => "force",
    };
    let (stiffness, damping) = motor_model_force_coefficients(joint.motor.motor_model, mass)?;
    Some(PrismaticDriveSnapshot {
        enabled: joint.motor.enabled,
        model,
        target_position: joint.motor.target_position,
        target_velocity: joint.motor.target_velocity,
        stiffness: stiffness as f64,
        damping: damping as f64,
        max_force: joint.motor.max_force as f64,
    })
}

/// Avian runs one biased contact solve and one relaxation solve per substep.
/// `ContactPoint::normal_impulse` accumulates the full clamped normal impulse
/// once in each of those two phases, so the exposed value is two solver-phase
/// sums of the physical impulse. Convert it to the load delivered over one
/// co-simulation master interval here, at the Avian/physics boundary.
const AVIAN_CONTACT_SOLVER_PHASES: f64 = 2.0;

/// Convert Avian's accumulated normal contact impulse to the physical load
/// delivered over one master physics interval.
///
/// Avian owns all internal substeps. The co-simulation master owns the
/// communication interval; callers must not divide by a substep count or run a
/// second participant loop.
#[inline]
pub fn contact_force_from_impulse(normal_impulse: f64, physics_dt: f64) -> f64 {
    if normal_impulse.is_finite() && physics_dt > 0.0 {
        normal_impulse / (physics_dt * AVIAN_CONTACT_SOLVER_PHASES)
    } else {
        0.0
    }
}

/// A target pose for a kinematic Avian body driven outside the physics clock.
///
/// The pose is in Avian's global physics frame (`Position`/`Rotation`), not in
/// Bevy's floating-origin render frame. The owner updates this target from its
/// authoritative interface cadence; [`apply_kinematic_drives`] applies it to
/// Avian and derives contact velocity for the next live physics step.
///
/// This is the supported Avian custom-position-integration path. The body must
/// also carry [`CustomPositionIntegration`], which prevents Avian's integrator
/// from advancing the pose a second time from the velocity we publish.
#[derive(Component, Clone, Copy, Debug)]
pub struct KinematicDrive {
    /// Desired global position in Avian's physics frame.
    pub position: DVec3,
    /// Desired global orientation in Avian's physics frame.
    pub rotation: DQuat,
}

impl KinematicDrive {
    /// Create a drive at an already-seated physics pose.
    pub fn new(position: DVec3, rotation: DQuat) -> Self {
        Self { position, rotation }
    }

    /// Replace the desired global pose.
    pub fn set_pose(&mut self, position: DVec3, rotation: DQuat) {
        self.position = position;
        self.rotation = rotation;
    }
}

/// Maximum contact velocity generated by one interface-cycle drive update.
///
/// The target pose itself is not clamped: the owning command boundary applies
/// scene/world validity rules. This bound only prevents a discontinuous input
/// sample from injecting an unbounded velocity into jointed dynamic bodies.
pub const MAX_KINEMATIC_DRIVE_SPEED: f64 = 1_000.0;

/// Apply [`KinematicDrive`] targets in the caller's authoritative interface
/// cadence.
///
/// This system intentionally does not read `Time<Physics>` and does not belong
/// to `FixedUpdate`: a paused physics clock must not make an editor drag
/// unresponsive. Avian consumes the resulting `Position`/`Rotation` on its
/// next live step; `CustomPositionIntegration` prevents a second velocity-based
/// position integration in that step.
pub fn apply_kinematic_drives(
    time: Res<Time>,
    mut bodies: Query<
        (
            &RigidBody,
            &KinematicDrive,
            &mut Position,
            &mut Rotation,
            &mut LinearVelocity,
            &mut AngularVelocity,
        ),
        (With<RigidBody>, With<CustomPositionIntegration>),
    >,
) {
    let dt = time.delta_secs_f64();
    for (body, drive, mut position, mut rotation, mut linear, mut angular) in &mut bodies {
        if !body.is_kinematic()
            || !drive.position.is_finite()
            || !drive.rotation.is_finite()
            || drive.rotation.length_squared() < 1.0e-24
        {
            warn_once!("ignoring invalid or non-kinematic KinematicDrive target");
            continue;
        }

        let target_rotation = drive.rotation.normalize();
        let position_delta = drive.position - position.0;
        if position_delta.length_squared() > 0.0 {
            position.0 = drive.position;
        }

        let rotation_delta = target_rotation * rotation.0.inverse();
        if rotation_delta.angle_between(DQuat::IDENTITY) > 1.0e-12 {
            rotation.0 = target_rotation;
        }

        if dt > 1e-6 {
            linear.0 = (position_delta / dt).clamp_length_max(MAX_KINEMATIC_DRIVE_SPEED);
            angular.0 =
                (rotation_delta.to_scaled_axis() / dt).clamp_length_max(MAX_KINEMATIC_DRIVE_SPEED);
        } else {
            linear.0 = DVec3::ZERO;
            angular.0 = DVec3::ZERO;
        }
    }
}

/// The set of reasons physics is currently suspended. Empty ⇒ physics integrates.
///
/// This is an **engine** authority. It is not, and must never become, a mirror of
/// the user's play/pause state (`lunco_time::TimeTransport`) — see the module docs.
#[derive(Resource, Debug, Clone, Default)]
pub struct PhysicsHolds {
    reasons: std::collections::BTreeSet<&'static str>,
}

impl PhysicsHolds {
    /// Terrain DEM build / collider-ring warm-up (`lunco-terrain-surface`).
    pub const TERRAIN_READY: &'static str = "terrain-ready";
    /// A body was promoted from its authored kinematic loading pose to Dynamic.
    /// This one-frame bridge prevents a freshly promoted rover from stepping in
    /// the fixed loop before the terrain ring has observed the new body and made
    /// its collider live beneath it.
    pub const GROUND_ACTIVATION: &'static str = "ground-activation";
    /// Something the world needs is not ready yet — a scene still composing, a
    /// program still compiling. Raised from [`lunco_readiness`] by
    /// [`readiness::apply_world_readiness_hold`]; the *scope* of a wait (world vs
    /// one object) is an authored policy decision, not a fact of the wait.
    pub const READINESS: &'static str = "readiness";
    /// A scripted cutscene / offline recording is choosing when the world moves.
    ///
    /// Held, physics is frozen but `Time<Virtual>` keeps running, so `FixedUpdate` —
    /// and the scenario script driving the shot — stays alive. That is the whole
    /// point: pausing the *world* clock (`lunco_time::TimeTransport`) also stops the
    /// script, so a paused scene can never run the script that would unpause it.
    /// Advance the world from a script with [`PhysicsStepRequest`] instead.
    pub const CINEMATIC: &'static str = "cinematic";
    /// A terminal runtime fault has invalidated the simulation state.
    pub const SAFETY_FAILURE: &'static str = "runtime-safety-failure";
    /// The explicitly bound Avian frame is missing or a physical entity is
    /// disconnected from it. This is raised by the coordinate-system owner
    /// before the solver is admitted, so invalid positions cannot accumulate
    /// force or contact state while the scene is being diagnosed.
    pub const FRAME_CONTRACT: &'static str = "physics-frame-contract";

    /// Is any subsystem holding physics?
    #[inline]
    pub fn is_held(&self) -> bool {
        !self.reasons.is_empty()
    }

    /// Is this specific reason holding?
    #[inline]
    pub fn holds(&self, reason: &'static str) -> bool {
        self.reasons.contains(reason)
    }

    /// Raise or clear one hold. Compare with [`Self::holds`] first so the `ResMut`
    /// is dereferenced only on a real edge (no per-frame change-detection churn).
    pub fn set(&mut self, reason: &'static str, held: bool) {
        let changed = if held {
            self.reasons.insert(reason)
        } else {
            self.reasons.remove(reason)
        };
        if changed {
            debug!(
                "[physics] hold {}: {reason}",
                if held { "raised" } else { "released" }
            );
        }
    }

    /// The reasons currently holding, for diagnostics/UI.
    pub fn reasons(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.reasons.iter().copied()
    }
}

/// Frames of physics owed to a caller that is otherwise holding the clock.
///
/// The step half of "hold, then step": with [`PhysicsHolds::CINEMATIC`] raised, a
/// script advances the world deliberately — one frame per step — instead of
/// play/pausing it. Each queued step lets exactly one frame of physics through.
///
/// Stepping exists because pause/unpause is unusable from inside a script: pausing
/// the world clock stops `FixedUpdate`, so the script that paused it cannot run
/// again to unpause itself. A physics hold keeps the script running, and stepping
/// gives it deterministic control over motion — the same guarantee frame-by-frame
/// capture wants, since the recorder advances virtual time by exactly `1/fps` per
/// captured frame.
#[derive(Resource, Debug, Clone, Default)]
pub struct PhysicsStepRequest {
    /// Frames of physics still owed. Decremented as each is granted.
    pub steps: u32,
}

impl PhysicsStepRequest {
    /// Queue `n` more frames of physics.
    pub fn request(&mut self, n: u32) {
        self.steps = self.steps.saturating_add(n);
    }

    /// Drop any owed frames (e.g. when the hold is released outright).
    pub fn clear(&mut self) {
        self.steps = 0;
    }
}

/// Run condition: **the solver will consume forces applied this tick.**
///
/// Every system that writes into avian's force accumulator (gravity, suspension,
/// wheel drive, thrusters, …) must be gated on this. avian clears the accumulator
/// *inside* the physics step, so a force applied while the step is skipped is never
/// cleared — it is ADDED TO on the next tick, and the next, and discharges in full
/// on the single step that eventually runs.
///
/// MEASURED, and the reason this exists (episode 2, six-wheel rover): shots 1-3 are
/// "frozen" beats, which by design hold *physics* while leaving the *world clock*
/// ticking (see `assets/scripting/prelude/recording.rhai` — pausing the world clock
/// would stop `FixedUpdate` and deadlock the very script that paused it). Across
/// those 28 s ≈ 1800 fixed ticks, `lunco_environment::apply_gravity_to_rigid_bodies`
/// kept adding `m·g` downward with nothing consuming it — ~4 MN accumulated. The
/// hold released at the start of shot 4 and discharged it in ONE step: the rover
/// left the surface at **224.20 m/s downward on that shot's first captured frame**
/// (HUD `elev -2.2`, `speed 224.20 m/s`), which is 3.7 m of travel per 1/60 s step
/// and so tunnels straight through the 1 m ground slab. It was never a fall: it was
/// a launch. Velocity then decayed under the chassis' `linearDamping = 0.5`
/// (169.94 m/s at frame 19, 36.87 m/s at frame 150) — the exponential signature that
/// identified an impulse rather than free fall in the first place.
///
/// Gating on `Time<Physics>` rather than on [`PhysicsHolds`] is deliberate: it is
/// the clock the solver actually integrates, so this is also correct for anything
/// that pauses physics out of band, exactly as [`apply_physics_holds`] is.
///
/// `Time<Virtual>` is NOT a substitute and was the original mistake in
/// `lunco-mobility`: a physics hold does not pause virtual time, so a virtual-clock
/// gate is open for the entire window this needs to close. Both are checked here —
/// a paused world clock must stop force application too, and `Time<Physics>` does
/// not report itself paused merely because virtual time is.
pub fn physics_is_live_state(
    physics_time: &Time<Physics>,
    virtual_time: &Time<Virtual>,
    faults: Option<&lunco_core::RuntimeFaults>,
) -> bool {
    !faults.is_some_and(|state| state.active())
        && !physics_time.is_paused()
        && !virtual_time.is_paused()
        && virtual_time.relative_speed_f64() > 0.0
}

/// Bevy run condition for systems that write force or torque into avian.
pub fn physics_is_live(
    physics_time: Res<Time<Physics>>,
    virtual_time: Res<Time<Virtual>>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
) -> bool {
    physics_is_live_state(&physics_time, &virtual_time, faults.as_deref())
}

/// Project [`PhysicsHolds`] onto avian's `Time<Physics>`.
///
/// Pausing the physics clock zeroes the physics delta, so the solver does not step
/// — while `Time<Virtual>` (and therefore the tick, epoch, ephemeris and animation)
/// keeps advancing. Runs in `PreUpdate`, ahead of the physics schedule, and is
/// change-driven: it only writes when the desired state differs from the actual, so
/// it is also self-healing if anything pauses the physics clock out of band.
pub fn apply_physics_holds(
    holds: Res<PhysicsHolds>,
    coupling: Option<Res<lunco_core::SimulationBarrier>>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    let held = holds.is_held()
        || coupling.is_some_and(|state| state.held)
        || faults.is_some_and(|state| state.active());
    if held {
        // `pause()` changes Avian's flag but leaves the previous generic
        // `Time<Physics>` delta intact. Its nested driver would then execute
        // one stale solver pass on the first held FixedPostUpdate. A hold is
        // therefore a paused *and zero-delta* boundary.
        physics_time.pause();
        physics_time.advance_by(Duration::ZERO);
    } else if physics_time.is_paused() {
        physics_time.unpause();
    }
}

/// Release exactly one queued [`PhysicsStepRequest`] frame through a hold.
///
/// Runs in `FixedPreUpdate`, **inside** the fixed loop — the same clock domain that
/// steps physics (avian integrates in `FixedPostUpdate` off `Time<Fixed>`). A step
/// granted from a render-frame schedule is not equivalent: `Time<Fixed>` only
/// accumulates on frames where virtual time advanced, so a grant landing on a
/// zero-delta frame is consumed without any physics running at all. Offline
/// recording makes that the common case, since it alternates advance and capture
/// frames. Consuming the debt here means one granted step is always exactly one
/// integrated step.
pub fn grant_physics_step(
    holds: Res<PhysicsHolds>,
    coupling: Option<Res<lunco_core::SimulationBarrier>>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    mut steps: ResMut<PhysicsStepRequest>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    if faults.is_some_and(|state| state.active()) {
        steps.clear();
        physics_time.pause();
        physics_time.advance_by(Duration::ZERO);
        return;
    }
    if coupling.is_some_and(|state| state.held) {
        // Modelica owns a solver barrier, not the caller's recording clock.
        // Keep queued cinematic steps until the in-flight model result releases
        // the barrier; dropping them makes a deterministic recorder capture the
        // same pose repeatedly while its Rhai frame index continues advancing.
        physics_time.pause();
        physics_time.advance_by(Duration::ZERO);
        return;
    }
    if !holds.is_held() {
        // Nothing to step past. Drop the debt rather than let it fire later against
        // an unrelated hold (a terrain bake, say).
        if steps.steps > 0 {
            steps.clear();
        }
        return;
    }

    if steps.steps > 0 {
        steps.steps -= 1;
        if physics_time.is_paused() {
            physics_time.unpause();
        }
    } else if !physics_time.is_paused() {
        physics_time.pause();
        physics_time.advance_by(Duration::ZERO);
    }
}

/// Correct the signed lower/upper limit projection for native prismatic joints.
///
/// Avian 0.7's `DistanceLimit::compute_correction_along_axis` returns a
/// positive correction for a signed lower-limit violation, but
/// `PositionConstraint::apply_positional_impulse` applies that impulse in the
/// opposite separation direction. The upper limit uses the opposite sign and
/// therefore does not expose the defect. This system is the generic solver
/// boundary for that missing lower-limit case: it uses the same joint frame,
/// active BigSpace-local solver pose, Jacobian, and effective mass as Avian's
/// native prismatic constraint, then leaves velocity projection to Avian.
fn correct_prismatic_limit_position(
    bodies: Query<
        (
            &mut avian3d::dynamics::solver::solver_body::SolverBody,
            &avian3d::dynamics::solver::solver_body::SolverBodyInertia,
        ),
        Without<avian3d::prelude::RigidBodyDisabled>,
    >,
    joints: Query<
        &avian3d::prelude::PrismaticJoint,
        (
            Without<avian3d::prelude::RigidBody>,
            Without<avian3d::prelude::JointDisabled>,
        ),
    >,
    poses: Query<
        (
            &avian3d::prelude::Position,
            &avian3d::prelude::Rotation,
            &ComputedCenterOfMass,
        ),
        Without<avian3d::prelude::RigidBodyDisabled>,
    >,
    time: Res<Time>,
) {
    let delta_secs = time.delta_secs_f64();
    let mut dummy_body1 = avian3d::dynamics::solver::solver_body::SolverBody::DUMMY;
    let mut dummy_body2 = avian3d::dynamics::solver::solver_body::SolverBody::DUMMY;

    for joint in &joints {
        let (mut body1, mut inertia1) = (
            &mut dummy_body1,
            &avian3d::dynamics::solver::solver_body::SolverBodyInertia::DUMMY,
        );
        let (mut body2, mut inertia2) = (
            &mut dummy_body2,
            &avian3d::dynamics::solver::solver_body::SolverBodyInertia::DUMMY,
        );

        if let Ok((body, inertia)) = unsafe { bodies.get_unchecked(joint.body1) } {
            body1 = body.into_inner();
            inertia1 = inertia;
        }
        if let Ok((body, inertia)) = unsafe { bodies.get_unchecked(joint.body2) } {
            body2 = body.into_inner();
            inertia2 = inertia;
        }

        // Match Avian's dominance rule: the more dominant body is the
        // immovable side of this constraint for the current solve.
        match (inertia1.dominance() - inertia2.dominance()).cmp(&0) {
            std::cmp::Ordering::Greater => {
                inertia1 = &avian3d::dynamics::solver::solver_body::SolverBodyInertia::DUMMY
            }
            std::cmp::Ordering::Less => {
                inertia2 = &avian3d::dynamics::solver::solver_body::SolverBodyInertia::DUMMY
            }
            std::cmp::Ordering::Equal => {}
        }

        let Some(limits) = joint.limits else {
            continue;
        };
        let (
            Ok((position1, rotation1, center_of_mass1)),
            Ok((position2, rotation2, center_of_mass2)),
        ) = (poses.get(joint.body1), poses.get(joint.body2))
        else {
            continue;
        };
        let (Some(local_anchor1), Some(local_anchor2), Some(local_basis1)) = (
            joint.local_anchor1(),
            joint.local_anchor2(),
            joint.local_basis1(),
        ) else {
            continue;
        };

        let local_r1 = rotation1.0 * (local_anchor1 - center_of_mass1.0);
        let local_r2 = rotation2.0 * (local_anchor2 - center_of_mass2.0);
        let world_r1 = body1.delta_rotation * local_r1;
        let world_r2 = body2.delta_rotation * local_r2;
        let axis = body1.delta_rotation * (rotation1.0 * local_basis1) * joint.slider_axis;
        let anchor1 =
            position1.0 + rotation1.0 * center_of_mass1.0 + body1.delta_position + world_r1;
        let anchor2 =
            position2.0 + rotation2.0 * center_of_mass2.0 + body2.delta_position + world_r2;
        let displacement = (anchor2 - anchor1).dot(axis);
        let correction = if displacement < limits.min {
            displacement - limits.min
        } else if displacement > limits.max {
            displacement - limits.max
        } else {
            0.0
        };
        if correction.abs() <= avian3d::math::Scalar::EPSILON {
            continue;
        }

        let inverse_mass1 = inertia1.effective_inv_mass();
        let inverse_mass2 = inertia2.effective_inv_mass();
        let inverse_angular_inertia1 = inertia1.effective_inv_angular_inertia();
        let inverse_angular_inertia2 = inertia2.effective_inv_angular_inertia();
        let angular_axis1 = world_r1.cross(axis);
        let angular_axis2 = world_r2.cross(axis);
        let effective_mass = axis.dot(inverse_mass1 * axis)
            + angular_axis1.dot(inverse_angular_inertia1 * angular_axis1)
            + axis.dot(inverse_mass2 * axis)
            + angular_axis2.dot(inverse_angular_inertia2 * angular_axis2);
        if effective_mass <= avian3d::math::Scalar::EPSILON {
            continue;
        }

        // The constraint residual is `displacement - limit`. Avian's
        // positional-impulse convention moves the measured separation by
        // `-effective_mass * impulse`, so the signed Lagrange update receives
        // the negated residual. This preserves the authored limit compliance
        // instead of turning a soft limit into an implicit hard stop.
        let delta_lagrange = avian3d::dynamics::solver::xpbd::compute_lagrange_update(
            0.0,
            -correction,
            &[effective_mass],
            joint.limit_compliance,
            delta_secs,
        );
        let impulse = axis * delta_lagrange;
        if !body1.flags.is_kinematic() {
            body1.delta_position += inverse_mass1 * impulse;
            let delta_rotation = avian3d::math::Quaternion::from_scaled_axis(
                inverse_angular_inertia1 * world_r1.cross(impulse),
            );
            body1.delta_rotation.0 = delta_rotation * body1.delta_rotation.0;
        }
        if !body2.flags.is_kinematic() {
            body2.delta_position -= inverse_mass2 * impulse;
            let delta_rotation = avian3d::math::Quaternion::from_scaled_axis(
                inverse_angular_inertia2 * world_r2.cross(-impulse),
            );
            body2.delta_rotation.0 = delta_rotation * body2.delta_rotation.0;
        }
    }
}

/// Complete the native contact/joint coupling for touching prismatic joints.
///
/// Avian prepares joint state before the substep loop and solves the native
/// joint before contact relaxation. Reusing that prepared native constraint
/// after contact relaxation gives a contact impulse one fixed Gauss-Seidel
/// return path through a long joint. This is deliberately a generic solver
/// boundary: it only considers native prismatic joints and touching,
/// non-sensor contacts, and it never inspects a scene path or authored asset.
fn solve_contact_prismatic_joint(
    bodies: Query<(&mut SolverBody, &SolverBodyInertia), Without<RigidBodyDisabled>>,
    mut joints: Query<
        (&mut PrismaticJoint, &mut PrismaticJointSolverData),
        (Without<RigidBody>, Without<JointDisabled>),
    >,
    collider_lists: Query<&RigidBodyColliders>,
    disabled_bodies: Query<(), With<RigidBodyDisabled>>,
    sensors: Query<(), With<Sensor>>,
    contact_graph: Res<ContactGraph>,
    time: Res<Time>,
) {
    let delta_secs = time.delta_secs_f64();
    let mut dummy_body1 = SolverBody::DUMMY;
    let mut dummy_body2 = SolverBody::DUMMY;

    for (mut joint, mut solver_data) in &mut joints {
        let [entity1, entity2] = joint.entities();
        let has_contact = [entity1, entity2].into_iter().any(|body| {
            let Ok(colliders) = collider_lists.get(body) else {
                return false;
            };
            colliders.into_iter().any(|collider| {
                contact_graph.contact_pairs_with(collider).any(|pair| {
                    pair.is_touching()
                        && !sensors.contains(pair.collider1)
                        && !sensors.contains(pair.collider2)
                        && pair
                            .body1
                            .is_some_and(|body| !disabled_bodies.contains(body))
                        && pair
                            .body2
                            .is_some_and(|body| !disabled_bodies.contains(body))
                })
            })
        });
        if !has_contact {
            continue;
        }

        let (mut body1, mut inertia1) = (&mut dummy_body1, &SolverBodyInertia::DUMMY);
        let (mut body2, mut inertia2) = (&mut dummy_body2, &SolverBodyInertia::DUMMY);
        if let Ok((body, inertia)) = unsafe { bodies.get_unchecked(entity1) } {
            body1 = body.into_inner();
            inertia1 = inertia;
        }
        if let Ok((body, inertia)) = unsafe { bodies.get_unchecked(entity2) } {
            body2 = body.into_inner();
            inertia2 = inertia;
        }
        match (inertia1.dominance() - inertia2.dominance()).cmp(&0) {
            std::cmp::Ordering::Greater => inertia1 = &SolverBodyInertia::DUMMY,
            std::cmp::Ordering::Less => inertia2 = &SolverBodyInertia::DUMMY,
            std::cmp::Ordering::Equal => {}
        }

        joint.solve(
            [body1, body2],
            [inertia1, inertia2],
            &mut solver_data,
            delta_secs,
        );
    }
}

/// Project relative angular velocity for native prismatic joints.
///
/// Avian's XPBD prismatic constraint projects orientation changes, but its
/// velocity projection leaves a relative angular velocity that arrived from a
/// contact impulse untouched when the orientation was already aligned. That is
/// physically inconsistent for a prismatic joint: all three relative angular
/// degrees of freedom are locked. A long slider then carries that residual rate
/// to its contact point and can turn a stationary support into persistent slip.
///
/// This is the velocity-level complement to Avian's native position solver. It
/// applies the unique impulse that removes relative angular velocity while
/// distributing the correction through the two effective inverse inertias. It
/// does not move transforms, repeat the XPBD solve, or add a scene-specific
/// correction.
fn project_prismatic_angular_velocity(
    bodies: Query<
        (
            &mut avian3d::dynamics::solver::solver_body::SolverBody,
            &avian3d::dynamics::solver::solver_body::SolverBodyInertia,
        ),
        Without<avian3d::prelude::RigidBodyDisabled>,
    >,
    joints: Query<
        &avian3d::prelude::PrismaticJoint,
        (
            Without<avian3d::prelude::RigidBody>,
            Without<avian3d::prelude::JointDisabled>,
        ),
    >,
) {
    let mut dummy_body1 = avian3d::dynamics::solver::solver_body::SolverBody::DUMMY;
    let mut dummy_body2 = avian3d::dynamics::solver::solver_body::SolverBody::DUMMY;

    for joint in &joints {
        let (mut body1, mut inertia1) = (
            &mut dummy_body1,
            &avian3d::dynamics::solver::solver_body::SolverBodyInertia::DUMMY,
        );
        let (mut body2, mut inertia2) = (
            &mut dummy_body2,
            &avian3d::dynamics::solver::solver_body::SolverBodyInertia::DUMMY,
        );

        if let Ok((body, inertia)) = unsafe { bodies.get_unchecked(joint.body1) } {
            body1 = body.into_inner();
            inertia1 = inertia;
        }
        if let Ok((body, inertia)) = unsafe { bodies.get_unchecked(joint.body2) } {
            body2 = body.into_inner();
            inertia2 = inertia;
        }

        // Match Avian's dominance rule: the more dominant body is the
        // immovable side of this constraint for the current solve.
        match (inertia1.dominance() - inertia2.dominance()).cmp(&0) {
            std::cmp::Ordering::Greater => {
                inertia1 = &avian3d::dynamics::solver::solver_body::SolverBodyInertia::DUMMY
            }
            std::cmp::Ordering::Less => {
                inertia2 = &avian3d::dynamics::solver::solver_body::SolverBodyInertia::DUMMY
            }
            std::cmp::Ordering::Equal => {}
        }

        let relative = body2.angular_velocity - body1.angular_velocity;
        if relative.length_squared() > avian3d::math::Scalar::EPSILON {
            let inv_inertia1 = inertia1.effective_inv_angular_inertia();
            let inv_inertia2 = inertia2.effective_inv_angular_inertia();
            let impulse = (inv_inertia1 + inv_inertia2).inverse_or_zero() * relative;

            if !body1.flags.is_kinematic() {
                body1.angular_velocity += inv_inertia1 * impulse;
            }
            if !body2.flags.is_kinematic() {
                body2.angular_velocity -= inv_inertia2 * impulse;
            }
        }
    }
}

/// Installs the physics readiness gate. Add wherever `avian3d`'s `PhysicsPlugins`
/// are added — whoever owns physics owns its gate.
pub struct PhysicsGatePlugin;

impl Plugin for PhysicsGatePlugin {
    fn build(&self, app: &mut App) {
        pose::register_spatial_query_providers(app);
        app.register_type::<PhysicsSupportFootprint>()
            .register_type::<PhysicsSupportContact>()
            .configure_sets(
                Update,
                (
                    PhysicsSupportSet::Publish,
                    PhysicsSupportSet::Apply,
                    PhysicsSupportSet::Consume,
                )
                    .chain(),
            )
            // Publishing uses Commands because support footprints are attached to
            // entities created by other runtime projections. Make that deferred
            // boundary explicit in the shared contract so consumers always read
            // the published component in the same Update schedule.
            .add_systems(Update, ApplyDeferred.in_set(PhysicsSupportSet::Apply))
            // Physics owns both readiness and the solver-resolution contract.
            // Avian's resource is the single runtime reader and live diagnostic
            // target; no app-level duplicate or target-specific selection is
            // permitted.
            .insert_resource(avian3d::prelude::SubstepCount(DEFAULT_SUBSTEP_COUNT))
            .init_resource::<PhysicsHolds>()
            .init_resource::<PhysicsStepRequest>()
            .add_systems(PreUpdate, apply_physics_holds)
            // Inside the fixed loop, ahead of avian's `FixedPostUpdate` integration,
            // so a granted step coincides with a step that actually runs.
            .add_systems(bevy::prelude::FixedPreUpdate, grant_physics_step)
            .add_systems(
                avian3d::prelude::SubstepSchedule,
                solve_contact_prismatic_joint
                    .before(correct_prismatic_limit_position)
                    .in_set(
                        avian3d::dynamics::solver::xpbd::XpbdSolverSystems::SolveUserConstraints,
                    ),
            )
            .add_systems(
                avian3d::prelude::SubstepSchedule,
                correct_prismatic_limit_position.in_set(
                    avian3d::dynamics::solver::xpbd::XpbdSolverSystems::SolveUserConstraints,
                ),
            )
            .add_systems(
                avian3d::prelude::SubstepSchedule,
                project_prismatic_angular_velocity
                    .after(avian3d::dynamics::solver::xpbd::XpbdSolverSystems::VelocityProjection)
                    .before(avian3d::dynamics::solver::schedule::SubstepSolverSystems::Damping),
            )
            .add_plugins(escape::EscapeDiagnosticPlugin)
            // Same reasoning: a readiness decision that nothing enforces is a
            // hold that silently does not hold.
            .add_plugins(readiness::ReadinessEffectPlugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_impulse_uses_the_master_interval_once() {
        assert_eq!(contact_force_from_impulse(32.4, 1.0), 16.2);
        assert_eq!(contact_force_from_impulse(0.324, 0.01), 16.2);
    }

    #[test]
    fn physics_gate_installs_the_authoritative_solver_resolution_contract() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            avian3d::prelude::PhysicsPlugins::default(),
            PhysicsGatePlugin,
        ));

        assert_eq!(
            app.world().resource::<avian3d::prelude::SubstepCount>().0,
            DEFAULT_SUBSTEP_COUNT
        );
        assert_eq!(DEFAULT_SUBSTEP_COUNT, 8);
    }

    #[test]
    fn live_substep_control_is_bounded_and_keeps_one_source_in_sync() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            avian3d::prelude::PhysicsPlugins::default(),
            PhysicsGatePlugin,
        ));

        set_solver_substeps(app.world_mut(), 16).expect("valid diagnostic value");
        assert_eq!(solver_substeps(app.world()), Some(16));

        assert!(set_solver_substeps(app.world_mut(), 0).is_err());
        assert!(set_solver_substeps(app.world_mut(), MAX_DIAGNOSTIC_SUBSTEP_COUNT + 1).is_err());
        assert_eq!(solver_substeps(app.world()), Some(16));
    }

    #[test]
    fn live_contact_friction_control_validates_and_updates_the_native_component() {
        use avian3d::prelude::Friction;

        let mut world = World::new();
        let entity = world.spawn(Friction::new(0.4)).id();

        assert_eq!(
            contact_friction_snapshot(&world, entity),
            Some(ContactFrictionParameters {
                dynamic: 0.4,
                static_coefficient: 0.4,
            })
        );
        set_contact_friction(
            &mut world,
            entity,
            ContactFrictionParameters {
                dynamic: 0.8,
                static_coefficient: 1.0,
            },
        )
        .expect("valid diagnostic friction");
        let snapshot = contact_friction_snapshot(&world, entity).expect("friction snapshot");
        assert!((snapshot.dynamic - 0.8).abs() < 1e-6);
        assert!((snapshot.static_coefficient - 1.0).abs() < 1e-6);

        assert!(set_contact_friction(
            &mut world,
            entity,
            ContactFrictionParameters {
                dynamic: -0.1,
                static_coefficient: 1.0,
            },
        )
        .is_err());
        assert_eq!(contact_friction_snapshot(&world, entity), Some(snapshot));
    }

    #[test]
    fn live_joint_damping_control_adds_only_avian_native_damping() {
        use avian3d::prelude::{JointDamping, Mass};

        let mut world = World::new();
        let hull = world.spawn_empty().id();
        let leg = world.spawn(Mass(40.0)).id();
        let joint = world
            .spawn(avian3d::prelude::PrismaticJoint::new(hull, leg))
            .id();

        assert_eq!(
            joint_damping_snapshot(&world, joint),
            Some(JointDampingParameters {
                linear: 0.0,
                angular: 0.0,
            })
        );
        set_joint_damping(
            &mut world,
            joint,
            JointDampingParameters {
                linear: 0.5,
                angular: 4.0,
            },
        )
        .expect("valid diagnostic damping");
        assert_eq!(
            world.get::<JointDamping>(joint).copied(),
            Some(JointDamping {
                linear: 0.5,
                angular: 4.0,
            })
        );
        assert!(set_joint_damping(
            &mut world,
            joint,
            JointDampingParameters {
                linear: -0.1,
                angular: 1.0,
            },
        )
        .is_err());
    }

    #[test]
    fn force_drive_conversion_preserves_the_authored_units() {
        let motor = force_drive_motor_model(4000.0, 1600.0, 40.0).expect("valid force drive");
        match motor {
            avian3d::prelude::MotorModel::SpringDamper {
                frequency,
                damping_ratio,
            } => {
                assert!((frequency - 10.0 / std::f64::consts::TAU).abs() < 1e-12);
                assert!((damping_ratio - 2.0).abs() < 1e-12);
            }
            other => panic!("expected implicit force-equivalent drive, got {other:?}"),
        }
    }

    #[test]
    fn force_drive_with_stiffness_requires_generalized_inertia() {
        assert_eq!(
            force_drive_motor_model(4000.0, 1600.0, 0.0),
            Err(ForceDriveMotorError::MissingGeneralizedInertia)
        );
    }

    #[test]
    fn prismatic_drive_can_be_tuned_without_replacing_the_joint() {
        use avian3d::prelude::Mass;

        let mut world = World::new();
        let hull = world.spawn_empty().id();
        let leg = world.spawn(Mass(40.0)).id();
        let joint = world
            .spawn(avian3d::prelude::PrismaticJoint::new(hull, leg))
            .id();

        set_prismatic_drive(
            &mut world,
            joint,
            PrismaticDriveParameters {
                stiffness: 4000.0,
                damping: 1600.0,
                max_force: 12000.0,
            },
        )
        .expect("joint drive should accept dimensional values");
        let snapshot = prismatic_drive_snapshot(&world, joint).expect("drive snapshot");
        assert_eq!(snapshot.model, "force");
        assert!((snapshot.stiffness - 4000.0).abs() < 1e-6);
        assert!((snapshot.damping - 1600.0).abs() < 1e-6);
        assert!((snapshot.max_force - 12000.0).abs() < 1e-6);
    }

    #[test]
    fn invalid_contact_interval_cannot_create_a_load() {
        assert_eq!(contact_force_from_impulse(10.0, 0.0), 0.0);
        assert_eq!(contact_force_from_impulse(f64::NAN, 1.0), 0.0);
    }

    #[test]
    fn holds_are_reason_keyed_and_release_independently() {
        let mut h = PhysicsHolds::default();
        assert!(!h.is_held());
        h.set(PhysicsHolds::TERRAIN_READY, true);
        h.set(PhysicsHolds::READINESS, true);
        assert!(h.is_held());
        // Releasing one leaves the other holding — no subsystem can resume physics
        // on another's behalf.
        h.set(PhysicsHolds::TERRAIN_READY, false);
        assert!(h.is_held());
        assert!(h.holds(PhysicsHolds::READINESS));
        h.set(PhysicsHolds::READINESS, false);
        assert!(!h.is_held());
    }

    #[test]
    fn force_production_is_closed_when_the_solver_cannot_consume_forces() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(Time::<Physics>::default());
        world.insert_resource(Time::<Virtual>::default());

        assert!(world.run_system_once(physics_is_live).unwrap());

        world.resource_mut::<Time<Physics>>().pause();
        assert!(!world.run_system_once(physics_is_live).unwrap());
        world.resource_mut::<Time<Physics>>().unpause();

        world.resource_mut::<Time<Virtual>>().pause();
        assert!(!world.run_system_once(physics_is_live).unwrap());
        world.resource_mut::<Time<Virtual>>().unpause();

        let mut faults = lunco_core::RuntimeFaults::default();
        faults.raise("physics-body-escaped", None, "rover", "out of bounds");
        world.insert_resource(faults);
        assert!(!world.run_system_once(physics_is_live).unwrap());
    }

    /// A queued step lets exactly ONE frame of physics through a hold, then the
    /// clock re-freezes. This is what lets a cutscene script advance the world
    /// deliberately instead of play/pausing it — pausing the world clock would stop
    /// `FixedUpdate` and the script could never run again to unpause itself.
    #[test]
    fn step_grants_exactly_one_frame_through_a_hold() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(PhysicsStepRequest::default());
        world.insert_resource(Time::<Physics>::default());

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::CINEMATIC, true);
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(
            world.resource::<Time<Physics>>().is_paused(),
            "held ⇒ frozen"
        );

        world.resource_mut::<PhysicsStepRequest>().request(1);
        world.run_system_once(grant_physics_step).unwrap();
        assert!(
            !world.resource::<Time<Physics>>().is_paused(),
            "the step frame runs"
        );

        // Debt paid: the next fixed step is frozen again without touching the hold.
        world.run_system_once(grant_physics_step).unwrap();
        assert!(world.resource::<Time<Physics>>().is_paused(), "re-freezes");
        assert_eq!(world.resource::<PhysicsStepRequest>().steps, 0);
    }

    /// The step is consumed in the FIXED loop, not on a render frame. Physics
    /// integrates off `Time<Fixed>`, which only accumulates when virtual time
    /// advanced — so a grant made on a zero-delta render frame would be spent
    /// without any physics running. `apply_physics_holds` (render frame) must
    /// therefore leave the debt alone; only `grant_physics_step` may spend it.
    #[test]
    fn render_frame_projection_does_not_spend_the_step_debt() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(PhysicsStepRequest::default());
        world.insert_resource(Time::<Physics>::default());

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::CINEMATIC, true);
        world.resource_mut::<PhysicsStepRequest>().request(1);

        // Several render frames pass with no fixed step in between.
        for _ in 0..3 {
            world.run_system_once(apply_physics_holds).unwrap();
        }
        assert_eq!(
            world.resource::<PhysicsStepRequest>().steps,
            1,
            "render frames must not burn the step"
        );

        // The fixed step finally runs and spends it.
        world.run_system_once(grant_physics_step).unwrap();
        assert!(!world.resource::<Time<Physics>>().is_paused());
        assert_eq!(world.resource::<PhysicsStepRequest>().steps, 0);
    }

    /// A Modelica result barrier delays a deliberate recording step; it must not
    /// erase that step. Otherwise the capture clock advances while the physical
    /// pose repeats until the next solver result happens to arrive.
    #[test]
    fn modelica_barrier_preserves_cinematic_step_debt() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(PhysicsStepRequest::default());
        world.insert_resource(Time::<Physics>::default());
        world.insert_resource(lunco_core::SimulationBarrier {
            held: true,
            ..Default::default()
        });

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::CINEMATIC, true);
        world.resource_mut::<PhysicsStepRequest>().request(1);
        world.run_system_once(grant_physics_step).unwrap();
        assert!(world.resource::<Time<Physics>>().is_paused());
        assert_eq!(world.resource::<PhysicsStepRequest>().steps, 1);

        world.resource_mut::<lunco_core::SimulationBarrier>().held = false;
        world.run_system_once(grant_physics_step).unwrap();
        assert!(!world.resource::<Time<Physics>>().is_paused());
        assert_eq!(world.resource::<PhysicsStepRequest>().steps, 0);
    }

    /// Steps queued with nothing holding are dropped, not banked — otherwise they
    /// would fire later against an unrelated hold (a terrain bake, say) and leak a
    /// frame of motion into whatever that hold was protecting.
    #[test]
    fn steps_do_not_bank_against_a_future_hold() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(PhysicsStepRequest::default());
        world.insert_resource(Time::<Physics>::default());

        world.resource_mut::<PhysicsStepRequest>().request(3);
        world.run_system_once(grant_physics_step).unwrap();
        assert_eq!(world.resource::<PhysicsStepRequest>().steps, 0);

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, true);
        world.run_system_once(apply_physics_holds).unwrap();
        world.run_system_once(grant_physics_step).unwrap();
        assert!(
            world.resource::<Time<Physics>>().is_paused(),
            "the later hold is not stepped past by stale debt"
        );
    }

    /// The contract: a hold pauses the PHYSICS clock and leaves the virtual clock
    /// (tick → epoch → ephemeris → animation) running. This is what stopped the
    /// sandbox from booting paused, and what keeps the planets moving while a
    /// heightfield bakes.
    #[test]
    fn hold_pauses_physics_clock_only_and_releases_cleanly() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(PhysicsStepRequest::default());
        world.insert_resource(Time::<Physics>::default());
        world.insert_resource(Time::<Virtual>::default());

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, true);
        world
            .resource_mut::<Time<Physics>>()
            .advance_by(Duration::from_millis(16));
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(world.resource::<Time<Physics>>().is_paused());
        assert_eq!(world.resource::<Time<Physics>>().delta(), Duration::ZERO);
        // The virtual clock — and so the tick, the epoch and the celestial chain —
        // is untouched by a physics hold.
        assert!(!world.resource::<Time<Virtual>>().is_paused());

        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, false);
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(!world.resource::<Time<Physics>>().is_paused());
    }

    /// A kinematic drive is an interface-clock operation: it updates the Avian
    /// pose even while the physics clock is paused, and publishes the bounded
    /// contact velocity for the next live solver step.
    #[test]
    fn kinematic_drive_applies_while_physics_clock_is_paused() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(Time::<()>::default());
        world.insert_resource(Time::<Physics>::default());
        world.resource_mut::<Time<Physics>>().pause();
        world
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f64(0.1));

        let target = DVec3::new(10.0, -2.0, 0.5);
        let target_rotation = DQuat::from_rotation_y(0.5);
        let entity = world
            .spawn((
                RigidBody::Kinematic,
                CustomPositionIntegration,
                Position(DVec3::ZERO),
                Rotation(DQuat::IDENTITY),
                LinearVelocity::default(),
                AngularVelocity::default(),
                KinematicDrive::new(target, target_rotation),
            ))
            .id();

        world.run_system_once(apply_kinematic_drives).unwrap();

        assert!(world.resource::<Time<Physics>>().is_paused());
        assert_eq!(world.get::<Position>(entity).unwrap().0, target);
        assert!(
            world
                .get::<Rotation>(entity)
                .unwrap()
                .0
                .angle_between(target_rotation)
                < 1.0e-12
        );
        assert_eq!(world.get::<LinearVelocity>(entity).unwrap().0, target / 0.1);
        assert!((world.get::<AngularVelocity>(entity).unwrap().0.y - 5.0).abs() < 1.0e-12);
    }
}
