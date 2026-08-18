//! Site-anchored solar hierarchy + celestial-bound entity placement (doc 43
//! §2.6).
//!
//! **Site anchoring**: the solar hierarchy is pinned and the site scene is
//! re-branched under its body's surface grid once that frame exists. The
//! solar hierarchy is pinned so the site's geodetic point coincides with the
//! scene origin and ENU aligns with the scene axes (East=+X, North=−Z,
//! Up=+Y). During the handoff the authored ENU pose is converted exactly into
//! the body's fixed Cartesian frame; preserving the old world pose would keep
//! ecliptic axes and rotate the ground away from gravity. Keeping the DEM,
//! globe handoff, camera, and surface operations in one body-fixed precision
//! branch avoids an AU-scale hierarchy joint between moving surface pieces.
//! Runs after `ephemeris_update_system` (which re-zeroes the solar grid on an
//! accepted celestial solve) and overrides it whenever a [`SiteAnchor`] is
//! authored. The caller applies the one shared celestial solve gate; this
//! module has no private epoch gate.
//!
//! **Bound entities**: prims with a [`GeodeticAnchor`] (ground stations) or a
//! [`KeplerOrbit`] (satellites) are re-parented onto their body's rotating
//! grid and positioned each epoch tick — body-fixed coordinates for anchors
//! (the grid's spin carries them), inverse-rotated inertial coordinates for
//! orbits. Without a matching grid (no solar hierarchy) they are hidden;
//! comms math is unaffected either way.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, FloatingOrigin, Grid};

use lunco_time::WorldTime;

use crate::big_space_setup::SolarSystemRoot;
use crate::coords::ecliptic_to_bevy;
use crate::ephemeris::EphemerisResource;
use crate::geo::{
    body_rotation, equatorial_frame, geodetic_to_body_fixed, solar_tangent_frame, GeodeticAnchor,
    LocalTangentFrame, SiteAnchor,
};
use crate::kepler::KeplerOrbit;
use crate::registry::{CelestialBodyRegistry, ReferenceFrame};

/// Map a site-authored pose into the body's rotating surface frame.
///
/// A `SiteAnchor` is an explicit USD frame contract: scene coordinates are
/// ENU (`+X = east`, `+Y = up`, `-Z = north`).  The body's surface grid is not
/// an arbitrary display grid; its axes are the body's fixed Cartesian axes and
/// its origin is the body centre.  Therefore the handoff is the unique rigid
/// transform defined by the authored geodetic anchor.  Keeping this conversion
/// here makes scene root, camera, terrain and physics use the same frame map.
fn site_enu_to_body_fixed_pose(
    anchor: &GeodeticAnchor,
    radius_m: f64,
    scene_position: DVec3,
    scene_rotation: DQuat,
) -> (DVec3, DQuat) {
    let tangent = LocalTangentFrame::body_fixed(&anchor.geodetic, radius_m);
    let scene_to_body = DQuat::from_mat3(&bevy::math::DMat3::from_cols(
        tangent.east,
        tangent.up,
        -tangent.north,
    ));
    (
        tangent.origin + scene_to_body * scene_position,
        scene_to_body * scene_rotation,
    )
}

/// Read an entity's pose in its direct Grid frame.
///
/// A site scene is mounted under the canonical `WorldGrid` before celestial
/// handoff. Its authored coordinates are therefore already local ENU values;
/// composing them through the whole solar hierarchy would mix that local frame
/// with the grid's storage cells. This helper deliberately stops at the direct
/// parent Grid, then the site-anchor conversion below changes semantic frames.
fn direct_grid_pose(
    entity: Entity,
    parent: Entity,
    q_grids: &Query<&Grid>,
    q_spatial: &Query<(Option<&CellCoord>, &Transform)>,
) -> Option<(DVec3, DQuat)> {
    let grid = q_grids.get(parent).ok()?;
    let (cell, transform) = q_spatial.get(entity).ok()?;
    let cell = cell.copied().unwrap_or_default();
    let position = grid.grid_position_double(&cell, transform);
    Some((position, transform.rotation.as_dquat()))
}

/// Celestial projection is multi-stage. Give the solar grid and ephemeris a
/// short settle window before reporting a structural anchoring failure.
const ANCHOR_SETTLE_FRAMES: u32 = 30;

/// Orbital view mode state.
///
/// The camera itself lives in the target body's explicit
/// [`crate::ReferenceFrame::EclipticJ2000`]. `big_space` propagates the floating origin through
/// that nested grid hierarchy in high precision. This resource is only the
/// cross-domain presentation fact consumed by visibility, gravity and lighting;
/// camera return state belongs to the avatar that owns the camera.
///
/// Remaining consumers of the mode flag:
/// * [`orbital_pin_scene_visibility`] — hides the local scene while orbital;
/// * `compute_local_gravity` — holds the last surface field;
/// * exit paths — the avatar restores its transactional orbit-entry snapshot.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct OrbitalViewPin {
    pub active: bool,
    /// Ephemeris id of the focused body.
    pub body: i32,
    /// Unit direction from the body centre toward the viewpoint in the body's
    /// star-fixed orbit-view frame.
    pub dir: DVec3,
    /// Viewpoint distance from the body centre, metres.
    pub distance: f64,
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
    frame_index: Res<crate::ReferenceFrameIndex>,
    q_site: Query<&GeodeticAnchor, With<SiteAnchor>>,
    q_site_changed: Query<(), Or<(Added<SiteAnchor>, Changed<GeodeticAnchor>)>>,
    // `With<Grid>` states a PRECONDITION of the code below (it does cell
    // arithmetic against this entity's own `Grid`), not a disambiguation.
    // It used to be the latter: the Sun body also carried `SolarSystemRoot`, and
    // this filter was the only reason `single_mut()` saw one entity. The marker is
    // singular at the spawn site now, held by `solar_system_root_is_singular`.
    mut q_solar: Query<
        (Entity, &mut CellCoord, &mut Transform),
        (With<SolarSystemRoot>, With<Grid>),
    >,
    mut q_align: Query<
        (Entity, &mut Transform),
        (
            With<crate::big_space_setup::SiteAlignGrid>,
            Without<SolarSystemRoot>,
            Without<ReferenceFrame>,
        ),
    >,
    mut commands: Commands,
    q_frames_stored: Query<
        (Entity, &ReferenceFrame, &CellCoord, &Transform, &ChildOf),
        (With<Grid>, Without<SolarSystemRoot>),
    >,
    q_parents: Query<&ChildOf, Without<SiteAnchor>>,
    q_grids: Query<&Grid>,
    // One-shot latch for the "declared a site but cannot anchor it" diagnostics
    // below. It is paired with a settle counter so asynchronous celestial
    // bootstrap does not produce a permanent false alarm during scene load.
    mut warned: Local<bool>,
    // Celestial entities and ephemeris data arrive in separate projections. Do
    // not turn their expected startup ordering into a permanent scene warning.
    mut unresolved_frames: Local<u32>,
) {
    // The diagnostics below latch so a structural fault doesn't repeat every
    // gated frame — but the latch belongs to the SCENE, not the process. A new
    // site anchor is a new scene: re-arm, or the twin loaded after the sandbox
    // scene inherits a spent latch and reports nothing at all.
    if !q_site_changed.is_empty() {
        *warned = false;
        *unresolved_frames = 0;
    }
    let Some(ephemeris) = ephemeris else { return };
    let anchor_opt = q_site.iter().next();
    // Without a site anchor the solar tree stays heliocentric.
    if anchor_opt.is_none() {
        return;
    }
    // From here on the scene HAS asked to be site-anchored, so every remaining
    // early return is a failure to deliver something the scene declared — and
    // its visible consequence is severe and silent: no `SiteAligned` ⇒
    // `update_sun_light_system` falls back to an IDENTITY alignment and aims the
    // sun along raw ECLIPTIC axes, which puts it below the local horizon and
    // renders the whole scene black. Say so once instead of letting a black
    // frame be the only symptom.
    let Ok((solar_entity, mut cell, mut tf)) = q_solar.single_mut() else {
        *unresolved_frames = (*unresolved_frames).saturating_add(1);
        if !*warned && *unresolved_frames >= ANCHOR_SETTLE_FRAMES {
            *warned = true;
            warn!(
                "[celestial] site-anchored scene has {} Solar Grid entities (need exactly 1) — \
                 cannot anchor the solar frame, so the sun stays ECLIPTIC-aligned and the \
                 scene will render unlit",
                q_solar.iter().count()
            );
        }
        return;
    };
    if *warned {
        info!(
            "[celestial] solar anchoring prerequisites are available again; the prior warning was startup ordering"
        );
        *warned = false;
    }
    *unresolved_frames = 0;

    // The shared celestial solve gate owns temporal cadence. Keeping a second
    // local epoch comparison here would allow site pinning to observe a
    // different epoch from the body pose it is derived from.
    let jd = world_time.epoch_jd;

    // Site tangent alignment (world axes) — identity when there is no anchor.
    let (align, site_frame_origin, site_geo_offset) = if let Some(anchor) = anchor_opt {
        let Some(desc) = registry.get(anchor.body) else {
            *unresolved_frames = (*unresolved_frames).saturating_add(1);
            if !*warned && *unresolved_frames >= ANCHOR_SETTLE_FRAMES {
                *warned = true;
                warn!(
                    "[celestial] site anchor names body {} but the registry declares no such \
                     body — cannot anchor the solar frame, so the scene will render unlit",
                    anchor.body
                );
            }
            return;
        };
        // No ephemeris ⇒ we do not know where the body IS, so we cannot anchor a site to it.
        // Leaving the anchor un-placed is honest; placing it at the Sun's centre is not.
        let Some(p) = ephemeris.provider.global_position(anchor.body, jd) else {
            *unresolved_frames = (*unresolved_frames).saturating_add(1);
            if !*warned && *unresolved_frames >= ANCHOR_SETTLE_FRAMES {
                *warned = true;
                warn!(
                    "[celestial] ephemeris has no position for body {} at JD {jd:.5} \
                     (NoOp provider, or the epoch is outside its span) — cannot anchor the \
                     solar frame, so the scene will render unlit",
                    anchor.body
                );
            }
            return;
        };
        if *warned {
            info!(
                "[celestial] Earth-relative ephemeris is available again; the prior warning was startup ordering"
            );
            *warned = false;
        }
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
        let start = frame_index.resolve(ReferenceFrame::BodyFixed { body: ephemeris_id })?;
        let mut current = q_frames_stored.get(start).ok();
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
    let site_in_solar = if let Some((body_id, geo_local)) = site_geo_offset {
        let Some(stored) = stored_in_solar(body_id, geo_local) else {
            error_once!(
                "[celestial] site pin refused: body-fixed frame {} has no complete stored Grid chain to the Solar frame",
                body_id
            );
            return;
        };
        stored
    } else {
        site_frame_origin
    };
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

/// Attach the site scene to the body's body-fixed surface frame.
///
/// The scene is initially mounted under `WorldGrid` because the USD loader has
/// no celestial knowledge at mount time. The root is atomically placed under
/// the body's rotating surface grid and becomes a nested BigSpace [`Grid`].
/// That nested grid is the authored ENU site frame and Avian's one stable
/// [`lunco_core::ActivePhysicsFrame`]. The Moon/Earth rotation remains above it,
/// so celestial motion changes rendering but never rewrites local physics
/// position, velocity, contacts, or joints.
pub fn attach_site_scene_to_surface_grid(
    q_site: Query<(Entity, &GeodeticAnchor, &ChildOf), With<SiteAnchor>>,
    q_bodies: Query<(
        Entity,
        &crate::registry::CelestialBody,
        &crate::globe_lod::GlobeLod,
    )>,
    q_avatars: Query<
        (Entity, &Transform, Option<&CellCoord>, &ChildOf),
        (With<lunco_core::Avatar>, With<FloatingOrigin>),
    >,
    q_physical: Query<Entity, With<avian3d::prelude::RigidBody>>,
    q_parents: Query<&ChildOf>,
    q_children: Query<&Children>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_desc_spatial: Query<
        (),
        (
            With<Transform>,
            With<GlobalTransform>,
            Without<CellCoord>,
            Without<Grid>,
        ),
    >,
    grid_config: Res<lunco_core::WorldGridConfig>,
    active_physics_frame: Option<Res<lunco_core::ActivePhysicsFrame>>,
    mut commands: Commands,
) {
    let Ok((scene_root, anchor, child_of)) = q_site.single() else {
        return;
    };
    let Some((body_entity, body, lod)) = q_bodies
        .iter()
        .find(|(_, body, _)| body.ephemeris_id == anchor.body)
    else {
        return;
    };
    let body_surface_grid = lod.surface_grid;
    let Ok(body_surface_grid_component) = q_grids.get(body_surface_grid) else {
        return;
    };
    let make_site_grid = || {
        Grid::new(
            grid_config.cell_edge_length,
            grid_config.switching_threshold,
        )
    };

    // Capture the relative avatar pose before changing the root's parent or
    // making it a Grid. The avatar is grid-direct because it carries the one
    // FloatingOrigin, while the rover/terrain remain descendants of the scene
    // root; this is the one intentional branch crossing in the handoff.
    let scene_root_world_pose =
        lunco_core::coords::world_pose(scene_root, &q_parents, &q_grids, &q_spatial);

    let root_is_site_grid = q_grids.get(scene_root).is_ok();
    if !root_is_site_grid {
        if child_of.parent() != body_surface_grid {
            let Some((scene_position, scene_rotation)) =
                direct_grid_pose(scene_root, child_of.parent(), &q_grids, &q_spatial)
            else {
                return;
            };
            let (body_position, body_rotation) =
                site_enu_to_body_fixed_pose(anchor, body.radius_m, scene_position, scene_rotation);
            let (cell, translation) =
                body_surface_grid_component.translation_to_grid(body_position);
            lunco_core::attach::migrate_to_grid(
                &mut commands,
                scene_root,
                body_surface_grid,
                cell,
                Transform::from_translation(translation).with_rotation(body_rotation.as_quat()),
            );
        }

        // The site root is both the authored frame identity (`SiteAnchor`) and
        // its BigSpace precision representation. No parallel frame entity or
        // lookup table can drift away from it.
        commands.entity(scene_root).try_insert(make_site_grid());
        stamp_low_precision_roots(scene_root, &q_children, &q_desc_spatial, &mut commands);
        info!(
            "[celestial] site scene mounted as ENU physics grid {:?} on body surface grid {:?} (body {})",
            scene_root, body_surface_grid, anchor.body
        );
    }

    if active_physics_frame.is_none_or(|frame| frame.0 != scene_root) {
        commands.insert_resource(lunco_core::ActivePhysicsFrame(scene_root));
    }

    // Move the FloatingOrigin avatar into the same site grid. The conversion is
    // relative to the root sampled above; ancestor translation/rotation cancels
    // and no body-fixed vector is ever mistaken for ENU.
    for (avatar, _avatar_transform, _avatar_cell, avatar_child) in &q_avatars {
        if avatar_child.parent() == scene_root {
            commands
                .entity(avatar)
                .try_insert(lunco_environment::GravityBody { body_entity });
            continue;
        }

        let site_pose = if root_is_site_grid {
            lunco_core::coords::pose_in_grid(avatar, scene_root, &q_parents, &q_grids, &q_spatial)
        } else {
            let Some((scene_world_position, scene_world_rotation)) = scene_root_world_pose else {
                continue;
            };
            let Some((avatar_world_position, avatar_world_rotation)) =
                lunco_core::coords::world_pose(avatar, &q_parents, &q_grids, &q_spatial)
            else {
                continue;
            };
            let inverse = scene_world_rotation.0.inverse();
            Some((
                inverse * (avatar_world_position.0 - scene_world_position.0),
                inverse * avatar_world_rotation.0,
            ))
        };
        let Some((site_position, site_rotation)) = site_pose else {
            continue;
        };
        let (cell, translation) = make_site_grid().translation_to_grid(site_position);
        lunco_core::attach::migrate_to_grid(
            &mut commands,
            avatar,
            scene_root,
            cell,
            Transform::from_translation(translation).with_rotation(site_rotation.as_quat()),
        );
        // A site-anchored avatar is physically associated with the authored
        // body even before it possesses a rover.  The avatar plugin consumes
        // this authoritative binding to enter surface-relative camera mode;
        // no altitude/name heuristic is needed.
        commands
            .entity(avatar)
            .try_insert(lunco_environment::GravityBody { body_entity });
        info!(
            "[celestial] site avatar {:?} attached to ENU physics grid {:?}",
            avatar, scene_root
        );
    }

    // Surface gravity is a property of every physical body mounted under the
    // site scene, not only of the camera. The USD physics projection owns the
    // rigid bodies; this celestial projection only supplies their explicit
    // gravitational parent at the scene-frame boundary.
    for physical in &q_physical {
        let mut current = physical;
        for _ in 0..32 {
            if current == scene_root {
                commands
                    .entity(physical)
                    .try_insert(lunco_environment::GravityBody { body_entity });
                break;
            }
            let Ok(parent) = q_parents.get(current) else {
                break;
            };
            current = parent.parent();
        }
    }
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
    frame_index: Res<crate::ReferenceFrameIndex>,
    q_grids: Query<&Grid>,
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
            // A terrain's anchor attributes describe the DEM's georeference;
            // they do not place the terrain entity as a second body-fixed
            // object. The site root already owns that placement, and the DEM
            // must remain in the same local branch as the rover/camera.
            Without<lunco_terrain_surface::TerrainGeoref>,
        ),
    >,
    // Descendant walk for the `LowPrecisionRoot` stamp below — same shape as
    // `orbital_pin_scene_visibility`'s `q_children` in this file.
    q_children: Query<&Children>,
    // Only spatial descendants (Transform + GlobalTransform) need the marker;
    // a non-spatial child is already a valid `AnyNonSpatial` archetype.
    q_spatial: Query<
        (),
        (
            With<Transform>,
            With<GlobalTransform>,
            Without<CellCoord>,
            Without<Grid>,
        ),
    >,
    mut commands: Commands,
) {
    if q_bound.is_empty() {
        return;
    }
    let jd = world_time.epoch_jd;
    // Temporal cadence is owned by `cadence::tracked_needs_solve` at the
    // registration boundary. A second Local epoch gate here used to place
    // bound entities at a different cadence from the body frames.

    for (entity, anchor, orbit, visibility) in q_bound.iter_mut() {
        let body = anchor.map(|a| a.body).or_else(|| orbit.map(|o| o.body));
        let Some(body) = body else { continue };
        let Some(desc) = registry.get(body) else {
            continue;
        };
        let Some(grid_entity) = frame_index.resolve(ReferenceFrame::BodyFixed { body }) else {
            // No solar hierarchy for this body: keep the prim out of the local
            // scene view. Comms math places it analytically regardless.
            if let Some(mut vis) = visibility {
                if *vis != Visibility::Hidden {
                    *vis = Visibility::Hidden;
                }
            }
            continue;
        };
        let Ok(grid) = q_grids.get(grid_entity) else {
            error_once!(
                "[celestial] resolved body-fixed frame {:?} is not a Grid",
                grid_entity
            );
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
    q_spatial: &Query<
        (),
        (
            With<Transform>,
            With<GlobalTransform>,
            Without<CellCoord>,
            Without<Grid>,
        ),
    >,
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
    q_built_dem: Query<(
        &lunco_terrain_surface::DemHeightField,
        Option<&lunco_terrain_surface::TerrainGeoref>,
    )>,
    q_globes: Query<(
        Entity,
        &crate::registry::CelestialBody,
        Option<&crate::globe_lod::GlobeHandoff>,
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
        for (e, _, handoff) in &q_globes {
            if handoff.is_some() {
                commands
                    .entity(e)
                    .remove::<crate::globe_lod::GlobeHandoff>();
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
            Some(prev) if prev != b => mixed = true,
            Some(_) => {}
        }
    }
    // No DEM is NOT "nothing to do" for curvature bookkeeping: a scene can stand
    // on a plain authored ground slab and still be site-anchored. It simply has no
    // source-driven globe handoff because there is no measured footprint.
    if mixed {
        error_once!(
            "terrains in this scene author different `lunco:anchor:body` values; \
             curvature is a single global radius, so the terrain projection is refused. \
             Author one body per scene."
        );
        return;
    }
    let has_dem = body.is_some();
    let body = body.unwrap_or(anchor.body);
    let Some(desc) = registry.get(body) else {
        return;
    };
    if has_dem && current.is_none_or(|c| c.radius_m != desc.radius_m) {
        commands.insert_resource(lunco_terrain_surface::TerrainBodyCurvature {
            radius_m: desc.radius_m,
        });
        debug!(
            "terrain anchored to body {}: DEM terrain curves to sphere radius {:.0} m",
            body, desc.radius_m
        );
    }
    // Build one source-backed handoff from the largest built footprint. The
    // current globe component is one handoff per body, so multiple same-body
    // DEMs are an explicit scene-level ambiguity rather than an entity-order
    // choice. The largest footprint remains the documented policy; equal
    // footprints use the source content key, which is stable across ECS spawn
    // order. A multi-site handoff needs a keyed component in a future schema.
    let candidates: Vec<_> = q_built_dem
        .iter()
        .filter(|(_, georef)| {
            georef.map_or(lunco_terrain_surface::DEFAULT_ANCHOR_BODY, |g| g.body) == body
        })
        .collect();
    if candidates.len() > 1 {
        warn_once!(
            "{} built DEM terrains author body {}; one globe handoff is available, \
             so the largest footprint is selected and equal footprints use source content \
             identity",
            candidates.len(),
            body
        );
    }
    let selected_dem = candidates.into_iter().max_by(|(a, _), (b, _)| {
        a.0.half_extent()
            .total_cmp(&b.0.half_extent())
            .then_with(|| a.0.surface_key().cmp(&b.0.surface_key()))
    });
    let half_extent = selected_dem.map_or(0.0, |(dem, _)| dem.0.half_extent() as f64);
    let oracle = selected_dem.map(|(dem, _)| dem.0.clone());
    for (e, globe, handoff) in &q_globes {
        if globe.ephemeris_id != body {
            continue;
        }
        let Some(oracle) = oracle.clone() else {
            if handoff.is_some() {
                commands
                    .entity(e)
                    .remove::<crate::globe_lod::GlobeHandoff>();
            }
            continue;
        };
        if half_extent <= 0.0 || half_extent >= desc.radius_m {
            if handoff.is_some() {
                commands
                    .entity(e)
                    .remove::<crate::globe_lod::GlobeHandoff>();
            }
            continue;
        }
        let tangent = LocalTangentFrame::body_fixed(&anchor.geodetic, desc.radius_m);
        let next = crate::globe_lod::GlobeHandoff::new(
            tangent.up,
            tangent.east,
            tangent.north,
            desc.radius_m,
            oracle,
            half_extent,
        );
        if handoff != Some(&next) {
            commands.entity(e).try_insert(next);
            debug!(
                "globe handoff composed at site body {body} (footprint ±{half_extent:.0} m, collar ±{:.0} m)",
                half_extent * 2.0
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
        let desc = registry
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == crate::ephemeris_id::MOON)
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

    /// Site-scene poses are converted once into the body's fixed surface frame:
    /// the authored +Y is exactly the local gravity/up direction, not ecliptic
    /// +Y.  This is the invariant that keeps the terrain below the camera at a
    /// non-equatorial site.
    #[test]
    fn site_pose_maps_authored_enu_to_body_fixed_axes() {
        let registry = CelestialBodyRegistry::default_system();
        let body = registry
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == crate::ephemeris_id::MOON)
            .unwrap();
        let anchor = GeodeticAnchor {
            body: crate::ephemeris_id::MOON,
            geodetic: Geodetic::new(25.28, 307.60, 0.0),
        };
        let (position, rotation) =
            site_enu_to_body_fixed_pose(&anchor, body.radius_m, DVec3::ZERO, DQuat::IDENTITY);
        let tangent = LocalTangentFrame::body_fixed(&anchor.geodetic, body.radius_m);
        assert!((position - tangent.origin).length() < 1e-9);
        assert!((rotation * DVec3::Y - tangent.up).length() < 1e-9);
        assert!((rotation * DVec3::X - tangent.east).length() < 1e-9);
        assert!((rotation * DVec3::NEG_Z - tangent.north).length() < 1e-9);
    }

    #[test]
    fn site_scene_avatar_and_physics_share_the_authored_surface_grid() {
        let mut app = App::new();
        app.insert_resource(lunco_core::WorldGridConfig::default());
        app.add_systems(Update, attach_site_scene_to_surface_grid);

        let world_grid = app
            .world_mut()
            .spawn((Grid::new(2_000.0, 100.0), CellCoord::ZERO))
            .id();
        let body_fixed_grid = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 100.0),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 100.0),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(body_fixed_grid),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn((
                crate::CelestialBody {
                    name: "Moon".into(),
                    ephemeris_id: crate::ephemeris_id::MOON,
                    radius_m: crate::MOON_MEAN_RADIUS_M,
                },
                crate::globe_lod::GlobeLod {
                    radius_m: crate::MOON_MEAN_RADIUS_M,
                    surface_grid,
                    look: lunco_materials::ShaderLook::new("shaders/blueprint.wgsl"),
                    res: 8,
                    max_lod: 1,
                    lod_distance_factor: 1.0,
                },
            ))
            .id();
        let site = app
            .world_mut()
            .spawn((
                SiteAnchor,
                GeodeticAnchor {
                    body: crate::ephemeris_id::MOON,
                    geodetic: Geodetic::new(25.28, 307.60, 0.0),
                },
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let rigid_body = app
            .world_mut()
            .spawn((
                avian3d::prelude::RigidBody::Dynamic,
                Transform::from_xyz(3.0, 1.0, -2.0),
                GlobalTransform::default(),
                ChildOf(site),
            ))
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                lunco_core::Avatar,
                FloatingOrigin,
                CellCoord::ZERO,
                Transform::from_xyz(0.0, 4.0, 8.0),
                GlobalTransform::default(),
                ChildOf(world_grid),
            ))
            .id();

        app.update();
        app.update();

        let world = app.world();
        assert_eq!(world.resource::<lunco_core::ActivePhysicsFrame>().0, site);
        assert_eq!(world.get::<ChildOf>(site).unwrap().parent(), surface_grid);
        assert!(world.get::<Grid>(site).is_some());
        assert_eq!(world.get::<ChildOf>(avatar).unwrap().parent(), site);
        assert_eq!(world.get::<ChildOf>(rigid_body).unwrap().parent(), site);
        assert_eq!(
            world
                .get::<lunco_environment::GravityBody>(avatar)
                .unwrap()
                .body_entity,
            body
        );
        assert_eq!(
            world
                .get::<lunco_environment::GravityBody>(rigid_body)
                .unwrap()
                .body_entity,
            body
        );

        let site_cell = *world.get::<CellCoord>(site).unwrap();
        let site_transform = *world.get::<Transform>(site).unwrap();
        let avatar_cell = *world.get::<CellCoord>(avatar).unwrap();
        let avatar_transform = *world.get::<Transform>(avatar).unwrap();
        app.world_mut()
            .get_mut::<Transform>(body_fixed_grid)
            .unwrap()
            .rotation = Quat::from_rotation_y(0.5);
        app.update();

        let world = app.world();
        assert_eq!(*world.get::<CellCoord>(site).unwrap(), site_cell);
        assert_eq!(*world.get::<Transform>(site).unwrap(), site_transform);
        assert_eq!(*world.get::<CellCoord>(avatar).unwrap(), avatar_cell);
        assert_eq!(*world.get::<Transform>(avatar).unwrap(), avatar_transform);
    }
}
