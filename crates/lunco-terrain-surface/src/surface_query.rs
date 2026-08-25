//! `GridSurfaceQuery` — the ONE way to ask the analytic terrain where the ground is.
//!
//! # Why this type exists
//!
//! The DEM oracle answers in the terrain owner's **local** frame. Streamed tiles
//! apply that owner's complete pose into its nearest BigSpace grid. This query
//! performs the same conversion into the one [`lunco_core::ActivePhysicsFrame`]
//! before exposing a result. Screen tools therefore cannot accidentally use the
//! persistent `WorldGrid` while a site scene lives in a rotating lunar branch.
//!
//! Three call sites had to know that, and each rediscovered it separately:
//!
//! - `query::TerrainHeightProvider` — got it right, after a fix.
//! - `stream_viz::dem_ground_height` — got it right, after a fix, but kept an
//!   ignored `&GlobalTransform` parameter that still advertised the wrong idea.
//! - `lunco-luncosim-edit`'s spawn ghost — kept a private copy that inverted the
//!   terrain's **render** `GlobalTransform` and marched the oracle in that frame.
//!
//! The third one is why a rover placed at Apollo 15 (a real site ~1.9 km below
//! the body datum) got no ghost and no waypoints away from the rover: the
//! analytic surface was computed one whole `big_space` cell underground, so
//! every cursor ray missed it and placement silently fell back to the collider
//! ring — which only exists in a small band around dynamic bodies.
//!
//! # The fix is structural, not documentary
//!
//! This param **does not expose a `GlobalTransform`**. Its query cannot see one.
//! The round-trip that caused the bug is not merely discouraged here, it is
//! unwritable — the same trick [`lunco_core::coords`] used to make render-vs-grid
//! mixing a compile error, and [`lunco_physics::GridSpatialQuery`] used to make
//! "raycast in the wrong frame" a thing you have to opt into by name.
//!
//! Everything that needs the analytic surface — placement preview, committed
//! spawn, waypoints, the scripting/HTTP query providers — goes through here.

use bevy::ecs::system::SystemParam;
use bevy::math::{DQuat, DVec3, Dir3};
use bevy::prelude::*;
use lunco_core::coords::{GridPos, RenderPos};
use lunco_terrain_core::{normal_at_bounded, HeightSource};

use crate::stream_viz::DemHeightField;

/// A terrain surface hit: the point ON the surface, in the grid frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceHit {
    /// Point on the terrain surface in [`Self::frame`].
    pub point: GridPos,
    /// Concrete BigSpace frame in which [`Self::point`] is expressed.
    pub frame: Entity,
    /// Distance from the ray origin to [`Self::point`]. Frame-independent, so it
    /// is directly comparable with a physics [`avian3d`] hit distance.
    pub distance: f64,
    /// Which terrain answered — for diagnostics and for "who owns this ground?".
    pub terrain: Entity,
}

/// A footprint fitted to the surface: where a body of a given size rests, and
/// how it is oriented by the slope under it.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceFit {
    /// Grid-absolute contact point at the footprint centre (mean corner height).
    pub point: GridPos,
    /// Up vector of the fitted plane.
    pub normal: DVec3,
    /// Orientation with `normal` as up and the requested forward projected onto
    /// the plane.
    pub rotation: Quat,
}

/// Derived rigid pose of one DEM oracle in the active physics frame.
///
/// Published in `PreUpdate` by the terrain plugin. Keeping this frame boundary
/// as data means [`GridSurfaceQuery`] does not need a read of every scene
/// `Transform`; editor systems can therefore move their own preview entities
/// while querying terrain without an ECS aliasing conflict.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct TerrainPoseInPhysicsFrame {
    pub frame: Entity,
    pub position: DVec3,
    pub rotation: DQuat,
}

/// Project each terrain owner's full hierarchy into the one active physics
/// frame. This is continuous spatial state: a terrain may live in a moving
/// nested grid, so the derived pose is refreshed before input every frame and
/// only written when its value actually changes.
pub fn update_terrain_physics_frame_poses(
    active_frame: Option<Res<lunco_core::ActivePhysicsFrame>>,
    terrains: Query<(Entity, Option<&TerrainPoseInPhysicsFrame>), With<DemHeightField>>,
    parents: Query<&ChildOf>,
    grids: Query<&big_space::prelude::Grid>,
    spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    mut commands: Commands,
) {
    let Some(frame) = active_frame.as_deref().map(|frame| frame.0) else {
        for (terrain, current) in &terrains {
            if current.is_some() {
                commands
                    .entity(terrain)
                    .try_remove::<TerrainPoseInPhysicsFrame>();
            }
        }
        return;
    };

    for (terrain, current) in &terrains {
        let Some((position, rotation)) =
            lunco_core::coords::pose_in_grid(terrain, frame, &parents, &grids, &spatial)
        else {
            if current.is_some() {
                commands
                    .entity(terrain)
                    .try_remove::<TerrainPoseInPhysicsFrame>();
            }
            continue;
        };
        let next = TerrainPoseInPhysicsFrame {
            frame,
            position,
            rotation,
        };
        if current != Some(&next) {
            commands.entity(terrain).try_insert(next);
        }
    }
}

/// The analytic terrain surface, queried in the grid frame.
///
/// See the module docs: this deliberately holds NO `GlobalTransform`.
#[derive(SystemParam)]
pub struct GridSurfaceQuery<'w, 's> {
    all_terrains: Query<'w, 's, (), With<DemHeightField>>,
    terrains: Query<
        'w,
        's,
        (
            Entity,
            &'static DemHeightField,
            &'static TerrainPoseInPhysicsFrame,
        ),
    >,
    active_frame: Option<Res<'w, lunco_core::ActivePhysicsFrame>>,
    grids: Query<'w, 's, &'static big_space::prelude::Grid>,
}

impl GridSurfaceQuery<'_, '_> {
    fn frame(&self) -> Option<(Entity, &big_space::prelude::Grid)> {
        let entity = self.active_frame.as_deref()?.0;
        Some((entity, self.grids.get(entity).ok()?))
    }

    /// Is there any analytic terrain at all? A scene may legitimately have none
    /// (orbital, an empty stage, a pure-prop sandbox) — callers fall back to
    /// physics colliders, but must be able to tell "no terrain here" from "the
    /// terrain said no", which are different bugs.
    pub fn has_terrain(&self) -> bool {
        !self.all_terrains.is_empty()
    }

    /// Convert a render-space point into the grid frame — the boundary crossing
    /// every screen-space tool makes exactly once, at the top.
    pub fn to_grid(&self, render_point: RenderPos) -> Option<GridPos> {
        let (_, grid) = self.frame()?;
        Some(lunco_core::coords::render_to_grid_absolute(
            grid,
            render_point,
        ))
    }

    /// Convert a complete render-space ray into the active physics frame.
    ///
    /// A point conversion alone is insufficient: a nested site grid may be
    /// rotated relative to the render frame, so carrying the render direction
    /// across unchanged makes terrain and Avian casts diverge. Screen tools use
    /// this once and keep both values together for every downstream query.
    pub fn ray_to_grid(&self, origin: RenderPos, direction: Dir3) -> Option<(GridPos, Dir3)> {
        let (_, grid) = self.frame()?;
        let origin = lunco_core::coords::render_to_grid_absolute(grid, origin);
        let inverse = grid.local_floating_origin().grid_transform().inverse();
        let direction = inverse
            .transform_vector3(direction.as_dvec3())
            .normalize_or_zero();
        Some((origin, Dir3::new(direction.as_vec3()).ok()?))
    }

    /// Surface height (grid-absolute Y) under a grid-absolute point, from
    /// whichever DEM covers it. `None` when no terrain covers this (x, z).
    ///
    /// The point's own Y is ignored: this asks "how high is the ground here?",
    /// not "is this point above it?".
    pub fn height_at(&self, p: GridPos) -> Option<f64> {
        self.sample(p).map(|(_, y)| y)
    }

    /// Slope angle under a grid-absolute point, in radians.
    ///
    /// This keeps seminar/runtime diagnostics on the same analytic terrain
    /// authority as placement and route planning; render-overlay normals are
    /// view/LOD dependent and are not suitable for a measured grade.
    pub fn slope_at(&self, p: GridPos, eps: f64) -> Option<f64> {
        let (frame, _) = self.frame()?;
        self.terrains.iter().find_map(|(_entity, terrain, pose)| {
            if pose.frame != frame {
                return None;
            }
            let (position, rotation) = (pose.position, pose.rotation);
            let local = rotation.inverse() * (p.0 - position);
            height_in_footprint(&terrain.0, GridPos(local)).map(|_| {
                let half = terrain.0.half_extent() as f64;
                let normal =
                    normal_at_bounded(terrain.0.as_ref(), local.x, local.z, eps.max(1.0e-6), half);
                let normal = rotation * DVec3::from_array(normal);
                normal
                    .normalize_or_zero()
                    .dot(DVec3::Y)
                    .clamp(-1.0, 1.0)
                    .acos()
            })
        })
    }

    /// [`Self::height_at`] plus which terrain answered.
    pub fn sample(&self, p: GridPos) -> Option<(Entity, f64)> {
        let (frame, _) = self.frame()?;
        for (entity, hf, pose) in self.terrains.iter() {
            if pose.frame != frame {
                continue;
            }
            let (position, rotation) = (pose.position, pose.rotation);
            let local = rotation.inverse() * (p.0 - position);
            if let Some(y) = height_in_footprint(&hf.0, GridPos(local)) {
                let point = position + rotation * DVec3::new(local.x, y, local.z);
                return Some((entity, point.y));
            }
        }
        None
    }

    /// Nearest analytic surface hit along a grid-absolute ray, across all DEMs.
    ///
    /// This — not a physics raycast — is the ground truth for placement UI: the
    /// terrain's colliders exist only in a streamed ring around dynamic bodies
    /// and are band-limited below the drawn micro-relief, so a collider cast over
    /// open ground either misses entirely or reports a surface above the drawn
    /// one.
    pub fn raycast(
        &self,
        origin: GridPos,
        direction: Dir3,
        max_distance: f64,
    ) -> Option<SurfaceHit> {
        if !origin.0.is_finite() {
            return None;
        }
        let (frame, _) = self.frame()?;
        let frame_direction = direction.as_dvec3();
        let mut best: Option<SurfaceHit> = None;
        for (entity, hf, pose) in self.terrains.iter() {
            if pose.frame != frame {
                continue;
            }
            let (terrain_position, terrain_rotation) = (pose.position, pose.rotation);
            let inverse = terrain_rotation.inverse();
            let local_origin = inverse * (origin.0 - terrain_position);
            let local_direction = inverse * frame_direction;
            let Some(local_point) =
                crate::oracle::raycast_surface(&hf.0, local_origin, local_direction, max_distance)
            else {
                continue;
            };
            let point = terrain_position + terrain_rotation * local_point;
            let distance = (point - origin.0).length();
            if best.is_none_or(|b| distance < b.distance) {
                best = Some(SurfaceHit {
                    point: GridPos(point),
                    frame,
                    distance,
                    terrain: entity,
                });
            }
        }
        best
    }

    /// Ray from a render-space origin (a camera/cursor ray) against the analytic
    /// surface. The origin is converted once, here, so callers never hand-roll
    /// the frame shift.
    pub fn raycast_render(
        &self,
        render_origin: RenderPos,
        direction: Dir3,
        max_distance: f64,
    ) -> Option<SurfaceHit> {
        let (origin, direction) = self.ray_to_grid(render_origin, direction)?;
        self.raycast(origin, direction, max_distance)
    }

    /// Split a grid-absolute point into the world grid's `(entity, cell, local)`
    /// so it can be spawned as a grid-direct child. The counterpart of
    /// [`Self::to_grid`] for the write side.
    pub fn grid_local(&self, p: GridPos) -> Option<(Entity, big_space::prelude::CellCoord, Vec3)> {
        let (entity, grid) = self.frame()?;
        let (cell, local) = grid.translation_to_grid(p.0);
        Some((entity, cell, local))
    }
}

/// Sample one oracle at a grid-absolute point, or `None` if the point lies
/// outside that DEM's square footprint.
///
/// The ONE place the footprint test and the sample live together. It is a free
/// function so the `&mut World` query providers ([`crate::query`]), which cannot
/// take a `SystemParam`, share this exact code instead of re-deriving the
/// footprint test — the re-derivation is what let two of them drift apart.
pub fn height_in_footprint(oracle: &crate::oracle::SurfaceOracle, p: GridPos) -> Option<f64> {
    let half = oracle.half_extent() as f64;
    // Footprint test in the GRID frame — the oracle's own frame. No transform in
    // between (module docs).
    if p.0.x.abs() > half || p.0.z.abs() > half {
        return None;
    }
    Some(HeightSource::height_at(oracle, p.0.x, p.0.z))
}

/// Fit a rectangular footprint to a surface: probe four corners, average them
/// for the rest height, and derive the slope orientation.
///
/// `sample` returns the ground height under a grid-absolute corner. It is a
/// closure rather than a fixed source because the caller decides the precedence
/// between the analytic surface and physics colliders (over open ground the
/// oracle is exact; standing on a structure, the physics down-ray is what the
/// footprint rests on) — but the geometry below is one implementation, shared by
/// the placement preview and the committed placement, which used to be two
/// copies that could (and did) disagree.
pub fn fit_footprint(
    center: GridPos,
    forward_xz: DVec3,
    half_w: f64,
    half_l: f64,
    sample: impl Fn(DVec3) -> Option<f64>,
) -> SurfaceFit {
    let forward_xz = {
        let f = DVec3::new(forward_xz.x, 0.0, forward_xz.z);
        if f.length_squared() < 1e-5 {
            DVec3::NEG_Z
        } else {
            f.normalize()
        }
    };
    let right_xz = forward_xz.cross(DVec3::Y).normalize();

    let corners = [
        center.0 + forward_xz * half_l - right_xz * half_w, // FL
        center.0 + forward_xz * half_l + right_xz * half_w, // FR
        center.0 - forward_xz * half_l - right_xz * half_w, // RL
        center.0 - forward_xz * half_l + right_xz * half_w, // RR
    ];
    let heights: Vec<DVec3> = corners
        .iter()
        .map(|c| DVec3::new(c.x, sample(*c).unwrap_or(center.0.y), c.z))
        .collect();
    let (fl, fr, rl, rr) = (heights[0], heights[1], heights[2], heights[3]);
    let avg_y = (fl.y + fr.y + rl.y + rr.y) / 4.0;

    let v_forward = ((fl - rl) + (fr - rr)) / 2.0;
    let v_right = ((fr - fl) + (rr - rl)) / 2.0;
    let mut normal = v_forward.cross(v_right);
    normal = if normal.length_squared() > 1e-5 {
        normal.normalize()
    } else {
        DVec3::Y
    };
    if normal.y < 0.0 {
        normal = -normal;
    }

    let mut spawn_forward = forward_xz - normal * forward_xz.dot(normal);
    spawn_forward = if spawn_forward.length_squared() < 1e-5 {
        forward_xz
    } else {
        spawn_forward.normalize()
    };
    let cross = spawn_forward.cross(normal);
    let spawn_right = if cross.length_squared() > 1e-5 {
        cross.normalize()
    } else {
        let mut perp = normal.cross(DVec3::X);
        if perp.length_squared() < 1e-5 {
            perp = normal.cross(DVec3::Z);
        }
        perp.normalize()
    };
    let spawn_backward = spawn_right.cross(normal).normalize();
    let rotation = Quat::from_mat3(&Mat3::from_cols(
        spawn_right.as_vec3(),
        normal.as_vec3(),
        spawn_backward.as_vec3(),
    ));

    SurfaceFit {
        point: GridPos(DVec3::new(center.0.x, avg_y, center.0.z)),
        normal,
        rotation,
    }
}

/// Report a transformed terrain that has no streamed representation.
///
/// Streamed tiles and [`GridSurfaceQuery`] both compose the terrain owner's full
/// pose into its nearest grid. A static legacy mesh is different: its analytic
/// oracle has no independently authored transform contract, so a transformed
/// owner would make placement disagree with that mesh. Reports; never panics.
pub fn report_unreachable_dem_frame(
    added: Query<
        (
            Entity,
            &Transform,
            bevy::ecs::query::Has<crate::stream_viz::TerrainLodViz>,
        ),
        Added<DemHeightField>,
    >,
    names: Query<&Name>,
) {
    for (entity, transform, streams_tiles) in added.iter() {
        if streams_tiles {
            continue;
        }
        let moved = transform.translation.length() > 1e-3 || transform.rotation != Quat::IDENTITY;
        if !moved {
            continue;
        }
        let name = names
            .get(entity)
            .map(|n| n.as_str().to_string())
            .unwrap_or_else(|_| format!("{entity}"));
        warn!(
            "[terrain-frame] static-mesh DEM terrain '{name}' carries a transform \
             (translation {:?}, rotation {:?}) that its surface oracle does not: the \
             static oracle has no authored transform contract, so placement, \
             waypoints and sensors can disagree with the drawn mesh. \
             Enable tile streaming (`lunco:layer:lodViz`) or author the pose into the DEM.",
            transform.translation, transform.rotation,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oracle::SurfaceOracle;
    use lunco_obstacle_field::field::HeightGrid;
    use std::sync::Arc;

    /// A DEM whose heights are ABSOLUTE body-datum metres — the real shape of a
    /// site like Apollo 15, whose ground sits ~1.9 km below the datum. Every
    /// pre-existing test used a terrain at y ≈ 0, where a frame bug and a correct
    /// implementation give the same answer; that is exactly why this class of bug
    /// survived three separate fixes.
    const SITE_ELEVATION: f64 = -1931.0;

    fn flat_dem_at(elevation: f64, half_extent: f32) -> Arc<SurfaceOracle> {
        let res = 8;
        let grid = HeightGrid {
            res,
            half_extent,
            heights: vec![elevation; res * res],
        };
        Arc::new(SurfaceOracle::bare(Arc::new(grid)))
    }

    fn world_with_terrain(elevation: f64) -> (App, Entity) {
        let mut app = App::new();
        let frame = app
            .world_mut()
            .spawn(lunco_core::WorldGridConfig::default().grid())
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(frame));
        let terrain = app
            .world_mut()
            .spawn((
                DemHeightField(flat_dem_at(elevation, 500.0)),
                Transform::IDENTITY,
                ChildOf(frame),
            ))
            .id();
        let project = app
            .world_mut()
            .register_system(update_terrain_physics_frame_poses);
        app.world_mut().run_system(project).unwrap();
        (app, terrain)
    }

    #[test]
    fn height_at_returns_absolute_elevation() {
        let (mut app, _) = world_with_terrain(SITE_ELEVATION);
        let sys = app.world_mut().register_system(|q: GridSurfaceQuery| {
            q.height_at(GridPos(DVec3::new(10.0, 0.0, -20.0)))
        });
        let got = app.world_mut().run_system(sys).unwrap();
        assert_eq!(got, Some(SITE_ELEVATION));
    }

    #[test]
    fn slope_at_uses_the_same_grid_absolute_terrain_authority() {
        let (mut app, _) = world_with_terrain(SITE_ELEVATION);
        let sys = app.world_mut().register_system(|q: GridSurfaceQuery| {
            q.slope_at(GridPos(DVec3::new(10.0, 0.0, -20.0)), 1.0)
        });
        let got = app.world_mut().run_system(sys).unwrap();
        assert_eq!(got, Some(0.0));
    }

    #[test]
    fn raycast_from_above_hits_the_absolute_surface() {
        let (mut app, _) = world_with_terrain(SITE_ELEVATION);
        // Origin 100 m above the real ground — not 100 m above y=0, which is
        // 1.9 km up and would still "work" under the old frame confusion.
        let origin = GridPos(DVec3::new(0.0, SITE_ELEVATION + 100.0, 0.0));
        let sys = app
            .world_mut()
            .register_system(move |q: GridSurfaceQuery| q.raycast(origin, Dir3::NEG_Y, 1000.0));
        let hit = app.world_mut().run_system(sys).unwrap().expect("hit");
        assert!((hit.point.0.y - SITE_ELEVATION).abs() < 0.05, "{hit:?}");
        assert!((hit.distance - 100.0).abs() < 0.05, "{hit:?}");
    }

    /// The regression: a ray aimed at the ground from a realistic altitude must
    /// hit. Under the old spawn-path implementation the ray was pushed through
    /// the terrain's render `GlobalTransform` first, which offset it by the
    /// terrain's world position (~one `big_space` cell at an elevated site) and
    /// this returned `None` — "the ghost never appears".
    #[test]
    fn shallow_ray_over_open_ground_hits() {
        let (mut app, _) = world_with_terrain(SITE_ELEVATION);
        let origin = GridPos(DVec3::new(-300.0, SITE_ELEVATION + 60.0, -300.0));
        let dir = Dir3::new(Vec3::new(0.6, -0.2, 0.77)).unwrap();
        let sys = app
            .world_mut()
            .register_system(move |q: GridSurfaceQuery| q.raycast(origin, dir, f64::INFINITY));
        let hit = app.world_mut().run_system(sys).unwrap();
        assert!(hit.is_some(), "shallow cursor ray found no ground");
        assert!((hit.unwrap().point.0.y - SITE_ELEVATION).abs() < 0.05);
    }

    #[test]
    fn no_terrain_is_distinguishable_from_no_hit() {
        let mut app = App::new();
        let sys = app.world_mut().register_system(|q: GridSurfaceQuery| {
            (q.has_terrain(), q.height_at(GridPos(DVec3::ZERO)))
        });
        let (has, height) = app.world_mut().run_system(sys).unwrap();
        assert!(!has);
        assert_eq!(height, None);
    }

    #[test]
    fn fit_footprint_rests_on_the_sampled_surface() {
        let center = GridPos(DVec3::new(0.0, 0.0, 0.0));
        let fit = fit_footprint(center, DVec3::NEG_Z, 1.5, 1.5, |_| Some(SITE_ELEVATION));
        assert!((fit.point.0.y - SITE_ELEVATION).abs() < 1e-9);
        assert!((fit.normal - DVec3::Y).length() < 1e-9);
    }

    #[test]
    fn fit_footprint_tilts_with_the_slope() {
        let center = GridPos(DVec3::ZERO);
        // Ground rising towards +x at 45°.
        let fit = fit_footprint(center, DVec3::NEG_Z, 1.0, 1.0, |c| Some(c.x));
        assert!(fit.normal.y > 0.0);
        assert!(fit.normal.x < 0.0, "normal must lean away from the rise");
    }
}
