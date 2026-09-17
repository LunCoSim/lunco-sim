//! Body-fixed surface-frame resolution in the BigSpace scene adapter.
//!
//! The semantic geodesy package provides the body-fixed ENU basis. This module
//! composes it with the live BigSpace branch and exposes the result to any
//! surface consumer. Camera realization is deliberately outside this module.

use bevy::math::{DQuat, DVec3, Vec3};
use bevy::prelude::{ChildOf, Entity, Query, Transform};
use big_space::prelude::{CellCoord, Grid};
use lunco_celestial::geo::LocalTangentFrame;

use crate::components::LocalGravityField;

/// Return the body's ENU tangent frame in an entity's immediate Grid frame.
pub fn surface_axes_in_grid<F: bevy::ecs::query::QueryFilter>(
    grid_entity: Entity,
    gravity: &LocalGravityField,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(Vec3, Vec3, Vec3)> {
    let body_entity = gravity.body_entity?;
    let (_, _, grid_to_body_frame, _, body_to_body_frame) =
        lunco_spatial::coords::common_grid_poses(
            grid_entity,
            body_entity,
            q_parents,
            q_grids,
            q_spatial,
        )?;
    let body_to_grid = grid_to_body_frame.inverse() * body_to_body_frame;
    Some(surface_axes_from_body_position(
        gravity.body_relative_position,
        body_to_grid,
    ))
}

/// Map a body-fixed ENU frame into a Grid after resolving its spatial branch.
pub fn surface_axes_from_body_position(
    body_relative_position: DVec3,
    body_to_grid: DQuat,
) -> (Vec3, Vec3, Vec3) {
    let tangent = LocalTangentFrame::from_body_fixed_position(body_relative_position);
    (
        (body_to_grid * tangent.east)
            .normalize_or(DVec3::X)
            .as_vec3(),
        (body_to_grid * tangent.north)
            .normalize_or(DVec3::NEG_Z)
            .as_vec3(),
        (body_to_grid * tangent.up).normalize_or(DVec3::Y).as_vec3(),
    )
}

/// Resolve a body-fixed ENU frame for a point already expressed in a Grid.
pub fn surface_axes_for_grid_position<F: bevy::ecs::query::QueryFilter>(
    grid_entity: Entity,
    grid_position: DVec3,
    body_entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<(Vec3, Vec3, Vec3)> {
    let (_, grid_body_position, grid_to_body_frame, body_position, body_to_body_frame) =
        lunco_spatial::coords::common_grid_poses(
            grid_entity,
            body_entity,
            q_parents,
            q_grids,
            q_spatial,
        )?;
    let body_relative_position = body_to_body_frame.inverse()
        * (grid_body_position + grid_to_body_frame * grid_position - body_position);
    let body_to_grid = grid_to_body_frame.inverse() * body_to_body_frame;
    Some(surface_axes_from_body_position(
        body_relative_position,
        body_to_grid,
    ))
}

/// Return gravity-up in an entity's immediate Grid frame.
///
/// If the live hierarchy cannot provide a valid body or world pose, returns
/// `None` so the caller can hold its current pose rather than inventing an
/// axis for malformed authored topology.
pub fn gravity_up_in_grid<F: bevy::ecs::query::QueryFilter>(
    grid_entity: Entity,
    gravity: &LocalGravityField,
    q_parents: &Query<&ChildOf>,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<Vec3> {
    if let Some((_, _, up)) =
        surface_axes_in_grid(grid_entity, gravity, q_parents, q_grids, q_spatial)
    {
        return Some(up);
    }

    let (_, grid_rotation) =
        lunco_spatial::coords::world_pose(grid_entity, q_parents, q_grids, q_spatial).ok()?;
    Some(
        (grid_rotation.0.inverse() * gravity.up)
            .normalize_or(DVec3::Y)
            .as_vec3(),
    )
}
