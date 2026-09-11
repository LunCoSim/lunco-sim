//! Shared cursor-to-surface resolution for editor tools.
//!
//! Camera rays enter the active physics frame once through `GridSurfaceQuery`.
//! All editor tools then resolve the analytic DEM and physics colliders through
//! this module so preview, placement, waypoint authoring, and terrain editing do
//! not drift into separate coordinate or precedence rules.

use bevy::math::{DVec3, Dir3};
use bevy::prelude::Entity;
use lunco_core::coords::GridPos;

/// Finite ray bound shared by placement tools. `GridSpatialQuery` requires a
/// finite distance, and this span covers the authored editor scene range.
pub(crate) const EDITOR_PLACEMENT_RAY_MAX_DISTANCE: f64 = 1.0e6;

/// Which surface wins when both the DEM and a physics collider answer the ray.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfacePickPolicy {
    /// Prefer the analytic DEM whenever it covers the ray; use physics only when
    /// no DEM surface answers. Terrain editing uses this policy.
    TerrainFirst,
    /// Select the nearest valid surface. Spawn and waypoint authoring use this
    /// policy so props can be placed on top of terrain.
    Nearest,
}

/// The resolved point and the evidence used to choose it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CursorSurfaceHit {
    /// Surface point in the active physics/grid frame.
    pub point: GridPos,
    /// Whether the analytic DEM supplied the selected point.
    pub terrain_primary: bool,
    /// The DEM hit remains available for footprint diagnostics when a physics
    /// prop is nearer.
    pub terrain: Option<lunco_terrain_surface::SurfaceHit>,
    pub physics_distance: Option<f64>,
    pub physics_entity: Option<Entity>,
}

/// Resolve one grid-frame cursor ray using the chosen surface policy.
pub(crate) fn cursor_surface_hit<F>(
    surface: &lunco_terrain_surface::GridSurfaceQuery,
    raycaster: &lunco_physics::GridSpatialQuery<'_, '_>,
    origin: GridPos,
    direction: Dir3,
    max_distance: f64,
    policy: SurfacePickPolicy,
    mut valid_physics: F,
) -> Option<CursorSurfaceHit>
where
    F: FnMut(Entity) -> bool,
{
    let terrain = surface.raycast(origin, direction, max_distance);
    let physics = match policy {
        SurfacePickPolicy::TerrainFirst if terrain.is_some() => None,
        _ => {
            let physics_limit = terrain
                .map(|hit| hit.distance)
                .unwrap_or(max_distance)
                .min(max_distance);
            raycaster
                .cast_ray_grid(
                    origin,
                    direction,
                    physics_limit,
                    false,
                    &avian3d::prelude::SpatialQueryFilter::default(),
                )
                .filter(|hit| valid_physics(hit.entity))
        }
    };

    resolve_cursor_surface(
        origin,
        direction.as_dvec3(),
        terrain,
        physics.map(|hit| hit.distance),
        physics.map(|hit| hit.entity),
        policy,
    )
}

pub(crate) fn resolve_cursor_surface(
    origin: GridPos,
    direction: DVec3,
    terrain: Option<lunco_terrain_surface::SurfaceHit>,
    physics_distance: Option<f64>,
    physics_entity: Option<Entity>,
    policy: SurfacePickPolicy,
) -> Option<CursorSurfaceHit> {
    match policy {
        SurfacePickPolicy::TerrainFirst => terrain
            .map(|hit| CursorSurfaceHit {
                point: hit.point,
                terrain_primary: true,
                terrain: Some(hit),
                physics_distance,
                physics_entity,
            })
            .or_else(|| {
                physics_distance.map(|distance| CursorSurfaceHit {
                    point: GridPos(origin.0 + direction * distance),
                    terrain_primary: false,
                    terrain: None,
                    physics_distance: Some(distance),
                    physics_entity,
                })
            }),
        SurfacePickPolicy::Nearest => match (physics_distance, terrain) {
            (Some(distance), Some(hit)) if distance < hit.distance => Some(CursorSurfaceHit {
                point: GridPos(origin.0 + direction * distance),
                terrain_primary: false,
                terrain: Some(hit),
                physics_distance: Some(distance),
                physics_entity,
            }),
            (_, Some(hit)) => Some(CursorSurfaceHit {
                point: hit.point,
                terrain_primary: true,
                terrain: Some(hit),
                physics_distance,
                physics_entity,
            }),
            (Some(distance), None) => Some(CursorSurfaceHit {
                point: GridPos(origin.0 + direction * distance),
                terrain_primary: false,
                terrain: None,
                physics_distance: Some(distance),
                physics_entity,
            }),
            (None, None) => None,
        },
    }
}
