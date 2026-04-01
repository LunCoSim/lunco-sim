use bevy::prelude::*;
use bevy::input::mouse::{MouseWheel, MouseMotion};
use big_space::prelude::*;
use crate::registry::CelestialBody;
use crate::coords::get_absolute_pos_in_root_double_ghost_aware;

#[derive(Component)]
pub struct ObserverCamera {
    pub focus_target: Option<Entity>,
    pub distance: f64,
    pub pitch: f32,
    pub yaw: f32,
}

impl Default for ObserverCamera {
    fn default() -> Self {
        Self {
            focus_target: None,
            distance: 15_000_000.0,
            pitch: 0.0,
            yaw: 0.0,
        }
    }
}

#[derive(Component)]
pub struct ActiveCamera;

pub struct CameraMigrationPlugin;

impl Plugin for CameraMigrationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (
            camera_migration_system,
            camera_selection_system,
            update_observer_camera_system,
            update_camera_clip_planes_system,
        ).chain());
    }
}

pub fn camera_selection_system(
    mut q_camera: Query<&mut ObserverCamera, With<ActiveCamera>>,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<ActiveCamera>>,
) {
    if !mouse_button.just_pressed(MouseButton::Left) { return; }
    
    let Some(window) = windows.iter().next() else { return; };
    let Some(mouse_pos) = window.cursor_position() else { return; };
    let Some((camera, cam_gtf)) = cameras.iter().next() else { return; };
    
    let Ok(ray) = camera.viewport_to_world(cam_gtf, mouse_pos) else { return; };
    
    let mut nearest_entity = None;
    let mut min_dist = f32::INFINITY;
    
    for (entity, body_gtf, body) in q_bodies.iter() {
        let center = body_gtf.translation();
        let radius = body.radius_m as f32;
        
        let oc = ray.origin - center;
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - radius * radius;
        let h = b * b - c;
        
        if h >= 0.0 {
            let t = -b - h.sqrt();
            if t > 0.0 && t < min_dist {
                min_dist = t;
                nearest_entity = Some(entity);
            }
        }
    }
    
    if let Some(target) = nearest_entity {
        for mut obs in q_camera.iter_mut() {
            info!("FOCUSING on {:?}", target);
            obs.focus_target = Some(target);
        }
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
    q_spatial: Query<(&CellCoord, &Transform), Without<ObserverCamera>>, // Disjoint Query to avoid B0001
    q_all_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    mut scroll_evr: MessageReader<MouseWheel>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mouse_button: Res<ButtonInput<MouseButton>>,
) {
    for (cam_entity, mut obs, mut cam_cell, mut cam_tf, _) in q_camera.iter_mut() {
        let Some(target_entity) = obs.focus_target else { continue; };
        
        for ev in scroll_evr.read() {
            obs.distance = (obs.distance - (ev.y as f64) * (obs.distance * 0.1)).clamp(10.0, 1.0e14);
        }
        
        if mouse_button.pressed(MouseButton::Middle) || mouse_button.pressed(MouseButton::Right) {
            for ev in mouse_motion.read() {
                obs.yaw -= ev.delta.x * 0.01;
                obs.pitch = (obs.pitch - ev.delta.y * 0.01).clamp(-1.5, 1.5);
            }
        } else {
            mouse_motion.clear();
        }

        let Ok(cam_child_of) = q_all_parents.get(cam_entity) else { continue; };
        let cam_grid_ent = cam_child_of.parent();
        let Ok(cam_grid) = q_grids.get(cam_grid_ent) else { continue; };
        
        // 1. Resolve Target absolute solar position
        // Planet/rover is NOT the camera, so it's in q_spatial
        let Ok((t_cell, t_tf)) = q_spatial.get(target_entity) else { continue; };
        let target_pos_solar = get_absolute_pos_in_root_double_ghost_aware(target_entity, t_cell, t_tf, &q_all_parents, &q_grids, &q_spatial);
        
        // 2. Resolve Camera Grid absolute solar position
        // Camera Grid is NOT the camera, so it's in q_spatial
        let Ok((g_cell, g_tf)) = q_spatial.get(cam_grid_ent) else { continue; };
        let cam_grid_pos_solar = get_absolute_pos_in_root_double_ghost_aware(cam_grid_ent, g_cell, g_tf, &q_all_parents, &q_grids, &q_spatial);
        
        let target_pos_in_cam_grid = target_pos_solar - cam_grid_pos_solar;
        
        let rotation = Quat::from_euler(EulerRot::YXZ, obs.yaw, obs.pitch, 0.0);
        let offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * obs.distance;
        
        let desired_pos_local = target_pos_in_cam_grid + offset;
        let (new_cell, new_tf) = cam_grid.translation_to_grid(desired_pos_local);
        
        *cam_cell = new_cell;
        cam_tf.translation = new_tf;
        cam_tf.look_at(target_pos_in_cam_grid.as_vec3(), Vec3::Y);
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
            perspective.near = (min_dist_to_surface * 0.001).max(0.1).min(10000.0) as f32;
        }
    }
}
