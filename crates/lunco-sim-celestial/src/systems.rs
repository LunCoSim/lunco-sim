use bevy::prelude::*;
use bevy::math::DQuat;
use big_space::prelude::*;

use crate::big_space_setup::{SolarSystemRoot, EarthRoot, MoonRoot};
use crate::camera::ObserverCamera;
use crate::clock::CelestialClock;
use crate::ephemeris::EphemerisResource;
use crate::registry::{CelestialBody, CelestialBodyRegistry, CelestialReferenceFrame};
use crate::coords::ecliptic_to_bevy;

/// Update body and frame positions based on ephemeris data.
/// Each entity is relative to its parent grid origin.
pub fn ephemeris_update_system(
    clock: Res<CelestialClock>,
    _registry: Res<CelestialBodyRegistry>,
    ephemeris: Option<Res<EphemerisResource>>,
    mut q_entities: Query<(Entity, &mut CellCoord, &mut Transform, Option<&CelestialBody>, Option<&CelestialReferenceFrame>)>,
    q_all_parents: Query<&ChildOf>,
    q_frames: Query<&CelestialReferenceFrame>,
    q_grids: Query<&Grid>,
) {
    let Some(ephemeris) = ephemeris else { return; };
    
    for (entity, mut cell, mut tf, body, frame) in q_entities.iter_mut() {
        let ephemeris_id = if let Some(b) = body { b.ephemeris_id } else if let Some(f) = frame { f.ephemeris_id } else { continue; };
        
        let current_pos_au = ephemeris.provider.position(ephemeris_id, clock.epoch);

        // Walk up to find the nearest Grid parent
        let mut parent = if let Ok(child_of) = q_all_parents.get(entity) { Some(child_of.parent()) } else { None };
        
        let mut depth = 0;
        while let Some(p) = parent {
            if depth > 10 { break; } // Safety break
            depth += 1;

            if let Ok(grid) = q_grids.get(p) {
                // If we found a grid, we update relative to it.
                let mut ref_ephemeris_id = None;
                if let Ok(p_frame) = q_frames.get(p) {
                   ref_ephemeris_id = Some(p_frame.ephemeris_id);
                }

                if let Some(ref_id) = ref_ephemeris_id {
                    let parent_pos_au = ephemeris.provider.position(ref_id, clock.epoch);
                    let relative_pos_au = current_pos_au - parent_pos_au;
                    let pos_bevy_m = ecliptic_to_bevy(relative_pos_au);
                    let (new_cell, new_translation) = grid.translation_to_grid(pos_bevy_m);

                    *cell = new_cell;
                    tf.translation = new_translation;
                }
                break;
            }
            parent = q_all_parents.get(p).ok().map(|c| c.parent());
        }
    }
}



/// Rotate each celestial body around its polar axis.
pub fn body_rotation_system(
    clock: Res<CelestialClock>,
    registry: Res<CelestialBodyRegistry>,
    mut q_bodies: Query<(&mut Transform, &CelestialBody)>,
) {
    let days_since_j2000 = clock.epoch - 2_451_545.0;
    
    for (mut tf, b) in q_bodies.iter_mut() {
        if let Some(desc) = registry.bodies.iter().find(|d| d.ephemeris_id == b.ephemeris_id) {
            if desc.rotation_rate_rad_per_day != 0.0 {
                let angle = days_since_j2000 * desc.rotation_rate_rad_per_day;
                let rot = DQuat::from_axis_angle(desc.polar_axis, angle);
                tf.rotation = rot.as_quat();
            }
        }
    }
}

pub fn update_sun_light_system(
    mut q_light: Query<(&mut Transform, &DirectionalLight)>,
    _q_sun: Query<&CelestialBody, With<SolarSystemRoot>>,
    q_camera: Query<(Entity, &GlobalTransform), With<ObserverCamera>>,
    q_all_parents: Query<&ChildOf>,
    q_grids_only: Query<&big_space::grid::Grid>,
    q_coords_only: Query<(&CellCoord, &Transform), Without<DirectionalLight>>,
) {
    let Some((mut light_tf, _)) = q_light.iter_mut().next() else { return; };
    let Some((cam_entity, _cam_gtf)) = q_camera.iter().next() else { return; };
    
    let mut current = cam_entity;
    let mut total_pos = bevy::math::DVec3::ZERO;
    
    let mut depth = 0;
    while let Ok(child_of) = q_all_parents.get(current) {
        if depth > 10 { break; } // Safety break
        depth += 1;

        let parent = child_of.parent();
        if parent == current { break; } // Self-parenting loop break

        // The children are in the coordinate space defined by the parent's grid
        if let Ok(grid) = q_grids_only.get(parent) {
            if let Ok((cell, tf)) = q_coords_only.get(current) {
                total_pos += grid.grid_position_double(cell, tf);
            }
        }
        current = parent;
    }
    
    let dir_to_cam = total_pos.normalize_or_zero().as_vec3();
    if dir_to_cam != Vec3::ZERO {
        light_tf.look_at(dir_to_cam, Vec3::Y);
    }
}

pub fn celestial_telemetry_system(
    clock: Res<crate::clock::CelestialClock>,
    q_earth: Query<(&Transform, &big_space::prelude::CellCoord), With<EarthRoot>>,
    q_moon: Query<(&Transform, &big_space::prelude::CellCoord), With<MoonRoot>>,
    q_sun: Query<&Transform, With<SolarSystemRoot>>,
    q_cam: Query<(&ObserverCamera, &Transform)>,
    mut timer: Local<u32>,
) {
    if *timer % 60 == 0 {
        if let Some((tf, cell)) = q_earth.iter().next() {
            info!("TELEMETRY: Epoch: {:.4}, Earth Cell: {:?}, Earth Pos: {:?}", clock.epoch, cell, tf.translation);
        }
        if let Some((tf, cell)) = q_moon.iter().next() {
            info!("TELEMETRY: Moon Cell: {:?}, Moon Pos: {:?}", cell, tf.translation);
        }
        if let Some(tf) = q_sun.iter().next() {
            info!("TELEMETRY: Sun Pos: {:?}", tf.translation);
        }
        if let Some((obs, tf)) = q_cam.iter().next() {
            info!("TELEMETRY: Camera Focus: {:?}, Camera Local Pos: {:?}", obs.focus_target, tf.translation);
        }
    }
    *timer += 1;
}


