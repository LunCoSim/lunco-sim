//! Generic typed camera commands.

use bevy::prelude::*;
use lunco_core::Command;

/// Focus a camera rig on a target without taking control of it.
#[Command]
pub struct FocusTarget {
    /// The camera rig, when the caller does not use the local presentation rig.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub camera: Option<Entity>,
    /// The entity to focus on.
    pub target: Entity,
}

/// Follow a target with a chase camera without taking control of it.
#[Command]
pub struct FollowTarget {
    /// The camera rig, when the caller does not use the local presentation rig.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub camera: Option<Entity>,
    /// The entity to follow.
    pub target: Entity,
}

/// Return a camera rig from celestial orbit to its saved camera state.
#[Command]
pub struct ReturnFromOrbit {
    /// The camera rig returning from orbit view.
    pub camera: Entity,
}

/// Tune pointer-to-camera response while the application is running.
#[Command(default)]
pub struct SetCameraInput {
    /// Camera radians per pointer-motion unit.
    pub look_radians_per_pointer_unit: Option<f32>,
    /// Lower bound for orbital rotation at the body's surface, in `[0, 1]`.
    pub orbit_surface_min_scale: Option<f64>,
    /// Positive exponent shaping the apparent-horizon distance response.
    pub orbit_distance_curve_exponent: Option<f64>,
}

/// Aim a camera from one point at another in the active presentation frame.
///
/// Camera adapters validate the addressed camera and realize the pose in their
/// owning spatial frame. Keeping the command contract here lets authored,
/// inspection, avatar, and cinematic camera adapters share one reflected API.
#[Command]
pub struct SetCameraLookAt {
    /// Camera entity to pose.
    pub camera: Entity,
    /// Camera position in the active physics frame, metres.
    pub eye: Vec3,
    /// Camera look-at point in the active physics frame, metres.
    pub target: Vec3,
}
