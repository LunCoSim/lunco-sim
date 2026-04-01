use bevy::prelude::*;
use bevy::input::mouse::{MouseWheel, MouseMotion};
use bevy_hierarchy::Parent;
use bevy::math::DVec3;
use big_space::prelude::*;
use crate::registry::CelestialBody;

#[derive(Component)]
pub struct ObserverCamera {
    pub focus_target: Option<Entity>,
    pub distance: f64,
}

#[derive(Component)]
pub struct ActiveCamera;

pub fn update_observer_camera_system(
    mut q_camera: Query<(Entity, &mut ObserverCamera, &mut CellCoord, &mut Transform, &ActiveCamera), Without<CelestialBody>>,
    q_targets: Query<(&CellCoord, &Transform, &GlobalTransform, &CelestialBody), Without<ObserverCamera>>,
    q_all_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    mut scroll_evr: MessageReader<MouseWheel>,
) {
    for (entity, mut obs, mut cam_cell, mut cam_tf, _) in q_camera.iter_mut() {
        let Some(target_entity) = obs.focus_target else { continue; };
        let Ok((target_cell, target_tf, _target_gtf, _body)) = q_targets.get(target_entity) else { continue; };
        
        // 1. Process zoom input
        for ev in scroll_evr.read() {
            obs.distance = (obs.distance - (ev.y as f64) * (obs.distance * 0.1)).clamp(1_000.0, 1.0e11);
        }

        // 2. Keep camera at 'distance' from target in target's parent grid
        if let (Ok(cam_child_of), Ok(target_child_of)) = (q_all_parents.get(entity), q_all_parents.get(target_entity)) {
            let target_parent = target_child_of.parent();
            let Ok(grid) = q_grids.get(target_parent) else { continue; };
            
            let target_pos = grid.grid_position_double(target_cell, target_tf);
            let offset = DVec3::new(0.0, obs.distance, 0.0);
            let desired_pos = target_pos + offset;

            let (new_cell, new_tf) = grid.translation_to_grid(desired_pos);
            
            if cam_child_of.parent() == target_parent {
                *cam_cell = new_cell;
                cam_tf.translation = new_tf;
                cam_tf.look_at(target_tf.translation, Vec3::Y);
            }
        }
    }
}


/// Dynamic near clip plane adjustment (FR-025)
pub fn update_camera_clip_planes_system(
    mut q_camera: Query<(&mut Projection, &GlobalTransform), With<Camera>>,
    q_bodies: Query<(&GlobalTransform, &CelestialBody)>,
) {
    for (mut projection, cam_gtf) in q_camera.iter_mut() {
        if let Projection::Perspective(ref mut perspective) = *projection {
            // Find distance to nearest body surface
            let mut min_altitude = 1.0e11; // Start large
            
            for (body_gtf, _body) in q_bodies.iter() {
                // In Phase 1, we don't have the radius in the component, so we might need it.
                // For now, let's just use the center distance as an approximation or 
                // just use the camera's absolute position if we had it.
                let dist = cam_gtf.translation().distance(body_gtf.translation()) as f64;
                if dist < min_altitude {
                    min_altitude = dist;
                }
            }
            
            // Adjust near plane: near = max(altitude * 0.001, 0.1), clamped [0.1, 1000.0]
            let near = (min_altitude * 0.001).max(0.1).min(1000.0) as f32;
            perspective.near = near;
        }
    }
}
