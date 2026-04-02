use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::{Grid, CellCoord};
use crate::registry::CelestialBody;
use crate::coords::get_absolute_pos_in_root_double_ghost_aware;
use crate::{SurfaceClickEvent, RoverClickEvent};
use lunco_sim_controller::ControllerLink;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObserverMode {
    Orbital,
    Flyby,
    Surface,
}

#[derive(Component)]
pub struct ObserverCamera {
    pub focus_target: Option<Entity>,
    pub mode: ObserverMode,
    pub distance: f64,
    pub pitch: f32,
    pub yaw: f32,
    pub local_flyby_pos: DVec3, 
    pub altitude: f64,
}

impl Default for ObserverCamera {
    fn default() -> Self {
        Self {
            focus_target: None,
            mode: ObserverMode::Orbital,
            distance: 15_000_000.0,
            pitch: 0.0,
            yaw: 0.0,
            local_flyby_pos: DVec3::ZERO,
            altitude: 0.0,
        }
    }
}

#[derive(Component)]
pub struct ActiveCamera;

#[derive(Resource, Default)]
pub struct CameraScroll {
    pub delta: f32,
}

pub struct CameraMigrationPlugin;

impl Plugin for CameraMigrationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraScroll>();
        app.add_systems(Update, (
            camera_migration_system,
            camera_selection_system,
            focus_transition_system,
            update_observer_camera_system,
            update_camera_clip_planes_system,
        ).chain());
    }
}

pub fn focus_transition_system(
    mut q_camera: Query<(&mut ObserverCamera, Entity), Changed<ObserverCamera>>,
    q_bodies: Query<&CelestialBody>,
    mut last_targets: Local<std::collections::HashMap<Entity, Option<Entity>>>,
) {
    for (mut obs, cam_ent) in q_camera.iter_mut() {
        let new_target = obs.focus_target;
        let old_target = last_targets.get(&cam_ent).copied().flatten();
        
        if new_target == old_target { continue; }
        last_targets.insert(cam_ent, new_target);

        if let Some(target_ent) = new_target {
             let mut old_radius = 0.0;
             if let Some(old_ent) = old_target {
                 if let Ok(body) = q_bodies.get(old_ent) {
                     old_radius = body.radius_m;
                 }
             }

             if let Ok(new_body) = q_bodies.get(target_ent) {
                 // Preserve altitude
                 let current_altitude = if old_radius > 0.0 {
                     obs.distance - old_radius
                 } else {
                     obs.altitude 
                 };
                 
                 obs.distance = (new_body.radius_m + current_altitude).max(10.0);
                 obs.altitude = current_altitude; 
                 
                 if new_body.name == "Earth" {
                     obs.mode = ObserverMode::Orbital;
                 }
             }
        }
    }
}

pub fn camera_selection_system(
    mut q_camera: Query<&mut ObserverCamera, With<ActiveCamera>>,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_rovers: Query<Entity, With<lunco_sim_core::RoverVessel>>,
    mut commands: Commands,
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<ActiveCamera>>,
) {
    if !mouse_button.just_pressed(MouseButton::Left) { return; }
    
    let Some(window) = windows.iter().next() else { return; };
    let Some(mouse_pos) = window.cursor_position() else { return; };
    let Some((camera, cam_gtf)) = cameras.iter().next() else { return; };
    
    let Ok(ray) = camera.viewport_to_world(cam_gtf, mouse_pos) else { return; };
    
    let mut nearest_body = None;
    let mut nearest_rover = None;
    let mut min_body_t = f32::INFINITY;
    let mut min_rover_t = f32::INFINITY;
    
    for (entity, body_gtf, body) in q_bodies.iter() {
        let center = body_gtf.translation();
        let radius = body.radius_m as f32;
        let oc = ray.origin - center;
        let h = oc.dot(oc) - radius * radius;
        if h <= 0.0 {
            let t = 0.01;
            if t < min_body_t { min_body_t = t; nearest_body = Some((entity, body_gtf, body, t)); }
        }
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - radius * radius;
        let discr = b * b - c;
        if discr >= 0.0 {
            let t = -b - discr.sqrt();
            if t > 0.0 && t < min_body_t { min_body_t = t; nearest_body = Some((entity, body_gtf, body, t)); }
        }
    }

    for rover_ent in q_rovers.iter() {
        let radius = 5.0;
        let Ok((_, rover_gtf, _)) = q_bodies.get(rover_ent) else { continue; };
        let center = rover_gtf.translation();
        let oc = ray.origin - center;
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - radius * radius;
        let h = b * b - c;
        if h >= 0.0 {
            let t = -b - h.sqrt();
            if t > 0.0 && t < min_rover_t { min_rover_t = t; nearest_rover = Some(rover_ent); }
        }
    }
    
    if let Some(rover) = nearest_rover {
        if min_rover_t < min_body_t {
             commands.trigger(RoverClickEvent { rover });
             return;
        }
    }

    if let Some((body_ent, body_gtf, _, t)) = nearest_body {
        if let Some(mut obs) = q_camera.iter_mut().next() {
            if obs.mode == ObserverMode::Orbital {
                 obs.focus_target = Some(body_ent);
            } else {
                let hit_point = ray.origin + ray.direction.as_vec3() * t;
                let body_pos: Vec3 = body_gtf.translation();
                let local_hit = (hit_point - body_pos).as_dvec3();
                let normal = local_hit.normalize_or_zero().as_vec3();
                commands.trigger(SurfaceClickEvent {
                    planet: body_ent,
                    click_pos_local: local_hit,
                    surface_normal: normal,
                });
            }
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
           if q_grids.contains(current) { found_grid = Some(current); break; }
           if let Ok(child_of) = q_all_parents.get(current) { current = child_of.parent(); } else { break; }
        }
        if let Some(grid_parent) = found_grid {
            let mut current_parent = None;
            if let Ok(cam_child_of) = q_all_parents.get(cam_entity) { current_parent = Some(cam_child_of.parent()); }
            if current_parent != Some(grid_parent) { commands.entity(cam_entity).set_parent_in_place(grid_parent); }
        }
    }
}

pub fn update_observer_camera_system(
    mut q_camera: Query<(Entity, &mut ObserverCamera, &mut CellCoord, &mut Transform, &ActiveCamera), (Without<CelestialBody>, Without<ControllerLink>)>,
    q_spatial: Query<(&CellCoord, &Transform, Option<&CelestialBody>), Without<ObserverCamera>>,
    q_all_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_coords: Query<(&CellCoord, &Transform), Without<ObserverCamera>>,
    mouse_button: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    windows: Query<&Window>,
    mut scroll_res: ResMut<CameraScroll>,
    mut last_mouse_pos: Local<Option<Vec2>>,
) {
    let window = windows.iter().next();
    let current_mouse_pos = window.and_then(|w| w.cursor_position());
    let mut mouse_delta = Vec2::ZERO;

    if let (Some(curr), Some(last)) = (current_mouse_pos, *last_mouse_pos) {
        if mouse_button.pressed(MouseButton::Middle) || mouse_button.pressed(MouseButton::Right) {
            mouse_delta = curr - last;
        }
    }
    *last_mouse_pos = current_mouse_pos;

    for (cam_entity, mut obs, mut cam_cell, mut cam_tf, _) in q_camera.iter_mut() {
        let Some(target_entity) = obs.focus_target else { continue; };
        let Ok((t_cell, t_tf, t_body)) = q_spatial.get(target_entity) else { continue; };
        let target_pos_solar = get_absolute_pos_in_root_double_ghost_aware(target_entity, t_cell, t_tf, &q_all_parents, &q_grids, &q_coords);
        let Ok(cam_child_of) = q_all_parents.get(cam_entity) else { continue; };
        let cam_grid_ent = cam_child_of.parent();
        let Ok(cam_grid) = q_grids.get(cam_grid_ent) else { continue; };
        let Ok((cg_cell, cg_tf)) = q_coords.get(cam_grid_ent) else { continue; };
        let cam_grid_pos_solar = get_absolute_pos_in_root_double_ghost_aware(cam_grid_ent, cg_cell, cg_tf, &q_all_parents, &q_grids, &q_coords);
        let target_pos_in_cam_grid = target_pos_solar - cam_grid_pos_solar;

        let altitude;
        if let Some(body) = t_body {
            let dist_to_center = if obs.mode == ObserverMode::Orbital { obs.distance } else { obs.local_flyby_pos.length() };
            altitude = dist_to_center - body.radius_m;
            
            // Switch to Surface: alt < 10km
            if altitude < 10_000.0 && obs.mode != ObserverMode::Surface {
                obs.mode = ObserverMode::Surface;
                // Transition: Convert offset to body-relative (counter-rotate by body rotation)
                let rel_offset = cam_tf.translation.as_dvec3() - target_pos_in_cam_grid;
                obs.local_flyby_pos = t_tf.rotation.inverse().mul_vec3(rel_offset.as_vec3()).as_dvec3();
            }
            
            // Switch to Flyby: alt > 15km (hysteresis)
            if altitude > 15_000.0 && obs.mode == ObserverMode::Surface {
                obs.mode = ObserverMode::Flyby;
                // Transition: Convert body-relative back to grid-relative
                obs.local_flyby_pos = t_tf.rotation.mul_vec3(obs.local_flyby_pos.as_vec3()).as_dvec3();
            }

            // Switch to Flyby: alt < 1,000km AND NOT Earth AND NOT Surface
            if altitude < 1_000_000.0 && obs.mode == ObserverMode::Orbital && body.name != "Earth" {
                obs.mode = ObserverMode::Flyby;
                let rot = Quat::from_euler(EulerRot::YXZ, obs.yaw, obs.pitch, 0.0);
                obs.local_flyby_pos = rot.mul_vec3(Vec3::Z).as_dvec3() * obs.distance;
            }
            
            // Switch to Orbital: alt > 1,500km (hysteresis) OR is Earth (prevent Flyby for Earth at high alt)
            if (altitude > 1_500_000.0 && obs.mode == ObserverMode::Flyby) || (body.name == "Earth" && obs.mode == ObserverMode::Flyby) {
                obs.mode = ObserverMode::Orbital;
                obs.distance = obs.local_flyby_pos.length();
                let dir = obs.local_flyby_pos.normalize_or_zero().as_vec3();
                obs.yaw = dir.x.atan2(dir.z);
                obs.pitch = (-dir.y).asin();
            }
        } else { altitude = 1_000_000.0; }
        obs.altitude = altitude;

        let mut scroll = scroll_res.delta as f64 * -0.01;
        if keys.pressed(KeyCode::Equal) { scroll += 1.0; }
        if keys.pressed(KeyCode::Minus) { scroll -= 1.0; }

        if obs.mode == ObserverMode::Orbital {
            obs.distance = (obs.distance - (scroll as f64) * (obs.distance * 0.1)).clamp(10.0, 1.0e14);
            obs.yaw -= mouse_delta.x * 0.01;
            obs.pitch = (obs.pitch - mouse_delta.y * 0.01).clamp(-1.5, 1.5);
            let rotation = Quat::from_euler(EulerRot::YXZ, obs.yaw, obs.pitch, 0.0);
            let offset = rotation.mul_vec3(Vec3::Z).as_dvec3() * obs.distance;
            let desired_pos_local = target_pos_in_cam_grid + offset;
            let (new_cell, new_tf) = cam_grid.translation_to_grid(desired_pos_local);
            *cam_cell = new_cell; cam_tf.translation = new_tf;
            cam_tf.look_at(target_pos_in_cam_grid.as_vec3(), Vec3::Y);
        } else if obs.mode == ObserverMode::Flyby {
            obs.yaw -= mouse_delta.x * 0.01;
            obs.pitch = (obs.pitch - mouse_delta.y * 0.01).clamp(-1.55, 1.55);
            let rotation = Quat::from_euler(EulerRot::YXZ, obs.yaw, obs.pitch, 0.0);
            cam_tf.rotation = rotation;
            let base_speed = if obs.altitude < 50_000.0 { 10_000.0 } else { 1000.0 };
            let speed = base_speed * (obs.altitude / 1000.0).max(1.0) * time.delta_secs_f64();
            let mut move_vec = DVec3::ZERO;
            let forward = rotation.mul_vec3(Vec3::NEG_Z).as_dvec3();
            let right = rotation.mul_vec3(Vec3::X).as_dvec3();
            if keys.pressed(KeyCode::KeyW) { move_vec += forward; }
            if keys.pressed(KeyCode::KeyS) { move_vec -= forward; }
            if keys.pressed(KeyCode::KeyD) { move_vec += right; }
            if keys.pressed(KeyCode::KeyA) { move_vec -= right; }
            if keys.pressed(KeyCode::Space) { move_vec += DVec3::Y; }
            obs.local_flyby_pos += move_vec * speed;
            let desired_pos_local = target_pos_in_cam_grid + obs.local_flyby_pos;
            let (new_cell, new_tf) = cam_grid.translation_to_grid(desired_pos_local);
            *cam_cell = new_cell; cam_tf.translation = new_tf;
        } else if obs.mode == ObserverMode::Surface {
            // Surface Mode: Orientation aligned to local normal (surface tangent)
            // local_flyby_pos here is relative to body and rotated BY body
            let body_rot = t_tf.rotation;
            let current_pos_offset = body_rot.mul_vec3(obs.local_flyby_pos.as_vec3());
            let _cam_pos_local = target_pos_in_cam_grid + current_pos_offset.as_dvec3();
            
            let up = current_pos_offset.normalize_or_zero();
            
            // Rotation: yaw around local Up, pitch around local Right
            obs.yaw -= mouse_delta.x * 0.01;
            obs.pitch = (obs.pitch - mouse_delta.y * 0.01).clamp(-1.5, 1.5);
            
            let look_quat = Quat::from_axis_angle(up, obs.yaw);
            let final_rot = look_quat * Quat::from_axis_angle(look_quat.mul_vec3(Vec3::X), obs.pitch);
            cam_tf.rotation = final_rot;

            // Movement: Control in body-relative frame
            let speed = (obs.altitude * 0.5 + 50.0).max(10.0) * time.delta_secs_f64();
            let _move_vec_body = DVec3::ZERO;
            
            // Controls relative to view
            let forward = final_rot.mul_vec3(Vec3::NEG_Z);
            let right_move = final_rot.mul_vec3(Vec3::X);
            
            let mut move_dir_world = Vec3::ZERO;
            if keys.pressed(KeyCode::KeyW) { move_dir_world += forward; }
            if keys.pressed(KeyCode::KeyS) { move_dir_world -= forward; }
            if keys.pressed(KeyCode::KeyD) { move_dir_world += right_move; }
            if keys.pressed(KeyCode::KeyA) { move_dir_world -= right_move; }
            if keys.pressed(KeyCode::Space) { move_dir_world += up; }
            if keys.pressed(KeyCode::ShiftLeft) { move_dir_world -= up; }

            // Convert world move to body-relative move
            let move_vec_body_space = body_rot.inverse().mul_vec3(move_dir_world).as_dvec3() * speed;
            obs.local_flyby_pos += move_vec_body_space;
            
            // Calculate final position in cam grid
            let final_pos_offset = body_rot.mul_vec3(obs.local_flyby_pos.as_vec3());
            let desired_pos_local = target_pos_in_cam_grid + final_pos_offset.as_dvec3();
            
            let (new_cell, new_tf) = cam_grid.translation_to_grid(desired_pos_local);
            *cam_cell = new_cell; cam_tf.translation = new_tf;
        }
    }
    scroll_res.delta = 0.0;
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
                if dist_to_surface < min_dist_to_surface { min_dist_to_surface = dist_to_surface; }
            }
            perspective.near = (min_dist_to_surface as f32 * 0.01).clamp(0.1, 100.0);
        }
    }
}
