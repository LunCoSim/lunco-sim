//! Atomic re-parenting of `GridAnchor` entities across big_space `Grid`s.
//!
//! Re-parenting a spatial entity has three coupled writes that MUST land in
//! the same `EntityCommands` call: the new `ChildOf(grid)`, the new
//! `CellCoord`, and the new local `Transform`. Splitting them — e.g.
//! `commands.entity(e).insert((cell, tf)); commands.entity(grid).add_child(e);`
//! — queues two separate commands; observers and propagation systems that
//! fire between the two see an inconsistent (parent, cell, local_tf) triple
//! and can mis-tag the entity (the same class of bug that marked rover
//! chassis as `RigidBody::Static`; see `lunco-usd-bevy::instantiate_usd_prim`).
//!
//! `migrate_to_grid` is the only sanctioned way to move a `GridAnchor`
//! between Grids. The workspace `clippy.toml` bans raw `add_child` /
//! `set_parent_in_place` to enforce this.

use bevy::prelude::*;
use big_space::prelude::*;

/// Atomically re-parent `entity` to `new_grid` with the given grid-local
/// `(CellCoord, Transform)`. Writes `(ChildOf, CellCoord, Transform)` in
/// one `insert` call so no system can observe a partially-migrated state.
///
/// Callers translating from absolute world coordinates compute the
/// grid-local pair via `new_grid_component.translation_to_grid(...)` and
/// pass the result.
pub fn migrate_to_grid(
    commands: &mut Commands,
    entity: Entity,
    new_grid: Entity,
    cell: CellCoord,
    local_transform: Transform,
) {
    commands.entity(entity).insert((
        ChildOf(new_grid),
        cell,
        local_transform,
    ));
}
