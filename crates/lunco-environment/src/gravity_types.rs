//! Gravity configuration **types** — the environmental-state vocabulary for
//! gravity, owned here alongside the lighting types.
//!
//! These are pure type/component/resource definitions (no systems). The gravity
//! *compute* systems live in two places:
//! - `lunco_environment`'s force application (`apply_gravity_to_rigid_bodies`,
//!   consuming the per-entity `LocalGravity` component), and
//! - `lunco_celestial`'s `update_local_gravity_field` + `PointMassGravity`
//!   (the `GravityModel` impl), which import these types from here.
//!
//! ## Gravity modes
//!
//! - **`Gravity::Flat`** — constant gravity, same for all entities.
//!   Used for sandbox, tests, and flat-ground simulations.
//! - **`Gravity::Surface`** — surface gravity for spherical bodies.
//!   Direction = `-normalize(entity_body_local_position)`; magnitude looked up
//!   from the body's [`GravityProvider`]. Entities need [`GravityBody`].

use bevy::math::DVec3;
use bevy::prelude::*;

// ─────────────────────────────────────────────────────────────────────────────
// Gravity models (orbital / multi-body)
// ─────────────────────────────────────────────────────────────────────────────

/// Trait for computing gravitational acceleration at a position.
pub trait GravityModel: Send + Sync + 'static {
    /// Compute acceleration vector at `relative_pos` (meters from body center).
    fn acceleration(&self, relative_pos: DVec3) -> DVec3;
}

/// Component marking an entity as a gravity source.
///
/// Placed on Body entities. The `GravityProvider` wraps a [`GravityModel`]
/// (typically `lunco_celestial::PointMassGravity`) to compute acceleration at
/// any position.
#[derive(Component)]
pub struct GravityProvider {
    /// The gravity model (e.g. point-mass, spherical harmonics).
    pub model: Box<dyn GravityModel>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Global Gravity Resource
// ─────────────────────────────────────────────────────────────────────────────

/// Mean lunar surface gravity, m/s².
///
/// The default for this simulator's scenes: its rovers' drivetrains, its
/// lander's struts and its propellant budgets are all sized against this
/// number, so a scene that says nothing about gravity gets the Moon rather than
/// the Earth. Scenes state it explicitly through `UsdPhysicsScene`
/// (`physics:gravityMagnitude`); this is the value behind that authoring.
pub const MOON_SURFACE_GRAVITY: f64 = 1.62;

/// Standard Earth surface gravity, m/s².
///
/// UsdPhysics uses "earth gravity" as the meaning of an unauthored
/// `physics:gravityMagnitude` (spelled as a negative value), so the USD reader
/// needs the constant even in a lunar simulator.
pub const EARTH_SURFACE_GRAVITY: f64 = 9.80665;

/// Global gravity configuration — replaces `avian3d::prelude::Gravity`.
///
/// Set this once during app setup. The gravity system runs in `FixedUpdate`
/// and automatically applies forces to all `RigidBody` entities.
///
/// A scene may override it (`UsdPhysicsScene`), so the app registers its
/// start-up value as the baseline that scene teardown restores — otherwise a
/// lunar scene would leave 1.62 behind for whatever loads next. That is what
/// `Clone` is for here; see `lunco_usd_bevy::scene_lifecycle`.
///
/// # Examples
///
/// ```rust
/// # use bevy::math::DVec3;
/// # use lunco_environment::Gravity;
/// // Sandbox: flat Earth gravity
/// let _ = Gravity::flat(9.81, DVec3::NEG_Y);
///
/// // Full client: surface gravity on spherical bodies
/// let _ = Gravity::surface();
/// ```
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
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
    /// Magnitude is looked up from the body's [`GravityProvider`].
    /// Entities must have [`GravityBody`] to identify their gravitational parent.
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
