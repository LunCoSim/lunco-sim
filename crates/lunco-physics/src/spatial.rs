//! `GridSpatialQuery` — the ONE sanctioned way to raycast / shapecast avian
//! colliders from a **bevy render-space** origin.
//!
//! # The bug this exists to make impossible
//!
//! With `big_space`, an entity's `GlobalTransform` is expressed in the
//! floating-origin **render** frame, while avian keeps every collider's
//! `Position` in the canonical world-grid **absolute** physics frame. The two
//! frames are related by the world grid's computed floating-origin affine
//! transform, including nested-grid rotation.
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
//! [`lunco_core::coords::WorldFrame`], the single owner of render↔world
//! conversion, so the ray meets colliders in every scene, at any elevation and
//! through nested/rotated grids with no per-call-site frame reasoning.
//!
//! When your origin is ALREADY in the physics frame (e.g. an avian `Position`, as
//! the wheel drive uses via `wheel_hub_pose`), use [`GridSpatialQuery::raw`] — the
//! unwrapped `SpatialQuery` — so you don't double-shift.

use avian3d::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::math::Dir3;
use lunco_core::coords::{GridPos, RenderPos};

/// A [`SpatialQuery`] that accepts ray/shape origins in **render space** and casts
/// them against avian colliders in the canonical **world-grid physics frame**,
/// applying the world shell's computed `big_space` conversion. See the module docs for why
/// this must be used instead of raw `SpatialQuery` whenever the origin comes from a
/// `GlobalTransform`.
#[derive(SystemParam)]
pub struct GridSpatialQuery<'w, 's> {
    spatial: SpatialQuery<'w, 's>,
    world_frame: lunco_core::coords::WorldFrame<'w, 's>,
}

impl<'w, 's> GridSpatialQuery<'w, 's> {
    /// Map a render-space point into the canonical world-grid physics frame.
    /// Returns `None` until the world shell exists; guessing a frame would make
    /// a valid nested scene silently raycast in the wrong place.
    #[inline]
    pub fn to_physics(&self, render_point: RenderPos) -> Option<GridPos> {
        self.world_frame.render_to_world(render_point)
    }

    /// Cast a ray whose **origin is in render space**. The origin is converted
    /// into the canonical physics frame. The returned [`RayHitData::distance`]
    /// is frame-independent; compute the corresponding render point as
    /// `render_origin + dir * distance`.
    #[inline]
    pub fn cast_ray_render(
        &self,
        render_origin: RenderPos,
        direction: Dir3,
        max_distance: f64,
        solid: bool,
        filter: &SpatialQueryFilter,
    ) -> Option<RayHitData> {
        self.spatial.cast_ray(
            self.to_physics(render_origin)?.0,
            direction,
            max_distance,
            solid,
            filter,
        )
    }

    /// Cast a ray whose origin is ALREADY grid-absolute (an avian `Position`,
    /// a composed [`lunco_core::coords::world_position`]). The typed
    /// counterpart of [`Self::cast_ray_render`]; together they retire most
    /// [`Self::raw`] call sites, each of which was an untyped assertion
    /// "trust me, this is already the physics frame".
    #[inline]
    pub fn cast_ray_grid(
        &self,
        origin: GridPos,
        direction: Dir3,
        max_distance: f64,
        solid: bool,
        filter: &SpatialQueryFilter,
    ) -> Option<RayHitData> {
        self.spatial
            .cast_ray(origin.0, direction, max_distance, solid, filter)
    }

    /// The wrapped [`SpatialQuery`], for shapecasts and query shapes the typed
    /// wrappers don't cover. For plain rays use [`Self::cast_ray_grid`] /
    /// [`Self::cast_ray_render`], which carry the frame in the type.
    #[inline]
    pub fn raw(&self) -> &SpatialQuery<'w, 's> {
        &self.spatial
    }
}
