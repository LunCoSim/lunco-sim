//! Typed commands for avatar camera control and vessel possession.

use bevy::prelude::*;
use lunco_core::Command;

/// Possess a vessel, taking direct control of it.
///
/// Switches the avatar to a vessel-locked camera mode and inserts a
/// `ControllerLink` so that input events are forwarded to the vessel.
#[Command]
pub struct PossessVessel {
    /// The avatar entity taking possession — a user's *local* representation in
    /// the world, used only to bind the chase camera. `None` for headless or
    /// direct API control with no avatar: the host-side authority claim keys on
    /// `target`, so possession works fully without one.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub avatar: Option<Entity>,
    /// The entity to possess (becomes the controlled vessel).
    pub target: Entity,
}

/// Release possession of the currently controlled vessel.
///
/// Removes the `ControllerLink` and returns the avatar to free-flight mode.
/// Keeps the camera at its current position — no jarring teleport.
#[Command]
pub struct ReleaseVessel {
    /// The avatar entity releasing possession.
    pub target: Entity,
}

/// Focus on a target without taking control.
///
/// Switches the avatar to `OrbitCamera` mode centered on the target.
#[Command]
pub struct FocusTarget {
    /// The avatar entity that is focusing (local camera representation). `None`
    /// for headless/direct control with no avatar.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub avatar: Option<Entity>,
    /// The entity to focus on.
    pub target: Entity,
}

/// Follow a target with the chase camera, without taking control.
///
/// Inserts `SpringArmCamera` so the camera tracks the target's heading,
/// but omits `ControllerLink` and vessel input bindings — keyboard input
/// stays inert toward the target. Use this for non-vessel objects (balloons,
/// props, observation targets) where the player wants to ride along but
/// not drive. `PossessVessel` is conceptually `FollowTarget` plus a
/// controller binding.
#[Command]
pub struct FollowTarget {
    /// The avatar entity that will follow (local camera representation). `None`
    /// for headless/direct control with no avatar.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub avatar: Option<Entity>,
    /// The entity to follow.
    pub target: Entity,
}

/// Update the profile name for the active user session.
#[Command(default)]
pub struct UpdateProfile {
    pub name: String,
}

