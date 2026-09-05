//! Runtime support geometry shared by physics producers and terrain.
//!
//! Avian colliders already describe the support footprint of ordinary rigid
//! bodies. A physics model that deliberately has no collider (for example a
//! raycast suspension or a probe-based landing leg) still has real spatial
//! support geometry. It publishes that geometry here instead of making the
//! terrain know about the model that produced it.

use bevy::math::DVec3;
use bevy::prelude::*;

/// Ordering boundary for the shared support-footprint contract.
///
/// Physics producers publish their authored support geometry before terrain or
/// any other spatial consumer decides residency and initial placement. Keeping
/// this boundary in the physics contract avoids coupling a consumer to a
/// particular mobility implementation or relying on plugin insertion order.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhysicsSupportSet {
    /// Runtime physics models publish support geometry for the current world.
    Publish,
    /// Apply deferred publisher changes before support consumers inspect them.
    Apply,
    /// Terrain and other spatial systems consume the published support contract.
    Consume,
}

/// A live edge in the physics assembly graph.
///
/// Physics producers publish this before the native joint is admitted to the
/// solver. Spatial consumers therefore see the complete articulated assembly
/// during startup placement as well as during normal simulation. The link is
/// removed with its owning joint entity; it is not a second constraint.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicsJointLink {
    /// The first body in the authored or synthesized joint.
    pub body0: Entity,
    /// The second body in the authored or synthesized joint.
    pub body1: Entity,
}

/// Marks an authored physics joint whose native constraint has not crossed the
/// admission boundary yet.
///
/// This remains present while USD is resolving the endpoints and while Avian is
/// parking the typed constraint. It is removed only when the native joint is
/// installed together with its collision policy. Terrain placement must wait
/// for that complete topology phase: moving only the root while an authored
/// child body is still waiting to resolve leaves a real joint violation for the
/// solver to repair on its first step.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhysicsJointPending;

/// Contact geometry that contributes to a body's terrain-support footprint.
///
/// Offsets are expressed in the owning body's local physics frame. The
/// publisher owns the meaning of the probe; consumers only need a conservative
/// radius around its transformed centre. A normal rigid body does not need this
/// component because its Avian collider AABBs are aggregated automatically.
#[derive(Component, Debug, Clone, Reflect, PartialEq)]
#[reflect(Component)]
pub struct PhysicsSupportFootprint(pub Vec<PhysicsSupportContact>);

/// One support contact in a body's local physics frame.
///
/// The probe description is part of the contract because initial placement
/// must target the same collider surface that the live support query uses. A
/// terrain oracle and a sampled heightfield can legitimately differ between
/// lattice points; placement therefore consumes the authoritative spatial
/// query rather than inventing a second surface approximation.
#[derive(Debug, Clone, Copy, Reflect, PartialEq)]
pub struct PhysicsSupportContact {
    /// Contact centre relative to the owning body's origin.
    pub local_offset: DVec3,
    /// Conservative horizontal support radius in metres.
    pub radius: f64,
    /// Probe origin relative to the owning body's origin.
    pub probe_origin: DVec3,
    /// Probe direction in the owning body's local physics frame.
    pub probe_direction: DVec3,
    /// Probe distance at the authored/rest support pose.
    pub probe_length: f64,
}
