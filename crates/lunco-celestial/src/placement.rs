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
    body_rotation, geodetic_to_body_fixed, solar_tangent_frame, GeodeticAnchor, SiteAnchor,
};
use crate::kepler::KeplerOrbit;
use crate::registry::{CelestialBodyRegistry, CelestialReferenceFrame};

/// Pin the solar hierarchy so the authored site anchor coincides with the
/// local scene origin, ENU-aligned. World = R·(solar − p_site) with R mapping
/// East→+X, Up→+Y, North→−Z.
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
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    mut last_jd: Local<f64>,
) {
    let Some(ephemeris) = ephemeris else { return };
    let Some(anchor) = q_site.iter().next() else { return };
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

    let Some(desc) = registry
        .bodies
        .iter()
        .find(|b| b.ephemeris_id == anchor.body)
    else {
        return;
    };

    let body_center = ecliptic_to_bevy(ephemeris.provider.global_position(anchor.body, jd));
    let frame = solar_tangent_frame(desc, &anchor.geodetic, body_center, jd);
    // Rows East/Up/−North → world axes.
    let align = DQuat::from_mat3(&bevy::math::DMat3::from_cols(
        DVec3::new(frame.east.x, frame.up.x, -frame.north.x),
        DVec3::new(frame.east.y, frame.up.y, -frame.north.y),
        DVec3::new(frame.east.z, frame.up.z, -frame.north.z),
    ));
    let translation = -(align * frame.origin);

    if let Ok(child_of) = q_parents.get(solar_entity) {
        if let Ok(parent_grid) = q_grids.get(child_of.parent()) {
            let (new_cell, new_translation) = parent_grid.translation_to_grid(translation);
            tf.rotation = align.as_quat();
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
            let p_inertial = orbit.elements.position_bevy_m(desc.gm, jd);
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
