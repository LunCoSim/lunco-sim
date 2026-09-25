//! Small ECS state shared across celestial spatial adapters.

use bevy::math::DVec3;
use bevy::prelude::*;

/// A scene-authored declaration that a celestial body exists.
///
/// This is the ECS projection of USD's `LunCoCelestialBodyAPI`
/// (`int lunco:body = 399`). The celestial runtime uses the component as its
/// scene-content gate; a scene with no declaration does not acquire a solar
/// hierarchy.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CelestialBodyDecl {
    /// NAIF id: 10 Sun, 399 Earth, 301 Moon.
    pub naif: i32,
}

/// The composed scene declares at least one celestial body source.
///
/// This marker records source-mode presence on the mounted scene root even
/// when a body's individual declaration is malformed. Runtime consumers use
/// it to reject the static authored-light source for a celestial scene instead
/// of masking a failed ephemeris projection with stale lighting.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CelestialSourcePresent;

/// Marker for the inertial solar-system root grid owned by the spatial runtime.
#[derive(Component)]
pub struct SolarSystemRoot;

/// A body map authored on a celestial body prim.
///
/// The value remains the authored asset reference. Resolution and loading are
/// owned by the asset layer and the runtime adapter that consumes this fact.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct AuthoredBodyAlbedo {
    /// Asset reference exactly as authored.
    pub asset: String,
}

/// Return whether the loaded scene declares any celestial body.
pub fn celestial_declared(q: Query<(), With<CelestialBodyDecl>>) -> bool {
    !q.is_empty()
}

/// Cached gravity state at the active avatar's position.
///
/// Camera, input, and UI adapters consume this shared frame fact. The
/// celestial runtime owns the system that derives it from BigSpace and the
/// active gravity provider.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct LocalGravityField {
    /// The body gravitationally bound to the active avatar.
    pub body_entity: Option<Entity>,
    /// Embodiment position relative to the bound body's centre in body-fixed axes.
    pub body_relative_position: DVec3,
    /// Up direction in world space.
    pub up: DVec3,
    /// Up direction in body-local space.
    pub local_up: DVec3,
    /// Surface gravity magnitude in metres per second squared.
    pub surface_g: f64,
}

impl Default for LocalGravityField {
    fn default() -> Self {
        Self {
            body_entity: None,
            body_relative_position: DVec3::ZERO,
            up: DVec3::Y,
            local_up: DVec3::Y,
            surface_g: 0.0,
        }
    }
}

/// Presentation state for an orbital view of a celestial body.
///
/// The camera is placed in the target body's explicit inertial BigSpace frame;
/// this resource records only the cross-domain mode fact. Camera return state
/// remains owned by the avatar camera transition runtime.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct OrbitalViewPin {
    /// Whether orbital presentation is active.
    pub active: bool,
    /// Ephemeris id of the focused body.
    pub body: i32,
    /// Unit direction from the body centre toward the viewpoint.
    pub dir: DVec3,
    /// Viewpoint distance from the body centre in metres.
    pub distance: f64,
}
