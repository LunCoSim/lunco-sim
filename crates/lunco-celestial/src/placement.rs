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
    mut q_solar: Query<
        (Entity, &mut CellCoord, &mut Transform),
        (With<SolarSystemRoot>, With<Grid>),
    >,
    mut q_align: Query<
        (Entity, &mut Transform),
        (
            With<crate::big_space_setup::SiteAlignGrid>,
            Without<SolarSystemRoot>,
            Without<CelestialReferenceFrame>,
        ),
    >,
    mut commands: Commands,
    q_frames_stored: Query<
        (
            Entity,
            &CelestialReferenceFrame,
            &CellCoord,
            &Transform,
            &ChildOf,
        ),
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
    let Ok((solar_entity, mut cell, mut tf)) = q_solar.single_mut() else {
        return;
    };

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
            Some((
                anchor.body,
                geodetic_to_body_fixed(&anchor.geodetic, desc.radius_m),
            )),
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
            let Some((_, _, c, t, child_of)) = current else {
                break None;
            };
            steps += 1;
            if steps > 8 {
                break None;
            }
            let parent = child_of.parent();
            let Ok(parent_grid) = q_grids.get(parent) else {
                break None;
            };
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
    // Site offset in the (rotating) body frame — rotated by the STORED
    // frame quat inside the walk, matching what tiles/children inherit.
    let site_in_solar = site_geo_offset
        .and_then(|(body_id, geo_local)| stored_in_solar(body_id, geo_local))
        .unwrap_or(site_frame_origin);
    let translation = -site_in_solar;

    // The site pin is the ONE writer of the scene's ecliptic→world placement, so
    // it is the one place this has to be checked: everything downstream — every
    // grid cell, every tile's sample coordinate, every collider — is derived from
    // the pair written just below, and a non-finite value here is not a bad pose,
    // it is a poisoned FRAME.
    //
    // big_space is what makes it unrecoverable rather than merely wrong.
    // `Grid::translation_to_grid` converts with `round(x / edge) as GridPrecision`,
    // and Rust's float→int cast SATURATES: `-inf as i64` is `i64::MIN`, `inf` is
    // `i64::MAX`. So an infinite translation does not produce an infinite cell that
    // later maths can carry — it produces an extreme FINITE cell that looks
    // legitimate to every consumer, while the returned remainder (`x - x_r*edge`
    // = `inf - inf`) is NaN. From there the damage is silent and total: the cell
    // magnitude overflows the drift diagnostics, and terrain samples the oracle at
    // NaN coordinates, baking all-NaN tiles whose AABB half-extent is NaN — which
    // is what finally trips `Aabb3d::new`'s `half_size >= 0.0` assertion over in
    // `bevy_picking`, an entire subsystem away from the cause.
    //
    // Refusing the write keeps the previous good pin (or the un-anchored
    // heliocentric default) instead, which is visibly wrong in ONE place rather
    // than subtly wrong everywhere.
    if !translation.is_finite() || !align_f32.is_finite() {
        bevy::log::error!(
            "[celestial] site pin REFUSED: non-finite site frame \
             (translation={translation:?}, align={align_f32:?}). \
             Anchor body {:?}, geodetic {:?}. Leaving the previous pin in place — \
             writing this would saturate the big_space cell and NaN every \
             derived frame.",
            anchor_opt.map(|a| a.body),
            anchor_opt.map(|a| &a.geodetic),
        );
        return;
    }

    if let Ok((align_entity, mut align_tf)) = q_align.single_mut() {
        if align_tf.rotation != align_f32 {
            align_tf.rotation = align_f32;
        }
        // Reaching here means a site anchor RESOLVED (body in the registry, an
        // ephemeris position for it) — so the rotation now on the grid is the real
        // ecliptic→world one. Say so on the entity: an identity quat here is
        // otherwise indistinguishable from the default a celestial-but-unanchored
        // scene leaves behind, and consumers that cannot tell aim the sun into the
        // ecliptic frame (see `SiteAligned`).
        commands
            .entity(align_entity)
            .try_insert(crate::big_space_setup::SiteAligned);
    }

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
        (
            With<lunco_core::GridAnchor>,
            Without<GeodeticAnchor>,
            Without<KeplerOrbit>,
        ),
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
    // Descendant walk for the `LowPrecisionRoot` stamp below — same shape as
    // `orbital_pin_scene_visibility`'s `q_children` in this file.
    q_children: Query<&Children>,
    // Only spatial descendants (Transform + GlobalTransform) need the marker;
    // a non-spatial child is already a valid `AnyNonSpatial` archetype.
    q_spatial: Query<(), (With<Transform>, With<GlobalTransform>)>,
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
            (
                body_rotation(desc, jd).inverse() * p_inertial,
                Quat::IDENTITY,
            )
        } else {
            continue;
        };

        let (new_cell, new_translation) = grid.translation_to_grid(local);
        commands.entity(entity).try_insert((
            new_cell,
            Transform {
                translation: new_translation,
                rotation,
                ..default()
            },
            lunco_core::GridAnchor,
            ChildOf(grid_entity),
        ));
        // The reparent above turns THIS prim into a high-precision cell entity
        // (CellCoord + ChildOf(grid)), but its USD-spawned descendants (mesh /
        // material / shader children) are untouched: they keep their plain
        // Transform + GlobalTransform + ChildOf(this prim) and so become
        // INVALID children of a "Non-root high precision spatial entity"
        // (big_space validation: a child of an HP entity must be a
        // `LowPrecisionRoot` subtree or a non-spatial entity). big_space's own
        // `tag_low_precision_roots` does NOT fix this — it only fires on the
        // CHILD's `Changed<ChildOf>`/`Added<Transform>`, and reparenting the
        // parent changes neither on the children. Same spawn-order window the
        // trajectory/mission/link-beam spawn paths hit and fix the same way
        // (trajectories.rs, missions.rs, link_beams.rs): explicitly stamp the
        // marker on every spatial descendant here.
        stamp_low_precision_roots(entity, &q_children, &q_spatial, &mut commands);
        if let Some(mut vis) = visibility {
            if *vis != Visibility::Inherited {
                *vis = Visibility::Inherited;
            }
        }
    }
}

/// Stamp [`LowPrecisionRoot`](big_space::grid::propagation::LowPrecisionRoot)
/// on every spatial descendant of `root`.
///
/// Called after `place_celestial_bound_entities` reparents an anchor/orbit
/// prim under a body `Grid` (writing `CellCoord` + `ChildOf(grid)` onto the
/// prim itself). That reparent makes the prim a high-precision cell entity but
/// leaves its USD-spawned mesh/material descendants as plain
/// `Transform`+`GlobalTransform` children — an invalid big_space child
/// archetype until tagged. `try_insert` is idempotent on the marker, so this
/// is safe to call on every epoch-change reparent.
fn stamp_low_precision_roots(
    root: Entity,
    q_children: &Query<&Children>,
    q_spatial: &Query<(), (With<Transform>, With<GlobalTransform>)>,
    commands: &mut Commands,
) {
    let mut stack: Vec<Entity> = Vec::new();
    if let Ok(children) = q_children.get(root) {
        stack.extend(children.iter());
    }
    while let Some(e) = stack.pop() {
        if q_spatial.get(e).is_ok() {
            commands
                .entity(e)
                .try_insert(big_space::grid::propagation::LowPrecisionRoot);
        }
        if let Ok(children) = q_children.get(e) {
            stack.extend(children.iter());
        }
    }
}

/// Feed the DEM terrain its parent body's radius whenever a site anchor
/// exists: inserts/updates [`lunco_terrain_surface::TerrainBodyCurvature`], so
/// every oracle composition folds a body-curvature modifier and the
/// tangent-plane DEM curves down onto the globe sphere instead of floating the
/// sagitta above it (the "terrain over the lunar surface" seam). Pending DEM
/// requests participate too, allowing the terrain builder to capture curvature
/// on its first pass rather than generating a provisional flat oracle first.
///
/// **The body comes from each terrain's own [`lunco_terrain_surface::TerrainGeoref`],
/// never from a `SiteAnchor` query.** The radius folds into the surface oracle,
/// so it decides the composed GEOMETRY and the `content_key` every tile/derived
/// cache keys on. Resolving it via `q_site.iter().next()` made that a function of
/// archetype order: a scene with a second anchor (ground stations author body 399
/// Earth) could curve a lunar DEM to Earth's 6371 km radius, and which anchor won
/// varied per launch with async USD load order — terrain that differed every boot
/// and re-baked its whole cache. `SiteAnchor` still gates curvature on/off (it is
/// what makes a scene site-anchored at all); it just no longer chooses the body.
pub fn sync_terrain_body_curvature(
    mut commands: Commands,
    registry: Res<CelestialBodyRegistry>,
    q_site: Query<&GeodeticAnchor, With<SiteAnchor>>,
    current: Option<Res<lunco_terrain_surface::TerrainBodyCurvature>>,
    q_dem: Query<
        Option<&lunco_terrain_surface::TerrainGeoref>,
        Or<(
            With<lunco_terrain_surface::DemHeightField>,
            With<lunco_terrain_surface::DemTerrainRequest>,
        )>,
    >,
    q_built_dem: Query<&lunco_terrain_surface::DemHeightField>,
    q_globes: Query<(
        Entity,
        &crate::registry::CelestialBody,
        Option<&crate::globe_lod::GlobePunch>,
    )>,
) {
    // The site anchor still places the scene on the globe (that IS its job, and it
    // is the scene root by intent) — it just no longer decides which BODY the
    // terrain curves to.
    let Some(anchor) = q_site.iter().next() else {
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
    // The body every terrain in this scene sits on, from the DOCUMENT. Reducing by
    // the authored id (`min`, not iteration order) keeps the pick a pure function
    // of the scene: a scene whose terrains disagree is malformed — one global
    // curvature resource cannot serve two radii — so say so rather than let load
    // order choose a winner.
    let mut body: Option<i32> = None;
    let mut mixed = false;
    for georef in &q_dem {
        let b = georef.map_or(lunco_terrain_surface::DEFAULT_ANCHOR_BODY, |g| g.body);
        match body {
            None => body = Some(b),
            Some(prev) if prev != b => {
                mixed = true;
                body = Some(prev.min(b));
            }
            Some(_) => {}
        }
    }
    // No DEM is NOT "nothing to do". A scene can stand on a plain authored ground
    // slab (`def Cube "Ground"` with `LunCoTerrainAPI`) and never build a
    // `DemHeightField` — episode 01 does exactly that — and such a scene still needs
    // the globe punched out from under it. Falling back to the site anchor's own body
    // keeps the curvature resource DEM-only (there is genuinely nothing to curve)
    // while letting the punch below run off the anchor alone.
    let has_dem = body.is_some();
    let body = body.unwrap_or(anchor.body);
    if mixed {
        warn_once!(
            "terrains in this scene author different `lunco:anchor:body` values; \
             curvature is a single global radius, so body {body} was taken. Author one \
             body per scene."
        );
    }
    let Some(desc) = registry.bodies.iter().find(|b| b.ephemeris_id == body) else {
        return;
    };
    if has_dem && current.is_none_or(|c| c.radius_m != desc.radius_m) {
        commands.insert_resource(lunco_terrain_surface::TerrainBodyCurvature {
            radius_m: desc.radius_m,
        });
        info!(
            "terrain anchored to body {}: DEM terrain curves to sphere radius {:.0} m",
            body, desc.radius_m
        );
    }
    // Globe hole-punch under the local surface.
    //
    // With a DEM, the footprint is the DEM's own half extent — punch exactly what the
    // terrain provably covers.
    //
    // WITHOUT one, the footprint has to come from the tile grid instead, and it must
    // be SITE-SCALE, not slab-scale. `tile_fully_in_punch` drops a tile only when the
    // whole tile fits inside the cone, and the Moon's finest tiles still subtend
    // ~90°/2^max_lod ≈ 0.35°; a 200 m slab subtends 0.007°, so a slab-sized cone would
    // pass the test for exactly zero tiles and punch nothing at all. `SITE_PUNCH_DEG`
    // is therefore sized in TILES, not in metres of authored ground: big enough that
    // the fine tiles around the site fall entirely inside it.
    //
    // What this buys, and why the near-field globe must go: standing on a site, the
    // globe's own tiles are coincident with the authored ground at the datum. While
    // they rendered as blueprint wireframe that only showed up as z-fight moiré; the
    // moment they carry real albedo (see `celestial_visuals_system`) they also become
    // opaque, and at 5 m altitude the near tiles wall off the sky as a brown smear of
    // ~5 km/texel mosaic. The local ground owns the near field; the globe owns the far
    // field and the limb, and the punch is the seam between them.
    const SITE_PUNCH_DEG: f64 = 2.0;
    let half_extent = if has_dem {
        q_built_dem
            .iter()
            .map(|d| d.0.half_extent() as f64)
            .fold(0.0, f64::max)
    } else {
        desc.radius_m * SITE_PUNCH_DEG.to_radians().sin()
    };
    for (e, globe, punch) in &q_globes {
        if globe.ephemeris_id != body {
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
            // WHERE on the globe to punch is the site anchor's business; only the
            // body RADIUS (which folds into the oracle) had to become the
            // terrain's own property.
            dir: geodetic_to_body_fixed(&anchor.geodetic, desc.radius_m).normalize(),
            cos_theta: (1.0 - sin_theta * sin_theta).sqrt(),
        };
        if punch != Some(&next) {
            commands.entity(e).try_insert(next);
            info!("globe hole-punched under site DEM (body {body}, footprint ±{half_extent:.0} m)");
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
        let desc = registry
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == 301)
            .unwrap();
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
