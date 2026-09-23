//! ECS projections of celestial identity.

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
