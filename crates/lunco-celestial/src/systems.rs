use bevy::prelude::*;
use bevy::math::DQuat;
use big_space::prelude::*;

use crate::big_space_setup::{SolarSystemRoot, EarthRoot, MoonRoot};
use crate::clock::CelestialClock;
use crate::ephemeris::EphemerisResource;
use crate::registry::{CelestialBody, CelestialBodyRegistry, CelestialReferenceFrame};
use crate::coords::ecliptic_to_bevy;
use crate::coords::get_absolute_pos_in_root_double_ghost_aware;
use crate::blueprint::BlueprintMaterial;

/// Update body and frame positions based on ephemeris data.
/// Optimized: Only re-computes if Epoch has changed significantly.
pub fn ephemeris_update_system(
    clock: Res<CelestialClock>,
    ephemeris: Option<Res<EphemerisResource>>,
    mut q_entities: Query<(Entity, &mut CellCoord, &mut Transform, Option<&CelestialBody>, Option<&CelestialReferenceFrame>)>,
    _q_all_parents: Query<&ChildOf>,
    _q_frames: Query<&CelestialReferenceFrame>,
    q_grids: Query<&Grid>,
    mut last_jd: Local<f64>,
) {
    let Some(ephemeris) = ephemeris else { return; };
    *last_jd = clock.epoch;
    
    for (entity, mut cell, mut tf, body, frame) in q_entities.iter_mut() {
        let ephemeris_id = if let Some(b) = body { b.ephemeris_id } else if let Some(f) = frame { f.ephemeris_id } else { continue; };
        
        // EphemerisProvider::position returns position relative to its parent defined in registry/hierarchy
        let rel_pos_au = ephemeris.provider.position(ephemeris_id, clock.epoch);
        let pos_bevy_m = ecliptic_to_bevy(rel_pos_au);

        // Find the grid this entity is in. Since body/frame entities are typically children of their reference frame grid:
        // We need to resolve which grid we are relative to. 
        // In our setup, Earth Body is child of Earth Local Grid.
        // If the provider returns pos relative to parent, then for Earth (399) it is relative to EMB (3).
        // But Earth Body is in Earth Local Grid, which is child of EMB Grid.
        // This means Earth Body should have translation = ZERO in its own local grid.
        
        // WAIT: The design is:
        // Solar Grid (10) -> Sun Body
        // Solar Grid (10) -> EMB Grid (3)
        // EMB Grid (3) -> Earth Grid (399)
        // EMB Grid (3) -> Moon Grid (301)
        // Earth Grid (399) -> Earth Body
        // Moon Grid (301) -> Moon Body
        
        // So:
        // EMB Grid position relative to Solar Grid = EMB helio (rel 10)
        // Earth Grid position relative to EMB Grid = Earth rel EMB (rel 3)
        // Moon Grid position relative to EMB Grid = Moon rel EMB (rel 3)
        // Earth Body position relative to Earth Grid = ZERO
        
        let mut depth = 0;
        let mut current = entity;
        // Search up for the first Grid parent
        while let Ok(child_of) = _q_all_parents.get(current) {
            if depth > 10 { break; } depth += 1;
            let parent = child_of.parent();
            if let Ok(grid) = q_grids.get(parent) {
                // If this is a Body entity, its position relative to its own Local Grid should be zero?
                // No, typically the Grid Anchor moves relative to ITS parent.
                // If 'entity' is a ReferenceFrame (the Grid Anchor itself):
                if frame.is_some() {
                    let (new_cell, new_translation) = grid.translation_to_grid(pos_bevy_m);
                    *cell = new_cell;
                    tf.translation = new_translation;
                } else if body.is_some() {
                    // Body is usually at center of its local grid
                    *cell = CellCoord::default();
                    tf.translation = Vec3::ZERO;
                }
                break;
            }
            current = parent;
        }
    }
}

/// Rotate each celestial body around its polar axis.
pub fn body_rotation_system(
    clock: Res<CelestialClock>,
    registry: Res<CelestialBodyRegistry>,
    mut q_bodies: Query<(&mut Transform, &CelestialBody)>,
    mut last_jd: Local<f64>,
) {
    *last_jd = clock.epoch;
    
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
    mut q_light: Query<&mut Transform, With<DirectionalLight>>,
    mut first_run: Local<bool>,
) {
    if !*first_run {
        *first_run = true;
        warn!("SUN_LIGHT: system running");
    }

    // Point sun light along +Z axis (toward Earth at current epoch).
    // This is a fixed direction that illuminates the Earth-Moon system.
    // The exact direction varies with Earth's orbit, but +Z is a reasonable
    // approximation that keeps the Moon illuminated from most viewing angles.
    let dir = bevy::math::Vec3::NEG_Z;
    if let Ok(mut light_tf) = q_light.single_mut() {
        light_tf.look_to(dir, bevy::math::Vec3::Y);
    }
}

pub fn celestial_telemetry_system(
    clock: Res<crate::clock::CelestialClock>,
    q_earth: Query<(&Transform, &big_space::prelude::CellCoord), With<EarthRoot>>,
    q_moon: Query<(&Transform, &big_space::prelude::CellCoord), With<MoonRoot>>,
    q_sun: Query<&Transform, With<SolarSystemRoot>>,
    q_cam: Query<&Transform, (With<Camera>, With<lunco_core::Avatar>)>,
    mut timer: Local<u32>,
) {
    if *timer % 60 == 0 {
        if let Some((tf, cell)) = q_earth.iter().next() { info!("TELEMETRY: Epoch: {:.4}, Earth Cell: {:?}, Earth Pos: {:?}", clock.epoch, cell, tf.translation); }
        if let Some((tf, cell)) = q_moon.iter().next() { info!("TELEMETRY: Moon Cell: {:?}, Moon Pos: {:?}", cell, tf.translation); }
        if let Some(tf) = q_sun.iter().next() { info!("TELEMETRY: Sun Pos: {:?}", tf.translation); }
        if let Some(tf) = q_cam.iter().next() { info!("TELEMETRY: Camera Local Pos: {:?}", tf.translation); }
    }
    *timer += 1;
}

pub fn celestial_visuals_system(
    mut materials: ResMut<Assets<BlueprintMaterial>>,
    q_camera: Query<(Entity, &CellCoord, &Transform), (With<Camera>, With<lunco_core::Avatar>)>,
    q_bodies: Query<(Entity, &CellCoord, &Transform, &MeshMaterial3d<BlueprintMaterial>, &CelestialBody)>,
    q_tiles: Query<(&MeshMaterial3d<BlueprintMaterial>, &crate::terrain::TileCoord), With<crate::terrain::ActiveTerrainTile>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(&CellCoord, &Transform)>,
) {
    let Some((cam_ent, cam_cell, cam_tf)) = q_camera.iter().next() else { return; };
    
    let mut body_transitions = std::collections::HashMap::new();

    for (body_ent, body_cell, body_tf, mat_handle, body) in q_bodies.iter() {
        // Resolve absolute positions to calculate distance
        let cam_pos = get_absolute_pos_in_root_double_ghost_aware(cam_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial);
        let body_pos = get_absolute_pos_in_root_double_ghost_aware(body_ent, body_cell, body_tf, &q_parents, &q_grids, &q_spatial);
        
        let distance = (cam_pos - body_pos).length();
        let altitude = (distance - body.radius_m).max(0.0);

        if let Some(mat) = materials.get_mut(mat_handle) {
            // High (0.0 transition) at 100km, Blueprint (1.0 transition) at 10km
            let start_transition_alt = 100_000.0;
            let end_transition_alt = 10_000.0;
            
            let transition = (((start_transition_alt - altitude) / (start_transition_alt - end_transition_alt)) as f64)
                .clamp(0.0, 1.0);
                
            mat.extension.transition = transition as f32;
            mat.extension.body_radius = body.radius_m as f32;
            body_transitions.insert(body_ent, transition as f32);
        }
    }

    for (mat_handle, coord) in q_tiles.iter() {
        if let Some(transition) = body_transitions.get(&coord.body) {
            if let Some(mat) = materials.get_mut(mat_handle) {
                mat.extension.transition = *transition;
            }
        }
    }
}
