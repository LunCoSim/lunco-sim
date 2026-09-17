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
//! `migrate_to_grid` is the only sanctioned way to move a spatial entity
//! between Grids. The workspace `clippy.toml` bans raw `add_child` /
//! `set_parent_in_place` to enforce this.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::*;

/// Atomically re-parent a spatial `entity` to `new_grid` with the given grid-local
/// `(CellCoord, Transform)`. Writes `(ChildOf, CellCoord, Transform)` in
/// one `insert` call so no system can observe a partially-migrated state.
///
/// Callers that already have a semantic f64 pose in the destination Grid use
/// [`migrate_to_grid_local_pose`] so the cell split and local transform remain
/// at this boundary. Callers that already own an authoritative stored pair can
/// pass it directly.
pub fn migrate_to_grid(
    commands: &mut Commands,
    entity: Entity,
    new_grid: Entity,
    cell: CellCoord,
    local_transform: Transform,
) {
    commands
        .entity(entity)
        .try_insert((ChildOf(new_grid), cell, local_transform));
}

/// Atomically migrate an entity whose semantic pose is already expressed in
/// `new_grid`'s local frame.
///
/// The grid-local f64 position is split into BigSpace storage coordinates here,
/// at the shared spatial boundary. Camera, avatar, scene, and simulation
/// adapters must use this operation instead of repeating `translation_to_grid`
/// and assembling a local `Transform` themselves.
pub fn migrate_to_grid_local_pose(
    commands: &mut Commands,
    entity: Entity,
    new_grid: Entity,
    grid: &Grid,
    local_position: DVec3,
    local_rotation: DQuat,
) {
    let (cell, local_transform) = local_pose_to_grid_storage(grid, local_position, local_rotation);
    migrate_to_grid(commands, entity, new_grid, cell, local_transform);
}

/// Convert a semantic pose in one Grid's local frame into BigSpace's stored
/// `(CellCoord, Transform)` representation.
pub fn local_pose_to_grid_storage(
    grid: &Grid,
    local_position: DVec3,
    local_rotation: DQuat,
) -> (CellCoord, Transform) {
    let (cell, translation) = grid.translation_to_grid(local_position);
    (
        cell,
        Transform::from_translation(translation).with_rotation(local_rotation.as_quat()),
    )
}
