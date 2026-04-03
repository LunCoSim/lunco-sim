use bevy::prelude::*;
use big_space::prelude::*;
use lunco_core::{Avatar, coords::get_absolute_pos_in_root_double_ghost_aware};
use crate::ViewPoint;

pub struct LunCoBlenderPlugin;

impl Plugin for LunCoBlenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, viewpoint_blender_system);
    }
}

/// System that smoothly interpolates the camera's transform and projection 
/// toward the desired ViewPoint, respecting both target rotation and local offsets.
fn viewpoint_blender_system(
    time: Res<Time>,
    mut q_camera: Query<(Entity, &mut CellCoord, &mut Transform, &mut Projection, &ViewPoint, &Avatar), With<Camera>>,
    q_targets: Query<(Entity, &CellCoord, &Transform), Without<Camera>>,
    q_all_grids: Query<&Grid>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(&CellCoord, &Transform), Without<Camera>>,
) {
    let dt = time.delta_secs();
    
    for (cam_ent, mut cam_cell, mut tf, mut projection, viewpoint, _) in q_camera.iter_mut() {
        if !viewpoint.active { continue; }
        
        if let Some(target_ent) = viewpoint.target {
            if let Ok((t_ent, t_cell, t_tf)) = q_targets.get(target_ent) {
                let lerp_factor = (viewpoint.speed * dt).min(1.0) as f64;
                
                // 1. Get absolute positions for high-precision math
                let target_pos_solar = get_absolute_pos_in_root_double_ghost_aware(t_ent, t_cell, t_tf, &q_parents, &q_all_grids, &q_spatial);
                
                let Ok(cam_child_of) = q_parents.get(cam_ent) else { continue; };
                let cam_grid_ent = cam_child_of.parent();
                let Ok(cam_grid) = q_all_grids.get(cam_grid_ent) else { continue; };
                let Ok((cg_cell, cg_tf)) = q_spatial.get(cam_grid_ent) else { continue; };
                let cam_grid_pos_solar = get_absolute_pos_in_root_double_ghost_aware(cam_grid_ent, cg_cell, cg_tf, &q_parents, &q_all_grids, &q_spatial);
                
                let target_pos_in_cam_grid = target_pos_solar - cam_grid_pos_solar;
                let current_pos_in_cam_grid = cam_grid.grid_position_double(&cam_cell, &tf);

                // 2. Calculate Desired Rotation (Target Rotation * ViewPoint Local Rotation)
                let target_rot = t_tf.rotation * viewpoint.rotation;

                // 3. Calculate Desired Position (Target Position + Rotated Offset)
                let desired_pos_in_cam_grid = target_pos_in_cam_grid + (target_rot * viewpoint.offset.as_vec3()).as_dvec3();
                
                let new_pos_in_cam_grid = current_pos_in_cam_grid.lerp(desired_pos_in_cam_grid, lerp_factor);
                
                // 4. Update camera spatial components
                let (new_cell, new_tf) = cam_grid.translation_to_grid(new_pos_in_cam_grid);
                *cam_cell = new_cell;
                tf.translation = new_tf;
                
                // Slerp Rotation toward target
                tf.rotation = tf.rotation.slerp(target_rot, lerp_factor as f32);

                // Interpolate FOV
                if let Projection::Perspective(ref mut p) = *projection {
                    p.fov = p.fov + (viewpoint.fov.to_radians() - p.fov) * lerp_factor as f32;
                }
            }
        }
    }
}
