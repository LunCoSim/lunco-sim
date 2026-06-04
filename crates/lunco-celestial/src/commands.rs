//! Typed commands for celestial surface operations.

use bevy::prelude::*;
use lunco_core::Command;

/// Teleport the avatar to a celestial body's surface.
///
/// Places the camera on the body's Grid in surface-relative mode.
#[Command]
pub struct TeleportToSurface {
    /// The avatar entity to teleport. (`Entity` → the id codec converts this
    /// gid↔local automatically; see `crates/lunco-networking/PH2_ID_CODEC.md`.)
    pub target: Entity,
    /// The body to teleport to, carried as raw local `Entity::to_bits()` and
    /// reconstructed in the observer.
    ///
    /// `u64`, not `Entity` — a variant of "**Pattern B**": the type-driven id
    /// codec converts only `Entity`-typed fields, so this `u64` opts out and is
    /// handled by hand. Unlike `MoveEntity::entity_id` (an `api_id` resolved via
    /// `ApiEntityRegistry`) this is a *local* entity bit-pattern, so it is only
    /// meaningful in-process. Left as-is by choice; the codec ignores it.
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
