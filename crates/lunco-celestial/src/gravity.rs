//! Per-entity gravity — plugin replacement for avian3d::prelude::Gravity.
//!
//! ## Usage
//!
//! ```rust
//! # use bevy::prelude::*;
//! # use bevy::math::DVec3;
//! # use lunco_environment::Gravity;
//! # let mut app = App::new();
//! // Sandbox / flat ground:
//! app.insert_resource(Gravity::flat(9.81, DVec3::NEG_Y));
//!
//! // Full client (surface gravity):
//! app.insert_resource(Gravity::surface());
//! ```
//!
//! ## Architecture
//!
//! The gravity system runs in `FixedUpdate` and automatically applies forces
//! to all `RigidBody` entities. This is a drop-in replacement for Avian3D's
//! built-in gravity.
//!
//! ### Gravity modes
//!
//! - **`Gravity::Flat`** — constant gravity, same for all entities.
//!   Used for sandbox, tests, and flat-ground simulations.
//!   Equivalent to `avian3d::prelude::Gravity`.
//!
//! - **`Gravity::Surface`** — surface gravity for spherical bodies.
//!   Direction = `-normalize(entity_body_local_position)`.
//!   Entities must have `GravityBody` to identify which body they're on.
//!
//! ### Body-local positions
//!
//! In the merged Body+Grid design, the Body entity IS the Grid. Surface
//! entities (rovers, tiles) are children of Body/Grid. Their `Transform.translation`
//! is in the body-fixed frame (origin = body center). For these entities,
//! `Transform.translation` IS the body-local position — no Grid lookup needed.
//!
//! For orbit cameras and entities NOT on the Body/Grid, we compute the
//! absolute position and subtract the body's absolute position to get
//! the body-relative offset.

use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};

// Gravity configuration *types* now live in `lunco-environment` (environmental
// state, sibling to lighting). This crate owns only the `PointMassGravity`
// model impl, the cached `LocalGravityField`, and the system that fills it.
use lunco_environment::{Gravity, GravityBody, GravityModel, GravityProvider};

// ─────────────────────────────────────────────────────────────────────────────
// Gravity models (orbital / multi-body)
// ─────────────────────────────────────────────────────────────────────────────

/// Point-mass gravity: a = GM/r² toward center. A [`GravityModel`] impl used as
/// the `model` inside a [`GravityProvider`].
pub struct PointMassGravity {
    pub gm: f64,
}

impl GravityModel for PointMassGravity {
    fn acceleration(&self, relative_pos: DVec3) -> DVec3 {
        let r2 = relative_pos.length_squared();
        if r2 < 1.0 {
            return DVec3::ZERO;
        }
        let r = r2.sqrt();
        -relative_pos * (self.gm / (r * r2))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Local gravity field (cached for camera/UI)
// ─────────────────────────────────────────────────────────────────────────────

/// Cached gravity state at the avatar's position.
///
/// Camera and UI systems read this resource to determine "up" direction
/// and surface gravity magnitude. Updated each frame in `PreUpdate`.
#[derive(Resource, Default)]
pub struct LocalGravityField {
    /// The body we're gravitationally bound to.
    pub body_entity: Option<Entity>,
    /// "Up" direction in world space.
    pub up: DVec3,
    /// "Up" direction in body-local space.
    pub local_up: DVec3,
    /// Surface gravity magnitude (m/s²).
    pub surface_g: f64,
}

// Note: Gravity force application moved to `lunco-environment`.
// See `lunco_environment::apply_gravity_to_rigid_bodies` — it consumes the
// per-entity `LocalGravity` component instead of recomputing per tick.

// ─────────────────────────────────────────────────────────────────────────────
// Local gravity field update (camera/UI)
// ─────────────────────────────────────────────────────────────────────────────

/// Updates `LocalGravityField` based on avatar position.
///
/// Uses the avatar's full grid position (CellCoord + Transform) to compute
/// the direction from the body center via absolute coordinate arithmetic.
/// This correctly handles nested grids and reparenting.
///
/// Runs in `PreUpdate` so camera systems see fresh data.
pub fn update_local_gravity_field(
    q_avatar: Query<(
        Entity,
        &Transform,
        &CellCoord,
        &ChildOf,
        Option<&GravityBody>,
    )>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_bodies: Query<&GravityProvider>,
    gravity: Res<Gravity>,
    mut field: ResMut<LocalGravityField>,
    orbital_pin: Res<crate::placement::OrbitalViewPin>,
) {
    // Orbital VIEW active: the camera has flown to the focused body, but the
    // scene/physics stayed at the site. A field computed at the camera's
    // position would be garbage for the site content (Earth gravity at the
    // Moon site), so HOLD the last surface field until the view returns.
    if orbital_pin.active {
        return;
    }
    let Some((avatar_ent, tf, cell, _, gravity_body)) = q_avatar.iter().next() else {
        return;
    };

    // Avatar absolute position in root frame.
    let cam_abs = crate::coords::world_position_seeded(
        avatar_ent, cell, tf, &q_parents, &q_grids, &q_spatial,
    );

    let (body_local, surface_g) = if let Some(gb) = gravity_body {
        // Compute body absolute position.
        let body_abs = if let Ok((b_cell, b_tf)) = q_spatial.get(gb.body_entity) {
            let cell = b_cell.copied().unwrap_or_default();
            crate::coords::world_position_seeded(
                gb.body_entity,
                &cell,
                b_tf,
                &q_parents,
                &q_grids,
                &q_spatial,
            )
        } else {
            DVec3::ZERO
        };

        let rel = cam_abs - body_abs;
        let g = if let Ok(gp) = q_bodies.get(gb.body_entity) {
            gp.model.acceleration(rel).length()
        } else {
            0.0
        };
        (rel, g)
    } else if let Some(body_ent) = field.body_entity {
        // Fall back to the last-known body from LocalGravityField.
        let body_abs = if let Ok((b_cell, b_tf)) = q_spatial.get(body_ent) {
            let cell = b_cell.copied().unwrap_or_default();
            crate::coords::world_position_seeded(
                body_ent, &cell, b_tf, &q_parents, &q_grids, &q_spatial,
            )
        } else {
            DVec3::ZERO
        };

        let rel = cam_abs - body_abs;
        let g = if let Ok(gp) = q_bodies.get(body_ent) {
            gp.model.acceleration(rel).length()
        } else {
            0.0
        };
        (rel, g)
    } else {
        (cam_abs, 0.0)
    };

    field.surface_g = surface_g;

    let dist = body_local.length();
    field.local_up = if dist > 1e-6 {
        body_local / dist
    } else {
        DVec3::Y
    };
    field.up = field.local_up;

    // For flat gravity, use the configured g.
    if let Gravity::Flat { g, direction } = gravity.as_ref() {
        field.surface_g = *g;
        field.local_up = -*direction / direction.length();
    }
}
