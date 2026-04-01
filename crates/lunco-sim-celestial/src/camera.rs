use bevy::prelude::*;
use bevy::input::mouse::MouseWheel;
// use bevy_hierarchy::Parent;
use bevy::math::{DVec3, DQuat};
use big_space::prelude::*;
use crate::registry::CelestialBody;

#[derive(Component)]
pub struct ObserverCamera {
    pub focus_target: Option<Entity>,
    pub distance: f64,
    pub yaw: f64,
    pub pitch: f64,
}

#[derive(Component)]
pub struct ActiveCamera;

pub fn update_observer_camera_system(
    mut q_camera: Query<(bevy::prelude::Entity, &mut ObserverCamera, &mut big_space::prelude::CellCoord, &mut bevy::prelude::Transform, &ActiveCamera), bevy::prelude::Without<CelestialBody>>,
    q_targets: Query<(&big_space::prelude::CellCoord, &bevy::prelude::Transform, &bevy::prelude::GlobalTransform, &CelestialBody), bevy::prelude::Without<ObserverCamera>>,
    q_all_parents: Query<&bevy::prelude::ChildOf>,
    q_grids: Query<&big_space::grid::Grid>,
    mut scroll_evr: MessageReader<MouseWheel>,
    mut motion_evr: MessageReader<bevy::input::mouse::MouseMotion>,
    mouse_input: Res<ButtonInput<MouseButton>>,
) {
    for (entity, mut obs, mut cam_cell, mut cam_tf, _) in q_camera.iter_mut() {
        let Some(target_entity) = obs.focus_target else { continue; };
        let Ok((target_cell, target_tf, _target_gtf, _body)) = q_targets.get(target_entity) else { continue; };
        
        // 1. Process zoom input
        for ev in scroll_evr.read() {
            obs.distance = (obs.distance - (ev.y as f64) * (obs.distance * 0.2)).clamp(1_000.0, 1.0e13);
        }

        // 2. Process rotation input (Right Mouse)
        if mouse_input.pressed(MouseButton::Right) {
            for ev in motion_evr.read() {
                obs.yaw -= (ev.delta.x as f64) * 0.005;
                obs.pitch -= (ev.delta.y as f64) * 0.005;
                obs.pitch = obs.pitch.clamp(-1.5, 1.5);
            }
        } else {
            let _ = motion_evr.read().count(); // Clear anyway
        }

        // 3. Keep camera at 'distance' from target in target's parent grid
        if let (Ok(cam_child_of), Ok(target_child_of)) = (q_all_parents.get(entity), q_all_parents.get(target_entity)) {
            let target_parent = target_child_of.parent();
            let Ok(grid) = q_grids.get(target_parent) else { continue; };
            
            let target_pos = grid.grid_position_double(target_cell, target_tf);
            
            let rot = DQuat::from_euler(EulerRot::YXZ, obs.yaw, obs.pitch, 0.0);
            let offset = rot * (DVec3::Z * obs.distance);
            let desired_pos = target_pos + offset;

            let (new_cell, new_tf) = grid.translation_to_grid(desired_pos);
            
            if cam_child_of.parent() == target_parent {
                *cam_cell = new_cell;
                cam_tf.translation = new_tf;
                
                // Keep the target at center
                let look_tgt = target_tf.translation;
                cam_tf.look_at(look_tgt, Vec3::Y);
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
            let mut min_altitude = 1.0e15; // Start very large
            
            for (body_gtf, body) in q_bodies.iter() {
                let dist = cam_gtf.translation().distance(body_gtf.translation()) as f64;
                let altitude = (dist - body.radius_m).max(1.0); 
                if altitude < min_altitude {
                    min_altitude = altitude;
                }
            }
            
            perspective.near = (min_altitude * 0.001).max(0.1).min(1000.0) as f32;
            perspective.far = 2.0e12; // 2 trillion meters (20 AU)
        }
    }
}
