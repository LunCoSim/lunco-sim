//! Debug-build runtime checks for the big_space architectural invariants.
//!
//! These fire warnings when code violates the rules from
//! `docs/architecture/big_space.md` (canonical: `CellCoord` lives only on
//! direct children of a `Grid`). They are gated to debug builds so they
//! impose no release-build cost.

use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};

use crate::markers::GridAnchor;

/// Warns when a newly-inserted `CellCoord` lands on an entity whose parent
/// is not a `Grid`. big_space's `propagate_high_precision` will silently
/// skip such entities — the runtime symptom is "GlobalTransform never
/// updates," which is the bug class this check exists to catch early.
fn warn_on_mid_hierarchy_cellcoord(
    q: Query<(Entity, &ChildOf, Option<&Name>), (Added<CellCoord>, Without<Grid>)>,
    q_grids: Query<(), With<Grid>>,
) {
    for (e, child_of, name) in q.iter() {
        if !q_grids.contains(child_of.parent()) {
            warn!(
                "[bigspace-invariant] CellCoord on entity {:?} ({}) — parent {:?} is not a Grid. \
                 big_space will not propagate its GlobalTransform. \
                 Move it to a Grid-direct slot or drop the CellCoord.",
                e,
                name.map(|n| n.as_str()).unwrap_or("<unnamed>"),
                child_of.parent()
            );
        }
    }
}

/// Warns when a `GridAnchor` lacks a `Grid` parent.
fn warn_on_orphaned_grid_anchor(
    q: Query<(Entity, Option<&ChildOf>, Option<&Name>), Added<GridAnchor>>,
    q_grids: Query<(), With<Grid>>,
) {
    for (e, child_of, name) in q.iter() {
        let parent_is_grid = child_of
            .map(|c| q_grids.contains(c.parent()))
            .unwrap_or(false);
        if !parent_is_grid {
            warn!(
                "[bigspace-invariant] GridAnchor on entity {:?} ({}) has no Grid parent. \
                 Either parent it under a Grid or remove the marker.",
                e,
                name.map(|n| n.as_str()).unwrap_or("<unnamed>"),
            );
        }
    }
}

/// Registers the invariant checks. Only adds systems in debug builds.
pub struct BigSpaceInvariantsPlugin;

impl Plugin for BigSpaceInvariantsPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(debug_assertions)]
        app.add_systems(
            PostUpdate,
            (warn_on_mid_hierarchy_cellcoord, warn_on_orphaned_grid_anchor),
        );
        let _ = app;
    }
}
