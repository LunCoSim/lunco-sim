//! Typed commands for celestial surface operations.

use bevy::prelude::*;
use lunco_core::{Command, on_command};

/// Teleport the avatar to a celestial body's surface.
///
/// Places the camera on the body's Grid in surface-relative mode.
#[Command]
pub struct TeleportToSurface {
    /// The avatar entity to teleport.
    pub target: Entity,
    /// The body entity to teleport to (as raw bits for entity reconstruction).
    pub body_entity: u64,
}

/// Leave the current body's surface and return to orbit view.
///
/// Teleports the camera to 3x body radius altitude and switches to
/// `OrbitCamera` mode, re-parenting to the EMB Grid.
#[Command]
pub struct LeaveSurface {
    /// The avatar entity leaving the surface.
    pub target: Entity,
}
