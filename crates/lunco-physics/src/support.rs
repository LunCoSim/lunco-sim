//! Runtime support geometry shared by physics producers and terrain.
//!
//! Avian colliders already describe the support footprint of ordinary rigid
//! bodies. A physics model that deliberately has no collider (for example a
//! raycast suspension or a probe-based landing leg) still has real spatial
//! support geometry. It publishes that geometry here instead of making the
//! terrain know about the model that produced it.

use bevy::math::DVec3;
use bevy::prelude::*;

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
