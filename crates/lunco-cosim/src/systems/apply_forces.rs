//! System set for the force-application phase of co-simulation.
//!
//! The actual consumer is [`crate::avian::apply_pending_forces`], which drains
//! the `force_*` ports' [`crate::avian::PendingForces`] accumulator into avian.
//! It lives in [`CosimSet::ApplyForces`], ordered after propagation.

use bevy::prelude::*;

/// System sets for applying propagated values into avian.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CosimSet {
    /// Apply propagated `force_*` values (via `PendingForces`) into avian.
    /// Runs after [`crate::systems::propagate::CosimSet::Propagate`].
    ApplyForces,
}
