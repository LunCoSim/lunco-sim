//! Atomic re-parenting of `GridAnchor` entities across big_space `Grid`s.
//!
//! Re-parenting a spatial entity has three coupled writes that MUST land in
//! the same `EntityCommands` call: the new `ChildOf(grid)`, the new
//! `CellCoord`, and the new `Transform`. Splitting them — e.g.
//! `commands.entity(e).insert((cell, tf)); commands.entity(grid).add_child(e);`
//! — queues two separate commands; observers and propagation systems that
//! fire between the two see an inconsistent (cell, parent) pair and can
//! mis-tag the entity (the same class of bug that marked rover chassis as
//! `RigidBody::Static` — see `lunco-usd-bevy` instantiate_usd_prim).
//!
//! `migrate_to_grid` is the only sanctioned way to move a `GridAnchor`
//! between Grids.

use bevy::prelude::*;
use big_space::prelude::*;

/// Re-parent `entity` to `new_grid`, placing it at `world_pos` (absolute
/// BigSpace-root coordinates). Resolves `(CellCoord, Transform)` against
/// the destination Grid's frame and applies all three writes atomically.
///
/// Caller is responsible for:
/// - `entity` being a `GridAnchor` (or equivalent direct child of a Grid).
/// - `world_pos` and `new_grid_world_pos` being expressed in the same
///   absolute frame (use [`crate::coords::world_position`] / `_seeded`).
pub fn migrate_to_grid(
    commands: &mut Commands,
    entity: Entity,
    new_grid: Entity,
    new_grid_grid: &Grid,
    world_pos: bevy::math::DVec3,
    new_grid_world_pos: bevy::math::DVec3,
) {
    let (cell, local_tf) = new_grid_grid.translation_to_grid(world_pos - new_grid_world_pos);
    commands.entity(entity).insert((
        ChildOf(new_grid),
        cell,
        Transform::from_translation(local_tf),
    ));
}
