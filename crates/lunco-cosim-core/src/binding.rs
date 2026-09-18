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

/// A monotonic, event-driven request to reconsider connection specifications.
///
/// The Avian-backed co-simulation crate owns the transaction that consumes this
/// state, while USD projections use it to publish endpoint and epoch changes
/// without depending on that transaction implementation.
#[derive(Resource, Debug, Default)]
pub struct BindingRevision {
    revision: u64,
    consumed: u64,
    epoch: u64,
    /// `true` only after the scene/instance projection epoch has settled.
    pub sealed: bool,
}

impl BindingRevision {
    /// Request a new binding transaction.
    pub fn request(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Return whether a binding request has not yet been consumed.
    pub fn pending(&self) -> bool {
        self.consumed != self.revision
    }

    /// Open a new projection epoch and request rebinding.
    pub fn open_epoch(&mut self) {
        if self.sealed {
            self.epoch = self.epoch.wrapping_add(1);
            self.sealed = false;
        }
        self.request();
    }

    /// Seal the current projection epoch and request the terminal binding pass.
    pub fn seal_epoch(&mut self) {
        if !self.sealed {
            self.epoch = self.epoch.wrapping_add(1);
        }
        self.sealed = true;
        self.request();
    }

    /// Return the current projection epoch for the binding transaction.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Consume one pending request, returning whether one was present.
    pub fn take_request(&mut self) -> bool {
        if self.consumed == self.revision {
            return false;
        }
        self.consumed = self.revision;
        true
    }
}
