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
pub mod scene_handoff;
pub mod world;

/// Maximum number of hierarchy levels traversed by generic spatial lookups.
///
/// Authored USD hierarchies are finite, but malformed runtime parent/child
/// state must not turn a lookup into unbounded work. Owners that walk a
/// hierarchy use this shared limit rather than inventing a local bound.
pub const MAX_HIERARCHY_WALK_DEPTH: usize = 16;

/// Resolve a component on an entity or its bounded descendant hierarchy.
///
/// `None` means the target has no matching component in the inspected
/// hierarchy. Callers must choose explicitly whether the original target is a
/// valid non-component target or whether the operation should be rejected.
pub fn find_descendant_or_self<T: Component>(
    target: Entity,
    q_children: &Query<&Children>,
    q_components: &Query<(Entity, &T)>,
) -> Option<Entity> {
    if q_components.get(target).is_ok() {
        return Some(target);
    }

    let mut pending = vec![target];
    for _ in 0..MAX_HIERARCHY_WALK_DEPTH {
        let mut next = Vec::new();
        for parent in pending.drain(..) {
            let Ok(children) = q_children.get(parent) else {
                continue;
            };
            for child in children.iter() {
                if q_components.get(child).is_ok() {
                    return Some(child);
                }
                next.push(child);
            }
        }
        if next.is_empty() {
            break;
        }
        pending = next;
    }

    None
}

pub use invariants::BigSpaceInvariantsPlugin;
pub use navigation::{approach_factor, nav_setpoint, steering_command, NavigationCommand};
pub use scene_handoff::SceneSpatialHandoffSet;
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
