use bevy::prelude::*;
use bevy_hierarchy::Parent;
use bevy::math::DQuat;
use big_space::prelude::*;

use crate::clock::CelestialClock;
use crate::ephemeris::EphemerisResource;
use crate::registry::{CelestialBody, CelestialBodyRegistry, CelestialReferenceFrame};
use crate::coords::ecliptic_to_bevy;

/// Update body and frame positions based on ephemeris data.
/// Each entity is relative to its parent grid origin.
pub fn ephemeris_update_system(
    clock: Res<CelestialClock>,
    registry: Res<CelestialBodyRegistry>,
    ephemeris: Option<Res<EphemerisResource>>,
    mut q_entities: Query<(Entity, &mut CellCoord, &mut Transform, Option<&CelestialBody>, Option<&CelestialReferenceFrame>)>,
    q_all_parents: Query<&ChildOf>,
    q_frames: Query<&CelestialReferenceFrame>,
    q_grids: Query<&Grid>,
) {
    let Some(ephemeris) = ephemeris else { return; };
    
    for (entity, mut cell, mut tf, body, frame) in q_entities.iter_mut() {
        let ephemeris_id = if let Some(b) = body { b.ephemeris_id } else if let Some(f) = frame { f.ephemeris_id } else { continue; };
        
        let parent_id = if let Ok(child_of) = q_all_parents.get(entity) {
            if let Ok(parent_frame) = q_frames.get(child_of.parent()) {
                Some(parent_frame.ephemeris_id)
            } else { None }
        } else { None };

        if let Ok(child_of) = q_all_parents.get(entity) {
            if let Ok(grid) = q_grids.get(child_of.parent()) {
                let mut pos_au = ephemeris.provider.position(ephemeris_id, clock.epoch);
                
                // Subtract parent position if it exists to get relative position
                if let Some(pid) = parent_id {
                    let parent_pos_au = ephemeris.provider.position(pid, clock.epoch);
                    pos_au -= parent_pos_au;
                }

                let pos_bevy_m = ecliptic_to_bevy(pos_au);
                let (new_cell, new_translation) = grid.translation_to_grid(pos_bevy_m);

                *cell = new_cell;
                tf.translation = new_translation;
            }
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
    q_sun: Query<&GlobalTransform, (With<CelestialBody>, With<crate::big_space_setup::SolarSystemRoot>)>,
    q_cam: Query<&GlobalTransform, With<Camera>>,
    mut q_light: Query<&mut Transform, With<DirectionalLight>>,
) {
    let Some(sun_gtf) = q_sun.iter().next() else { return; }; 
    let Some(cam_gtf) = q_cam.iter().next() else { return; };
    let Some(mut light_tf) = q_light.iter_mut().next() else { return; };
    
    let dir = (cam_gtf.translation() - sun_gtf.translation()).try_normalize().unwrap_or(Vec3::NEG_Z);
    light_tf.look_at(cam_gtf.translation() + dir, Vec3::Y);
}

pub fn celestial_telemetry_system(
    clock: Res<crate::clock::CelestialClock>,
    q_earth: Query<(&Transform, &big_space::prelude::CellCoord), With<crate::big_space_setup::EarthRoot>>,
    q_moon: Query<(&Transform, &big_space::prelude::CellCoord), With<crate::big_space_setup::MoonRoot>>,
    q_sun: Query<&Transform, With<crate::big_space_setup::SolarSystemRoot>>,
    q_cam: Query<(&crate::camera::ObserverCamera, &Transform)>,
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


