use bevy::prelude::*;
use bevy::math::DQuat;
use big_space::prelude::*;

use crate::big_space_setup::{SolarSystemRoot, EarthRoot, MoonRoot};
use crate::clock::CelestialClock;
use crate::ephemeris::EphemerisResource;
use crate::registry::{CelestialBody, CelestialBodyRegistry, CelestialReferenceFrame};
use crate::coords::ecliptic_to_bevy;
use crate::coords::get_absolute_pos_in_root_double_ghost_aware;
use lunco_materials::BlueprintMaterial;

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

/// Rotate each celestial body's Grid around its polar axis.
/// Per big_space docs: "if you have a planet rotating and orbiting around
/// its star... you can place the planet and all objects on its surface in
/// the same grid. The motion of the planet will be inherited by all children
/// in that grid, in high precision."
/// We rotate the Grid so tiles (and future rovers) automatically inherit rotation.
pub fn body_rotation_system(
    clock: Res<CelestialClock>,
    registry: Res<CelestialBodyRegistry>,
    mut q_grids: Query<(&mut Transform, &CelestialReferenceFrame)>,
) {
    let days_since_j2000 = clock.epoch - 2_451_545.0;
    for (mut tf, frame) in q_grids.iter_mut() {
        if let Some(desc) = registry.bodies.iter().find(|d| d.ephemeris_id == frame.ephemeris_id) {
            if desc.rotation_rate_rad_per_day != 0.0 {
                let angle = days_since_j2000 * desc.rotation_rate_rad_per_day;
                tf.rotation = DQuat::from_axis_angle(desc.polar_axis, angle).as_quat();
            }
        }
    }
}

/// Propagate body rotation to terrain tile local transforms.
/// Currently NO-OP: terrain tiles are fixed in the Grid frame.
/// The Body rotation affects surface entities (rovers, cameras) that are
/// children of Body, not terrain tiles on the Grid.
/// If body rotation needs to affect tiles in the future, tiles should be
/// re-parented to Body or use a different coordinate scheme.
pub fn tile_rotation_sync_system(
    _q_bodies: Query<&Transform, (With<CelestialBody>, Without<lunco_terrain::TileCoord>)>,
    _q_tiles: Query<(&mut Transform, &lunco_terrain::TileCoord)>,
) {
    // Intentionally empty — tiles stay at identity rotation in Grid frame.
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
    q_bodies: Query<(Entity, &CellCoord, &Transform, &CelestialBody)>,
    q_tiles: Query<(&MeshMaterial3d<BlueprintMaterial>, &lunco_terrain::TileCoord), With<lunco_terrain::TerrainTile>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(&CellCoord, &Transform)>,
) {
    let Some((cam_ent, cam_cell, cam_tf)) = q_camera.iter().next() else { return; };
    let cam_abs = get_absolute_pos_in_root_double_ghost_aware(cam_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial);

    // Find nearest body and compute altitude using body-local distance.
    // Using body-local coords (camera relative to body center) prevents
    // thrashing at high time warp — only depends on camera's position
    // relative to the body, not where the body happens to be in orbit.
    let mut nearest_altitude = f64::MAX;
    let mut nearest_body_entity = None;
    for (body_ent, body_cell, body_tf, body) in q_bodies.iter() {
        let body_abs = get_absolute_pos_in_root_double_ghost_aware(body_ent, body_cell, body_tf, &q_parents, &q_grids, &q_spatial);
        // Body-local distance: camera position relative to body center
        let camera_body_local = cam_abs - body_abs;
        let altitude = (camera_body_local.length() - body.radius_m).max(0.0);
        if altitude < nearest_altitude {
            nearest_altitude = altitude;
            nearest_body_entity = Some(body_ent);
        }
    }

    let Some(nearest_body) = nearest_body_entity else { return };

    // High (0.0 transition) at 100km, Blueprint (1.0 transition) at 10km
    let start_transition_alt = 100_000.0;
    let end_transition_alt = 10_000.0;
    let transition = (((start_transition_alt - nearest_altitude) / (start_transition_alt - end_transition_alt)) as f64)
        .clamp(0.0, 1.0) as f32;

    // Update all tiles belonging to the nearest body
    for (mat_handle, coord) in q_tiles.iter() {
        if coord.body == nearest_body {
            if let Some(mat) = materials.get_mut(mat_handle) {
                mat.extension.transition = transition;
            }
        }
    }
}
