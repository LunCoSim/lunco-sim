//! Schedule contracts shared by co-simulation participants.
//!
//! These anchors describe the generic exchange phases; the Avian-backed
//! orchestration package installs the systems that run in them. Keeping the
//! labels in the backend-neutral package lets environmental and other
//! participants order their producers without depending on a physics adapter.

use bevy::prelude::*;

/// Fixed-step signal propagation phase.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CosimSet {
    /// Propagate source outputs into target inputs.
    Propagate,
}

/// Fixed-step actuator application phase.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CosimApplySet {
    /// Apply propagated actuator values to the participant backend.
    ApplyForces,
}
