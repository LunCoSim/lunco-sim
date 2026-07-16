//! `GridSpatialQuery` — the ONE sanctioned way to raycast / shapecast avian
//! colliders from a **bevy render-space** origin.
//!
//! # The bug this exists to make impossible
//!
//! With `big_space`, an entity's `GlobalTransform` is expressed in the
//! floating-origin **render** frame, while avian keeps every collider's
//! `Position` in the grid-**absolute** physics frame. The two frames differ by a
//! pure translation — the floating origin's grid-absolute position, which is ~2 km
//! at a lunar site like the moonbase (≈ 1945 m elevation) and ~0 near the origin.
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
//! [`GridSpatialQuery`] instead of raw [`SpatialQuery`]. It recovers the
//! render→physics translation once (from any physics body: its grid-absolute
//! `Position` minus its render translation — a global constant) and applies it, so
//! the ray meets the colliders in every scene, at any elevation, with no
//! per-call-site frame reasoning. Directions are frame-invariant (the two frames
//! share rotation), so only the origin is shifted.
//!
//! When your origin is ALREADY in the physics frame (e.g. an avian `Position`, as
//! the wheel drive uses via `wheel_hub_pose`), use [`GridSpatialQuery::raw`] — the
//! unwrapped `SpatialQuery` — so you don't double-shift.

use avian3d::prelude::*;
use bevy::ecs::system::SystemParam;
use bevy::math::{DVec3, Dir3};
use bevy::prelude::*;

/// A [`SpatialQuery`] that accepts ray/shape origins in **render space** and casts
/// them against avian colliders in the **grid-absolute physics frame**, applying
/// the big_space render→physics translation for you. See the module docs for why
/// this must be used instead of raw `SpatialQuery` whenever the origin comes from a
/// `GlobalTransform`.
#[derive(SystemParam)]
pub struct GridSpatialQuery<'w, 's> {
    spatial: SpatialQuery<'w, 's>,
    /// Any physics body serves as the frame reference: its grid-absolute
    /// `Position` minus its render `GlobalTransform` translation is the
    /// (entity-independent) render→physics shift.
    frame_ref: Query<'w, 's, (&'static Position, &'static GlobalTransform), With<RigidBody>>,
}

impl<'w, 's> GridSpatialQuery<'w, 's> {
    /// The render→physics translation: add it to a render-space point to get its
    /// grid-absolute physics-frame position. It is a global constant (identical for
    /// every entity — the floating origin's grid-absolute position), so any single
    /// body yields it; `DVec3::ZERO` when no body exists (a near-origin scene needs
    /// no shift).
    #[inline]
    pub fn frame_shift(&self) -> DVec3 {
        self.frame_ref
            .iter()
            .next()
            .map(|(p, gt)| p.0 - gt.translation().as_dvec3())
            .unwrap_or(DVec3::ZERO)
    }

    /// Map a render-space point into the grid-absolute physics frame.
    #[inline]
    pub fn to_physics(&self, render_point: DVec3) -> DVec3 {
        render_point + self.frame_shift()
    }

    /// Map a grid-absolute physics point back into render space — e.g. to place a
    /// visual (ghost, marker) at a returned hit point.
    #[inline]
    pub fn to_render(&self, physics_point: DVec3) -> DVec3 {
        physics_point - self.frame_shift()
    }

    /// Cast a ray whose **origin is in render space**. The origin is shifted into
    /// the physics frame; the direction is passed through unchanged (frames share
    /// rotation). The returned [`RayHitData::distance`] is frame-independent; if you
    /// need the world hit POINT, compute it in render space as
    /// `render_origin + dir * distance`, or map an absolute point back with
    /// [`Self::to_render`].
    #[inline]
    pub fn cast_ray_render(
        &self,
        render_origin: DVec3,
        direction: Dir3,
        max_distance: f64,
        solid: bool,
        filter: &SpatialQueryFilter,
    ) -> Option<RayHitData> {
        self.spatial
            .cast_ray(render_origin + self.frame_shift(), direction, max_distance, solid, filter)
    }

    /// The wrapped [`SpatialQuery`], for origins ALREADY in the grid-absolute
    /// physics frame (an avian `Position`). Casting a physics-frame origin through
    /// [`Self::cast_ray_render`] would double-shift it.
    #[inline]
    pub fn raw(&self) -> &SpatialQuery<'w, 's> {
        &self.spatial
    }
}
