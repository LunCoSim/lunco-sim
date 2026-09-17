//! Celestial spatial adapter for generic surface cameras.
//!
//! [`lunco_camera_core`] owns the backend-neutral `SurfaceCameraFrame`
//! contract and [`lunco_camera_runtime`] consumes it. This package is the
//! narrow adapter that resolves that contract from a live BigSpace hierarchy
//! and a [`lunco_environment::GravityBody`] binding. Avatar-specific orbital
//! placement is supplied by `lunco-avatar-camera`.

use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_camera_core::{
    CameraPoseLock, CameraRig, CameraUpdateSet, SurfaceCamera, SurfaceCameraFrame,
};
use lunco_celestial_spatial::surface_axes_for_grid_position;
use lunco_core::CelestialBody;
use lunco_environment::GravityBody;

/// Publishes body-fixed surface frames for cameras bound to a celestial body.
pub struct CelestialSurfaceCameraPlugin;

impl Plugin for CelestialSurfaceCameraPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        app.add_systems(
            lunco_time::InteractionSchedule,
            publish_surface_camera_frames.before(CameraUpdateSet),
        );
        app.add_systems(
            PostUpdate,
            update_celestial_clip_planes.after(TransformSystems::Propagate),
        );
    }
}

/// Update perspective precision from the celestial bounds visible to a camera.
///
/// Distances are measured from propagated origin-relative transforms, so both
/// camera and body share the same BigSpace frame. The generic math stays in
/// `lunco-camera-core`; this adapter owns only the celestial-body query.
fn update_celestial_clip_planes(
    mut q_camera: Query<
        (&mut Projection, &GlobalTransform),
        (With<Camera>, With<lunco_camera_core::AdaptiveNearPlane>),
    >,
    q_bodies: Query<(&CelestialBody, &GlobalTransform)>,
) {
    for (mut projection, cam_gt) in q_camera.iter_mut() {
        let Projection::Perspective(current) = &*projection else {
            continue;
        };
        let cam_pos = cam_gt.translation().as_dvec3();
        let mut min_dist = f64::INFINITY;
        let mut max_far = 0.0_f64;
        for (body, body_gt) in q_bodies.iter() {
            let center_distance = cam_pos.distance(body_gt.translation().as_dvec3());
            min_dist = min_dist.min(center_distance - body.radius_m);
            max_far = max_far.max(center_distance + body.radius_m);
        }
        let Some((near, far)) = lunco_camera_core::math::adaptive_clip_planes(min_dist, max_far)
        else {
            error!("[camera] cannot derive finite clip planes from celestial body bounds");
            continue;
        };
        let moved = (current.near - near).abs() > near.abs() * 1e-4
            || (current.far - far).abs() > far.abs() * 1e-4;
        if moved {
            if let Projection::Perspective(perspective) = &mut *projection {
                perspective.near = near;
                perspective.far = far;
            }
        }
    }
}

/// Resolve each surface camera's ENU basis in its direct parent Grid.
///
/// The camera owns the position being framed, so this adapter does not read a
/// global avatar-only gravity cache. A missing body binding or disconnected
/// BigSpace branch removes the derived frame; the generic camera runtime then
/// holds its orientation instead of inventing an axis.
fn publish_surface_camera_frames(
    mut commands: Commands,
    q_cameras: Query<
        (
            Entity,
            &ChildOf,
            &CellCoord,
            &Transform,
            Option<&GravityBody>,
            Option<&SurfaceCameraFrame>,
        ),
        (
            With<CameraRig>,
            With<SurfaceCamera>,
            Without<CameraPoseLock>,
        ),
    >,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
) {
    for (entity, child_of, cell, transform, gravity_body, current_frame) in q_cameras.iter() {
        let next_frame = gravity_body.and_then(|gravity_body| {
            let grid = q_grids.get(child_of.parent()).ok()?;
            let grid_position = grid.grid_position_double(cell, transform);
            let (east, north, up) = surface_axes_for_grid_position(
                child_of.parent(),
                grid_position,
                gravity_body.body_entity,
                &q_parents,
                &q_grids,
                &q_spatial,
            )?;
            SurfaceCameraFrame::new(east, north, up)
        });

        if current_frame != next_frame.as_ref() {
            let mut entity_commands = commands.entity(entity);
            if let Some(frame) = next_frame {
                entity_commands.insert(frame);
            } else {
                entity_commands.remove::<SurfaceCameraFrame>();
            }
        }
    }
}
