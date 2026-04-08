//! Per-entity gravity — plugin replacement for avian3d::prelude::Gravity.
//!
//! ## Usage
//!
//! ```rust
//! // Sandbox / flat ground:
//! app.insert_resource(Gravity::Flat(9.81, DVec3::NEG_Y));
//!
//! // Full client (surface gravity):
//! app.insert_resource(Gravity::Surface);
//! ```
//!
//! The gravity system runs in `FixedUpdate` and automatically applies
//! forces to all `RigidBody` entities. No per-entity gravity component needed.
//!
//! ## Gravity modes
//!
//! - **`Gravity::Flat(g, direction)`** — constant gravity, same for all entities.
//!   Used for sandbox, tests, and flat-ground simulations. Equivalent to
//!   `avian3d::prelude::Gravity`.
//!
//! - **`Gravity::Surface`** — surface gravity for spherical bodies.
//!   Direction = `-normalize(entity_body_local_position)`.
//!   Entities must have `GravityBody` to identify which body they're on.

use bevy::prelude::*;
use bevy::math::DVec3;
use avian3d::prelude::{Forces, Mass, RigidBody, WriteRigidBodyForces};

// ─────────────────────────────────────────────────────────────────────────────
// Legacy: Orbital gravity models (kept for future multi-body transfers)
// ─────────────────────────────────────────────────────────────────────────────

/// Trait for computing gravitational acceleration at a position.
pub trait GravityModel: Send + Sync + 'static {
    fn acceleration(&self, relative_pos: DVec3) -> DVec3;
}

/// Component marking an entity as a gravity source.
#[derive(Component)]
pub struct GravityProvider {
    pub model: Box<dyn GravityModel>,
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

// ─────────────────────────────────────────────────────────────────────────────
// Global Gravity Resource (replacement for avian3d::prelude::Gravity)
// ─────────────────────────────────────────────────────────────────────────────

/// Global gravity — replaces `avian3d::prelude::Gravity`.
///
/// Set this once during app setup. The gravity system runs in `FixedUpdate`
/// and automatically applies forces to all `RigidBody` entities.
///
/// # Examples
///
/// ```rust
/// // Sandbox: flat Earth gravity
/// app.insert_resource(Gravity::Flat(9.81, DVec3::NEG_Y));
///
/// // Full client: surface gravity on spherical bodies
/// app.insert_resource(Gravity::Surface);
/// ```
#[derive(Resource)]
pub enum Gravity {
    /// Flat constant gravity — same magnitude and direction for all bodies.
    /// Equivalent to `avian3d::prelude::Gravity`.
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
/// forces accordingly. This is a drop-in replacement for Avian3D's
/// built-in gravity.
///
/// - `Gravity::Flat` — same constant force applied to every body
/// - `Gravity::Surface` — direction from body-local position, magnitude from body's GM
pub fn gravity_system(
    gravity: Res<Gravity>,
    mut q_entities: Query<(
        Entity,
        &Transform,
        &Mass,
        Option<&GravityBody>,
    ), With<RigidBody>>,
    q_bodies: Query<&GravityProvider>,
    mut forces: Query<Forces>,
) {
    match gravity.as_ref() {
        Gravity::Flat { g, direction } => {
            // Same as avian3d::prelude::Gravity — constant pull in one direction
            for (entity, _tf, mass, _) in q_entities.iter_mut() {
                let force = *direction * g * mass.0 as f64;
                if let Ok(mut f) = forces.get_mut(entity) {
                    f.apply_force(force);
                }
            }
        }
        Gravity::Surface => {
            // Direction = -normalize(body_local_position), magnitude = GM/R²
            for (entity, tf, mass, gb) in q_entities.iter_mut() {
                let Some(gb) = gb else { continue; };
                let local_pos = tf.translation.as_dvec3();
                let dist = local_pos.length();
                if dist < 1e-6 { continue; }
                let dir = -local_pos / dist;

                // Look up surface g from the body's GravityProvider
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
pub fn update_local_gravity_field(
    q_avatar: Query<(&Transform, Option<&GravityBody>)>,
    q_bodies: Query<&GravityProvider>,
    gravity: Res<Gravity>,
    mut field: ResMut<LocalGravityField>,
) {
    let Some((tf, gravity_body)) = q_avatar.iter().next() else { return };

    let local_pos = tf.translation.as_dvec3();
    let dist = local_pos.length();
    field.local_up = if dist > 1e-6 { local_pos / dist } else { DVec3::Y };
    field.up = field.local_up;

    if let Some(gb) = gravity_body {
        field.body_entity = Some(gb.body_entity);
        if let Ok(gp) = q_bodies.get(gb.body_entity) {
            let accel = gp.model.acceleration(tf.translation.as_dvec3());
            field.surface_g = accel.length();
        }
    }

    // For flat gravity, use the configured g
    if let Gravity::Flat { g, direction } = gravity.as_ref() {
        field.surface_g = *g;
        field.local_up = -*direction / direction.length();
    }
}
