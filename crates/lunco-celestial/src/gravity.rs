//! Per-entity gravity — plugin replacement for avian3d::prelude::Gravity.
//!
//! ## Usage
//!
//! ```rust
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

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::{Forces, Mass, RigidBody, WriteRigidBodyForces};
use big_space::prelude::{Grid, CellCoord};

// ─────────────────────────────────────────────────────────────────────────────
// Gravity models (orbital / multi-body)
// ─────────────────────────────────────────────────────────────────────────────

/// Trait for computing gravitational acceleration at a position.
pub trait GravityModel: Send + Sync + 'static {
    /// Compute acceleration vector at `relative_pos` (meters from body center).
    fn acceleration(&self, relative_pos: DVec3) -> DVec3;
}

/// Point-mass gravity: a = GM/r² toward center.
pub struct PointMassGravity {
    pub gm: f64,
}

impl GravityModel for PointMassGravity {
    fn acceleration(&self, relative_pos: DVec3) -> DVec3 {
        let r2 = relative_pos.length_squared();
        if r2 < 1.0 { return DVec3::ZERO; }
        let r = r2.sqrt();
        -relative_pos * (self.gm / (r * r2))
    }
}

/// Component marking an entity as a gravity source.
///
/// Placed on Body entities. The `GravityProvider` wraps a `GravityModel`
/// (typically `PointMassGravity`) to compute acceleration at any position.
#[derive(Component)]
pub struct GravityProvider {
    /// The gravity model (e.g. point-mass, spherical harmonics).
    pub model: Box<dyn GravityModel>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Global Gravity Resource
// ─────────────────────────────────────────────────────────────────────────────

/// Global gravity configuration — replaces `avian3d::prelude::Gravity`.
///
/// Set this once during app setup. The gravity system runs in `FixedUpdate`
/// and automatically applies forces to all `RigidBody` entities.
///
/// # Examples
///
/// ```rust
/// // Sandbox: flat Earth gravity
/// app.insert_resource(Gravity::flat(9.81, DVec3::NEG_Y));
///
/// // Full client: surface gravity on spherical bodies
/// app.insert_resource(Gravity::surface());
/// ```
#[derive(Resource)]
pub enum Gravity {
    /// Flat constant gravity — same magnitude and direction for all bodies.
    Flat {
        /// Surface gravity magnitude (m/s²).
        g: f64,
        /// Direction of gravity (e.g. `NEG_Y`).
        direction: DVec3,
    },
    /// Surface gravity for spherical bodies.
    ///
    /// Direction is computed per-entity as `-normalize(body_local_position)`.
    /// Magnitude is looked up from the body's `GravityProvider`.
    /// Entities must have `GravityBody` to identify their gravitational parent.
    Surface,
}

impl Gravity {
    /// Convenience constructor for flat gravity.
    pub const fn flat(g: f64, direction: DVec3) -> Self {
        Self::Flat { g, direction }
    }

    /// Convenience constructor for surface gravity.
    pub const fn surface() -> Self {
        Self::Surface
    }
}

/// Links an entity to the celestial body it is gravitationally bound to.
///
/// Required for `Gravity::Surface` mode. Not needed for `Gravity::Flat`.
#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct GravityBody {
    /// The Body entity this entity orbits or sits on.
    pub body_entity: Entity,
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

// ─────────────────────────────────────────────────────────────────────────────
// Gravity System
// ─────────────────────────────────────────────────────────────────────────────

/// Applies gravity to all `RigidBody` entities.
///
/// Runs in `FixedUpdate`. Reads the global `Gravity` resource and applies
/// forces accordingly.
///
/// - **`Gravity::Flat`** — same constant force applied to every body.
/// - **`Gravity::Surface`** — direction from body-local position, magnitude
///   from body's `GravityProvider`. For surface entities (children of Body/Grid),
///   `Transform.translation` IS the body-local position since the entity origin
///   coincides with the body center.
pub fn gravity_system(
    gravity: Res<Gravity>,
    mut q_entities: Query<(Entity, &Transform, &Mass, Option<&GravityBody>), With<RigidBody>>,
    q_bodies: Query<&GravityProvider>,
    mut forces: Query<Forces>,
) {
    match gravity.as_ref() {
        Gravity::Flat { g, direction } => {
            for (entity, _tf, mass, _) in q_entities.iter_mut() {
                let force = *direction * g * mass.0 as f64;
                if let Ok(mut f) = forces.get_mut(entity) {
                    f.apply_force(force);
                }
            }
        }
        Gravity::Surface => {
            for (entity, tf, mass, gb) in q_entities.iter_mut() {
                let Some(gb) = gb else { continue; };

                // The entity is a child of Body/Grid. Its Transform.translation
                // is in the body-fixed frame (origin = body center).
                let local_pos = tf.translation.as_dvec3();
                let dist = local_pos.length();
                if dist < 1e-6 { continue; }
                let dir = -local_pos / dist;

                // Look up surface g from the body's GravityProvider.
                let g = if let Ok(gp) = q_bodies.get(gb.body_entity) {
                    let accel = gp.model.acceleration(local_pos);
                    accel.length()
                } else {
                    0.0
                };

                let force = dir * g * mass.0 as f64;
                if let Ok(mut f) = forces.get_mut(entity) {
                    f.apply_force(force);
                }
            }
        }
    }
}

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
    q_avatar: Query<(Entity, &Transform, &CellCoord, &ChildOf, Option<&GravityBody>)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(&CellCoord, &Transform)>,
    q_bodies: Query<&GravityProvider>,
    gravity: Res<Gravity>,
    mut field: ResMut<LocalGravityField>,
) {
    let Some((avatar_ent, tf, cell, _, gravity_body)) = q_avatar.iter().next() else { return };

    // Avatar absolute position in root frame.
    let cam_abs = crate::coords::get_absolute_pos_in_root_double_ghost_aware(
        avatar_ent, cell, tf, &q_parents, &q_grids, &q_spatial,
    );

    let (body_local, surface_g) = if let Some(gb) = gravity_body {
        // Compute body absolute position.
        let body_abs = if let Ok((b_cell, b_tf)) = q_spatial.get(gb.body_entity) {
            crate::coords::get_absolute_pos_in_root_double_ghost_aware(
                gb.body_entity, b_cell, b_tf, &q_parents, &q_grids, &q_spatial,
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
            crate::coords::get_absolute_pos_in_root_double_ghost_aware(
                body_ent, b_cell, b_tf, &q_parents, &q_grids, &q_spatial,
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
    field.local_up = if dist > 1e-6 { body_local / dist } else { DVec3::Y };
    field.up = field.local_up;

    // For flat gravity, use the configured g.
    if let Gravity::Flat { g, direction } = gravity.as_ref() {
        field.surface_g = *g;
        field.local_up = -*direction / direction.length();
    }
}
