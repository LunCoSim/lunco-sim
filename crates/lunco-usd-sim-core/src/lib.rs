//! Shared contracts between specialized USD simulation projection and cosim.
//!
//! This package intentionally contains no projection systems. It owns only the
//! small ECS and scheduling vocabulary that must be understood by both the
//! vehicle adapter and the co-simulation adapter. Keeping that vocabulary out
//! of either implementation crate prevents one large projector from becoming
//! the dependency of the other.

use bevy_ecs::prelude::*;

/// Ordered phases shared by the USD simulation projections.
///
/// The vehicle projector publishes generic physical intent in `Projection`;
/// the co-simulation projector consumes it in its `Scene` phase. The
/// application composition configures the complete ordering after installing
/// both projection plugins.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UsdSimSet {
    /// Publishes the composed USD simulation and celestial components.
    Projection,
    /// Converts `ShouldBeDynamic` bodies only after their ground is known ready.
    ActivateDynamicBodies,
}

/// Marks a USD prim after its simulation projection has completed.
#[derive(Component)]
pub struct UsdSimProcessed;

/// Authored gear-joint data held until all referenced bodies are admitted.
#[derive(Component)]
pub struct PendingDifferential {
    /// Composed prim path of the frame both hinges turn against.
    pub chassis: String,
    /// Composed prim path of the first geared body.
    pub rocker_a: String,
    /// Composed prim path of the second geared body.
    pub rocker_b: String,
    /// Authored gear ratio.
    pub ratio: f64,
    /// Authored rest offset.
    pub rest_offset: f64,
    /// Authored target velocity.
    pub target_velocity: f64,
    /// Authored stiffness.
    pub stiffness: f64,
    /// Authored damping.
    pub damping: f64,
    /// Authored maximum force.
    pub max_force: f64,
    /// Authored drive type.
    pub drive_type: lunco_mobility::DifferentialDriveType,
}
