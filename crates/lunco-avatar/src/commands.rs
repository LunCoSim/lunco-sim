//! Typed commands for avatar camera control and vessel possession.

use bevy::prelude::*;
use lunco_core::{Command, on_command};

/// Possess a vessel, taking direct control of it.
///
/// Switches the avatar to a vessel-locked camera mode and inserts a
/// `ControllerLink` so that input events are forwarded to the vessel.
#[Command]
pub struct PossessVessel {
    /// The avatar entity that is taking possession.
    pub avatar: Entity,
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
    /// The avatar entity that is focusing.
    pub avatar: Entity,
    /// The entity to focus on.
    pub target: Entity,
}
