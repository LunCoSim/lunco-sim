//! Site-anchored solar hierarchy + celestial-bound entity placement (doc 43
//! §2.6).
//!
//! **Site anchoring**: the site scene is re-branched under its body's surface
//! grid once that frame exists. During the handoff the authored ENU pose is
//! converted exactly into the body's fixed Cartesian frame; preserving the old
//! world pose would keep ecliptic axes and rotate the ground away from gravity.
//! Keeping the DEM, globe handoff, camera, and surface operations in one
//! body-fixed precision branch avoids an AU-scale hierarchy joint between
//! moving surface pieces. The solar grid remains inertial and is never re-posed
//! to make the site coincide with the world origin. The caller applies the one
//! shared celestial solve gate; this module has no private epoch gate.
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

use crate::geo::{
    body_rotation, equatorial_frame, geodetic_to_body_fixed, GeodeticAnchor, LocalTangentFrame,
    SiteAnchor,
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
/// The USD loader mounts a site scene under the canonical world grid before
/// celestial handoff. Its authored coordinates are already local ENU values;
/// composing them through the full solar hierarchy would mix that semantic
/// frame with BigSpace storage cells. The helper stops at the direct parent
/// grid, after which the site-anchor conversion changes semantic frames.
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
/// The SITE-ANCHORED scene is the opposite case and stays VISIBLE: the one-time
/// surface-grid attachment places it at its true geodetic point on the anchor body, and under
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
    // locals toggle with the mode; the site-anchored subtree is mounted onto its
    // body surface Grid and is force-VISIBLE every pass (also self-heals scenes hidden by the
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
    use crate::geo::{solar_tangent_frame, Geodetic};

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
