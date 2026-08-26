//! `GridSpatialQuery` — the ONE sanctioned way to raycast / shapecast avian
//! colliders from a **bevy render-space** origin.
//!
//! # The bug this exists to make impossible
//!
//! With `big_space`, an entity's `GlobalTransform` is expressed in the
//! floating-origin **render** frame, while avian keeps every collider's
//! `Position` in the one [`lunco_core::ActivePhysicsFrame`]. The source frame
//! may be the render frame or any named BigSpace [`Grid`]; neither is silently
//! assumed to be the Avian frame.
//!
//! So a raw `SpatialQuery::cast_ray(global_transform.translation(), …)` casts the
//! ray from ~2 km away from where the colliders actually are, and misses
//! everything at an elevated site. That single frame-mixing mistake, rediscovered
//! independently, is the root of a whole family of bugs:
//!
//! - **wheels won't drive** — the suspension ray missed the terrain, so the wheel
//!   reported no ground contact and the drive gate bailed;
//! - **altimeter reads through the ground** — the down-ray never met the surface;
//! - **spawn ghost won't place** — the placement ray found nothing to rest on.
//!
//! Each of those "works flat in the sandbox, breaks on the DEM project" because
//! near the origin the two frames coincide.
//!
//! # The contract
//!
//! Any code that casts against avian colliders with an origin taken from a
//! `GlobalTransform` (or any render/world-space point) MUST go through
//! [`GridSpatialQuery`] instead of raw [`SpatialQuery`]. It delegates to
//! the active frame's BigSpace transform, so the ray meets colliders in every
//! scene, at any elevation and through nested/rotated grids with no per-call-site
//! frame reasoning. Grid-local callers use [`GridSpatialQuery::cast_ray_in_grid`]
//! and name the source grid explicitly.
//!
//! When your origin is ALREADY in the physics frame (e.g. an avian `Position`, as
//! the wheel drive uses via `wheel_hub_pose`), use [`GridSpatialQuery::cast_ray_grid`]
//! so the frame contract remains visible at the call site.

use avian3d::prelude::*;
use bevy::ecs::system::{SystemParam, SystemState};
use bevy::math::Dir3;
use bevy::prelude::{ChildOf, Query, Res, Resource, Transform, World};
use big_space::prelude::{CellCoord, Grid};
use lunco_core::coords::{pose_in_grid, render_to_grid_absolute, GridPos, RenderPos};
use std::sync::Mutex;

/// Bevy state for the read-only API raycast adapter.
///
/// `GridSpatialQuery` is a normal system parameter and therefore its state is
/// initialized from `&mut World`. API query providers intentionally receive
/// only `&World`; this resource initializes that state once during startup and
/// serializes its read-only access without duplicating Avian's raycast kernel.
#[derive(Resource, Default)]
pub struct GridSpatialQueryState {
    state: Mutex<Option<SystemState<GridSpatialQuery<'static, 'static>>>>,
}

impl GridSpatialQueryState {
    /// Initialize the system parameter after all physics plugins have built.
    pub fn initialize(&self, world: &mut World) {
        let mut state = self.state.lock().expect("grid query state mutex poisoned");
        *state = Some(SystemState::new(world));
    }

    /// Run a read-only raycast query without exposing mutable world access.
    pub fn with_query<R>(
        &self,
        world: &World,
        f: impl FnOnce(&GridSpatialQuery<'_, '_>) -> R,
    ) -> Result<R, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "GridSpatialQuery: query state mutex is poisoned".to_string())?;
        let state = state
            .as_mut()
            .ok_or_else(|| "GridSpatialQuery: query state is not initialized".to_string())?;
        let query = state
            .get(world)
            .map_err(|error| format!("GridSpatialQuery: query state is unavailable ({error})"))?;
        Ok(f(&query))
    }
}

/// A [`SpatialQuery`] that accepts ray/shape origins in **render space** and casts
/// them against avian colliders in the canonical **world-grid physics frame**,
/// applying the world shell's computed `big_space` conversion. See the module docs for why
/// this must be used instead of raw `SpatialQuery` whenever the origin comes from a
/// `GlobalTransform`.
#[derive(SystemParam)]
pub struct GridSpatialQuery<'w, 's> {
    spatial: SpatialQuery<'w, 's>,
    physics_frame: Option<Res<'w, lunco_core::ActivePhysicsFrame>>,
    grids: Query<'w, 's, &'static Grid>,
    parents: Query<'w, 's, &'static ChildOf>,
    // `cast_ray_in_grid` composes GRID entities only. Restricting the query to
    // that domain is both the semantic contract and what keeps this reusable
    // SystemParam disjoint from callers mutating ordinary body/camera
    // Transforms in the same system.
    nodes:
        Query<'w, 's, (Option<&'static CellCoord>, &'static Transform), bevy::prelude::With<Grid>>,
}

impl<'w, 's> GridSpatialQuery<'w, 's> {
    /// Map a render-space point into the canonical world-grid physics frame.
    /// Returns `None` until the world shell exists; guessing a frame would make
    /// a valid nested scene silently raycast in the wrong place.
    #[inline]
    pub fn to_physics(&self, render_point: RenderPos) -> Option<GridPos> {
        let frame = self.physics_frame.as_deref()?;
        let grid = self.grids.get(frame.0).ok()?;
        Some(render_to_grid_absolute(grid, render_point))
    }

    /// Cast a ray whose **origin is in render space**. The origin is converted
    /// into the canonical physics frame. The returned [`RayHitData::distance`]
    /// is frame-independent; compute the corresponding render point as
    /// `render_origin + dir * distance`.
    ///
    /// A non-finite origin yields `None` — a ray from nowhere hits nothing. This is
    /// a HARD requirement of the backend, not defensiveness: obvhs asserts
    /// `origin.is_finite()`, so a single NaN reaching it panics the compute pool and
    /// kills the run. Bodies do go non-finite (a diverging solve), and when they do
    /// every probe attached to them casts from garbage in the same tick.
    ///
    /// Deliberately SILENT. `lunco_physics::escape` already reports a body that left
    /// the world, by name and position, once. This helper runs per-probe per-frame —
    /// a wheel-suspension ray, a sensor, a clearance fan — so warning here would
    /// flood the log at precisely the moment it has to stay readable, and would say
    /// less than the report the escape guard already prints.
    #[inline]
    pub fn cast_ray_render(
        &self,
        render_origin: RenderPos,
        direction: Dir3,
        max_distance: f64,
        solid: bool,
        filter: &SpatialQueryFilter,
    ) -> Option<RayHitData> {
        if !render_origin.0.is_finite()
            || !direction.as_vec3().is_finite()
            || !max_distance.is_finite()
            || max_distance < 0.0
        {
            return None;
        }
        let origin = self.to_physics(render_origin)?.0;
        if !origin.is_finite() {
            return None;
        }
        let frame = self.physics_frame.as_deref()?;
        let grid = self.grids.get(frame.0).ok()?;
        let direction = grid
            .local_floating_origin()
            .grid_transform()
            .inverse()
            .transform_vector3(direction.as_dvec3());
        let direction = Dir3::new(direction.as_vec3()).ok()?;
        self.spatial
            .cast_ray(origin, direction, max_distance, solid, filter)
    }

    /// A non-finite origin yields `None`, for the same hard reason as
    /// [`Self::cast_ray_render`] — and this is the likelier path into it, since a
    /// grid-absolute origin usually IS an avian `Position`, which is exactly what
    /// goes non-finite when a solve diverges.
    pub fn cast_ray_grid(
        &self,
        origin: GridPos,
        direction: Dir3,
        max_distance: f64,
        solid: bool,
        filter: &SpatialQueryFilter,
    ) -> Option<RayHitData> {
        if !origin.0.is_finite()
            || !direction.as_vec3().is_finite()
            || !max_distance.is_finite()
            || max_distance < 0.0
        {
            return None;
        }
        self.spatial
            .cast_ray(origin.0, direction, max_distance, solid, filter)
    }

    /// Cast a ray expressed in a named BigSpace `source_grid`.
    ///
    /// This is the required entry point for systems that solve geometry in an
    /// entity's immediate grid (camera rigs, body-local sensors, streamed scene
    /// tools). Both the point and direction are rigidly composed into the one
    /// [`lunco_core::ActivePhysicsFrame`] before Avian sees them. Supplying the
    /// source grid makes the otherwise-untyped meaning of [`GridPos`] explicit
    /// at the boundary and prevents a sibling/rotated grid from being accepted
    /// merely because its numbers are finite.
    pub fn cast_ray_in_grid(
        &self,
        source_grid: bevy::prelude::Entity,
        origin: GridPos,
        direction: Dir3,
        max_distance: f64,
        solid: bool,
        filter: &SpatialQueryFilter,
    ) -> Option<RayHitData> {
        if !origin.0.is_finite()
            || !direction.as_vec3().is_finite()
            || !max_distance.is_finite()
            || max_distance < 0.0
        {
            return None;
        }
        let frame = self.physics_frame.as_deref()?;
        let (source_origin, source_rotation) = pose_in_grid(
            source_grid,
            frame.0,
            &self.parents,
            &self.grids,
            &self.nodes,
        )?;
        let physics_origin = source_origin + source_rotation * origin.0;
        let physics_direction = source_rotation * direction.as_dvec3();
        let physics_direction = Dir3::new(physics_direction.as_vec3()).ok()?;
        self.spatial.cast_ray(
            physics_origin,
            physics_direction,
            max_distance,
            solid,
            filter,
        )
    }
}
