//! Site-anchored solar hierarchy + celestial-bound entity placement (doc 43
//! §2.6).
//!
//! **Site anchoring**: rather than moving a loaded scene to the Moon, the
//! solar hierarchy is pinned so the site's geodetic point coincides with the
//! scene origin and ENU aligns with the scene axes (East=+X, North=−Z,
//! Up=+Y). Scene content and physics never move; the globe appears under the
//! terrain patch and Earth/Sun stand in the correct sky. Runs after
//! `ephemeris_update_system` (which re-zeroes the solar grid on epoch change)
//! and overrides it whenever a [`SiteAnchor`] is authored.
//!
//! **Bound entities**: prims with a [`GeodeticAnchor`] (ground stations) or a
//! [`KeplerOrbit`] (satellites) are re-parented onto their body's rotating
//! grid and positioned each epoch tick — body-fixed coordinates for anchors
//! (the grid's spin carries them), inverse-rotated inertial coordinates for
//! orbits. Without a matching grid (no solar hierarchy) they are hidden;
//! comms math is unaffected either way.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};

use lunco_time::WorldTime;

use crate::big_space_setup::SolarSystemRoot;
use crate::coords::ecliptic_to_bevy;
use crate::ephemeris::EphemerisResource;
use crate::geo::{
    body_rotation, equatorial_frame, geodetic_to_body_fixed, solar_tangent_frame, GeodeticAnchor,
    SiteAnchor,
};
use crate::kepler::KeplerOrbit;
use crate::registry::{CelestialBodyRegistry, CelestialReferenceFrame};

/// Orbital view MODE state. Since the traveling-origin change (doc 47 Phase
/// 6) this resource no longer re-poses the world: the CAMERA — it carries the
/// `FloatingOrigin` — migrates onto the focused body's inertial parent grid
/// and orbits there (`lunco-avatar::orbit_system`), so a drag moves only the
/// floating origin and big_space recomputes every `GlobalTransform` against
/// it atomically. (The previous design slid the entire solar tree around a
/// parked camera each drag frame; at Earth range that moved the tree by
/// ~1e6 m per frame, and ANY writer that lagged one frame — mesh rebuild,
/// body spin, LOD tiles, markers — displaced its entity by megameters:
/// "planets jump around when I rotate".)
///
/// Remaining consumers of the mode flag:
/// * [`orbital_pin_scene_visibility`] — hides the local scene while orbital;
/// * `compute_local_gravity` — holds the last surface field;
/// * exit paths — `anchor_world`/`anchor_rotation` restore the parked
///   surface camera pose.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct OrbitalViewPin {
    pub active: bool,
    /// Ephemeris id of the focused body.
    pub body: i32,
    /// Unit direction from the body centre toward the viewpoint, in the
    /// focused body's INERTIAL host-grid axes (+Y = engine north — the frame
    /// the orbital camera renders in; only `orbit_system` consumes this).
    pub dir: DVec3,
    /// Viewpoint distance from the body centre, metres.
    pub distance: f64,
    /// The parked camera's root-frame position at mode entry (constant).
    pub anchor_world: DVec3,
    /// The parked camera's rotation at mode entry (constant).
    pub anchor_rotation: Quat,
}

/// Pin the solar hierarchy so the authored site anchor coincides with the
/// local scene origin, ENU-aligned. World = R·(solar − p_site) with R mapping
/// East→+X, Up→+Y, North→−Z. Runs only on epoch/site changes — the orbital
/// view never re-poses the world (see [`OrbitalViewPin`]).
#[allow(clippy::type_complexity)]
pub fn anchor_solar_frame_to_site(
    world_time: Res<WorldTime>,
    ephemeris: Option<Res<EphemerisResource>>,
    registry: Res<CelestialBodyRegistry>,
    q_site: Query<&GeodeticAnchor, With<SiteAnchor>>,
    q_site_changed: Query<(), Or<(Added<SiteAnchor>, Changed<GeodeticAnchor>)>>,
    // `SolarSystemRoot` also tags the Sun body — the grid filter picks the
    // one Solar Grid entity.
    mut q_solar: Query<(Entity, &mut CellCoord, &mut Transform), (With<SolarSystemRoot>, With<Grid>)>,
    mut q_align: Query<
        &mut Transform,
        (
            With<crate::big_space_setup::SiteAlignGrid>,
            Without<SolarSystemRoot>,
            Without<CelestialReferenceFrame>,
        ),
    >,
    q_frames_stored: Query<
        (Entity, &CelestialReferenceFrame, &CellCoord, &Transform, &ChildOf),
        (With<Grid>, Without<SolarSystemRoot>),
    >,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    mut last_jd: Local<f64>,
) {
    let Some(ephemeris) = ephemeris else { return };
    let anchor_opt = q_site.iter().next();
    // Without a site anchor the solar tree stays heliocentric.
    if anchor_opt.is_none() {
        return;
    }
    let Ok((solar_entity, mut cell, mut tf)) = q_solar.single_mut() else { return };

    let jd = world_time.epoch_jd;
    // Same cadence as the ephemeris projection + site edits; ordering after
    // `ephemeris_update_system` guarantees we override its origin re-zero on
    // the frames it re-projects.
    let epoch_changed = (jd - *last_jd).abs() >= 1e-9;
    if !epoch_changed && q_site_changed.is_empty() && *last_jd != 0.0 {
        return;
    }
    *last_jd = jd;

    // Site tangent alignment (world axes) — identity when there is no anchor.
    let (align, site_frame_origin, site_geo_offset) = if let Some(anchor) = anchor_opt {
        let Some(desc) = registry
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == anchor.body)
        else {
            return;
        };
        // No ephemeris ⇒ we do not know where the body IS, so we cannot anchor a site to it.
        // Leaving the anchor un-placed is honest; placing it at the Sun's centre is not.
        let Some(p) = ephemeris.provider.global_position(anchor.body, jd) else {
            return;
        };
        let body_center = ecliptic_to_bevy(p).raw();
        let frame = solar_tangent_frame(desc, &anchor.geodetic, body_center, jd);
        // Rows East/Up/−North → world axes.
        let align = DQuat::from_mat3(&bevy::math::DMat3::from_cols(
            DVec3::new(frame.east.x, frame.up.x, -frame.north.x),
            DVec3::new(frame.east.y, frame.up.y, -frame.north.y),
            DVec3::new(frame.east.z, frame.up.z, -frame.north.z),
        ));
        (
            align,
            frame.origin,
            Some((anchor.body, geodetic_to_body_fixed(&anchor.geodetic, desc.radius_m))),
        )
    } else {
        (DQuat::IDENTITY, DVec3::ZERO, None)
    };

    // The pin must cancel what the RENDERER will actually compose — the
    // STORED (`CellCoord` + f32 `Transform`) grid chain — not the ideal f64
    // ephemeris. Each stored pose still carries an f32 remainder whose ULP
    // grows with the grid's cell edge, and as the bodies move those remainders
    // step in ULP increments. A pin computed from smooth f64 positions does
    // NOT track the steps, so the whole moon subtree (the visible surface)
    // stepped against the scene — "lunar surface falling and jumping".
    // Composing the site from the stored chain (every f32 read back into f64
    // is exact) makes the rendered site land on the origin EXACTLY; the
    // rounding moves to the far bodies, where metres are sub-pixel.
    // Compose a point's solar-frame position from the STORED grid chain:
    // start at the frame with `ephemeris_id`, offset `p0` in that (possibly
    // rotating) frame, walk up to the Solar Grid over stored (cell,
    // Transform) values — every f32 read back into f64 is exact.
    let stored_in_solar = |ephemeris_id: i32, p0: DVec3| -> Option<DVec3> {
        let mut current = q_frames_stored
            .iter()
            .find(|(_, f, ..)| f.ephemeris_id == ephemeris_id);
        let mut p = p0;
        let mut steps = 0;
        loop {
            let Some((_, _, c, t, child_of)) = current else { break None };
            steps += 1;
            if steps > 8 {
                break None;
            }
            let parent = child_of.parent();
            let Ok(parent_grid) = q_grids.get(parent) else { break None };
            let edge = parent_grid.cell_edge_length() as f64;
            p = t.rotation.as_dquat() * p
                + DVec3::new(c.x as f64, c.y as f64, c.z as f64) * edge
                + t.translation.as_dvec3();
            if parent == solar_entity {
                break Some(p);
            }
            current = q_frames_stored.iter().find(|(e, ..)| *e == parent);
        }
    };

    // The ENU `align` rotation goes on the ZERO-TRANSLATION Site Align Grid
    // (the Solar Grid's parent), NOT on the Solar Grid itself. big_space's
    // origin propagation multiplies a grid's stored f32 quat into the
    // origin's position vector at that node: on the Solar Grid that vector
    // is heliocentric (~1.5e11 m), so the f32 quat's ~1e-7 relative error
    // put a 15–20 km ULP STAIRCASE between the site branch and the celestial
    // branch — the globe judders seen from the ground, the terrain judders
    // seen from orbit. At the align node the origin vector is near-zero, so
    // the same rotation costs sub-millimetres, and the 1 AU offset below
    // travels through the Solar Grid's EXACT i64 cells in ecliptic axes.
    //
    // Cancellation is exact BY CONSTRUCTION now: the Solar pose is
    // −site_in_solar in the SAME (ecliptic) axes the site composes through,
    // so the rendered site lands on the origin whatever precision `align`
    // has — the old "compute the translation from the rounded f32 quat"
    // trick is obsolete.
    let align_f32 = align.as_quat();
    if let Ok(mut align_tf) = q_align.single_mut() {
        if align_tf.rotation != align_f32 {
            align_tf.rotation = align_f32;
        }
    }
    // Site offset in the (rotating) body frame — rotated by the STORED
    // frame quat inside the walk, matching what tiles/children inherit.
    let site_in_solar = site_geo_offset
        .and_then(|(body_id, geo_local)| stored_in_solar(body_id, geo_local))
        .unwrap_or(site_frame_origin);
    let translation = -site_in_solar;

    if let Ok(child_of) = q_parents.get(solar_entity) {
        if let Ok(parent_grid) = q_grids.get(child_of.parent()) {
            let (new_cell, new_translation) = parent_grid.translation_to_grid(translation);
            if tf.rotation != Quat::IDENTITY {
                tf.rotation = Quat::IDENTITY;
            }
            *cell = new_cell;
            tf.translation = new_translation;
            return;
        }
    }
    // No parent grid → NO write. A raw f32 pose at heliocentric magnitude
    // (~1.5e11 m) quantizes in ~16 km steps — every epoch tick the whole sky
    // would leap kilometres (the "moon jumps around / LOD flaps / black
    // frames" failure). `setup_big_space_hierarchy` parents the Solar Grid
    // under the shell's `WorldGrid` precisely so this path never triggers.
    bevy::log::warn_once!(
        "[celestial] site pin skipped: Solar Grid's parent has no `Grid` — \
         cannot express a heliocentric pose precisely"
    );
}

/// Hide UNANCHORED local scene roots while the orbital view is active; restore
/// on exit. Geometry parked at the world origin has no celestial identity, so
/// from an orbital viewpoint it would float in space in front of the body.
///
/// The SITE-ANCHORED scene is the opposite case and stays VISIBLE: the site
/// pin places it at its true geodetic point on the anchor body, and under
/// doc 47 Phase 6 the camera flies while the scene never moves — so from
/// lunar orbit the moonbase genuinely lies on the Moon, exactly where it
/// belongs. (The blanket hide dated from the retired world-pin design, where
/// the celestial tree was slid away from the site and the local scene stayed
/// glued to the parked camera, filling the foreground — "focused Earth but it
/// shows ground". That geometry no longer exists.)
///
/// Subtlety established by experiment: hiding a scene ROOT is not enough —
/// USD prims spawn with an explicit `Visibility::Visible`, which overrides an
/// ancestor's `Hidden` rather than inheriting it. Every descendant must be
/// toggled.
#[allow(clippy::type_complexity)]
pub fn orbital_pin_scene_visibility(
    orbital_pin: Res<OrbitalViewPin>,
    q_children: Query<&Children>,
    // Plain local scene roots (no celestial binding).
    q_local: Query<
        Entity,
        (With<lunco_core::GridAnchor>, Without<GeodeticAnchor>, Without<KeplerOrbit>),
    >,
    // The site-anchored scene root (carries GeodeticAnchor + SiteAnchor).
    q_site_root: Query<Entity, With<SiteAnchor>>,
    // Single `&mut Visibility` param: several overlapping ones are a B0001
    // conflict panic.
    mut q_vis: Query<&mut Visibility>,
    mut was_active: Local<bool>,
) {
    // Re-apply EVERY frame while pinned, not just on the activation edge: the
    // USD scene may finish spawning (or re-spawn on `LoadScene`) after the pin
    // activated, and fresh prims come up `Visibility::Visible`. An edge-only
    // toggle then leaves the ground on screen — an intermittent "focused Earth
    // but it shows ground", depending on load timing. On release, one edge pass
    // restores the scene.
    let edge = orbital_pin.active != *was_active;
    *was_active = orbital_pin.active;
    if !orbital_pin.active && !edge {
        return;
    }
    let target = if orbital_pin.active {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };

    // Collect each root plus its full subtree — descendants override the root's
    // visibility, so the root alone would leave the ground on screen. Unanchored
    // locals toggle with the mode; the site-anchored subtree is pinned onto its
    // body and is force-VISIBLE every pass (also self-heals scenes hidden by the
    // pre-Phase-6 blanket hide).
    let mut targets: Vec<(Entity, Visibility)> = Vec::new();
    let mut stack: Vec<(Entity, Visibility)> = q_local.iter().map(|e| (e, target)).collect();
    stack.extend(q_site_root.iter().map(|e| (e, Visibility::Inherited)));
    while let Some((e, t)) = stack.pop() {
        targets.push((e, t));
        if let Ok(children) = q_children.get(e) {
            stack.extend(children.iter().map(|c| (c, t)));
        }
    }

    for (e, t) in targets {
        if let Ok(mut vis) = q_vis.get_mut(e) {
            if *vis != t {
                *vis = t;
            }
        }
    }
}

/// Place `GeodeticAnchor`/`KeplerOrbit` prims on their body's rotating grid;
/// hide them when no matching grid exists. The site-anchor root is the scene
/// itself and is never moved.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn place_celestial_bound_entities(
    world_time: Res<WorldTime>,
    registry: Res<CelestialBodyRegistry>,
    q_frames: Query<(Entity, &CelestialReferenceFrame, &Grid)>,
    mut q_bound: Query<
        (
            Entity,
            Option<&GeodeticAnchor>,
            Option<&KeplerOrbit>,
            Option<&mut Visibility>,
        ),
        (
            Or<(With<GeodeticAnchor>, With<KeplerOrbit>)>,
            Without<SiteAnchor>,
        ),
    >,
    q_added: Query<(), Or<(Added<GeodeticAnchor>, Added<KeplerOrbit>)>>,
    mut commands: Commands,
    mut last_jd: Local<f64>,
) {
    if q_bound.is_empty() {
        return;
    }
    let jd = world_time.epoch_jd;
    let epoch_changed = (jd - *last_jd).abs() >= 1e-9;
    if !epoch_changed && q_added.is_empty() && *last_jd != 0.0 {
        return;
    }
    *last_jd = jd;

    for (entity, anchor, orbit, visibility) in q_bound.iter_mut() {
        let body = anchor.map(|a| a.body).or(orbit.map(|o| o.body));
        let Some(body) = body else { continue };
        let Some(desc) = registry.bodies.iter().find(|b| b.ephemeris_id == body) else {
            continue;
        };
        let grid = q_frames
            .iter()
            .find(|(_, frame, _)| frame.ephemeris_id == body);
        let Some((grid_entity, _, grid)) = grid else {
            // No solar hierarchy for this body: keep the prim out of the local
            // scene view. Comms math places it analytically regardless.
            if let Some(mut vis) = visibility {
                if *vis != Visibility::Hidden {
                    *vis = Visibility::Hidden;
                }
            }
            continue;
        };

        // Grid-local pose. The body grids ROTATE (body_rotation_system):
        // anchors are body-fixed (constant in the grid), orbits are inertial
        // (inverse-rotated into the grid).
        let (local, rotation) = if let Some(anchor) = anchor {
            let p = geodetic_to_body_fixed(&anchor.geodetic, desc.radius_m);
            let up = p.normalize_or_zero();
            (p, DQuat::from_rotation_arc(DVec3::Y, up).as_quat())
        } else if let Some(orbit) = orbit {
            // Elements are referenced to the body's EQUATOR (`kepler.rs`), so
            // lift them out of the orbit frame with `equatorial_frame` before
            // cancelling the body's spin. Without that lift the two rotations
            // collapsed (`R⁻¹·p` rendered through the grid's `R` gives back
            // `p`) and inclination silently ended up measured about the
            // ECLIPTIC pole — 23.4° off Earth's, ±23.4° of ground-track error.
            let p_orbit = orbit.elements.position_bevy_m(desc.gm, jd);
            let p_inertial = equatorial_frame(desc, jd) * p_orbit;
            (body_rotation(desc, jd).inverse() * p_inertial, Quat::IDENTITY)
        } else {
            continue;
        };

        let (new_cell, new_translation) = grid.translation_to_grid(local);
        commands.entity(entity).insert((
            new_cell,
            Transform {
                translation: new_translation,
                rotation,
                ..default()
            },
            lunco_core::GridAnchor,
            ChildOf(grid_entity),
        ));
        if let Some(mut vis) = visibility {
            if *vis != Visibility::Inherited {
                *vis = Visibility::Inherited;
            }
        }
    }
}

/// Feed the DEM terrain its parent body's radius whenever a site anchor
/// exists: inserts/updates [`lunco_terrain_surface::TerrainBodyCurvature`], so
/// every oracle composition folds a body-curvature modifier and the
/// tangent-plane DEM curves down onto the globe sphere instead of floating the
/// sagitta above it (the "terrain over the lunar surface" seam). The terrain
/// side re-composes on resource change, so the ordering between the USD site
/// anchor landing and the DEM build starting doesn't matter.
pub fn sync_terrain_body_curvature(
    mut commands: Commands,
    registry: Res<CelestialBodyRegistry>,
    q_site: Query<&GeodeticAnchor, With<SiteAnchor>>,
    current: Option<Res<lunco_terrain_surface::TerrainBodyCurvature>>,
    q_dem: Query<&lunco_terrain_surface::DemHeightField>,
    q_globes: Query<(
        Entity,
        &crate::registry::CelestialBody,
        Option<&crate::globe_lod::GlobePunch>,
    )>,
) {
    let anchor = q_site.iter().next();
    let Some(anchor) = anchor else {
        // Site gone (scene unload): stop curving future DEM builds and
        // restore full globe coverage.
        if current.is_some() {
            commands.remove_resource::<lunco_terrain_surface::TerrainBodyCurvature>();
        }
        for (e, _, punch) in &q_globes {
            if punch.is_some() {
                commands.entity(e).remove::<crate::globe_lod::GlobePunch>();
            }
        }
        return;
    };
    let Some(desc) = registry.bodies.iter().find(|b| b.ephemeris_id == anchor.body) else {
        return;
    };
    if current.is_none_or(|c| c.radius_m != desc.radius_m) {
        commands.insert_resource(lunco_terrain_surface::TerrainBodyCurvature {
            radius_m: desc.radius_m,
        });
        info!(
            "site anchored to body {}: DEM terrain curves to sphere radius {:.0} m",
            anchor.body, desc.radius_m
        );
    }
    // Globe hole-punch under the DEM footprint (needs the built DEM for its
    // half extent; until then the globe stays whole — the curved terrain sits
    // `edge_lift_m` above it, so the brief overlap cannot z-fight).
    let half_extent = q_dem.iter().map(|d| d.0.half_extent() as f64).fold(0.0, f64::max);
    for (e, body, punch) in &q_globes {
        if body.ephemeris_id != anchor.body {
            continue;
        }
        if half_extent <= 0.0 || half_extent >= desc.radius_m {
            if punch.is_some() {
                commands.entity(e).remove::<crate::globe_lod::GlobePunch>();
            }
            continue;
        }
        // Punch only what the terrain provably covers: the inscribed disc of
        // the square footprint, shrunk a hair so the boundary ring keeps its
        // globe backing under the feathered terrain edge.
        let sin_theta = (half_extent * 0.999) / desc.radius_m;
        let next = crate::globe_lod::GlobePunch {
            dir: geodetic_to_body_fixed(&anchor.geodetic, desc.radius_m).normalize(),
            cos_theta: (1.0 - sin_theta * sin_theta).sqrt(),
        };
        if punch != Some(&next) {
            commands.entity(e).insert(next);
            info!(
                "globe hole-punched under site DEM (body {}, footprint ±{:.0} m)",
                anchor.body, half_extent
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::Geodetic;

    /// The align quaternion maps the site ENU axes onto the scene axes.
    #[test]
    fn align_rotation_maps_enu_to_scene_axes() {
        let registry = CelestialBodyRegistry::default_system();
        let desc = registry.bodies.iter().find(|b| b.ephemeris_id == 301).unwrap();
        let center = DVec3::new(1.0e11, 2.0e10, -3.0e10);
        let geo = Geodetic::new(-89.45, -136.7, 1200.0);
        let frame = solar_tangent_frame(desc, &geo, center, 2461000.5);
        let align = DQuat::from_mat3(&bevy::math::DMat3::from_cols(
            DVec3::new(frame.east.x, frame.up.x, -frame.north.x),
            DVec3::new(frame.east.y, frame.up.y, -frame.north.y),
            DVec3::new(frame.east.z, frame.up.z, -frame.north.z),
        ));
        assert!((align * frame.east - DVec3::X).length() < 1e-9);
        assert!((align * frame.up - DVec3::Y).length() < 1e-9);
        assert!((align * frame.north - DVec3::NEG_Z).length() < 1e-9);
        // And the full map sends the site origin to the scene origin.
        let world = align * (frame.origin - frame.origin);
        assert!(world.length() < 1e-9);
    }
}
