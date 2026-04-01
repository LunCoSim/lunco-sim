use bevy::prelude::*;
use bevy::input::mouse::{MouseWheel};
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

pub struct CameraMigrationPlugin;

impl Plugin for CameraMigrationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (
            camera_migration_system,
            update_observer_camera_system,
            update_camera_clip_planes_system,
        ).chain());
    }
}

pub fn camera_migration_system(
    mut commands: Commands,
    q_camera: Query<(Entity, &ObserverCamera), Changed<ObserverCamera>>,
    q_all_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
) {
    for (cam_entity, obs) in q_camera.iter() {
        let Some(target) = obs.focus_target else { continue; };
        let mut current = target;
        let mut found_grid = None;
        for _ in 0..10 { 
           if q_grids.contains(current) {
               found_grid = Some(current);
               break;
           }
           if let Ok(child_of) = q_all_parents.get(current) {
               current = child_of.parent();
           } else {
               break;
           }
        }
        if let Some(grid_parent) = found_grid {
            let mut current_parent = None;
            if let Ok(cam_child_of) = q_all_parents.get(cam_entity) {
                current_parent = Some(cam_child_of.parent());
            }
            if current_parent != Some(grid_parent) {
                info!("MIGRATING CAMERA to grid anchor: {:?}", grid_parent);
                commands.entity(cam_entity).set_parent_in_place(grid_parent);
            }
        }
    }
}

pub fn update_observer_camera_system(
    mut q_camera: Query<(Entity, &mut ObserverCamera, &mut CellCoord, &mut Transform, &ActiveCamera), Without<CelestialBody>>,
    q_targets: Query<(&CellCoord, &Transform), Without<ObserverCamera>>,
    q_all_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    mut scroll_evr: MessageReader<MouseWheel>,
) {
    for (entity, mut obs, mut cam_cell, mut cam_tf, _) in q_camera.iter_mut() {
        let Some(target_entity) = obs.focus_target else { continue; };
        for ev in scroll_evr.read() {
            obs.distance = (obs.distance - (ev.y as f64) * (obs.distance * 0.1)).clamp(100.0, 1.0e14);
        }
        let Ok(cam_child_of) = q_all_parents.get(entity) else { continue; };
        let grid_entity = cam_child_of.parent();
        let Ok(grid) = q_grids.get(grid_entity) else { continue; };
        
        if let Ok((target_cell, target_tf)) = q_targets.get(target_entity) {
            let target_pos_local = grid.grid_position_double(target_cell, target_tf);
            let offset = DVec3::new(0.0, obs.distance * 0.707, obs.distance * 0.707);
            let desired_pos_local = target_pos_local + offset;
            let (new_cell, new_tf) = grid.translation_to_grid(desired_pos_local);
            *cam_cell = new_cell;
            cam_tf.translation = new_tf;
            cam_tf.look_at(target_pos_local.as_vec3(), Vec3::Y);
        }
    }
}

pub fn update_camera_clip_planes_system(
    mut q_camera: Query<(&mut Projection, &GlobalTransform), With<Camera>>,
    q_bodies: Query<(&GlobalTransform, &CelestialBody)>,
) {
    for (mut projection, cam_gtf) in q_camera.iter_mut() {
        if let Projection::Perspective(ref mut perspective) = *projection {
            perspective.far = 1.0e15; 
            
            let mut min_dist_to_surface = 1.0e15;
            for (body_gtf, body) in q_bodies.iter() {
                let dist_to_center = cam_gtf.translation().distance(body_gtf.translation()) as f64;
                let dist_to_surface = (dist_to_center - body.radius_m).max(1.0);
                if dist_to_surface < min_dist_to_surface {
                    min_dist_to_surface = dist_to_surface;
                }
            }
            
            // Adaptive near plane, but clamped to 10km to avoid 'eating' the Sun/Planets at scale
            perspective.near = (min_dist_to_surface * 0.001).max(0.1).min(10000.0) as f32;
        }
    }
}
