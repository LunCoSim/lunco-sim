//! Shared BigSpace spatial substrate.
//!
//! This package owns the high-precision coordinate representation, the
//! persistent world shell, grid migration, hierarchy invariants, and the
//! vehicle-neutral navigation law. [`lunco_core`] remains the dependency-light
//! engine substrate; it does not depend on BigSpace or this package.

pub mod attach;
pub mod coords;
pub mod invariants;
pub mod navigation;
pub mod world;

pub use invariants::BigSpaceInvariantsPlugin;
pub use navigation::{approach_factor, nav_setpoint, steering_command, NavigationCommand};
pub use world::{
    ensure_world_root, ActivePhysicsFrame, OriginAnchor, WorldGrid, WorldGridConfig, WorldRoot,
    WorldShellPlugin, WorldShellSet,
};

use bevy::prelude::*;
use big_space::prelude::CellCoord;

/// A spatial entity that moves as a single unit — rover, ball, vessel, avatar,
/// terrain tile, or scene-level light.
///
/// A `GridAnchor` is a direct child of a BigSpace `Grid` and carries the
/// `CellCoord` required by high-precision propagation. Selection, dragging,
/// possession, and SOI migration operate on this unit root rather than on its
/// descendants.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[require(CellCoord)]
#[reflect(Component)]
pub struct GridAnchor;

/// Steering geometry exposed by an authored vehicle command surface.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Component)]
pub enum SteeringGeometry {
    /// Independent left/right wheel demand with direct yaw authority.
    Differential,
    /// Front-steered geometry with finite-radius turning.
    Ackermann,
}

/// Parse the authored steering-geometry token. Unknown or empty values are
/// rejected so a vehicle cannot silently receive the wrong steering law.
pub fn parse_steering_geometry(s: &str) -> Option<SteeringGeometry> {
    match s.trim().to_ascii_lowercase().as_str() {
        "differential" | "skid" | "skid_steer" => Some(SteeringGeometry::Differential),
        "ackermann" | "front_steer" => Some(SteeringGeometry::Ackermann),
        _ => None,
    }
}
