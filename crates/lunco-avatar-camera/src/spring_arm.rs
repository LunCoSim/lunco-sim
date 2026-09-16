use bevy::math::{DVec3, StableInterpolate};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_avatar_camera_core::CAMERA_ZOOM_SENSITIVITY;
use lunco_avatar_core::roles::{Avatar, LocalAvatar};
use lunco_camera_core::{
    CameraDefaults, CameraZoomInput, FollowAttitude, FreeFlightCamera, OrbitCamera,
    SpringArmCamera, SurfaceCamera, SurfaceRelativeMode,
    math::{
        apply_scroll_zoom, camera_decay_rate, resolve_camera_arm_length, surface_camera_angles,
        surface_camera_rotation,
    },
};
use lunco_celestial_spatial::{
    LocalGravityField, gravity_up_in_grid, surface_axes_for_grid_position,
};
use lunco_physics::GridSpatialQuery;
use lunco_spatial::coords::GridPos;

use crate::collision::{VesselCollisionFilterCache, VesselCollisionTopology, VesselJoints};

pub(crate) fn spring_arm_system(
    time: Res<Time<Real>>,
    mut q_avatar: Query<
        (
            Entity,
            &mut Transform,
            &mut CellCoord,
            &mut SpringArmCamera,
            &ChildOf,
            Option<&SurfaceRelativeMode>,
            &mut CameraZoomInput,
        ),
        (
            With<Avatar>,
            With<LocalAvatar>,
            Without<Grid>,
            Without<OrbitCamera>,
            Without<FreeFlightCamera>,
            Without<SurfaceCamera>,
            Without<lunco_camera_core::CameraPoseLock>,
        ),
    >,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<Avatar>>,
    q_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    gravity: Res<LocalGravityField>,
    q_dragging: Query<(), With<lunco_interaction_core::GizmoDragging>>,
    q_children: Query<&Children>,
    defaults: Res<CameraDefaults>,
    keys: Res<ButtonInput<KeyCode>>,
    spatial_query: Option<GridSpatialQuery>,
    joints: VesselJoints,
    mut collision_filters: Local<VesselCollisionFilterCache>,
    mut topology: VesselCollisionTopology,
) {
    if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        return;
    }
    let dt = time.delta_secs();
    collision_filters.refresh(&joints, &mut topology);

    for (_avatar_ent, mut tf, mut cell, mut arm, child_of, surface_mode, mut zoom) in
        q_avatar.iter_mut()
    {
        if q_dragging.get(arm.target).is_ok() {
            continue;
        }

        let Ok(grid) = q_grids.get(child_of.0) else {
            continue;
        };
        let Some((target_pos, target_rotation)) = lunco_spatial::coords::grid_relative_pose(
            arm.target, child_of.0, &q_parents, &q_grids, &q_spatial,
        ) else {
            continue;
        };
        let surface_axes = surface_mode.and_then(|_| {
            gravity.body_entity.and_then(|body_entity| {
                surface_axes_for_grid_position(
                    child_of.0,
                    target_pos,
                    body_entity,
                    &q_parents,
                    &q_grids,
                    &q_spatial,
                )
            })
        });

        apply_scroll_zoom(
            &mut arm.distance,
            &mut zoom.delta,
            CAMERA_ZOOM_SENSITIVITY,
            5.0,
            200.0,
        );

        let desired_rot = match arm.attitude {
            FollowAttitude::FullAttitude => {
                target_rotation.as_quat() * Quat::from_euler(EulerRot::YXZ, arm.yaw, arm.pitch, 0.0)
            }
            FollowAttitude::WorldLocked => Quat::from_euler(EulerRot::YXZ, arm.yaw, arm.pitch, 0.0),
            FollowAttitude::Heading => {
                let target_heading_d = if arm.track_heading {
                    if let Some((east, north, up)) = surface_axes {
                        surface_camera_angles(east, north, up, target_rotation.as_quat()).0 as f64
                    } else {
                        let target_fwd_d = target_rotation.mul_vec3(Vec3::NEG_Z.as_dvec3());
                        if target_fwd_d.x.abs() > 1e-6 || target_fwd_d.z.abs() > 1e-6 {
                            -target_fwd_d.x.atan2(-target_fwd_d.z)
                        } else {
                            0.0
                        }
                    }
                } else {
                    0.0
                };
                let final_yaw = (target_heading_d + arm.yaw as f64) as f32;
                if let Some((east, north, up)) = surface_axes {
                    surface_camera_rotation(east, north, up, final_yaw, arm.pitch)
                } else {
                    Quat::from_euler(EulerRot::YXZ, final_yaw, arm.pitch, 0.0)
                }
            }
        };

        let damping = arm.damping.unwrap_or(defaults.damping);
        let mut next_rotation = tf.rotation;
        next_rotation.smooth_nudge(
            &desired_rot,
            camera_decay_rate(defaults.rotation_rate, damping),
            dt,
        );
        if tf.rotation != next_rotation {
            tf.rotation = next_rotation;
        }

        let offset = tf.rotation.mul_vec3(Vec3::Z).as_dvec3() * arm.distance;
        let vertical_offset: DVec3 = if surface_mode.is_some() {
            let Some(up) = surface_axes.map(|(_, _, up)| up).or_else(|| {
                gravity_up_in_grid(child_of.0, &gravity, &q_parents, &q_grids, &q_spatial)
            }) else {
                continue;
            };
            up.as_dvec3() * arm.vertical_offset as f64
        } else {
            DVec3::Y * arm.vertical_offset as f64
        };
        let desired_pos = target_pos + offset + vertical_offset;
        let ray_origin = target_pos;
        let ray_dir = (desired_pos - target_pos).normalize_or(DVec3::Y);
        let ray_len = desired_pos.distance(target_pos);
        let castable = ray_origin.is_finite() && ray_len.is_finite();
        let hit = match &spatial_query {
            Some(spatial_query) if castable => {
                let filter = collision_filters.filter_for(arm.target, &q_children);
                spatial_query.cast_ray_in_grid(
                    child_of.0,
                    GridPos(ray_origin),
                    Dir3::new(ray_dir.as_vec3()).unwrap_or(Dir3::Y),
                    ray_len,
                    true,
                    filter,
                )
            }
            _ => None,
        };

        let desired_len = ray_len;
        let target_len = match hit {
            Some(hit_data) => ((hit_data.distance - 0.5).min(desired_len)).max(0.0),
            None => desired_len,
        };
        let current_pos = grid.grid_position_double(&cell, &tf);
        let current_len = current_pos.distance(target_pos);
        let final_len = resolve_camera_arm_length(
            current_len,
            target_len,
            hit.is_some(),
            defaults.position_rate,
            damping,
            dt,
        );
        let final_pos = target_pos + ray_dir * final_len;

        let (new_cell, new_tf) = grid.translation_to_grid(final_pos);
        cell.set_if_neq(new_cell);
        if tf.translation != new_tf {
            tf.translation = new_tf;
        }
    }
}
