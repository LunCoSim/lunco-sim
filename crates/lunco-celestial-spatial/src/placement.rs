//! Site-anchored solar hierarchy + celestial-bound entity placement (doc 43
//! §2.6).
//!
//! **Site anchoring**: as soon as the projected scene root has a `SiteAnchor`,
//! it becomes the scene's nested BigSpace grid and the active physics frame.
//! Once the matching celestial body surface grid exists, that same root is
//! atomically re-branched beneath it. During the handoff the authored ENU pose
//! is converted exactly into the body's fixed Cartesian frame; preserving the
//! old world pose would keep ecliptic axes and rotate the ground away from
//! gravity. Keeping the DEM, globe handoff, camera, and surface operations in
//! one body-fixed precision branch avoids an AU-scale hierarchy joint between
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
use big_space::prelude::{CellCoord, Grid};

use lunco_time::CelestialTime;

use lunco_celestial::geo::{
    GeodeticAnchor, LocalTangentFrame, SiteAnchor, body_rotation, equatorial_frame,
    geodetic_to_body_fixed,
};
use lunco_celestial::kepler::KeplerOrbit;
use lunco_celestial::{CelestialBody, CelestialBodyRegistry, ReferenceFrame};
use lunco_celestial_spatial_core::{OrbitalViewPin, ReferenceFrameIndex};

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
    let scene_to_body = tangent.scene_to_frame_rotation();
    (
        tangent.to_frame(scene_position),
        scene_to_body * scene_rotation,
    )
}

/// Attach the site scene to the body's body-fixed surface frame.
///
/// The scene is initially mounted under `WorldGrid` because the USD loader has
/// no celestial knowledge at mount time. As soon as the root's `SiteAnchor` is
/// projected, it becomes a nested BigSpace [`Grid`] and Avian's one stable
/// [`lunco_spatial::ActivePhysicsFrame`]. When the body's rotating surface grid is
/// ready, the same root is atomically migrated beneath it; the active frame
/// does not change. The Moon/Earth rotation remains above it, so celestial
/// motion changes rendering but never rewrites local physics position,
/// velocity, contacts, or joints.
pub fn attach_site_scene_to_surface_grid(
    q_site: Query<(Entity, &GeodeticAnchor, &ChildOf), With<SiteAnchor>>,
    q_bodies: Query<(Entity, &CelestialBody, &crate::globe_lod::GlobeLod)>,
    // Environment probes are physical assembly consumers too: a probe nested
    // under a rigid body samples that body's local environment. Keep the
    // celestial ownership binding on the probe instead of making the
    // environment bridge infer a body from a prim name or a model port.
    q_physical: Query<
        Entity,
        Or<(
            With<avian3d::prelude::RigidBody>,
            With<lunco_environment::EnvironmentProbe>,
        )>,
    >,
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
    grid_config: Res<lunco_spatial::WorldGridConfig>,
    active_physics_frame: Option<Res<lunco_spatial::ActivePhysicsFrame>>,
    mut commands: Commands,
) {
    let Ok((scene_root, anchor, child_of)) = q_site.single() else {
        return;
    };
    let make_site_grid = || grid_config.grid();
    // Establish the authored scene frame as soon as its SiteAnchor is
    // projected. The celestial hierarchy is itself staged: the USD scene can
    // materialise physical descendants before the SolarSystem payload has
    // finished projecting its body declarations. Leaving the root under the
    // WorldGrid during that window lets Avian seed early bodies in a different
    // frame from later bodies, which makes an otherwise valid joint start with
    // astronomical-scale error. The root Grid is the stable semantic frame;
    // its parent is upgraded to the body's surface Grid below when that Grid
    // becomes available, without changing ActivePhysicsFrame.
    if q_grids.get(scene_root).is_err() {
        commands.entity(scene_root).try_insert(make_site_grid());
        stamp_low_precision_roots(scene_root, &q_children, &q_desc_spatial, &mut commands);
    }
    if active_physics_frame.is_none_or(|frame| frame.0 != scene_root) {
        commands.insert_resource(lunco_spatial::ActivePhysicsFrame(scene_root));
    }

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
    let needs_surface_mount = child_of.parent() != body_surface_grid;
    if needs_surface_mount {
        let Some((scene_position, scene_rotation)) = lunco_spatial::coords::grid_relative_pose(
            scene_root,
            child_of.parent(),
            &q_parents,
            &q_grids,
            &q_spatial,
        ) else {
            return;
        };
        let (body_position, body_rotation) =
            site_enu_to_body_fixed_pose(anchor, body.radius_m, scene_position, scene_rotation);
        let (cell, translation) = body_surface_grid_component.translation_to_grid(body_position);
        lunco_spatial::attach::migrate_to_grid(
            &mut commands,
            scene_root,
            body_surface_grid,
            cell,
            Transform::from_translation(translation).with_rotation(body_rotation.as_quat()),
        );
    }

    if needs_surface_mount {
        info!(
            "[celestial] site scene mounted as ENU physics grid {:?} on body surface grid {:?} (body {})",
            scene_root, body_surface_grid, anchor.body
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
/// The SITE-ANCHORED scene is not managed here: the one-time surface-grid
/// attachment places it at its true geodetic point on the anchor body, and the
/// camera flies while the scene stays in that physical frame. Its descendants
/// retain their authored visibility, including hidden render-only templates
/// consumed by runtime drivers.
///
/// Subtlety established by experiment: hiding a scene ROOT is not enough —
/// USD prims spawn with an explicit `Visibility::Visible`, which overrides an
/// ancestor's `Hidden` rather than inheriting it. Every descendant of an
/// unanchored root must be toggled.
#[allow(clippy::type_complexity)]
pub fn orbital_pin_scene_visibility(
    orbital_pin: Res<OrbitalViewPin>,
    q_children: Query<&Children>,
    // Plain local scene roots (no celestial binding).
    q_local: Query<
        Entity,
        (
            With<lunco_spatial::GridAnchor>,
            Without<GeodeticAnchor>,
            Without<KeplerOrbit>,
        ),
    >,
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

    // Collect each unanchored root plus its full subtree — descendants override
    // the root's visibility, so the root alone would leave the ground on screen.
    // Site-anchored content is already in its physical body-fixed frame and is
    // deliberately absent from this ownership boundary: its authored child
    // visibility must not be replaced by the orbital presentation mode.
    let mut targets: Vec<(Entity, Visibility)> = Vec::new();
    let mut stack: Vec<(Entity, Visibility)> = q_local.iter().map(|e| (e, target)).collect();
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
    celestial_time: Res<CelestialTime>,
    registry: Res<CelestialBodyRegistry>,
    frame_index: Res<ReferenceFrameIndex>,
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
    let jd = celestial_time.epoch_jd;
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
        commands
            .entity(entity)
            .try_insert(lunco_spatial::GridAnchor);
        lunco_spatial::attach::migrate_to_grid(
            &mut commands,
            entity,
            grid_entity,
            new_cell,
            Transform {
                translation: new_translation,
                rotation,
                ..default()
            },
        );
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
        // spacecraft/link-beam spawn paths hit and fix the same way
        // (trajectories.rs, celestial_views.rs, link_beams.rs): explicitly stamp the
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

/// Body selection shared by the physical curvature input and the visual globe
/// handoff. Terrain georeferencing is authoritative whenever a DEM exists;
/// only a scene with no DEM uses its single site anchor as the body declaration.
#[derive(Clone, Copy)]
struct TerrainBodySelection {
    body: i32,
    has_dem: bool,
}

enum TerrainBodySelectionError {
    MixedBodies,
    SiteAnchorCardinality(usize),
}

fn select_terrain_body<'g, 'a>(
    terrain_georefs: impl IntoIterator<Item = Option<&'g lunco_terrain_surface::TerrainGeoref>>,
    site_anchors: &[&'a GeodeticAnchor],
) -> Result<TerrainBodySelection, TerrainBodySelectionError> {
    let bodies: std::collections::BTreeSet<i32> = terrain_georefs
        .into_iter()
        .map(|georef| {
            georef.map_or(lunco_terrain_surface::DEFAULT_ANCHOR_BODY, |value| {
                value.body
            })
        })
        .collect();
    match bodies.len() {
        1 => Ok(TerrainBodySelection {
            body: *bodies.first().expect("one body was counted"),
            has_dem: true,
        }),
        n if n > 1 => Err(TerrainBodySelectionError::MixedBodies),
        _ => match site_anchors {
            [anchor] => Ok(TerrainBodySelection {
                body: anchor.body,
                has_dem: false,
            }),
            anchors => Err(TerrainBodySelectionError::SiteAnchorCardinality(
                anchors.len(),
            )),
        },
    }
}

fn terrain_diagnostic(
    producer: &str,
    code: &str,
    subject: String,
    message: String,
) -> lunco_core::RuntimeDiagnostic {
    lunco_core::RuntimeDiagnostic {
        code: code.to_string(),
        severity: lunco_core::DiagnosticSeverity::Error,
        producer: producer.to_string(),
        subject,
        message,
    }
}

fn replace_terrain_diagnostic(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    producer: &str,
    diagnostic: Option<lunco_core::RuntimeDiagnostic>,
) {
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        if let Some(diagnostic) = diagnostic {
            diagnostics.replace_producer(producer, [diagnostic]);
        } else {
            diagnostics.replace_producer(producer, std::iter::empty());
        }
    }
}

/// Publish the celestial radius before a DEM build captures its immutable
/// oracle inputs. This small system is part of the authoritative terrain-build
/// boundary; it does not select or mutate visual globe geometry.
#[derive(bevy::ecs::system::SystemParam)]
pub struct TerrainCurvatureChangeTracker<'w, 's> {
    changed: Query<
        'w,
        's,
        (),
        Or<(
            Changed<GeodeticAnchor>,
            Changed<SiteAnchor>,
            Changed<lunco_terrain_surface::DemHeightField>,
            Changed<lunco_terrain_surface::DemTerrainRequest>,
            Changed<lunco_terrain_surface::TerrainGeoref>,
        )>,
    >,
    removed_site: RemovedComponents<'w, 's, SiteAnchor>,
    removed_anchor: RemovedComponents<'w, 's, GeodeticAnchor>,
    removed_dem: RemovedComponents<'w, 's, lunco_terrain_surface::DemHeightField>,
    removed_request: RemovedComponents<'w, 's, lunco_terrain_surface::DemTerrainRequest>,
    removed_georef: RemovedComponents<'w, 's, lunco_terrain_surface::TerrainGeoref>,
}

impl TerrainCurvatureChangeTracker<'_, '_> {
    fn has_changes(&mut self) -> bool {
        let removed = self.removed_site.read().count()
            + self.removed_anchor.read().count()
            + self.removed_dem.read().count()
            + self.removed_request.read().count()
            + self.removed_georef.read().count();
        !self.changed.is_empty() || removed > 0
    }
}

pub fn sync_terrain_body_curvature(
    mut commands: Commands,
    registry: Res<CelestialBodyRegistry>,
    mut changes: TerrainCurvatureChangeTracker<'_, '_>,
    mut initialized: Local<bool>,
    q_site: Query<&GeodeticAnchor, With<SiteAnchor>>,
    current: Option<Res<lunco_terrain_surface::TerrainBodyCurvature>>,
    q_terrain: Query<
        Option<&lunco_terrain_surface::TerrainGeoref>,
        Or<(
            With<lunco_terrain_surface::DemHeightField>,
            With<lunco_terrain_surface::DemTerrainRequest>,
        )>,
    >,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let inputs_changed = changes.has_changes();
    if *initialized && !registry.is_changed() && !inputs_changed {
        return;
    }
    *initialized = true;

    let site_anchors: Vec<_> = q_site.iter().collect();
    let producer = "celestial-terrain-curvature";
    if site_anchors.is_empty() {
        if current.is_some() {
            commands.remove_resource::<lunco_terrain_surface::TerrainBodyCurvature>();
        }
        replace_terrain_diagnostic(&mut diagnostics, producer, None);
        return;
    }

    let selection = match select_terrain_body(q_terrain.iter(), &site_anchors) {
        Ok(selection) => selection,
        Err(TerrainBodySelectionError::MixedBodies) => {
            if current.is_some() {
                commands.remove_resource::<lunco_terrain_surface::TerrainBodyCurvature>();
            }
            replace_terrain_diagnostic(
                &mut diagnostics,
                producer,
                Some(terrain_diagnostic(
                    producer,
                    "mixed-terrain-body",
                    "TerrainGeoref".to_string(),
                    "one terrain scene cannot curve DEMs against multiple body radii".to_string(),
                )),
            );
            return;
        }
        Err(TerrainBodySelectionError::SiteAnchorCardinality(count)) => {
            if current.is_some() {
                commands.remove_resource::<lunco_terrain_surface::TerrainBodyCurvature>();
            }
            replace_terrain_diagnostic(
                &mut diagnostics,
                producer,
                Some(terrain_diagnostic(
                    producer,
                    "site-anchor-cardinality",
                    "SiteAnchor".to_string(),
                    format!(
                        "terrain without georeferencing requires exactly one SiteAnchor, found {count}"
                    ),
                )),
            );
            return;
        }
    };

    if !selection.has_dem {
        if current.is_some() {
            commands.remove_resource::<lunco_terrain_surface::TerrainBodyCurvature>();
        }
        replace_terrain_diagnostic(&mut diagnostics, producer, None);
        return;
    }
    let Some(body) = registry.get(selection.body) else {
        if current.is_some() {
            commands.remove_resource::<lunco_terrain_surface::TerrainBodyCurvature>();
        }
        replace_terrain_diagnostic(
            &mut diagnostics,
            producer,
            Some(terrain_diagnostic(
                producer,
                "terrain-body-missing",
                format!("CelestialBody({})", selection.body),
                format!(
                    "terrain references body {}, which is not in the active celestial registry",
                    selection.body
                ),
            )),
        );
        return;
    };

    if current.is_none_or(|curvature| curvature.radius_m != body.radius_m) {
        commands.insert_resource(lunco_terrain_surface::TerrainBodyCurvature {
            radius_m: body.radius_m,
        });
        debug!(
            "terrain anchored to body {}: DEM terrain curves to sphere radius {:.0} m",
            selection.body, body.radius_m
        );
    }
    replace_terrain_diagnostic(&mut diagnostics, producer, None);
}

/// Change tracking for presentation-side DEM-to-globe handoff construction.
#[derive(bevy::ecs::system::SystemParam)]
pub struct TerrainHandoffChangeTracker<'w, 's> {
    changed: Query<
        'w,
        's,
        (),
        Or<(
            Changed<GeodeticAnchor>,
            Changed<SiteAnchor>,
            Changed<lunco_terrain_surface::DemHeightField>,
            Changed<lunco_terrain_surface::DemTerrainRequest>,
            Changed<lunco_terrain_surface::TerrainGeoref>,
            Changed<lunco_terrain_surface::FlatSiteSurface>,
            Changed<CelestialBody>,
            Changed<crate::globe_lod::GlobeLod>,
        )>,
    >,
    removed_site: RemovedComponents<'w, 's, SiteAnchor>,
    removed_anchor: RemovedComponents<'w, 's, GeodeticAnchor>,
    removed_dem: RemovedComponents<'w, 's, lunco_terrain_surface::DemHeightField>,
    removed_request: RemovedComponents<'w, 's, lunco_terrain_surface::DemTerrainRequest>,
    removed_georef: RemovedComponents<'w, 's, lunco_terrain_surface::TerrainGeoref>,
    removed_flat: RemovedComponents<'w, 's, lunco_terrain_surface::FlatSiteSurface>,
    removed_body: RemovedComponents<'w, 's, CelestialBody>,
    removed_lod: RemovedComponents<'w, 's, crate::globe_lod::GlobeLod>,
}

impl TerrainHandoffChangeTracker<'_, '_> {
    fn has_changes(&mut self) -> bool {
        let removed = self.removed_site.read().count()
            + self.removed_anchor.read().count()
            + self.removed_dem.read().count()
            + self.removed_request.read().count()
            + self.removed_georef.read().count()
            + self.removed_flat.read().count()
            + self.removed_body.read().count()
            + self.removed_lod.read().count();
        !self.changed.is_empty() || removed > 0
    }
}

fn clear_globe_handoffs(
    commands: &mut Commands<'_, '_>,
    globes: &mut Query<
        '_,
        '_,
        (
            Entity,
            &CelestialBody,
            Option<&crate::globe_lod::GlobeHandoff>,
            Option<&mut crate::globe_lod::GlobeHandoffPreparation>,
        ),
    >,
    mut should_clear: impl FnMut(&CelestialBody) -> bool,
) {
    for (entity, globe, handoff, preparation) in globes.iter_mut() {
        if !should_clear(globe) {
            continue;
        }
        let mut entity_commands = commands.entity(entity);
        if handoff.is_some() {
            entity_commands.remove::<crate::globe_lod::GlobeHandoff>();
        }
        if preparation
            .as_ref()
            .is_some_and(|preparation| preparation.is_complete())
        {
            entity_commands.remove::<crate::globe_lod::GlobeHandoffPreparation>();
        }
    }
}

/// Prepare the measured DEM-to-globe collar from the visualization cycle.
/// Boundary statistics and slope sampling run on the compute pool; this system
/// only polls completed work and commits a result whose input key is still current.
pub(crate) fn sync_globe_handoffs(
    mut commands: Commands,
    registry: Res<CelestialBodyRegistry>,
    mut changes: TerrainHandoffChangeTracker<'_, '_>,
    mut initialized: Local<bool>,
    q_site: Query<&GeodeticAnchor, With<SiteAnchor>>,
    q_terrain: Query<
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
    q_flat: Query<&lunco_terrain_surface::FlatSiteSurface>,
    mut q_globes: Query<(
        Entity,
        &CelestialBody,
        Option<&crate::globe_lod::GlobeHandoff>,
        Option<&mut crate::globe_lod::GlobeHandoffPreparation>,
    )>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    for (_, _, _, preparation) in &mut q_globes {
        if let Some(mut preparation) = preparation {
            preparation.poll();
        }
    }
    let has_preparation = q_globes
        .iter_mut()
        .any(|(_, _, _, preparation)| preparation.is_some());
    let inputs_changed = changes.has_changes();
    if *initialized && !registry.is_changed() && !inputs_changed && !has_preparation {
        return;
    }
    *initialized = true;
    let producer = "celestial-terrain-handoff";
    replace_terrain_diagnostic(&mut diagnostics, producer, None);

    let site_anchors: Vec<_> = q_site.iter().collect();
    if site_anchors.is_empty() {
        clear_globe_handoffs(&mut commands, &mut q_globes, |_| true);
        return;
    }

    let terrain_georefs: Vec<_> = q_terrain.iter().collect();
    let selection = match select_terrain_body(terrain_georefs.iter().copied(), &site_anchors) {
        Ok(selection) => selection,
        Err(TerrainBodySelectionError::MixedBodies) => {
            clear_globe_handoffs(&mut commands, &mut q_globes, |_| true);
            replace_terrain_diagnostic(
                &mut diagnostics,
                producer,
                Some(terrain_diagnostic(
                    producer,
                    "mixed-terrain-body",
                    "TerrainGeoref".to_string(),
                    "one terrain scene cannot join DEMs authored against multiple bodies"
                        .to_string(),
                )),
            );
            return;
        }
        Err(TerrainBodySelectionError::SiteAnchorCardinality(count)) => {
            clear_globe_handoffs(&mut commands, &mut q_globes, |_| true);
            replace_terrain_diagnostic(
                &mut diagnostics,
                producer,
                Some(terrain_diagnostic(
                    producer,
                    "site-anchor-cardinality",
                    "SiteAnchor".to_string(),
                    format!(
                        "terrain without georeferencing requires exactly one SiteAnchor, found {count}"
                    ),
                )),
            );
            return;
        }
    };
    let body = selection.body;
    let Some(desc) = registry.get(body) else {
        clear_globe_handoffs(&mut commands, &mut q_globes, |_| true);
        replace_terrain_diagnostic(
            &mut diagnostics,
            producer,
            Some(terrain_diagnostic(
                producer,
                "terrain-body-missing",
                format!("CelestialBody({body})"),
                format!(
                    "terrain references body {body}, which is not in the active celestial registry"
                ),
            )),
        );
        return;
    };

    let flat_surface = if selection.has_dem {
        None
    } else {
        match q_flat.iter().collect::<Vec<_>>().as_slice() {
            [surface] if surface.is_valid() => {
                let square = (surface.half_extent_x_m - surface.half_extent_z_m).abs()
                    <= 1.0e-5_f64.max(surface.half_extent_x_m * 1.0e-6);
                let centered =
                    surface.center_x_m.abs() <= 1.0e-5 && surface.center_z_m.abs() <= 1.0e-5;
                if square && centered {
                    Some(**surface)
                } else {
                    replace_terrain_diagnostic(
                        &mut diagnostics,
                        producer,
                        Some(terrain_diagnostic(
                            producer,
                            "flat-surface-contract",
                            "FlatSiteSurface".to_string(),
                            "flat-site surface must be a square Plane centered at the site ENU origin".to_string(),
                        )),
                    );
                    None
                }
            }
            [] => {
                replace_terrain_diagnostic(
                    &mut diagnostics,
                    producer,
                    Some(terrain_diagnostic(
                        producer,
                        "flat-surface-missing",
                        "SiteAnchor".to_string(),
                        "site-anchored non-DEM terrain requires exactly one terrain prim with lunco:terrain:surfaceRole=\"flat-site\"".to_string(),
                    )),
                );
                None
            }
            _ => {
                replace_terrain_diagnostic(
                    &mut diagnostics,
                    producer,
                    Some(terrain_diagnostic(
                        producer,
                        "flat-surface-cardinality",
                        "FlatSiteSurface".to_string(),
                        "site-anchored non-DEM terrain requires exactly one flat-site surface owner".to_string(),
                    )),
                );
                None
            }
        }
    };
    if !selection.has_dem && flat_surface.is_none() {
        clear_globe_handoffs(&mut commands, &mut q_globes, |_| true);
        return;
    }

    let candidates: Vec<_> = q_built_dem
        .iter()
        .filter(|(_, georef)| {
            georef.map_or(lunco_terrain_surface::DEFAULT_ANCHOR_BODY, |value| {
                value.body
            }) == body
        })
        .collect();
    if candidates.len() > 1 {
        clear_globe_handoffs(&mut commands, &mut q_globes, |globe| {
            globe.ephemeris_id == body
        });
        replace_terrain_diagnostic(
            &mut diagnostics,
            producer,
            Some(terrain_diagnostic(
                producer,
                "multiple-dem-crops",
                format!("CelestialBody({body})"),
                format!(
                    "{} built DEM crops target body {body}; the globe handoff requires one active crop",
                    candidates.len()
                ),
            )),
        );
        return;
    }
    let selected_dem = candidates.into_iter().next();
    let half_extent = selected_dem.map_or_else(
        || flat_surface.map_or(0.0, |surface| surface.half_extent_x_m),
        |(dem, _)| dem.0.half_extent() as f64,
    );
    let oracle = selected_dem.map(|(dem, _)| dem.0.clone());
    let matching_anchors: Vec<_> = site_anchors
        .iter()
        .copied()
        .filter(|anchor| anchor.body == body)
        .collect();
    let anchor = match matching_anchors.as_slice() {
        [anchor] => Some(*anchor),
        [] => {
            replace_terrain_diagnostic(
                &mut diagnostics,
                producer,
                Some(terrain_diagnostic(
                    producer,
                    "site-anchor-body",
                    "SiteAnchor".to_string(),
                    format!("terrain body {body} has no matching SiteAnchor for globe handoff"),
                )),
            );
            None
        }
        _ => {
            replace_terrain_diagnostic(
                &mut diagnostics,
                producer,
                Some(terrain_diagnostic(
                    producer,
                    "site-anchor-body",
                    "SiteAnchor".to_string(),
                    format!(
                        "terrain body {body} requires exactly one matching SiteAnchor, found {}",
                        matching_anchors.len()
                    ),
                )),
            );
            None
        }
    };

    for (entity, globe, handoff, preparation) in &mut q_globes {
        let mut entity_commands = commands.entity(entity);
        if globe.ephemeris_id != body {
            if handoff.is_some() {
                entity_commands.remove::<crate::globe_lod::GlobeHandoff>();
            }
            if preparation
                .as_ref()
                .is_some_and(|preparation| preparation.is_complete())
            {
                entity_commands.remove::<crate::globe_lod::GlobeHandoffPreparation>();
            }
            continue;
        }
        if half_extent <= 0.0 || half_extent >= desc.radius_m {
            if handoff.is_some() {
                entity_commands.remove::<crate::globe_lod::GlobeHandoff>();
            }
            if preparation
                .as_ref()
                .is_some_and(|preparation| preparation.is_complete())
            {
                entity_commands.remove::<crate::globe_lod::GlobeHandoffPreparation>();
            }
            continue;
        }
        let Some(anchor) = anchor else {
            if handoff.is_some() {
                entity_commands.remove::<crate::globe_lod::GlobeHandoff>();
            }
            if preparation
                .as_ref()
                .is_some_and(|preparation| preparation.is_complete())
            {
                entity_commands.remove::<crate::globe_lod::GlobeHandoffPreparation>();
            }
            continue;
        };

        let tangent = LocalTangentFrame::body_fixed(&anchor.geodetic, desc.radius_m);
        if let Some(oracle) = oracle.clone() {
            let input_key = crate::globe_lod::GlobeHandoff::dem_input_key(
                tangent.up,
                tangent.east,
                tangent.north,
                desc.radius_m,
                &oracle,
                half_extent,
            );
            if handoff.is_some_and(|handoff| handoff.matches_dem_input(input_key)) {
                if preparation
                    .as_ref()
                    .is_some_and(|preparation| preparation.is_complete())
                {
                    entity_commands.remove::<crate::globe_lod::GlobeHandoffPreparation>();
                }
                replace_terrain_diagnostic(&mut diagnostics, producer, None);
                continue;
            }

            if let Some(mut preparation) = preparation {
                if preparation.input_key() == input_key {
                    let Some(result) = preparation.take_result() else {
                        continue;
                    };
                    entity_commands.remove::<crate::globe_lod::GlobeHandoffPreparation>();
                    match result {
                        Ok(next) => {
                            let collar_m = next.collar_m;
                            commands.entity(entity).try_insert(next);
                            debug!(
                                "globe handoff composed at site body {body} (footprint ±{half_extent:.0} m, measured-source collar {collar_m:.0} m)"
                            );
                            replace_terrain_diagnostic(&mut diagnostics, producer, None);
                        }
                        Err(reason) => {
                            if handoff.is_some() {
                                commands
                                    .entity(entity)
                                    .remove::<crate::globe_lod::GlobeHandoff>();
                            }
                            replace_terrain_diagnostic(
                                &mut diagnostics,
                                producer,
                                Some(terrain_diagnostic(
                                    producer,
                                    "dem-handoff-invalid",
                                    "DemHeightField".to_string(),
                                    format!(
                                        "cannot join this cropped DEM to the body sphere: {reason}"
                                    ),
                                )),
                            );
                        }
                    }
                } else if preparation.is_complete() {
                    entity_commands
                        .remove::<crate::globe_lod::GlobeHandoffPreparation>()
                        .insert(crate::globe_lod::GlobeHandoffPreparation::spawn_dem(
                            input_key,
                            tangent.up,
                            tangent.east,
                            tangent.north,
                            desc.radius_m,
                            oracle,
                            half_extent,
                        ));
                }
                continue;
            }

            entity_commands.insert(crate::globe_lod::GlobeHandoffPreparation::spawn_dem(
                input_key,
                tangent.up,
                tangent.east,
                tangent.north,
                desc.radius_m,
                oracle,
                half_extent,
            ));
            continue;
        }

        if let Some(preparation) = preparation
            && preparation.is_complete()
        {
            entity_commands.remove::<crate::globe_lod::GlobeHandoffPreparation>();
        }
        let Some(surface) = flat_surface else {
            continue;
        };
        let next = crate::globe_lod::GlobeHandoff::new_flat(
            tangent.up,
            tangent.east,
            tangent.north,
            desc.radius_m,
            anchor.geodetic.height_m + surface.top_y_m,
            half_extent,
        );
        if handoff != Some(&next) {
            let collar_m = next.collar_m;
            commands.entity(entity).try_insert(next);
            debug!(
                "flat globe handoff composed at site body {body} (footprint ±{half_extent:.0} m, collar {collar_m:.0} m)"
            );
        }
        replace_terrain_diagnostic(&mut diagnostics, producer, None);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use lunco_celestial::geo::{Geodetic, solar_tangent_frame};

    /// The align quaternion maps the site ENU axes onto the scene axes.
    #[test]
    fn align_rotation_maps_enu_to_scene_axes() {
        let registry = CelestialBodyRegistry::default_system();
        let desc = registry
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == lunco_celestial::ephemeris_id::MOON)
            .unwrap();
        let center = DVec3::new(1.0e11, 2.0e10, -3.0e10);
        let geo = Geodetic::new(-89.45, -136.7, 1200.0);
        let frame = solar_tangent_frame(desc, &geo, center, 2461000.5);
        let align = frame.frame_to_scene_rotation();
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
            .find(|b| b.ephemeris_id == lunco_celestial::ephemeris_id::MOON)
            .unwrap();
        let anchor = GeodeticAnchor {
            body: lunco_celestial::ephemeris_id::MOON,
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
    fn orbital_visibility_preserves_authored_site_descendant_visibility() {
        let mut app = App::new();
        app.insert_resource(OrbitalViewPin {
            active: true,
            body: lunco_celestial::ephemeris_id::MOON,
            ..default()
        });
        app.add_systems(Update, orbital_pin_scene_visibility);

        let site = app
            .world_mut()
            .spawn((
                SiteAnchor,
                GeodeticAnchor {
                    body: lunco_celestial::ephemeris_id::MOON,
                    geodetic: Geodetic::new(25.28, 307.60, 0.0),
                },
                lunco_spatial::GridAnchor,
                Visibility::Visible,
            ))
            .id();
        let hidden_template = app
            .world_mut()
            .spawn((Visibility::Hidden, ChildOf(site)))
            .id();
        let visible_mesh = app
            .world_mut()
            .spawn((Visibility::Visible, ChildOf(site)))
            .id();

        app.update();

        let world = app.world();
        assert_eq!(
            *world.get::<Visibility>(hidden_template).unwrap(),
            Visibility::Hidden,
            "orbital presentation must not expose a site-owned render template"
        );
        assert_eq!(
            *world.get::<Visibility>(visible_mesh).unwrap(),
            Visibility::Visible,
            "orbital presentation must not rewrite authored site visibility"
        );
    }

    #[test]
    fn site_scene_and_globe_lod_share_the_body_fixed_surface_grid() {
        let mut app = App::new();
        app.insert_resource(lunco_spatial::WorldGridConfig::default());
        app.add_systems(Update, attach_site_scene_to_surface_grid);

        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                lunco_spatial::WorldGrid,
            ))
            .id();
        let body_fixed_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(body_fixed_grid),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn((
                CelestialBody {
                    name: "Moon".into(),
                    ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                    radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
                },
                crate::globe_lod::GlobeLod {
                    radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
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
                    body: lunco_celestial::ephemeris_id::MOON,
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
        app.update();
        app.update();

        let world = app.world();
        assert_eq!(
            world.resource::<lunco_spatial::ActivePhysicsFrame>().0,
            site
        );
        assert_eq!(world.get::<ChildOf>(site).unwrap().parent(), surface_grid);
        let lod = world.get::<crate::globe_lod::GlobeLod>(body).unwrap();
        assert_eq!(lod.surface_grid, surface_grid);
        assert!(world.get::<Grid>(lod.surface_grid).is_some());
        assert!(world.get::<Grid>(site).is_some());
        assert_eq!(world.get::<ChildOf>(rigid_body).unwrap().parent(), site);
        assert_eq!(
            world
                .get::<lunco_environment::GravityBody>(rigid_body)
                .unwrap()
                .body_entity,
            body
        );

        let site_cell = *world.get::<CellCoord>(site).unwrap();
        let site_transform = *world.get::<Transform>(site).unwrap();
        app.world_mut()
            .get_mut::<Transform>(body_fixed_grid)
            .unwrap()
            .rotation = Quat::from_rotation_y(0.5);
        app.update();

        let world = app.world();
        assert_eq!(*world.get::<CellCoord>(site).unwrap(), site_cell);
        assert_eq!(*world.get::<Transform>(site).unwrap(), site_transform);
    }

    #[test]
    fn site_frame_is_stable_while_celestial_hierarchy_loads() {
        let mut app = App::new();
        app.insert_resource(lunco_spatial::WorldGridConfig::default());
        app.add_systems(Update, attach_site_scene_to_surface_grid);

        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                lunco_spatial::WorldGrid,
            ))
            .id();
        let site = app
            .world_mut()
            .spawn((
                SiteAnchor,
                GeodeticAnchor {
                    body: lunco_celestial::ephemeris_id::MOON,
                    geodetic: Geodetic::new(25.28, 307.60, 0.0),
                },
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(world_grid),
            ))
            .id();

        // The USD scene root is available before the celestial payload creates
        // the body and its surface grid. It must still become the one local
        // physics frame immediately, so early and late rigid bodies share it.
        app.update();
        assert!(app.world().get::<Grid>(site).is_some());
        assert_eq!(
            app.world()
                .resource::<lunco_spatial::ActivePhysicsFrame>()
                .0,
            site
        );
        assert_eq!(
            app.world().get::<ChildOf>(site).unwrap().parent(),
            world_grid
        );

        let body_fixed_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(body_fixed_grid),
            ))
            .id();
        app.world_mut().spawn((
            CelestialBody {
                name: "Moon".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
            },
            crate::globe_lod::GlobeLod {
                radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
                surface_grid,
                look: lunco_materials::ShaderLook::new("shaders/blueprint.wgsl"),
                res: 8,
                max_lod: 1,
                lod_distance_factor: 1.0,
            },
        ));

        app.update();
        assert_eq!(
            app.world().get::<ChildOf>(site).unwrap().parent(),
            surface_grid
        );
        assert_eq!(
            app.world()
                .resource::<lunco_spatial::ActivePhysicsFrame>()
                .0,
            site
        );
    }

    #[test]
    fn site_frame_is_published_before_declared_surface_grid_is_ready() {
        let mut app = App::new();
        app.insert_resource(lunco_spatial::WorldGridConfig::default());
        app.add_systems(Update, attach_site_scene_to_surface_grid);

        let world_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                lunco_spatial::WorldGrid,
            ))
            .id();
        let surface_grid = app.world_mut().spawn(ChildOf(world_grid)).id();
        let site = app
            .world_mut()
            .spawn((
                SiteAnchor,
                GeodeticAnchor {
                    body: lunco_celestial::ephemeris_id::MOON,
                    geodetic: Geodetic::new(25.28, 307.60, 0.0),
                },
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(world_grid),
            ))
            .id();
        app.world_mut().spawn((
            CelestialBody {
                name: "Moon".into(),
                ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
            },
            crate::globe_lod::GlobeLod {
                radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
                surface_grid,
                look: lunco_materials::ShaderLook::new("shaders/blueprint.wgsl"),
                res: 8,
                max_lod: 1,
                lod_distance_factor: 1.0,
            },
        ));

        // The body declaration exists, but its surface Grid is still being
        // projected. The site frame must already own the physics boundary.
        app.update();
        assert!(app.world().get::<Grid>(site).is_some());
        assert_eq!(
            app.world()
                .resource::<lunco_spatial::ActivePhysicsFrame>()
                .0,
            site
        );
        assert_eq!(
            app.world().get::<ChildOf>(site).unwrap().parent(),
            world_grid
        );

        app.world_mut().entity_mut(surface_grid).insert((
            lunco_spatial::WorldGridConfig::default().grid(),
            CellCoord::ZERO,
            Transform::default(),
        ));
        app.update();
        assert_eq!(
            app.world().get::<ChildOf>(site).unwrap().parent(),
            surface_grid
        );
        assert_eq!(
            app.world()
                .resource::<lunco_spatial::ActivePhysicsFrame>()
                .0,
            site
        );
    }

    #[test]
    fn site_placement_leaves_camera_frame_ownership_alone() {
        let mut app = App::new();
        app.insert_resource(lunco_spatial::WorldGridConfig::default());
        app.add_systems(Update, attach_site_scene_to_surface_grid);

        let world_grid = app.world_mut().spawn(lunco_spatial::WorldGrid).id();
        let body_fixed_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let surface_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(body_fixed_grid),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn((
                CelestialBody {
                    name: "Moon".into(),
                    ephemeris_id: lunco_celestial::ephemeris_id::MOON,
                    radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
                },
                crate::globe_lod::GlobeLod {
                    radius_m: lunco_celestial::MOON_MEAN_RADIUS_M,
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
                    body: lunco_celestial::ephemeris_id::MOON,
                    geodetic: Geodetic::new(25.28, 307.60, 0.0),
                },
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
                lunco_spatial::WorldGridConfig::default().grid(),
                ChildOf(surface_grid),
            ))
            .id();
        let camera_grid = app
            .world_mut()
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::ZERO,
                Transform::default(),
                ChildOf(body_fixed_grid),
            ))
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                lunco_embodiment_core::roles::Embodiment,
                CellCoord::ZERO,
                Transform::from_xyz(10.0, 20.0, -30.0),
                GlobalTransform::default(),
                ChildOf(camera_grid),
            ))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<ChildOf>(avatar).unwrap().parent(),
            camera_grid
        );
        assert!(
            app.world()
                .get::<lunco_environment::GravityBody>(avatar)
                .is_none()
        );
        assert_eq!(
            app.world()
                .resource::<lunco_spatial::ActivePhysicsFrame>()
                .0,
            site
        );
        assert!(app.world().get::<CelestialBody>(body).is_some());
        assert!(
            app.world()
                .get::<lunco_spatial::WorldGrid>(world_grid)
                .is_some()
        );
    }
}
