//! Typed commands for avatar camera control and vessel possession.

use bevy::prelude::*;
use lunco_core::Command;

/// Possess a vessel, taking direct control of it.
#[Command]
pub struct PossessVessel {
    /// The avatar entity taking possession, when a camera should be bound.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub avatar: Option<Entity>,
    /// The entity exposing the writable `InputPorts` surface to possess.
    pub target: Entity,
    /// Whether possession also binds the avatar's camera to the vessel.
    #[serde(default = "default_true")]
    #[reflect(default = "default_true")]
    pub bind_camera: bool,
}

fn default_true() -> bool {
    true
}

/// Release possession of the currently controlled vessel.
#[Command]
pub struct ReleaseVessel {
    /// The avatar entity releasing possession.
    pub target: Entity,
}

/// Return the local camera from celestial orbit to its saved camera state.
#[Command]
pub struct ReturnFromOrbit {
    /// The local avatar camera returning from orbit view.
    pub target: Entity,
}

/// Focus on a target without taking control.
#[Command]
pub struct FocusTarget {
    /// The avatar entity that is focusing, when a local camera exists.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub avatar: Option<Entity>,
    /// The entity to focus on.
    pub target: Entity,
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

/// Follow a target with the chase camera without taking control.
#[Command]
pub struct FollowTarget {
    /// The avatar entity that will follow, when a local camera exists.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub avatar: Option<Entity>,
    /// The entity to follow.
    pub target: Entity,
}
