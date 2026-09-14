//! Typed commands for celestial surface operations.

use bevy::prelude::*;
use lunco_core::Command;

/// Teleport the avatar to a celestial body's surface.
///
/// Places the camera on the body's Grid in surface-relative mode.
#[Command]
pub struct TeleportToSurface {
    /// The avatar entity to teleport.
    pub target: Entity,
    /// The celestial body whose surface should receive the avatar.
    pub body_entity: Entity,
}

/// Leave the current body's surface and return to orbit view.
///
/// Opens a transactional `OrbitCamera` view in the body's explicit star-fixed
/// orbit grid. Returning restores the avatar's exact prior surface frame.
#[Command]
pub struct LeaveSurface {
    /// The avatar entity leaving the surface.
    pub target: Entity,
}
