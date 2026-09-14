//! Backend-neutral physical actuator contracts.
//!
//! These components describe a force or torque source authored by a domain
//! adapter. A physics backend consumes the metadata and applies its own
//! solver-specific realization; the authored actuator reader does not need to
//! depend on that backend.

use bevy::prelude::*;

/// A force actuator authored by a domain adapter.
///
/// The mount position and direction are expressed in the owning body's local
/// frame. The component is metadata only; a backend keeps the live command
/// separately so port writes never mutate the authored description.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct ForceActuator {
    /// Mount position relative to the owning rigid-body origin (m).
    pub local_position: Vec3,
    /// Unit force direction in the owning body's local frame.
    pub direction_local: Vec3,
    /// Maximum accepted force for this actuator (N).
    pub max_force_n: f64,
}

/// A torque actuator authored by a domain adapter.
///
/// Reaction wheels, control-moment gyros, and other torque sources use this
/// same contract. The axis is expressed in the owning body's local frame.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct TorqueActuator {
    /// Torque axis in the owning body's local frame.
    pub axis_local: Vec3,
    /// Maximum torque magnitude (N·m).
    pub max_torque_nm: f64,
}
