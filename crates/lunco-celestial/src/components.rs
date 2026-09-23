//! ECS projections of celestial identity and authored ephemeris placement.

use bevy::prelude::*;

/// Represents a major celestial body in the simulation.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct CelestialBody {
    /// Name of the celestial body.
    pub name: String,
    /// Unique identifier for ephemeris data retrieval.
    pub ephemeris_id: i32,
    /// Mean radius in meters, used for rendering and approximate physics.
    pub radius_m: f64,
}

/// Marks an authored entity as a spacecraft for selection, possession, and
/// camera framing. Its USD `Name` supplies the display name.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct Spacecraft {
    /// Radius for analytic possession and camera framing in metres.
    pub hit_radius_m: f64,
}

/// Places an authored prim from ephemeris state relative to a selected body.
///
/// The USD prim owns its geometry, material, scale, and authored orientation.
/// This component owns only the relationship between the prim and ephemeris
/// bodies; unavailable state leaves the prim hidden.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct EphemerisPosition {
    /// Ephemeris id of the body whose position drives this prim.
    pub target_id: i32,
    /// Ephemeris id of the origin used for the prim's local translation.
    pub reference_id: i32,
}
