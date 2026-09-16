//! Shared cursor-interaction contracts between editor producers and runtime consumers.
//!
//! The contracts are independent of the editor implementation: an editor
//! publishes the current drag state and affected-entity marker, while camera,
//! possession, and follow runtimes decide how to stand down.

use bevy::prelude::*;

/// Resource indicating that an entity transform is being dragged.
#[derive(Resource, Default)]
pub struct DragModeActive {
    /// Whether a transform-gizmo drag is active.
    pub active: bool,
}

/// Marker for an entity currently being moved by an editor transform gizmo.
#[derive(Component, Default)]
pub struct GizmoDragging;
