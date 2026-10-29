//! Native physics admission, deterministic ordering, continuous contacts and joints.
//!
//! [`PhysicsHolds`] retains independent owner reasons for suspending Avian.
//! [`apply_physics_holds`] combines those reasons with the co-simulation barrier
//! and runtime faults, projecting a paused, zero-delta `Time<Physics>` before
//! force application and solver execution. Each owner releases only its own hold.
//!
//! Required scene, terrain and Modelica preparation also participate in the shared
//! `lunco_core_runtime::SimulationProgress` admission boundary. Their pending work
//! holds the fixed simulation clock, including co-simulation and scenario timers;
//! application, interaction and presentation work can continue. The time owner
//! retains pacing demand and the user's transport state independently of native
//! physics admission.

use lunco_core::AppSceneExt;

use avian3d::prelude::{
    AngularVelocity, ContactGraph, CustomPositionIntegration, LinearVelocity, Position, RigidBody,
    Rotation, Sensor,
};
pub use avian3d::prelude::{Physics, Substeps};
pub use avian3d::schedule::PhysicsTime;
use bevy::ecs::schedule::ApplyDeferred;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use std::time::Duration;

pub mod assembly;
pub mod avian_backend;
pub mod body_mass;
pub mod drive;
pub mod escape;
pub mod force_ports;
pub mod joint;
pub mod kinematics;
pub mod occlusion;
pub mod order;
pub mod pose;
pub mod raycast;
pub mod readiness;
pub mod spatial;
pub mod support;
pub use assembly::{
    DynamicJointIslandMass, DynamicJointIslandMasses, aggregate_dynamic_joint_island_masses,
    dynamic_joint_islands, refresh_dynamic_joint_island_masses,
};
pub use avian_backend::{
    avian_backend_aabb_is_valid, avian_backend_collider_is_leaf,
    avian_backend_collider_shape_is_valid, avian_backend_collider_shape_kind,
    avian_backend_point_is_valid, avian_backend_pose_is_valid, avian_backend_rotation_is_valid,
    avian_backend_vector_is_valid,
};
pub use escape::{
    EscapeDiagnosticPlugin, PhysicsObjectPaused, UnboundedPhysicsDomain, WorldBounds,
    pause_physics_members,
};
pub use order::{
    PhysicsOrderError, PhysicsOrderKey, ordered_physics_entities, report_invalid_physics_order,
};
pub use pose::{
    PhysicsPoseSeeded, PrescribedSimulationPose, SimulationPoseQuery, SimulationPoseReadState,
};
pub use readiness::{Integrable, ReadinessEffectPlugin};
pub use spatial::{GridSpatialQuery, GridSpatialQueryState};
pub use support::{
    PHYSICS_INITIALIZATION_POLICY_HOOK, PhysicsInitializationCheck, PhysicsInitializationDecision,
    PhysicsInitializationExternalValidator, PhysicsInitializationInvalid,
    PhysicsInitializationPaused, PhysicsInitializationPending, PhysicsInitializationPolicy,
    PhysicsInitializationSubject, PhysicsJointDetachRequested, PhysicsJointDetachSet,
    PhysicsJointLink, PhysicsJointPending, PhysicsJointPrepared, PhysicsJointTopologyPending,
    PhysicsSupportContact, PhysicsSupportFootprint, PhysicsSupportSet, PhysicsSupportState,
    PhysicsWheelContact, PhysicsWheelRaycastFilter, STRICT_AUTHORED_INITIALIZATION_POLICY,
    evaluate_initialization_policy, physics_initialization_context, physics_initialization_facts,
};

/// Number of Avian solver substeps in one authoritative fixed physics tick.
///
/// The fixed tick remains owned by `lunco-core::FIXED_HZ`; this value controls
/// only the solver resolution inside that tick. It is deliberately one
/// cross-platform contract: changing solver resolution by target architecture
/// changes the physical result and cannot be a hidden application fallback.
///
pub const DEFAULT_SUBSTEP_COUNT: u32 = 8;

/// Native CCD configuration for an explicitly admitted continuous body.
/// Speculative contacts expand moving broad-phase bounds before solving;
/// nonlinear sweeps additionally catch motion past thin or rotating geometry.
/// Both mechanisms are disabled when the authored body opts out.
pub fn continuous_collision_components(
    enabled: bool,
) -> (avian3d::prelude::SweptCcd, avian3d::prelude::SpeculativeCcd) {
    use avian3d::prelude::{CcdFilter, SpeculativeCcd, SweptCcd};
    (
        SweptCcd::default().with_filter(if enabled {
            CcdFilter::ALL
        } else {
            CcdFilter::NONE
        }),
        if enabled {
            SpeculativeCcd::MAX
        } else {
            SpeculativeCcd::ZERO
        },
    )
}

/// Bounds for the live solver-resolution diagnostic control.
///
/// These are deliberately separate from [`DEFAULT_SUBSTEP_COUNT`]. A scenario
/// may temporarily vary the Avian resolution while diagnosing a scene, but the
/// authored/runtime contract remains the default above unless the application
/// owner changes that default in this crate.
pub const MIN_DIAGNOSTIC_SUBSTEP_COUNT: u32 = 1;
pub const MAX_DIAGNOSTIC_SUBSTEP_COUNT: u32 = 64;

/// Effective Bevy compute-pool width observed by the physics composition.
///
/// This reports the execution profile; it does not claim that the physics
/// result or whole simulation is deterministic.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhysicsComputeProfile {
    /// Effective compute-pool width observed at application composition.
    /// `None` means the pool was unavailable when the profile was captured.
    pub compute_threads: Option<usize>,
}

impl PhysicsComputeProfile {
    /// Capture the effective compute-pool width observed by the host.
    pub const fn from_compute_threads(compute_threads: Option<usize>) -> Self {
        Self { compute_threads }
    }
}

/// Read the live Avian solver resolution.
pub fn solver_substeps(world: &World) -> Option<u32> {
    world
        .get_resource::<avian3d::prelude::SubstepCount>()
        .map(|count| count.0)
}

/// Fingerprint the live physics solver clock profile used by the current
/// application. Scene and body configuration remain represented by their
/// owning document/provider generations in evidence records.
pub fn solver_configuration_fingerprint(world: &World) -> Option<u64> {
    let substeps = u64::from(solver_substeps(world)?);
    let fixed_step_ns =
        u64::try_from(world.get_resource::<Time<Fixed>>()?.timestep().as_nanos()).ok()?;
    if fixed_step_ns == 0 || substeps == 0 {
        return None;
    }
    let mut fingerprint = lunco_hash::Fnv1a::new();
    fingerprint.write_bytes(b"lunco.physics.solver-clock.v2");
    fingerprint.write_u64(substeps);
    fingerprint.write_u64(fixed_step_ns);
    fingerprint.write_u64(u64::from(
        world
            .get_resource::<avian3d::dynamics::solver::xpbd::JointVelocitySolverConfig>()?
            .iterations
            .get(),
    ));
    Some(fingerprint.finish())
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

/// Read native joint/contact velocity resolution. This is also included in the
/// solver configuration fingerprint; it is independent of wheel/soil friction.
pub fn joint_velocity_iterations(world: &World) -> Option<u8> {
    world
        .get_resource::<avian3d::dynamics::solver::xpbd::JointVelocitySolverConfig>()
        .map(|config| config.iterations.get())
}

/// Select an explicit native relaxation budget for a numerical convergence run.
pub fn set_joint_velocity_iterations(world: &mut World, count: u32) -> Result<(), String> {
    let count = u8::try_from(count)
        .ok()
        .and_then(core::num::NonZeroU8::new)
        .ok_or_else(|| "joint velocity iterations must be in 1..=255".to_string())?;
    let mut config = world
        .get_resource_mut::<avian3d::dynamics::solver::xpbd::JointVelocitySolverConfig>()
        .ok_or_else(|| "Avian joint/contact velocity solver is not installed".to_string())?;
    config.iterations = count;
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
        || friction.dynamic > f64::from(f32::MAX)
        || friction.static_coefficient > f64::from(f32::MAX)
    {
        return Err(format!(
            "invalid contact friction: coefficients must be finite, non-negative, and representable by Avian, got {friction:?}"
        ));
    }
    let Some(mut value) = world.get_mut::<Friction>(entity) else {
        return Err(format!("entity {entity:?} has no Avian Friction component"));
    };
    value.dynamic_coefficient = friction.dynamic as f32;
    value.static_coefficient = friction.static_coefficient as f32;
    Ok(())
}

/// Read one entity's live Avian contact friction, if authored/projected.
pub fn contact_friction_snapshot(
    world: &World,
    entity: Entity,
) -> Option<ContactFrictionParameters> {
    let value = world.get::<avian3d::prelude::Friction>(entity)?;
    Some(ContactFrictionParameters {
        dynamic: f64::from(value.dynamic_coefficient),
        static_coefficient: f64::from(value.static_coefficient),
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

/// Add or replace passive relative joint damping for a live diagnostic
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
        || damping.linear > f64::from(f32::MAX)
        || damping.angular > f64::from(f32::MAX)
    {
        return Err(format!(
            "invalid joint damping: rates must be finite, non-negative, and representable by Avian, got {damping:?}"
        ));
    }
    if !has_supported_joint(world, entity) {
        return Err(format!("entity {entity:?} has no supported Avian joint"));
    }

    let value = avian3d::prelude::JointDamping {
        linear: damping.linear as f32,
        angular: damping.angular as f32,
    };
    if let Some(mut existing) = world.get_mut::<avian3d::prelude::JointDamping>(entity) {
        *existing = value;
    } else {
        world.entity_mut(entity).insert(value);
    }
    Ok(())
}

/// Read passive relative joint damping. An installed joint without a
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
        linear: f64::from(value.linear),
        angular: f64::from(value.angular),
    })
}

/// Aggregate the live Avian contacts owned by one physics entity.
///
/// This is deliberately a read-only diagnostic at the physics boundary. It
/// reports the effective manifold coefficient and the impulses Avian retained
/// for the next warm start; it does not infer a force or mutate the solver.
/// `pair_count` includes overlapping AABBs, while `touching_pair_count` and the
/// native impulse fields include only actual touching contacts. Continuous
/// response also includes impacts within the completed step before discrete preparation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContactDebugSnapshot {
    /// Contact colliders and available compound-part evidence, including continuous impacts.
    pub parts: Vec<ContactPartDebugSnapshot>,
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

/// Collider identity, available composite membership and applied response.
#[derive(Clone, Debug, PartialEq)]
pub struct ContactPartDebugSnapshot {
    pub collider1: Entity,
    pub collider2: Entity,
    /// Stock discrete manifolds do not retain composite-part identity.
    pub part1: Option<usize>,
    pub part2: Option<usize>,
    pub normal_impulse: f64,
}

/// Authored source entity whose observations this cooked collider supplies.
/// Native body attachment remains owned by Avian's `ColliderOf` relationship.
#[derive(Component, Debug)]
#[relationship(relationship_target = SourceColliders)]
pub struct ColliderSourceEntity(pub Entity);

/// Cooked colliders belonging to one authored shape. Bevy maintains membership
/// and removes retired source links without a parallel contact-state cache.
#[derive(Component, Debug, Default)]
#[relationship_target(relationship = ColliderSourceEntity)]
pub struct SourceColliders(Vec<Entity>);

/// Native colliders for a body, standalone collider, or authored shape source.
/// Body observations include every attached shape; a source observes only its
/// own cooked geometry. Nested rigid bodies remain separate owners.
pub fn collider_entities(world: &World, entity: Entity) -> impl Iterator<Item = Entity> + '_ {
    let body_colliders = world.get::<avian3d::prelude::RigidBodyColliders>(entity);
    std::iter::once(entity)
        .filter(move |candidate| {
            world
                .get::<avian3d::prelude::Collider>(*candidate)
                .is_some()
        })
        .chain(
            body_colliders
                .into_iter()
                .flat_map(|colliders| colliders.iter())
                .filter(move |candidate| {
                    *candidate != entity
                        && world
                            .get::<avian3d::prelude::Collider>(*candidate)
                            .is_some()
                }),
        )
        .chain(
            world
                .get::<SourceColliders>(entity)
                .into_iter()
                .filter(move |_| body_colliders.is_none())
                .flat_map(|sources| sources.0.iter().copied())
                .filter(move |candidate| {
                    *candidate != entity
                        && world
                            .get::<avian3d::prelude::Collider>(*candidate)
                            .is_some()
                }),
        )
}

pub fn contact_debug_snapshot(world: &World, entity: Entity) -> Option<ContactDebugSnapshot> {
    contact_debug_snapshot_filtered(world, entity, None)
}

/// Narrow-phase evidence restricted to the other body's hierarchy, including
/// independently simulated wheels. Attachment joints do not widen this scope.
pub fn contact_debug_between(
    world: &World,
    entity: Entity,
    other: Entity,
) -> Option<ContactDebugSnapshot> {
    contact_debug_snapshot_filtered(world, entity, Some(other))
}
fn contact_debug_snapshot_filtered(
    world: &World,
    entity: Entity,
    other: Option<Entity>,
) -> Option<ContactDebugSnapshot> {
    use avian3d::prelude::{AngularVelocity, ContactGraph, LinearVelocity};
    let belongs = |mut child: Entity, root: Entity| {
        loop {
            if child == root {
                return true;
            }
            let Some(parent) = world.get::<ChildOf>(child) else {
                return false;
            };
            child = parent.parent();
        }
    };

    let graph = world.get_resource::<ContactGraph>()?;
    let mut snapshot = ContactDebugSnapshot::default();
    for pair in
        collider_entities(world, entity).flat_map(|collider| graph.contact_pairs_with(collider))
    {
        if other.is_some_and(|root| {
            ![pair.collider1, pair.collider2]
                .into_iter()
                .any(|candidate| belongs(candidate, root))
        }) {
            continue;
        }
        snapshot.pair_count += 1;
        if !pair.is_touching() {
            continue;
        }
        snapshot.touching_pair_count += 1;
        for manifold in &pair.manifolds {
            snapshot.parts.push(ContactPartDebugSnapshot {
                collider1: pair.collider1,
                collider2: pair.collider2,
                part1: None,
                part2: None,
                normal_impulse: f64::from(manifold.total_normal_impulse()),
            });
            snapshot.manifold_count += 1;
            snapshot.max_friction = snapshot.max_friction.max(f64::from(manifold.friction));
            snapshot.contact_normal = manifold.normal.as_dvec3().to_array();
            let normal_impulse = (manifold.normal * manifold.total_normal_impulse()).as_dvec3();
            snapshot.normal_impulse_vector[0] += normal_impulse.x;
            snapshot.normal_impulse_vector[1] += normal_impulse.y;
            snapshot.normal_impulse_vector[2] += normal_impulse.z;
            for point in &manifold.points {
                snapshot.total_normal_impulse += f64::from(point.normal_impulse);
                snapshot.max_normal_speed = snapshot
                    .max_normal_speed
                    .max(f64::from(point.normal_speed.abs()));
                snapshot.max_penetration =
                    snapshot.max_penetration.max(f64::from(point.penetration));
                let velocity_at = |body: Option<Entity>, anchor: Vec3| {
                    let Some(body) = body else {
                        return Vec3::ZERO;
                    };
                    let linear = world
                        .get::<LinearVelocity>(body)
                        .map_or(Vec3::ZERO, |velocity| velocity.0);
                    let angular = world
                        .get::<AngularVelocity>(body)
                        .map_or(Vec3::ZERO, |velocity| velocity.0);
                    linear + angular.cross(anchor)
                };
                let relative =
                    velocity_at(pair.body2, point.anchor2) - velocity_at(pair.body1, point.anchor1);
                let tangent = relative - manifold.normal * relative.dot(manifold.normal);
                snapshot.max_tangent_speed =
                    snapshot.max_tangent_speed.max(f64::from(tangent.length()));
                snapshot.total_warm_start_tangent_impulse +=
                    f64::from(point.warm_start_tangent_impulse.length());
                snapshot.contact_point = [point.point.x, point.point.y, point.point.z];
            }
        }
    }
    Some(snapshot)
}

/// Dimensional drive coefficients for one native linear or angular coordinate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointDriveParameters {
    /// N/m for a slider; N m/rad for a hinge.
    pub stiffness: f64,
    /// N s/m for a slider; N m s/rad for a hinge.
    pub damping: f64,
    /// Force in N for a slider; torque in N m for a hinge.
    pub max_effort: f64,
}

/// Why a dimensional force drive cannot be lowered to Avian's implicit model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForceDriveMotorError {
    /// Targets/gains are invalid, or the force limit is negative or NaN.
    InvalidParameters,
}

/// Validate dimensional coefficients and retain their native motor metadata.
pub fn force_drive_motor_model(
    stiffness: f64,
    damping: f64,
) -> Result<avian3d::prelude::MotorModel, ForceDriveMotorError> {
    if !stiffness.is_finite()
        || !damping.is_finite()
        || stiffness < 0.0
        || damping < 0.0
        || stiffness > f64::from(f32::MAX)
        || damping > f64::from(f32::MAX)
    {
        return Err(ForceDriveMotorError::InvalidParameters);
    }
    Ok(avian3d::prelude::MotorModel::ForceBased {
        stiffness: stiffness as f32,
        damping: damping as f32,
    })
}

/// Read dimensional force coefficients. Frequency and acceleration models do
/// not carry newton coefficients and must not be converted using a body's mass.
pub fn motor_model_force_coefficients(model: avian3d::prelude::MotorModel) -> Option<(f64, f64)> {
    match model {
        avian3d::prelude::MotorModel::ForceBased { stiffness, damping } => {
            Some((f64::from(stiffness), f64::from(damping)))
        }
        _ => None,
    }
}

/// A live coordinate-drive readout in SI units. Angular values use radians,
/// including stiffness/damping; USD degree conversion belongs to its reader.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointDriveSnapshot {
    pub angular: bool,
    pub enabled: bool,
    /// `"force"`; this readout exists only for dimensional force drives.
    pub model: &'static str,
    pub target_position: f64,
    pub target_velocity: f64,
    pub stiffness: f64,
    pub damping: f64,
    pub max_effort: f64,
}

/// Replace a joint's dimensional force/torque-drive coefficients in place.
///
/// The target position/velocity are intentionally preserved: position commands
/// remain on the joint's existing co-simulation ports, while this operation
/// changes only the physical drive law. The joint entity must already carry the
/// native Avian joint; no replacement constraint is created.
pub fn set_joint_drive(
    world: &mut World,
    entity: Entity,
    drive: JointDriveParameters,
) -> Result<(), String> {
    use avian3d::prelude::{PrismaticJoint, RevoluteJoint};

    if !drive.stiffness.is_finite()
        || !drive.damping.is_finite()
        || drive.max_effort.is_nan()
        || drive.stiffness < 0.0
        || drive.damping < 0.0
        || drive.max_effort < 0.0
        || (drive.max_effort.is_finite() && drive.max_effort > f64::from(f32::MAX))
    {
        return Err(format!(
            "invalid joint drive: stiffness/damping must be non-negative and max_effort non-negative, got {drive:?}"
        ));
    }
    let model = force_drive_motor_model(drive.stiffness, drive.damping)
        .map_err(|error| format!("cannot lower joint force drive: {error:?}"))?;
    let linear = world.get::<PrismaticJoint>(entity).is_some();
    let angular = world.get::<RevoluteJoint>(entity).is_some();
    if linear == angular {
        return Err(format!(
            "entity {entity:?} must have exactly one prismatic or revolute joint"
        ));
    }
    // Explicit Avian scalar boundary, after finite/range validation above.
    let effort = drive.max_effort as f32;
    let bodies = if linear {
        let mut joint = world
            .get_mut::<PrismaticJoint>(entity)
            .expect("validated slider");
        if joint.motor.enabled
            && joint.motor.motor_model == model
            && joint.motor.max_force == effort
        {
            return Ok(());
        }
        joint.motor.enabled = true;
        joint.motor.motor_model = model;
        joint.motor.max_force = effort;
        [joint.body1, joint.body2]
    } else {
        let mut joint = world
            .get_mut::<RevoluteJoint>(entity)
            .expect("validated hinge");
        if joint.motor.enabled
            && joint.motor.motor_model == model
            && joint.motor.max_torque == effort
        {
            return Ok(());
        }
        joint.motor.enabled = true;
        joint.motor.motor_model = model;
        joint.motor.max_torque = effort;
        [joint.body1, joint.body2]
    };
    joint::wake_sleeping_joint_bodies(world, bodies);
    Ok(())
}

/// Read a force-based coordinate drive's native dimensional coefficients.
/// Frequency and acceleration models do not expose force coefficients.
pub fn joint_drive_snapshot(world: &World, entity: Entity) -> Option<JointDriveSnapshot> {
    use avian3d::prelude::{PrismaticJoint, RevoluteJoint};

    let linear = world.get::<PrismaticJoint>(entity);
    let angular = world.get::<RevoluteJoint>(entity);
    let (is_angular, enabled, model, position, velocity, effort) = match (linear, angular) {
        (Some(joint), None) => (
            false,
            joint.motor.enabled,
            joint.motor.motor_model,
            joint.motor.target_position,
            joint.motor.target_velocity,
            joint.motor.max_force,
        ),
        (None, Some(joint)) => (
            true,
            joint.motor.enabled,
            joint.motor.motor_model,
            joint.motor.target_position,
            joint.motor.target_velocity,
            joint.motor.max_torque,
        ),
        _ => return None,
    };
    let (stiffness, damping) = motor_model_force_coefficients(model)?;
    Some(JointDriveSnapshot {
        angular: is_angular,
        enabled,
        model: "force",
        target_position: f64::from(position),
        target_velocity: f64::from(velocity),
        stiffness,
        damping,
        max_effort: f64::from(effort),
    })
}

/// Convert Avian's accumulated normal contact impulse to the physical load
/// delivered over one master physics interval.
///
/// Avian owns all internal substeps. The co-simulation master owns the
/// communication interval; callers must not divide by a substep count or run a
/// second participant loop.
#[inline]
pub fn contact_force_from_impulse(normal_impulse: f64, physics_dt: f64) -> f64 {
    if normal_impulse.is_finite() && physics_dt > 0.0 {
        normal_impulse / physics_dt
    } else {
        0.0
    }
}

/// Whether a collider is touching anything, and the total contact normal
/// force (N) on it, as computed by Avian's contact graph.
///
/// `contact_pairs_with` includes pairs whose AABBs overlap without touching;
/// only pairs for which Avian reports `is_touching()` contribute to the result.
/// The force conversion uses the native representation of the master physics
/// interval and the physical impulse contract in [`contact_force_from_impulse`].
/// The graph retains the last solved interval. Admission can zero Avian's
/// current delta while awaiting a participant or pausing; that scheduling
/// permission must not change the physical meaning of an already solved impulse.
pub fn contact_of(
    graph: &ContactGraph,
    colliders: impl IntoIterator<Item = Entity>,
) -> (bool, f64) {
    contact_of_filtered(graph, colliders, |_| false)
}

/// Read contact while excluding explicitly authored overlap-only colliders.
/// The predicate is supplied by the caller because `ContactGraph` stores pair
/// topology, not ECS component semantics.
fn contact_of_filtered(
    graph: &ContactGraph,
    colliders: impl IntoIterator<Item = Entity>,
    is_sensor: impl Fn(Entity) -> bool,
) -> (bool, f64) {
    let mut normal_impulse = 0.0;
    let mut touching = false;
    for pair in colliders
        .into_iter()
        .flat_map(|collider| graph.contact_pairs_with(collider))
    {
        if is_sensor(pair.collider1) || is_sensor(pair.collider2) || !pair.is_touching() {
            continue;
        }
        touching = true;
        for manifold in &pair.manifolds {
            for point in &manifold.points {
                normal_impulse += f64::from(point.normal_impulse);
            }
        }
    }
    (
        touching,
        contact_force_from_impulse(
            normal_impulse,
            Duration::from_secs_f64(lunco_core_runtime::SECS_PER_TICK).as_secs_f64(),
        ),
    )
}

/// Read contact for a caller holding only a `&World`.
///
/// Contact timing belongs to [`contact_of`], independently of live admission.
/// A missing graph means physics has not started and has no contact sample.
pub fn contact_from_world(world: &World, entity: Entity) -> (bool, f64) {
    let Some(graph) = world.get_resource::<ContactGraph>() else {
        return (false, 0.0);
    };
    let (touching, force) =
        contact_of_filtered(graph, collider_entities(world, entity), |candidate| {
            world.get::<Sensor>(candidate).is_some()
        });
    (touching, force)
}

/// Maximum closing normal speed from the native pre-solver contact samples.
/// Separating and absent solid contacts contribute zero. Missing native graph
/// state remains unavailable, so a consumer cannot admit a fabricated sample.
pub fn contact_approach_speed_from_world(world: &World, entity: Entity) -> Option<f64> {
    let graph = world.get_resource::<ContactGraph>()?;
    Some(
        collider_entities(world, entity)
            .flat_map(|collider| graph.contact_pairs_with(collider))
            .filter(|pair| {
                pair.is_touching()
                    && world.get::<Sensor>(pair.collider1).is_none()
                    && world.get::<Sensor>(pair.collider2).is_none()
            })
            .flat_map(|pair| &pair.manifolds)
            .flat_map(|manifold| &manifold.points)
            .map(|point| f64::from(-point.normal_speed).max(0.0))
            .fold(0.0, f64::max),
    )
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

        let rotation_delta = target_rotation * rotation.0.as_dquat().inverse();
        if rotation_delta.angle_between(DQuat::IDENTITY) > 1.0e-12 {
            rotation.0 = target_rotation.as_quat();
        }

        if dt > 1e-6 {
            linear.0 = (position_delta / dt)
                .clamp_length_max(MAX_KINEMATIC_DRIVE_SPEED)
                .as_vec3();
            angular.0 = (rotation_delta.to_scaled_axis() / dt)
                .clamp_length_max(MAX_KINEMATIC_DRIVE_SPEED)
                .as_vec3();
        } else {
            linear.0 = Vec3::ZERO;
            angular.0 = Vec3::ZERO;
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
    /// Something the world needs is not ready yet — a scene still composing, a
    /// program still compiling. Raised from [`lunco_readiness`] by
    /// [`readiness::apply_world_readiness_hold`]; the *scope* of a wait (world vs
    /// one object) is an authored policy decision, not a fact of the wait.
    pub const READINESS: &'static str = "readiness";
    /// A composed physical body or its constraints have not crossed the
    /// deterministic admission boundary yet. The USD simulation projector owns
    /// this reason; integration resumes only after the complete pending
    /// admission set is ready.
    pub const BODY_ADMISSION: &'static str = "body-admission";
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

/// Run condition: **the solver will consume forces applied this tick.**
///
/// Every system that writes into Avian's transient force accumulator (suspension,
/// wheel drive, thrusters, …) must be gated on this. Avian clears that accumulator
/// *inside* the physics step, so a force applied while the step is skipped is never
/// cleared and is added to the next live step. Persistent acceleration components,
/// such as the environment's `ConstantLinearAcceleration` projection, are not
/// transient force writes and must not use this gate.
///
/// Gating on `Time<Physics>` rather than on [`PhysicsHolds`] is deliberate: it is
/// the clock the solver actually integrates, so this is also correct for anything
/// that pauses physics out of band, exactly as [`apply_physics_holds`] is.
///
/// Native admission and the shared pacing clock must both be live before a
/// transient force can be written. Shared manual steps open both clocks for
/// their admitted cycle.
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
/// The paused flag and zero delta gate native force application and integration.
/// Shared simulation admission and the time owner retain the global clock and
/// pacing demand. Projection runs in `PreUpdate` and at the admitted fixed-step
/// boundary: an asynchronous solver result can release the coupling barrier
/// after PreUpdate, so forces and integration must consume the same state.
pub fn apply_physics_holds(
    holds: Res<PhysicsHolds>,
    coupling: Option<Res<lunco_core_runtime::SimulationBarrier>>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    mut physics_time: ResMut<Time<Physics>>,
    mut progress: Option<ResMut<lunco_core_runtime::SimulationProgress>>,
    virtual_time: Option<Res<Time<Virtual>>>,
) {
    if let Some(progress) = progress.as_mut() {
        let key = lunco_core_runtime::SimulationProgressKey::physics_integration();
        let preparing = holds.is_held()
            && !faults
                .as_deref()
                .is_some_and(lunco_core::RuntimeFaults::active);
        if preparing != progress.contains(key) {
            if preparing {
                progress.acquire(key, "Native physics integration is held");
            } else {
                progress.release(key);
            }
        }
    }
    let held = holds.is_held()
        || virtual_time.is_some_and(|time| time.is_paused())
        || coupling.is_some_and(|state| state.held)
        || faults.is_some_and(|state| state.active());
    if held {
        // `pause()` changes Avian's flag but leaves the previous generic
        // `Time<Physics>` delta intact. Its nested driver would then execute
        // one stale solver pass on the first held FixedPostUpdate. A hold is
        // therefore a paused *and zero-delta* boundary.
        if !physics_time.is_paused() || !physics_time.delta().is_zero() {
            physics_time.pause();
            physics_time.advance_by(Duration::ZERO);
        }
    } else if physics_time.is_paused() {
        physics_time.unpause();
    }
}

/// Validate the native clock against the shared fixed cycle. Before native
/// integration, an active cycle may still expose the preceding completed time.
/// Completed boundaries require exact agreement in the clocks' native Duration.
pub fn validate_physics_clock(
    physics_time: &Time<Physics>,
    fixed_time: &Time<Fixed>,
    cycle_in_progress: bool,
) -> Result<(), String> {
    lunco_core_runtime::simulation_timestep_seconds(fixed_time)?;
    let upper = fixed_time.elapsed();
    let lower = if cycle_in_progress {
        upper.saturating_sub(fixed_time.timestep())
    } else {
        upper
    };
    let actual = physics_time.elapsed();
    if actual < lower || actual > upper {
        return Err(format!(
            "native physics time {actual:?} is outside the shared fixed-cycle interval [{lower:?}, {upper:?}]"
        ));
    }
    Ok(())
}

fn verify_completed_physics_clock(
    physics_time: Res<Time<Physics>>,
    fixed_time: Res<Time<Fixed>>,
    control: Option<Res<lunco_time::SimulationStepControl>>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
) {
    if control.is_none() || faults.active() {
        return;
    }
    if let Err(error) = validate_physics_clock(&physics_time, &fixed_time, false) {
        error!("[physics] {error}");
        faults.raise(
            "simulation-physics-clock-diverged",
            None,
            "shared physics clock",
            error,
        );
    }
}

/// Clear all scene-owned physics admission state before the outgoing entities
/// are reclaimed. The replacement scene rebuilds its readiness/hold decisions
/// from its own authored and runtime state. The time owner separately retires
/// shared deliberate-step demand.
fn reset_scene_physics_state(
    mut progress: Option<ResMut<lunco_core_runtime::SimulationProgress>>,
    mut physics_time: Option<ResMut<Time<Physics>>>,
) {
    if let Some(progress) = progress.as_deref_mut() {
        progress.release(lunco_core_runtime::SimulationProgressKey::physics_integration());
    }
    if let Some(mut physics_time) = physics_time.take() {
        physics_time.unpause();
        physics_time.advance_by(Duration::ZERO);
    }
}

/// Admit a finite authored pose in a scene with no domain-specific surface
/// validator. Terrain installs the external-validator flag and owns the richer
/// support/penetration check instead.
fn validate_surface_independent_initialization(
    external_validator: Res<PhysicsInitializationExternalValidator>,
    coordinator: Option<Res<lunco_core::SceneTransitionCoordinator>>,
    q_pending: Query<
        (
            Entity,
            &Position,
            &Rotation,
            Option<&PhysicsInitializationPolicy>,
            Option<&PhysicsInitializationSubject>,
        ),
        (
            With<PhysicsInitializationPending>,
            Without<PhysicsInitializationInvalid>,
            With<PhysicsPoseSeeded>,
        ),
    >,
    mut commands: Commands,
    mut diagnostics: ResMut<lunco_core::RuntimeDiagnostics>,
) {
    if external_validator.0 {
        return;
    }
    let mut findings = Vec::new();
    for (entity, position, rotation, policy, subject) in &q_pending {
        let subject = subject.map(|subject| subject.0.as_str());
        let subject_label = subject.unwrap_or("<unidentified physics body>");
        let policy = policy.cloned().unwrap_or_default();
        if !avian_backend::avian_backend_pose_is_valid(position.0, rotation.0.as_dquat()) {
            findings.push(lunco_core::RuntimeDiagnostic {
                code: "physics-initialization-backend-invalid".to_string(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "physics-initialization".to_string(),
                subject: subject_label.to_string(),
                message: "authored initial pose requires a finite f32-representable position and a unit rotation; dynamic admission remains held".to_string(),
            });
            continue;
        }
        let result = if policy.is_strict_authored() {
            Ok(support::PhysicsInitializationDecision::Accept)
        } else if let Some(subject) = subject {
            let facts = physics_initialization_facts(
                &policy,
                subject,
                position.0,
                1,
                support::PhysicsInitializationCheck::AuthoredPose,
            );
            evaluate_initialization_policy(
                &policy,
                facts,
                physics_initialization_context(coordinator.as_deref()),
            )
        } else {
            Err(
                "custom physics initialization policy requires a stable USD prim subject path"
                    .to_string(),
            )
        };
        match result {
            Ok(support::PhysicsInitializationDecision::Accept) => {
                commands
                    .entity(entity)
                    .try_remove::<PhysicsInitializationPending>();
            }
            Ok(support::PhysicsInitializationDecision::Pause) => {
                findings.push(lunco_core::RuntimeDiagnostic {
                    code: "physics-initialization-policy".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "physics-initialization".to_string(),
                    subject: subject_label.to_string(),
                    message: format!(
                        "initialization policy `{}` requested a pause without a validated support penetration; authored pose remains held",
                        policy.0
                    ),
                });
            }
            Err(message) => findings.push(lunco_core::RuntimeDiagnostic {
                code: "physics-initialization-policy".to_string(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "physics-initialization".to_string(),
                subject: subject_label.to_string(),
                message,
            }),
        }
    }
    diagnostics.replace_producer("physics-initialization", findings);
}

/// Installs the physics readiness gate. Add wherever `avian3d`'s `PhysicsPlugins`
/// are added — whoever owns physics owns its gate.
pub struct PhysicsGatePlugin;

impl Plugin for PhysicsGatePlugin {
    fn build(&self, app: &mut App) {
        pose::register_spatial_query_providers(app);
        kinematics::install(app);
        app.register_type::<avian3d::prelude::SleepThreshold>()
            .register_type::<PhysicsInitializationPolicy>()
            .register_type::<PhysicsObjectPaused>()
            .register_type::<UnboundedPhysicsDomain>()
            .register_type::<PhysicsOrderKey>()
            .register_type::<PhysicsInitializationPending>()
            .register_type::<PhysicsInitializationInvalid>()
            .register_type::<PhysicsInitializationPaused>()
            .register_type::<PhysicsInitializationSubject>()
            .register_type::<PhysicsSupportFootprint>()
            .register_type::<PhysicsSupportContact>()
            .register_type::<PhysicsSupportState>()
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
            .init_resource::<PhysicsComputeProfile>()
            .init_scene_resource::<PhysicsHolds>()
            .init_scene_resource_with::<ContactGraph>(ContactGraph::clear)
            .init_scene_resource_with::<avian3d::dynamics::solver::constraint_graph::ConstraintGraph>(
                avian3d::dynamics::solver::constraint_graph::ConstraintGraph::clear,
            )
            .init_resource::<PhysicsInitializationExternalValidator>()
            .init_resource::<lunco_core::RuntimeFaults>()
            .init_resource::<lunco_core::RuntimeDiagnostics>()
            .add_systems(
                Update,
                validate_surface_independent_initialization.in_set(PhysicsSupportSet::Consume),
            )
            .add_systems(
                PreUpdate,
                apply_physics_holds.in_set(lunco_time::SimulationAdmissionSet),
            )
            .add_systems(lunco_core::SceneTeardown, reset_scene_physics_state)
            .add_systems(FixedLast, verify_completed_physics_clock)
            // Refresh native admission at the shared fixed-cycle boundary.
            .add_systems(bevy::prelude::FixedPreUpdate, apply_physics_holds)
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
    fn contact_impulse_is_physical_momentum_over_interval() {
        assert!((contact_force_from_impulse(16.2, 1.0) - 16.2).abs() < 1.0e-12);
        assert!((contact_force_from_impulse(0.162, 0.01) - 16.2).abs() < 1.0e-12);
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
    fn terrain_free_scene_admits_finite_authored_pose_without_rewriting_it() {
        let mut app = App::new();
        app.init_resource::<PhysicsInitializationExternalValidator>()
            .init_resource::<lunco_core::RuntimeDiagnostics>()
            .add_systems(Update, validate_surface_independent_initialization);
        let authored = DVec3::new(3.0, 42.0, -7.0);
        let entity = app
            .world_mut()
            .spawn((
                Position(authored),
                Rotation(Quat::IDENTITY),
                PhysicsPoseSeeded,
                PhysicsInitializationPending,
                PhysicsInitializationPolicy::default(),
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<Position>(entity).unwrap().0, authored);
        assert!(
            app.world()
                .get::<PhysicsInitializationPending>(entity)
                .is_none()
        );
    }

    #[test]
    fn non_finite_authored_pose_is_reported_and_remains_pending() {
        let mut app = App::new();
        app.init_resource::<PhysicsInitializationExternalValidator>()
            .init_resource::<lunco_core::RuntimeDiagnostics>()
            .add_systems(Update, validate_surface_independent_initialization);
        let authored = DVec3::new(f64::NAN, 42.0, -7.0);
        let entity = app
            .world_mut()
            .spawn((
                Position(authored),
                Rotation(Quat::IDENTITY),
                PhysicsPoseSeeded,
                PhysicsInitializationPending,
                PhysicsInitializationPolicy::default(),
            ))
            .id();

        app.update();

        assert!(
            app.world()
                .get::<PhysicsInitializationPending>(entity)
                .is_some()
        );
        assert_eq!(
            app.world().resource::<lunco_core::RuntimeDiagnostics>().findings,
            vec![lunco_core::RuntimeDiagnostic {
                code: "physics-initialization-backend-invalid".into(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "physics-initialization".into(),
                subject: "<unidentified physics body>".into(),
                message: "authored initial pose requires a finite f32-representable position and a unit rotation; dynamic admission remains held".into(),
            }]
        );
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
                dynamic: f64::from(0.4_f32),
                static_coefficient: f64::from(0.4_f32),
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

        assert!(
            set_contact_friction(
                &mut world,
                entity,
                ContactFrictionParameters {
                    dynamic: -0.1,
                    static_coefficient: 1.0,
                },
            )
            .is_err()
        );
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
        assert!(
            set_joint_damping(
                &mut world,
                joint,
                JointDampingParameters {
                    linear: -0.1,
                    angular: 1.0,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn force_drive_coefficients_remain_dimensional() {
        let motor = force_drive_motor_model(4000.0, 1600.0).expect("valid force drive");
        assert_eq!(
            motor_model_force_coefficients(motor),
            Some((4000.0, 1600.0))
        );
        assert!(force_drive_motor_model(f64::NAN, 1.0).is_err());
        assert!(force_drive_motor_model(1.0, -1.0).is_err());
        assert_eq!(
            motor_model_force_coefficients(avian3d::prelude::MotorModel::SpringDamper {
                frequency: 3.0,
                damping_ratio: 2.0
            }),
            None
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

        set_joint_drive(
            &mut world,
            joint,
            JointDriveParameters {
                stiffness: 4000.0,
                damping: 1600.0,
                max_effort: 12000.0,
            },
        )
        .expect("joint drive should accept dimensional values");
        let snapshot = joint_drive_snapshot(&world, joint).expect("drive snapshot");
        assert_eq!(snapshot.model, "force");
        assert!((snapshot.stiffness - 4000.0).abs() < 1e-6);
        assert!((snapshot.damping - 1600.0).abs() < 1e-6);
        assert!((snapshot.max_effort - 12000.0).abs() < 1e-6);
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

    #[test]
    fn fixed_admission_refreshes_a_barrier_released_after_preupdate() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.init_scene_resource::<PhysicsHolds>()
            .init_resource::<lunco_core_runtime::SimulationBarrier>()
            .init_resource::<Time<Physics>>()
            .init_resource::<Time<Virtual>>()
            .add_systems(FixedPreUpdate, apply_physics_holds);
        app.world_mut()
            .resource_mut::<lunco_core_runtime::SimulationBarrier>()
            .held = true;
        app.world_mut()
            .run_system_once(apply_physics_holds)
            .unwrap();
        assert!(!app.world_mut().run_system_once(physics_is_live).unwrap());
        // The worker finishes after the render-frame pause was projected.
        app.world_mut()
            .resource_mut::<lunco_core_runtime::SimulationBarrier>()
            .held = false;
        app.world_mut().run_schedule(FixedPreUpdate);
        assert!(app.world_mut().run_system_once(physics_is_live).unwrap());
        // A terrain hold still prevents force delivery after solver completion.
        app.world_mut()
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, true);
        app.world_mut().run_schedule(FixedPreUpdate);
        assert!(!app.world_mut().run_system_once(physics_is_live).unwrap());
    }

    #[test]
    fn shared_clock_validation_distinguishes_active_and_completed_cycles() {
        let mut fixed = Time::<Fixed>::from_hz(60.0);
        let mut physical = Time::<Physics>::default();
        let quantum = fixed.timestep();
        fixed.advance_by(quantum);
        assert!(validate_physics_clock(&physical, &fixed, true).is_ok());
        assert!(validate_physics_clock(&physical, &fixed, false).is_err());
        physical.advance_by(quantum);
        assert!(validate_physics_clock(&physical, &fixed, false).is_ok());
        physical.advance_by(Duration::from_nanos(1));
        assert!(validate_physics_clock(&physical, &fixed, true).is_err());
    }

    #[test]
    fn native_hold_projects_shared_admission_and_releases_only_its_key() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<PhysicsHolds>();
        world.init_resource::<lunco_core_runtime::SimulationProgress>();
        world.init_resource::<Time<Physics>>();
        let independent = lunco_core_runtime::SimulationProgressKey::scene_interaction();
        world
            .resource_mut::<lunco_core_runtime::SimulationProgress>()
            .acquire(independent, "editor");
        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, true);
        world
            .resource_mut::<Time<Physics>>()
            .advance_by(Duration::from_millis(16));
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(world.resource::<Time<Physics>>().is_paused());
        assert_eq!(world.resource::<Time<Physics>>().delta(), Duration::ZERO);
        let key = lunco_core_runtime::SimulationProgressKey::physics_integration();
        assert!(
            world
                .resource::<lunco_core_runtime::SimulationProgress>()
                .contains(key)
        );
        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::TERRAIN_READY, false);
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(
            !world
                .resource::<lunco_core_runtime::SimulationProgress>()
                .contains(key)
        );
        assert!(
            world
                .resource::<lunco_core_runtime::SimulationProgress>()
                .contains(independent)
        );
        assert!(!world.resource::<Time<Physics>>().is_paused());
    }

    #[test]
    fn terminal_fault_retires_physics_loading_progress() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<PhysicsHolds>();
        world.init_resource::<Time<Physics>>();
        world.init_resource::<lunco_core::RuntimeFaults>();
        world.init_resource::<lunco_core_runtime::SimulationProgress>();
        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::SAFETY_FAILURE, true);
        let key = lunco_core_runtime::SimulationProgressKey::physics_integration();
        world
            .resource_mut::<lunco_core_runtime::SimulationProgress>()
            .acquire(key, "physics preparing");
        world.resource_mut::<lunco_core::RuntimeFaults>().raise(
            "physics-continuous-contact-invalid",
            None,
            "contact solve",
            "not converged",
        );
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(world.resource::<Time<Physics>>().is_paused());
        assert!(
            !world
                .resource::<lunco_core_runtime::SimulationProgress>()
                .contains(key),
            "terminal faults must not masquerade as loading"
        );
    }

    #[test]
    fn terminal_runtime_fault_pauses_physics_until_cleared() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(Time::<Physics>::default());
        world.insert_resource(Time::<Virtual>::default());

        let mut faults = lunco_core::RuntimeFaults::default();
        faults.raise("physics-body-escaped", None, "rover", "out of bounds");
        world.insert_resource(faults);
        world
            .resource_mut::<Time<Physics>>()
            .advance_by(Duration::from_millis(16));

        world.run_system_once(apply_physics_holds).unwrap();
        assert!(world.resource::<Time<Physics>>().is_paused());
        assert_eq!(world.resource::<Time<Physics>>().delta(), Duration::ZERO);
        assert!(!world.resource::<Time<Virtual>>().is_paused());

        world.resource_mut::<lunco_core::RuntimeFaults>().clear();
        world.run_system_once(apply_physics_holds).unwrap();
        assert!(!world.resource::<Time<Physics>>().is_paused());
    }

    #[test]
    fn native_admission_observes_the_shared_clock_pause() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<PhysicsHolds>();
        world.insert_resource(Time::<Virtual>::default());
        world.insert_resource(Time::<Physics>::default());
        world
            .resource_mut::<Time<Physics>>()
            .advance_by(Duration::from_millis(16));
        world.resource_mut::<Time<Virtual>>().pause();

        world.run_system_once(apply_physics_holds).unwrap();

        assert!(world.resource::<Time<Physics>>().is_paused());
        assert_eq!(world.resource::<Time<Physics>>().delta(), Duration::ZERO);
    }

    #[test]
    fn scene_teardown_clears_physics_holds_and_integration_admission() {
        let mut app = App::new();
        app.init_scene_resource::<PhysicsHolds>()
            .init_resource::<lunco_core_runtime::SimulationProgress>()
            .init_resource::<Time<Physics>>()
            .add_systems(lunco_core::SceneTeardown, reset_scene_physics_state);
        app.world_mut()
            .resource_mut::<lunco_core_runtime::SimulationProgress>()
            .acquire(
                lunco_core_runtime::SimulationProgressKey::physics_integration(),
                "native integration",
            );
        app.world_mut()
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::READINESS, true);
        app.world_mut().resource_mut::<Time<Physics>>().pause();
        lunco_core::run_scene_teardown(app.world_mut());
        let world = app.world();
        assert!(!world.resource::<PhysicsHolds>().is_held());
        assert!(
            !world
                .resource::<lunco_core_runtime::SimulationProgress>()
                .is_held()
        );
        assert!(!world.resource::<Time<Physics>>().is_paused());
        assert_eq!(world.resource::<Time<Physics>>().delta(), Duration::ZERO);
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
                Rotation(Quat::IDENTITY),
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
                .angle_between(target_rotation.as_quat())
                < 1.0e-12
        );
        assert_eq!(
            world.get::<LinearVelocity>(entity).unwrap().0,
            (target / 0.1).as_vec3()
        );
        assert!((world.get::<AngularVelocity>(entity).unwrap().0.y - 5.0).abs() < 1.0e-12);
    }
}

impl lunco_core::SceneResource for PhysicsHolds {}
