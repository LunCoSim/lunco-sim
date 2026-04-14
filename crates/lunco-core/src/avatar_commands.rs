//! Typed command types for avatar possession, focus, and surface operations.
//!
//! These commands are defined in `lunco-core` (not `lunco-avatar`) to avoid
//! circular dependencies — both `lunco-avatar` and `lunco-celestial` can
//! import them from the shared core crate.

use bevy::prelude::*;
use crate::{Command, on_command};

/// Possess a vessel, taking direct control of it.
#[Command]
pub struct PossessVessel {
    pub avatar: Entity,
    pub target: Entity,
}

/// Release possession of the currently controlled vessel.
#[Command]
pub struct ReleaseVessel {
    pub target: Entity,
}

/// Focus on a target without taking control.
#[Command]
pub struct FocusTarget {
    pub avatar: Entity,
    pub target: Entity,
}

/// Teleport the avatar to a celestial body's surface.
#[Command]
pub struct TeleportToSurface {
    pub target: Entity,
    pub body_entity: u64,
}

/// Leave the current body's surface and return to orbit view.
#[Command]
pub struct LeaveSurface {
    pub target: Entity,
}
