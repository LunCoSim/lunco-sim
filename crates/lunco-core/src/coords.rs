//! DVec3 helpers that abstract over the big_space hierarchy.
//!
//! Consumers should never assemble cross-Grid math themselves. They go
//! through these helpers. The previous practice — querying
//! `(&CellCoord, &Transform)` on arbitrary targets and calling
//! `grid.grid_position_double(...)` — works only inside one Grid and
//! breaks across Grid boundaries; these helpers cover both cases.

use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::*;
use bevy::ecs::query::QueryFilter;

use crate::markers::GridAnchor;

/// Walks ancestors of `entity` and returns the first one with a `Grid`
/// component. The `Grid` itself does not count — if `entity` is a Grid,
/// returns the entity's *parent* Grid (or `None` if it's the BigSpace root).
pub fn ancestor_grid(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<(), With<Grid>>,
) -> Option<Entity> {
    let mut current = entity;
    for _ in 0..32 {
        let Ok(child_of) = q_parents.get(current) else { return None };
        let parent = child_of.parent();
        if q_grids.contains(parent) { return Some(parent); }
        current = parent;
    }
    None
}

/// Walks ancestors of `entity` and returns the nearest one tagged
/// `GridAnchor`. Returns `entity` itself if it is already a `GridAnchor`.
///
/// This is the canonical "what unit am I touching?" lookup for UI:
/// selection, gizmo target, possession all use this to resolve a clicked
/// mesh entity to the rover/ball/vessel it belongs to.
pub fn ancestor_grid_anchor(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_anchors: &Query<(), With<GridAnchor>>,
) -> Option<Entity> {
    let mut current = entity;
    for _ in 0..32 {
        if q_anchors.contains(current) { return Some(current); }
        let Ok(child_of) = q_parents.get(current) else { return None };
        current = child_of.parent();
    }
    None
}

/// Absolute world position of `entity` expressed in the BigSpace root
/// frame, as a `DVec3`.
///
/// Walks ancestors. Each `(CellCoord, Transform)` step under a `Grid`
/// contributes `grid.grid_position_double(cell, tf)` in DVec3 to the
/// accumulator. Plain-`Transform` ancestors compose their `Transform`
/// onto the running pose.
///
/// Returns `None` if `entity` has no spatial component at all.
pub fn world_position(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Option<DVec3> {
    let (mut cell, mut tf) = {
        let (c, t) = q_spatial.get(entity).ok()?;
        (c.copied().unwrap_or_default(), *t)
    };
    let mut total = DVec3::ZERO;
    let mut current = entity;

    for _ in 0..32 {
        let Ok(child_of) = q_parents.get(current) else {
            total += tf.translation.as_dvec3();
            return Some(total);
        };
        let parent = child_of.parent();

        if let Ok(grid) = q_grids.get(parent) {
            // Crossing a Grid boundary: convert our (cell, tf) to the
            // parent Grid's frame in DVec3.
            total += grid.grid_position_double(&cell, &tf);
            let Ok((p_cell, p_tf)) = q_spatial.get(parent) else {
                return Some(total);
            };
            cell = p_cell.copied().unwrap_or_default();
            tf = *p_tf;
            current = parent;
        } else if let Ok((p_cell_opt, p_tf)) = q_spatial.get(parent) {
            // Mid-hierarchy plain-Transform parent: compose its Transform
            // onto our running local pose and continue upward.
            tf.translation = p_tf.translation + p_tf.rotation * tf.translation;
            tf.rotation = p_tf.rotation * tf.rotation;
            cell = p_cell_opt.copied().unwrap_or(cell);
            current = parent;
        } else {
            total += tf.translation.as_dvec3();
            return Some(total);
        }
    }
    Some(total)
}

/// Vector from `from` to `to` in DVec3 absolute world space.
pub fn world_vector(
    from: Entity,
    to: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Option<DVec3> {
    let a = world_position(from, q_parents, q_grids, q_spatial)?;
    let b = world_position(to, q_parents, q_grids, q_spatial)?;
    Some(b - a)
}

/// Decompose an absolute world position into `(CellCoord, Vec3)` under a
/// target Grid. `target_grid_world` is the target Grid's own absolute
/// world position (obtain via [`world_position`] on the Grid entity).
pub fn world_to_grid_local(
    world_pos: DVec3,
    target_grid_world: DVec3,
    target_grid: &Grid,
) -> (CellCoord, Vec3) {
    target_grid.translation_to_grid(world_pos - target_grid_world)
}

/// Absolute world position of `entity`, seeded with an explicit
/// `(initial_cell, initial_tf)`.
///
/// Use this when `entity` is not present in `q_spatial` (typically because
/// `q_spatial` carries a `Without<...>` disjointness filter against another
/// `mut` query that holds `entity`). For the no-seed variant — when entity
/// IS in `q_spatial` — use [`world_position`].
pub fn world_position_seeded<F: QueryFilter>(
    entity: Entity,
    initial_cell: &CellCoord,
    initial_tf: &Transform,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> bevy::math::DVec3 {
    let mut total_pos = bevy::math::DVec3::ZERO;

    let mut current_tf = *initial_tf;
    let mut current_cell = *initial_cell;
    let mut current_entity = entity;

    let mut depth = 0;
    while depth < 20 {
        depth += 1;
        if let Ok(child_of) = q_parents.get(current_entity) {
            let parent = child_of.parent();
            if let Ok(grid) = q_grids.get(parent) {
                // Cross a grid boundary: convert current local state to parent coordinate space
                total_pos += grid.grid_position_double(&current_cell, &current_tf);

                // Now continue recursion from the grid entity itself
                if let Ok((p_cell, p_tf)) = q_spatial.get(parent) {
                    current_entity = parent;
                    current_cell = p_cell.copied().unwrap_or_default();
                    current_tf = *p_tf;
                } else {
                    break;
                }
            } else {
                // Intermediate parent (not a grid): accumulate local transform.
                // Mid-hierarchy entities should NOT have CellCoord under the new
                // architecture; fall back to default if missing.
                if let Ok((p_cell_opt, p_tf)) = q_spatial.get(parent) {
                    current_tf.translation = p_tf.translation + p_tf.rotation * current_tf.translation;
                    current_tf.rotation = p_tf.rotation * current_tf.rotation;
                    current_cell = p_cell_opt.copied().unwrap_or(current_cell);
                    current_entity = parent;
                } else {
                    total_pos += current_tf.translation.as_dvec3();
                    break;
                }
            }
        } else {
            total_pos += current_tf.translation.as_dvec3();
            break;
        }
    }
    total_pos
}
