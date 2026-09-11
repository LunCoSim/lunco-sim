//! Generic authored-model state invalidation.
//!
//! Authoring systems use [`ModelStateRevision`] to say that the state a live
//! model was built from changed. The producer does not choose a backend action:
//! a Modelica, Rhai, physics, or future tool adapter decides whether that
//! revision requires a rebuild, reset, or ordinary live-input update.

use bevy::prelude::*;

/// Monotonic revision of an authored model instance's build-relevant state.
///
/// This is an invalidation signal, not a compile request. It deliberately has
/// no knowledge of USD, Modelica, Rhai, or the rebuild operation. Backend
/// adapters observe `Changed<ModelStateRevision>` and own their lifecycle.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct ModelStateRevision(pub u64);

impl ModelStateRevision {
    /// Advance the revision without allowing a wrap to look like an older
    /// state to an adapter.
    pub fn advance(&mut self) {
        self.0 = self.0.checked_add(1).unwrap_or(1);
    }
}
