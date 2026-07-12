use bevy::prelude::*;
use big_space::prelude::*;

use crate::big_space_setup::{SolarSystemRoot, EarthRoot, MoonRoot};
use crate::ephemeris::EphemerisResource;
use lunco_time::WorldTime;
use crate::registry::{CelestialBody, CelestialBodyRegistry, CelestialReferenceFrame};
use crate::coords::ecliptic_to_bevy;
use crate::coords::world_position_seeded;
use lunco_materials::{ParamValue, ShaderMaterial};

/// Update body and frame positions based on ephemeris data.
/// Optimized: Only re-computes if Epoch has changed significantly.
pub fn ephemeris_update_system(
    world: Res<WorldTime>,
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
    if (world.epoch_jd - *last_jd).abs() < 1e-9 {
        return;
    }
    *last_jd = world.epoch_jd;

    for (entity, mut cell, mut tf, body, frame) in q_entities.iter_mut() {
        let ephemeris_id = if let Some(b) = body { b.ephemeris_id } else if let Some(f) = frame { f.ephemeris_id } else { continue; };

        // NEVER write the Solar Grid (id 10) here. Its parent-relative
        // position is zero by definition (`position(10) == 0`), so in an
        // un-anchored scene this write was a no-op — but in a site-anchored
        // scene it ZEROED the pin that `anchor_solar_frame_to_site` re-applies
        // later in the chain. Within that window the whole solar hierarchy sat
        // at its raw heliocentric pose (~1.5e11 m off), and any UNORDERED
        // reader that interleaved there (gravity field, focus commands, GT
        // propagation for freshly spawned tiles) captured garbage: alternating
        // gravity (surface jitter), Earth tiles frozen 1e11 m away (blinking
        // Earth), camera teleports into empty space (click-to-focus black
        // screen). Skipping the write means no frame — mid-chain or otherwise
        // — ever holds the un-anchored pose.
        if ephemeris_id == 10 {
            continue;
        }

        // EphemerisProvider::position returns position relative to its parent defined in registry/hierarchy
        let rel_pos_au = ephemeris.provider.position(ephemeris_id, world.epoch_jd);
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
    world: Res<WorldTime>,
    registry: Res<CelestialBodyRegistry>,
    mut q_grids: Query<(&mut Transform, &CelestialReferenceFrame)>,
) {
    for (mut tf, frame) in q_grids.iter_mut() {
        if let Some(desc) = registry.bodies.iter().find(|d| d.ephemeris_id == frame.ephemeris_id) {
            if desc.rotation_rate_rad_per_day != 0.0 {
                // Shared with the geodesy math (`geo::body_rotation`) so
                // rendered grids and comms/anchor positions cannot diverge.
                let next = crate::geo::body_rotation(desc, world.epoch_jd).as_quat();
                // Guarded write: an unconditional `tf.rotation = …` dirties the
                // Transform every frame even when the value is unchanged (paused
                // clock), re-running propagation and re-rounding the f32 compose
                // chain. At orbital-pin distances that re-rounding is a sub-pixel
                // per-frame wobble of the focused body — worst at its limb
                // ("Earth jitters" with the clock paused). Only write on change.
                if tf.rotation != next {
                    tf.rotation = next;
                }
            }
        }
    }
}

// NOTE: a `tile_rotation_sync_system` used to live here — an intentionally
// EMPTY body ("tiles stay at identity rotation in the Grid frame") whose
// `.after(TransformSystems::Propagate)` orderings were silently meaningless in
// PreUpdate (those sets have no members there). Deleted 2026-07-11; tiles are
// carried by their (rotating) grid, which is the correct scheme.

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
    world: Res<WorldTime>,
    mut q_light: Query<(&mut Transform, &DirectionalLight)>,
    q_solar: Query<
        &Transform,
        (
            With<crate::big_space_setup::SolarSystemRoot>,
            With<big_space::prelude::Grid>,
            Without<DirectionalLight>,
        ),
    >,
    q_site: Query<(), With<crate::geo::SiteAnchor>>,
) {
    // ONLY steer in site-anchored scenes: there the world frame is the site
    // ENU and the ephemeris direction is physically meaningful. In a plain
    // scene (default sandbox) the world frame is arbitrary — steering would
    // clobber the authored fallback-sun direction with a near-horizontal
    // ecliptic vector and plunge the whole scene into grazing darkness.
    if q_site.is_empty() {
        return;
    }
    let Some(ephemeris) = ephemeris else { return; };

    let p_sun = ephemeris.provider.global_position(10, world.epoch_jd);
    let p_moon = ephemeris.provider.global_position(301, world.epoch_jd);
    let Some(dir) = sun_emit_direction(p_sun, p_moon) else {
        // NoOp / degenerate ephemeris — leave the light to manual control.
        return;
    };
    // `dir` is in ECLIPTIC (solar-frame) axes. The rendered world frame is
    // the Solar Grid's parent frame: identity in luncosim, but the site-ENU
    // frame in a site-anchored scene (`anchor_solar_frame_to_site` rotates
    // the Solar Grid by `align`). Re-express the direction, or a Shackleton
    // scene gets its sun at an arbitrary elevation instead of the real
    // grazing ~1° — terrain lit from nowhere.
    let dir = q_solar
        .iter()
        .next()
        .map(|solar_tf| (solar_tf.rotation * dir).normalize())
        .unwrap_or(dir);
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
    world: Res<WorldTime>,
    q_earth: Query<(&Transform, &big_space::prelude::CellCoord), With<EarthRoot>>,
    q_moon: Query<(&Transform, &big_space::prelude::CellCoord), With<MoonRoot>>,
    q_sun: Query<&Transform, With<SolarSystemRoot>>,
    mut timer: Local<u32>,
) {
    if *timer % 60 == 0 {
        if let Some((tf, cell)) = q_earth.iter().next() { info!("TELEMETRY: Epoch: {:.4}, Earth Cell: {:?}, Earth Pos: {:?}", world.epoch_jd, cell, tf.translation); }
        if let Some((tf, cell)) = q_moon.iter().next() { info!("TELEMETRY: Moon Cell: {:?}, Moon Pos: {:?}", cell, tf.translation); }
        if let Some(tf) = q_sun.iter().next() { info!("TELEMETRY: Sun Pos: {:?}", tf.translation); }
    }
    *timer += 1;
}

/// Force per-frame `GlobalTransform` recomputation for the celestial subtree.
///
/// **Measured to be load-bearing (2026-07-11, `LUNCO_JUMP_PROBE=1`).** An
/// attempt to delete this after the root-grid fix strobed the ENTIRE
/// celestial tree to the world origin ~1 frame in 5–9 (whole-chain
/// plain-f32 compat output surviving to the renderer; 3385 jump events in
/// 15 s). Ordering `PropagateHighPrecision.after(propagate_parent_transforms)`
/// is evidently not sufficient on those frames for entities the HP pass
/// doesn't rewrite — force-dirtying their `Transform` guarantees it rewrites
/// them all, every frame. The real fix is retiring the root `Transform`
/// (Avian bubble, doc 47 Phase 5 / doc 45 two-space split); until then this
/// stays.
///
/// The `Or<>` must cover EVERY cell-entity living on a celestial grid.
/// `GeodeticAnchor`/`KeplerOrbit` prims (ground stations, satellites) were
/// missing — they kept flickering between their anchor pose and the collapsed
/// compat pose while the listed entities were protected (the "DSS ping-pong",
/// task #18). `Without<SiteAnchor>` is REQUIRED with them: the site-anchored
/// SCENE ROOT also carries `GeodeticAnchor`, and force-dirtying it dirties
/// the whole local scene every frame ("everything is jumping around",
/// 2026-07-10 regression).
#[allow(clippy::type_complexity)]
pub fn touch_celestial_transforms(
    q_site: Query<(), With<crate::geo::SiteAnchor>>,
    mut q: Query<
        &mut Transform,
        (
            Or<(
                With<CelestialBody>,
                With<CelestialReferenceFrame>,
                With<lunco_terrain_globe::TerrainTile>,
                With<crate::geo::GeodeticAnchor>,
                With<crate::kepler::KeplerOrbit>,
                With<crate::trajectories::TrajectoryPath>,
            )>,
            Without<crate::geo::SiteAnchor>,
        ),
    >,
) {
    // Only needed while a site anchor re-pins the solar frame; a plain scene
    // has no moving grid chain to go stale against.
    if q_site.is_empty() {
        return;
    }
    for mut tf in q.iter_mut() {
        tf.set_changed();
    }
}

/// While the ORBITAL view is active, force-dirty the site-anchored scene
/// subtree onto the same high-precision propagation path the globe uses.
///
/// The orbital camera (floating origin) lives on the focused body's inertial
/// host grid; the site scene lives in the WorldGrid, on the OTHER side of the
/// Solar Grid's ~1 AU `CellCoord`. big_space rebases only against the
/// origin's cell in each entity's IMMEDIATE grid, so that ancestor offset
/// does not cancel for the terrain — and on frames the HP pass skips it, its
/// GT falls to the plain-f32 compat value, which quantizes the ~1.06e11 m
/// offset in ~16 km ULP buckets. As the site pin advances per frame the
/// terrain slid smoothly within one bucket, then SNAPPED at each ULP wrap —
/// "the ground moves along the moon and jumps back" (and the shadows wobble
/// with it). Force-dirtying makes `PropagateHighPrecision` compose the
/// subtree in i64 every frame, exactly like the globe tiles.
///
/// GROUND view keeps the `Without<SiteAnchor>` exclusion above: with the
/// camera a direct WorldGrid child the site subtree needs no re-composition,
/// and force-dirtying it there caused the 2026-07-10 "everything jumps"
/// regression. Physics is safe either way — the avian bridge is shadow-gated
/// on VALUES, and `set_changed` never alters the value.
pub fn touch_site_scene_transforms(
    orbital_pin: Option<Res<crate::placement::OrbitalViewPin>>,
    q_site_roots: Query<Entity, With<crate::geo::SiteAnchor>>,
    q_children: Query<&Children>,
    mut q_tf: Query<&mut Transform>,
) {
    let Some(pin) = orbital_pin else { return };
    if !pin.active {
        return;
    }
    let mut stack: Vec<Entity> = q_site_roots.iter().collect();
    while let Some(e) = stack.pop() {
        if let Ok(mut tf) = q_tf.get_mut(e) {
            tf.set_changed();
        }
        if let Ok(children) = q_children.get(e) {
            stack.extend(children.iter());
        }
    }
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

    // Per-body camera altitude → per-body texture↔blueprint transition.
    // Body-local coords (camera relative to body center) prevent thrashing
    // at high time warp — only depends on camera's position relative to the
    // body, not where the body happens to be in orbit.
    //
    // EVERY body gets its transition — not just the nearest. The old
    // nearest-only version left distant bodies' tiles on the material default
    // forever: Earth seen from a lunar site rendered as the blueprint
    // wireframe (invisible thin lines against black sky) — the long-standing
    // "no Earth in the sky" bug. With per-body altitudes a distant body
    // computes transition 0.0 = fully textured globe.
    //
    // High (0.0 transition) at 100 km, Blueprint (1.0 transition) at 10 km.
    let start_transition_alt = 100_000.0;
    let end_transition_alt = 10_000.0;
    let mut per_body: std::collections::HashMap<Entity, f32> = std::collections::HashMap::new();
    for (body_ent, body_cell, body_tf, body) in q_bodies.iter() {
        let body_abs = world_position_seeded(body_ent, body_cell, body_tf, &q_parents, &q_grids, &q_spatial);
        let altitude = ((cam_abs - body_abs).length() - body.radius_m).max(0.0);
        let transition = ((start_transition_alt - altitude)
            / (start_transition_alt - end_transition_alt))
            .clamp(0.0, 1.0) as f32;
        per_body.insert(body_ent, transition);
    }

    for (mat_handle, coord) in q_tiles.iter() {
        if let Some(&transition) = per_body.get(&coord.body) {
            if let Some(mut mat) = materials.get_mut(mat_handle) {
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
