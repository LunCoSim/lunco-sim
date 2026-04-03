use bevy::prelude::*;
use bevy::math::DVec3;
use big_space::prelude::{Grid, CellCoord};
use lunco_core::{Avatar, CelestialBody, Spacecraft, RoverVessel};
use lunco_core::coords::get_absolute_pos_in_root_double_ghost_aware;

mod transitions;
pub use transitions::*;

pub struct LunCoCameraPlugin;

impl Plugin for LunCoCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraScroll>();
        app.register_type::<ViewPoint>();
        app.register_type::<ObserverCamera>();
        app.register_type::<ObserverMode>();
        app.register_type::<CameraTransition>();
        
        app.add_systems(Update, (
            camera_migration_system,
            camera_selection_system,
            focus_transition_system,
            update_observer_camera_system,
            update_camera_clip_planes_system,
            viewpoint_blender_system,
            camera_transition_system,
        ).chain());
    }
}

/// A target-relative viewport configuration.
/// Used for smooth transitions between different camera perspectives.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct ViewPoint {
    /// The entity to follow/look at
    pub target: Option<Entity>,
    /// High-precision offset in the target's coordinate frame
    pub offset: Vec3,
    /// Desired rotation (relative to target or absolute if target is None)
    pub yaw: f32,
    pub pitch: f32,
    /// Desired Field of View
    pub fov: f32,
    /// Blending speed multiplier
    pub speed: f32,
    /// Whether this viewpoint is currently active
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect, Default)]
pub enum ObserverMode {
    #[default]
    Orbital,
    Flyby,
    Surface,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct ObserverCamera {
    pub focus_target: Option<Entity>,
    pub mode: ObserverMode,
    pub distance: f64,
    pub pitch: f32,
    pub yaw: f32,
    pub local_flyby_pos: DVec3, 
    pub altitude: f64,
    pub smooth_focus_pos: DVec3,
    pub is_first_frame: bool,
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
            smooth_focus_pos: DVec3::ZERO,
            is_first_frame: true,
        }
    }
}

#[derive(Component)]
pub struct ActiveCamera;

#[derive(Resource, Default)]
pub struct CameraScroll {
    pub delta: f32,
}

/// System that smoothly interpolates the camera's transform and projection 
/// toward the desired ViewPoint.
fn viewpoint_blender_system(
    time: Res<Time>,
    mut q_camera: Query<(&mut Transform, &mut Projection, &ViewPoint, &Avatar), With<Camera>>,
    q_targets: Query<&GlobalTransform>,
) {
    let dt = time.delta_secs();
    
    for (mut tf, mut projection, viewpoint, _) in q_camera.iter_mut() {
        if !viewpoint.active { continue; }
        
        if let Some(target_ent) = viewpoint.target {
            if let Ok(target_gtf) = q_targets.get(target_ent) {
                let lerp_factor = (viewpoint.speed * dt).min(1.0);
                
                // Interpolate Transform
                let desired_pos = target_gtf.translation() + target_gtf.back() * viewpoint.offset.z + target_gtf.right() * viewpoint.offset.x + target_gtf.up() * viewpoint.offset.y;
                tf.translation = tf.translation.lerp(desired_pos, lerp_factor);
                
                // Interpolate Rotation toward target
                let target_rot = target_gtf.compute_transform().rotation;
                tf.rotation = tf.rotation.slerp(target_rot, lerp_factor);

                // Interpolate FOV
                if let Projection::Perspective(ref mut p) = *projection {
                    p.fov = p.fov + (viewpoint.fov.to_radians() - p.fov) * lerp_factor;
                }
            }
        }
    }
}

pub fn focus_transition_system(
    mut q_camera: Query<(&mut ObserverCamera, Entity), Changed<ObserverCamera>>,
    q_bodies: Query<&CelestialBody>,
    q_spacecraft: Query<&Spacecraft>,
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
                 } else if let Ok(sc) = q_spacecraft.get(old_ent) {
                     old_radius = sc.hit_radius_m as f64;
                 }
             }

             let mut new_radius = 0.0;
             if let Ok(new_body) = q_bodies.get(target_ent) {
                 new_radius = new_body.radius_m;
             } else if let Ok(sc) = q_spacecraft.get(target_ent) {
                 new_radius = sc.hit_radius_m as f64;
             }

             let current_altitude = if old_radius > 0.0 {
                 obs.distance - old_radius
             } else {
                 obs.altitude 
             };
             
             obs.distance = (new_radius + current_altitude).max(10.0);
             obs.altitude = current_altitude; 
             obs.mode = ObserverMode::Orbital;
        }
    }
}

pub fn camera_selection_system(
    mut q_camera: Query<&mut ObserverCamera, With<Avatar>>,
    q_bodies: Query<(Entity, &GlobalTransform, &CelestialBody)>,
    q_rovers: Query<Entity, With<RoverVessel>>,
    q_spacecraft: Query<(Entity, &Spacecraft)>,
    q_gtfs: Query<&GlobalTransform>,
    mut commands: Commands,
    mouse_button: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform), With<Avatar>>,
) {
    if !mouse_button.just_pressed(MouseButton::Left) { return; }
    
    let Some(window) = windows.iter().next() else { return; };
    let Some(mouse_pos) = window.cursor_position() else { return; };
    let Some((camera, cam_gtf)) = cameras.iter().next() else { return; };
    
    let Ok(ray) = camera.viewport_to_world(cam_gtf, mouse_pos) else { return; };
    
    let mut nearest_body = None;
    let mut nearest_vessel = None;
    let mut min_body_t = f32::INFINITY;
    let mut min_vessel_t = f32::INFINITY;
    
    for (entity, body_gtf, body) in q_bodies.iter() {
        let center = body_gtf.translation();
        let radius = body.radius_m as f32;
        let oc = ray.origin - center;
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - radius * radius;
        let discr = b * b - c;
        if discr >= 0.0 {
            let t = -b - discr.sqrt();
            if t > 0.0 && t < min_body_t { min_body_t = t; nearest_body = Some((entity, body_gtf, body, t)); }
        }
    }

    for rover_ent in q_rovers.iter() {
        let radius = 10.0;
        let Ok(vessel_gtf) = q_gtfs.get(rover_ent) else { continue; };
        let center = vessel_gtf.translation();
        let oc = ray.origin - center;
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - radius * radius;
        let h = b * b - c;
        if h >= 0.0 {
            let t = -b - h.sqrt();
            if t > 0.0 && t < min_vessel_t { min_vessel_t = t; nearest_vessel = Some(rover_ent); }
        }
    }
    
    for (sc_ent, sc) in q_spacecraft.iter() {
        let radius = sc.hit_radius_m;
        let Ok(vessel_gtf) = q_gtfs.get(sc_ent) else { continue; };
        let center = vessel_gtf.translation();
        let oc = ray.origin - center;
        let b = oc.dot(ray.direction.as_vec3());
        let c = oc.dot(oc) - radius * radius;
        let h = b * b - c;
        if h >= 0.0 {
            let t = -b - h.sqrt();
            if t > 0.0 && t < min_vessel_t { min_vessel_t = t; nearest_vessel = Some(sc_ent); }
        }
    }
    
    if let Some(vessel) = nearest_vessel {
        if min_vessel_t < min_body_t {
             if q_rovers.contains(vessel) {
                // Trigger event: Entity scoping will be handled by the caller
                commands.trigger(lunco_core::architecture::CommandMessage {
                    id: 0, 
                    target: vessel,
                    name: "FOCUS".to_string(),
                    args: smallvec::smallvec![],
                    source: Entity::PLACEHOLDER,
                });
             } else {
                if let Some(mut obs) = q_camera.iter_mut().next() {
                    obs.focus_target = Some(vessel);
                }
             }
             return;
        }
    }

    if let Some((body_ent, body_gtf, _, t)) = nearest_body {
        if let Some(mut obs) = q_camera.iter_mut().next() {
            if obs.mode == ObserverMode::Orbital {
                 obs.focus_target = Some(body_ent);
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
    mut q_camera: Query<(Entity, &mut ObserverCamera, &mut CellCoord, &mut Transform, &Avatar)>,
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

        if obs.is_first_frame {
            obs.smooth_focus_pos = target_pos_in_cam_grid;
            obs.is_first_frame = false;
        } else {
            let lerp_factor = (time.delta_secs() * 5.0).min(1.0) as f64;
            obs.smooth_focus_pos = obs.smooth_focus_pos.lerp(target_pos_in_cam_grid, lerp_factor);
        }
        let focus_pos = obs.smooth_focus_pos;

        let altitude;
        if let Some(body) = t_body {
            let dist_to_center = if obs.mode == ObserverMode::Orbital { obs.distance } else { obs.local_flyby_pos.length() };
            altitude = dist_to_center - body.radius_m;
            
            if altitude < 10_000.0 && obs.mode != ObserverMode::Surface {
                obs.mode = ObserverMode::Surface;
                let rel_offset = (cam_tf.translation.as_dvec3() - target_pos_in_cam_grid).normalize() * (body.radius_m + 10.0);
                obs.local_flyby_pos = t_tf.rotation.inverse().mul_vec3(rel_offset.as_vec3()).as_dvec3();
                obs.yaw = 0.0;
                obs.pitch = 0.0;
            }
            
            if altitude > 15_000.0 && obs.mode == ObserverMode::Surface {
                obs.mode = ObserverMode::Flyby;
                obs.local_flyby_pos = t_tf.rotation.mul_vec3(obs.local_flyby_pos.as_vec3()).as_dvec3();
            }

            if altitude < 1_000_000.0 && obs.mode == ObserverMode::Orbital && body.name != "Earth" {
                obs.mode = ObserverMode::Flyby;
                let rot = Quat::from_euler(EulerRot::YXZ, obs.yaw, obs.pitch, 0.0);
                obs.local_flyby_pos = rot.mul_vec3(Vec3::Z).as_dvec3() * obs.distance;
            }
            
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
            let desired_pos_local = focus_pos + offset;
            let (new_cell, new_tf) = cam_grid.translation_to_grid(desired_pos_local);
            *cam_cell = new_cell; cam_tf.translation = new_tf;
            cam_tf.look_at(focus_pos.as_vec3(), Vec3::Y);
        } else if obs.mode == ObserverMode::Flyby {
            obs.yaw -= mouse_delta.x * 0.01;
            obs.pitch = (obs.pitch - mouse_delta.y * 0.01).clamp(-1.55, 1.55);
            let rotation = Quat::from_euler(EulerRot::YXZ, obs.yaw, obs.pitch, 0.0);
            cam_tf.rotation = rotation;
            
            let mut speed = if obs.altitude < 50_000.0 { 10_000.0 } else { 1000.0 };
            if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) { speed *= 10.0; }
            speed *= (obs.altitude / 1000.0).max(1.0) * time.delta_secs_f64();

            let mut move_vec = DVec3::ZERO;
            let forward = rotation.mul_vec3(Vec3::NEG_Z).as_dvec3();
            let right = rotation.mul_vec3(Vec3::X).as_dvec3();
            let cam_up = rotation.mul_vec3(Vec3::Y).as_dvec3();

            if keys.pressed(KeyCode::KeyW) { move_vec += forward; }
            if keys.pressed(KeyCode::KeyS) { move_vec -= forward; }
            if keys.pressed(KeyCode::KeyD) { move_vec += right; }
            if keys.pressed(KeyCode::KeyA) { move_vec -= right; }
            if keys.pressed(KeyCode::KeyE) { move_vec += cam_up; }
            if keys.pressed(KeyCode::KeyQ) { move_vec -= cam_up; }
            
            obs.local_flyby_pos += move_vec * speed;
            let desired_pos_local = focus_pos + obs.local_flyby_pos;
            let (new_cell, new_tf) = cam_grid.translation_to_grid(desired_pos_local);
            *cam_cell = new_cell; cam_tf.translation = new_tf;
        } else if obs.mode == ObserverMode::Surface {
            let Some(body) = t_body else { continue; };
            let body_rot = t_tf.rotation;
            let current_pos_offset = body_rot.mul_vec3(obs.local_flyby_pos.as_vec3());
            let up = current_pos_offset.normalize_or_zero();
            let surface_base_rot = Quat::from_rotation_arc(Vec3::Y, up);
            
            obs.yaw -= mouse_delta.x * 0.01;
            obs.pitch = (obs.pitch - mouse_delta.y * 0.01).clamp(-1.5, 1.5);
            let final_rot = surface_base_rot * Quat::from_euler(EulerRot::YXZ, obs.yaw, obs.pitch, 0.0);
            cam_tf.rotation = final_rot;

            let mut speed = (obs.altitude * 0.5 + 50.0).max(10.0);
            if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) { speed *= 10.0; }
            speed *= time.delta_secs_f64();
            
            let forward = final_rot.mul_vec3(Vec3::NEG_Z);
            let right_move = final_rot.mul_vec3(Vec3::X);
            
            let mut move_dir_world = Vec3::ZERO;
            if keys.pressed(KeyCode::KeyW) { move_dir_world += forward; }
            if keys.pressed(KeyCode::KeyS) { move_dir_world -= forward; }
            if keys.pressed(KeyCode::KeyD) { move_dir_world += right_move; }
            if keys.pressed(KeyCode::KeyA) { move_dir_world -= right_move; }
            if keys.pressed(KeyCode::KeyE) { move_dir_world += up; }
            if keys.pressed(KeyCode::KeyQ) { move_dir_world -= up; }

            let move_vec_body_space = body_rot.inverse().mul_vec3(move_dir_world).as_dvec3() * speed;
            obs.local_flyby_pos += move_vec_body_space;
            
            let curr_len = obs.local_flyby_pos.length();
            if curr_len < body.radius_m + 2.0 {
                obs.local_flyby_pos = obs.local_flyby_pos.normalize() * (body.radius_m + 2.0);
            }
            
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
