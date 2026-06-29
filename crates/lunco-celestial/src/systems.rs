use bevy::prelude::*;
use bevy::math::DQuat;
use big_space::prelude::*;

use crate::big_space_setup::{SolarSystemRoot, EarthRoot, MoonRoot};
use crate::clock::CelestialClock;
use crate::ephemeris::EphemerisResource;
use crate::registry::{CelestialBody, CelestialBodyRegistry, CelestialReferenceFrame};
use crate::coords::ecliptic_to_bevy;
use crate::coords::world_position_seeded;
use lunco_materials::{ParamValue, ShaderMaterial};

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

    // Gate: re-project the whole body/frame hierarchy only when the epoch
    // has actually advanced. `last_jd` starts at 0.0 (a JD this clock never
    // takes), so the first real epoch always runs; thereafter a paused /
    // time-warp-stopped clock skips the full recompute. (This is the gate
    // the doc comment always promised but never wired up.)
    if (clock.epoch - *last_jd).abs() < 1e-9 {
        return;
    }
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
    _q_bodies: Query<&Transform, (With<CelestialBody>, Without<lunco_terrain_globe::TileCoord>)>,
    _q_tiles: Query<(&mut Transform, &lunco_terrain_globe::TileCoord)>,
) {
    // Intentionally empty — tiles stay at identity rotation in Grid frame.
}

/// Pure direction math for [`update_sun_light_system`]: the direction a
/// `DirectionalLight` should EMIT along (its local `-Z` / forward) so sunlight
/// travels from the Sun toward the scene, given heliocentric Sun and Moon
/// positions (ecliptic J2000, AU). Returns `None` when degenerate (e.g. the
/// `NoOpEphemerisProvider` returns ZERO for everything).
pub(crate) fn sun_emit_direction(p_sun: bevy::math::DVec3, p_moon: bevy::math::DVec3) -> Option<Vec3> {
    // `to_sun` = Moon→Sun in Bevy world space; the light emits the other way.
    let to_sun = crate::coords::ecliptic_to_bevy(p_sun - p_moon).as_vec3().normalize_or_zero();
    if to_sun.length_squared() < 0.5 {
        return None;
    }
    Some(-to_sun)
}

/// Point the scene's primary `DirectionalLight` along the **ephemeris** Sun
/// direction at the current epoch (architecture doc 19 — T2; replaces the old
/// hardcoded `Vec3::NEG_Z`).
///
/// The Sun sits at the heliocentre, so the Moon→Sun direction is just
/// `-ecliptic_to_bevy(global_position(Moon))` (mirrors the solar-panel pointing
/// in [`crate::missions`]). A `DirectionalLight` emits along its local forward
/// (`-Z`) and rays travel FROM the Sun INTO the scene, so the light's forward is
/// set to `-to_sun`. The brightest light is taken as the sun (the Earthshine
/// fill is ~12 lx vs ~128 000 lx), matching the canonical `pick_sun` rule and
/// avoiding both a marker dependency and the `single_mut()`-fails-with-two-lights
/// trap.
///
/// With the default `NoOpEphemerisProvider` every position is ZERO, so `to_sun`
/// degenerates and the system returns early — leaving the light under manual
/// `SetEnvironmentLight` (yaw/pitch) control. The ephemeris is therefore
/// authoritative ONLY when a real provider (`lunco-celestial-ephemeris`) is
/// installed; sandbox / NoOp contexts keep dynamic manual control. That single
/// authoritative writer per context resolves the earlier web-build conflict
/// where two systems fought over the sun direction every frame.
pub fn update_sun_light_system(
    ephemeris: Option<Res<EphemerisResource>>,
    clock: Res<crate::clock::CelestialClock>,
    mut q_light: Query<(&mut Transform, &DirectionalLight)>,
) {
    let Some(ephemeris) = ephemeris else { return; };

    let p_sun = ephemeris.provider.global_position(10, clock.epoch);
    let p_moon = ephemeris.provider.global_position(301, clock.epoch);
    let Some(dir) = sun_emit_direction(p_sun, p_moon) else {
        // NoOp / degenerate ephemeris — leave the light to manual control.
        return;
    };
    let up = if dir.dot(Vec3::Y).abs() > 0.99 { Vec3::X } else { Vec3::Y };

    // The sun is the brightest `DirectionalLight` (Earthshine fill is far dimmer).
    if let Some((mut light_tf, _)) = q_light
        .iter_mut()
        .max_by(|a, b| a.1.illuminance.total_cmp(&b.1.illuminance))
    {
        light_tf.look_to(dir, up);
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
    mut materials: ResMut<Assets<ShaderMaterial>>,
    q_camera: Query<(Entity, &CellCoord, &Transform), (With<Camera>, With<lunco_core::Avatar>)>,
    q_bodies: Query<(Entity, &CellCoord, &Transform, &CelestialBody)>,
    q_tiles: Query<(&MeshMaterial3d<ShaderMaterial>, &lunco_terrain_globe::TileCoord), With<lunco_terrain_globe::TerrainTile>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
) {
    let Some((cam_ent, cam_cell, cam_tf)) = q_camera.iter().next() else { return; };
    let cam_abs = world_position_seeded(cam_ent, cam_cell, cam_tf, &q_parents, &q_grids, &q_spatial);

    // Find nearest body and compute altitude using body-local distance.
    // Using body-local coords (camera relative to body center) prevents
    // thrashing at high time warp — only depends on camera's position
    // relative to the body, not where the body happens to be in orbit.
    let mut nearest_altitude = f64::MAX;
    let mut nearest_body_entity = None;
    for (body_ent, body_cell, body_tf, body) in q_bodies.iter() {
        let body_abs = world_position_seeded(body_ent, body_cell, body_tf, &q_parents, &q_grids, &q_spatial);
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
                mat.set("transition", ParamValue::F32(transition));
            }
        }
    }
}

#[cfg(test)]
mod sun_dir_tests {
    //! Pure ephemeris→sun-direction math ([`sun_emit_direction`], doc 19 — T2).
    use super::*;
    use bevy::math::DVec3;

    #[test]
    fn degenerate_ephemeris_yields_no_direction() {
        // NoOpEphemerisProvider returns ZERO for every body → no sun direction,
        // so the system leaves the light under manual control.
        assert!(sun_emit_direction(DVec3::ZERO, DVec3::ZERO).is_none());
    }

    #[test]
    fn emit_direction_is_unit_and_points_away_from_sun() {
        // Sun at the heliocentre, Moon offset along +X (ecliptic).
        let d = sun_emit_direction(DVec3::ZERO, DVec3::new(1.0, 0.0, 0.0))
            .expect("non-degenerate");
        assert!((d.length() - 1.0).abs() < 1e-5, "emit dir must be unit length");

        // The light emits AWAY from the Sun: with the Moon on the far side, the
        // emit direction flips to the antipode.
        let d_opp = sun_emit_direction(DVec3::ZERO, DVec3::new(-1.0, 0.0, 0.0))
            .expect("non-degenerate");
        assert!((d + d_opp).length() < 1e-5, "antipodal Moon → antipodal light");
    }

    #[test]
    fn emit_direction_tracks_the_moon_position() {
        // Two distinct Moon positions give two distinct light directions — i.e.
        // advancing the epoch (which moves the Moon) re-aims the sun.
        let a = sun_emit_direction(DVec3::ZERO, DVec3::new(1.0, 0.2, 0.0)).unwrap();
        let b = sun_emit_direction(DVec3::ZERO, DVec3::new(1.0, 0.0, 0.3)).unwrap();
        assert!((a - b).length() > 1e-3, "different Moon positions → different sun aim");
    }
}
