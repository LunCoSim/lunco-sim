//! Backend-neutral connection binding state.
//!
//! The co-simulation engine owns the transaction that resolves ports, but the
//! resulting ECS state is part of the connection contract. Keeping these
//! markers here lets read-only projections observe connection readiness without
//! depending on the Avian-backed orchestration crate.

use bevy::prelude::*;

/// Binding result for an immutable [`crate::SimConnection`] specification.
#[derive(Component, Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionBinding {
    /// The connection is waiting for endpoint or port resolution.
    #[default]
    Pending,
    /// Both endpoints resolved and the connection participates in propagation.
    Bound,
    /// Binding reached a terminal state but an endpoint or port was invalid.
    Failed,
}

/// Marker for a [`crate::SimConnection`] admitted to the propagation fabric.
///
/// This marker is separate from [`ConnectionBinding`] so systems can use a
/// zero-sized query filter on the fixed-step path while API and diagnostics
/// consumers inspect the richer binding state.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct BoundConnection;
