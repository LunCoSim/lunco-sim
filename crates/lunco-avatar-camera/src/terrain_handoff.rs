use bevy::ecs::query::QueryFilter;
use bevy::ecs::system::SystemParam;
use bevy::math::DVec3;
use bevy::prelude::{ChildOf, Entity, Query, Res, Transform, Without};
use big_space::prelude::{CellCoord, Grid};
use lunco_embodiment_core::roles::Embodiment;
use lunco_spatial::{ActivePhysicsFrame, coords::GridPos};
use lunco_terrain_surface::GridSurfaceQuery;

#[derive(SystemParam)]
pub(crate) struct TerrainHandoffFrame<'w, 's> {
    pub(crate) active_frame: Option<Res<'w, ActivePhysicsFrame>>,
    pub(crate) terrain: GridSurfaceQuery<'w, 's>,
    pub(crate) grids: Query<'w, 's, &'static Grid>,
    pub(crate) parents: Query<'w, 's, &'static ChildOf>,
    pub(crate) spatial:
        Query<'w, 's, (Option<&'static CellCoord>, &'static Transform), Without<Embodiment>>,
}

fn clearance_from_height(position: DVec3, ground_y: f64) -> Option<f64> {
    let clearance = position.y - ground_y;
    clearance.is_finite().then_some(clearance)
}

fn radius_to_surface(camera_position: DVec3, ground_y: f64, body_position: DVec3) -> Option<f64> {
    let ground_position = DVec3::new(camera_position.x, ground_y, camera_position.z);
    let radius = (ground_position - body_position).length();
    (radius.is_finite() && radius > 0.0).then_some(radius)
}

/// Sample DEM clearance for a camera pose in the active physics frame.
pub(crate) fn clearance_m<F: QueryFilter>(
    camera: Entity,
    cell: Option<&CellCoord>,
    transform: &Transform,
    active_frame: Option<&ActivePhysicsFrame>,
    terrain: &GridSurfaceQuery,
    parents: &Query<&ChildOf>,
    grids: &Query<&Grid>,
    spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<f64> {
    let frame = active_frame?.0;
    let (position, _) = lunco_spatial::coords::pose_in_grid_seeded(
        camera, frame, cell, transform, parents, grids, spatial,
    )?;
    let ground_y = terrain.height_at(GridPos(position))?;
    clearance_from_height(position, ground_y)
}

/// Radius from the body's centre to the DEM point under the camera projection.
pub(crate) fn terrain_radius_m<F: QueryFilter>(
    body: Entity,
    camera: Entity,
    cell: Option<&CellCoord>,
    transform: &Transform,
    active_frame: Option<&ActivePhysicsFrame>,
    terrain: &GridSurfaceQuery,
    parents: &Query<&ChildOf>,
    grids: &Query<&Grid>,
    spatial: &Query<(Option<&CellCoord>, &Transform), F>,
) -> Option<f64> {
    let frame = active_frame?.0;
    let (camera_position, _) = lunco_spatial::coords::pose_in_grid_seeded(
        camera, frame, cell, transform, parents, grids, spatial,
    )?;
    let ground_y = terrain.height_at(GridPos(camera_position))?;
    let (body_position, _) =
        lunco_spatial::coords::pose_in_grid(body, frame, parents, grids, spatial)?;
    radius_to_surface(camera_position, ground_y, body_position)
}

#[cfg(test)]
mod tests {
    use super::{clearance_from_height, radius_to_surface};
    use bevy::math::DVec3;

    #[test]
    fn handoff_clearance_and_orbital_floor_follow_dem_elevation() {
        let reference_radius_m = 1_737_400.0;
        let camera_position = DVec3::new(0.0, reference_radius_m - 900.0, 0.0);
        let terrain_height_m = reference_radius_m - 1_900.0;

        assert_eq!(
            clearance_from_height(camera_position, terrain_height_m),
            Some(1_000.0)
        );
        assert_eq!(
            radius_to_surface(camera_position, terrain_height_m, DVec3::ZERO),
            Some(reference_radius_m - 1_900.0)
        );
        assert_eq!(clearance_from_height(camera_position, f64::NAN), None);
    }
}
