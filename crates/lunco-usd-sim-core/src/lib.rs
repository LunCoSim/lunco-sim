//! Shared contracts between specialized USD simulation projection and cosim.
//!
//! This package intentionally contains no projection systems. It owns only the
//! small ECS and scheduling vocabulary that must be understood by both the
//! vehicle adapter and the co-simulation adapter. Keeping that vocabulary out
//! of either implementation crate prevents one large projector from becoming
//! the dependency of the other.

use bevy_ecs::prelude::*;
use bevy_math::{Quat, Vec3};

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

/// Set while a ground provider's static collider is still building.
///
/// The application-level terrain coordinator owns the flag, while the USD
/// simulation projector consumes it at the dynamic-body admission boundary.
/// Keeping this readiness contract here lets scene runners observe the same
/// state without depending on the full vehicle projector.
#[derive(Resource, Default)]
pub struct GroundColliderPending(pub bool);

/// State needed to display and animate a joint-based wheel.
///
/// The vehicle projector owns the physical wheel, while render and editor
/// systems consume this shared component to identify its visual child and
/// reconstruct proxy motion. Keeping the data contract here prevents those
/// consumers from depending on the complete vehicle projector.
#[derive(Component, Debug, Clone)]
pub struct PhysicalWheel {
    /// The visual mesh child (the entity whose local rotation we author on a
    /// client proxy). `None` if the wheel prim carried no mesh.
    pub visual_entity: Option<Entity>,
    /// Rolling radius (m); the proxy roll rate is `ω = v_long / r`.
    pub wheel_radius: f32,
    /// Authored wheel width (m), retained so a live width edit can rebuild the
    /// collider instead of changing density while leaving the old shape in place.
    pub wheel_width: f32,
    /// Visual base orientation (the USD cylinder `axis`). The roll axle is
    /// `axis_rot · Y` and the visual base composes as `roll · axis_rot`.
    pub axis_rot: Quat,
    /// Integrated roll angle (rad), wrapped to `[0, 2π)`. Client display state;
    /// unused on the host (the body carries the real rotation there).
    pub spin_angle: f32,
    /// Wheel mount offset in the enclosing vehicle frame. A client proxy can
    /// reconstruct the wheel's position as `chassis_pos + chassis_rot · mount_local`.
    pub mount_local: Vec3,
}
