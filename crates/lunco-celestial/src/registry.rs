//! # Celestial Registry & Reference Frame Definitions
//!
//! This module acts as the "Master Manifest" for all gravitational bodies 
//! in the solar system. 
//!
//! ## The "Why": Standardized Interplanetary Navigation
//! To maintain compatibility with real-world aerospace tools (like GMAT, 
//! SPICE, or Orekit), LunCoSim uses the **NAIF ID** system (e.g., 399 for 
//! Earth, 301 for the Moon). 
//!
//! ## Reference Frame Anchoring
//! The [CelestialReferenceFrame] is the "Anchor" component. It marks an 
//! entity as the center of a localized [big_space] coordinate system. 
//! All physics and rendering calculations within a body's [SOI] are 
//! calculated relative to this frame, effectively implementing the 
//! **Heliocentric -> Geocentric -> Body-Fixed** transition hierarchy 
//! required for long-duration spaceflight.

use bevy::prelude::*;
use bevy::math::DVec3;

/// Centralized catalog of all celestial bodies and their physical constants.
///
/// This resource is initialized during startup and serves as the 
/// single source of truth for the [EphemerisProvider] and gravity systems.
#[derive(Resource, Clone, Reflect)]
#[reflect(Resource)]
pub struct CelestialBodyRegistry {
    /// The collection of all known celestial bodies.
    pub bodies: Vec<BodyDescriptor>,
}

pub use lunco_core::CelestialBody;

/// Component that identifies an entity as a center of a celestial reference frame.
///
/// **Why**: Essential for the ephemeris system to resolve absolute 
/// positions back to a specific body's local coordinate system.
#[derive(Component, Debug, Clone)]
pub struct CelestialReferenceFrame {
    /// The NAIF ID of the body this frame is anchored to.
    pub ephemeris_id: i32,
}

/// Static physical and orbital properties of a celestial body.
///
/// **Theory**: These constants are extracted from the **IAU WGCCRE** 
/// recommendations and **NAIF** kernel headers to ensure high-fidelity 
/// gravitational and rotational modeling.
#[derive(Clone, Debug, Reflect)]
pub struct BodyDescriptor {
    /// Human-readable name.
    pub name: String,
    /// Standard NAIF SPICE ID (e.g., 10 for Sun, 399 for Earth).
    pub ephemeris_id: i32,
    /// Average radius in meters for collision and visual scaling.
    pub radius_m: f64,
    /// Gravitational Parameter (G * Mass) in m³/s².
    pub gm: f64,
    /// Sphere of Influence radius in meters. Handover logic happens at this boundary.
    pub soi_radius_m: Option<f64>,
    /// NAIF ID of the body this body orbits (e.g., Moon parent is Earth-Moon Barycenter).
    pub parent_id: Option<i32>,
    /// Optional asset path for planetary surface textures.
    pub texture_path: Option<String>,
    /// Sidereal rotation rate in radians per day.
    pub rotation_rate_rad_per_day: f64,
    /// The body's spin axis in local J2000 coordinates.
    pub polar_axis: DVec3,
}

impl CelestialBodyRegistry {
    /// Generates a manifest of the primary inner solar system bodies.
    ///
    /// **Note**: Coordinates and polar axes are re-mapped to align with 
    /// the simulation's Right-Handed (Y-Up) convention.
    pub fn default_system() -> Self {
        Self {
            bodies: vec![
                BodyDescriptor {
                    name: "Sun".to_string(),
                    ephemeris_id: 10,
                    radius_m: 695_700_000.0,
                    gm: 1.327_124_400_18e20,
                    soi_radius_m: None,
                    parent_id: None,
                    texture_path: None,
                    rotation_rate_rad_per_day: 0.0,
                    polar_axis: DVec3::Y,
                },
                BodyDescriptor {
                    name: "Earth-Moon Barycenter".to_string(),
                    ephemeris_id: 3,
                    radius_m: 0.0,
                    gm: 0.0,
                    soi_radius_m: None,
                    parent_id: Some(10), // Sun
                    texture_path: None,
                    rotation_rate_rad_per_day: 0.0,
                    polar_axis: DVec3::Y,
                },
                BodyDescriptor {
                    name: "Earth".to_string(),
                    ephemeris_id: 399,
                    radius_m: 6371.0e3,
                    gm: 3.986004418e14,
                    soi_radius_m: Some(924.0e6),
                    parent_id: Some(3), // EMB
                    texture_path: Some("textures/earth.png".to_string()),
                    rotation_rate_rad_per_day: 6.300_388_098_9, // 2π / 0.99726968 rad/day
                    polar_axis: DVec3::Y, 
                },
                BodyDescriptor {
                    name: "Moon".to_string(),
                    ephemeris_id: 301,
                    radius_m: 1737.0e3,
                    gm: 4.9048695e12,
                    soi_radius_m: Some(66.0e6),
                    parent_id: Some(3), // EMB
                    texture_path: Some("textures/moon.png".to_string()),
                    rotation_rate_rad_per_day: 0.229_970_835_5, // 2π / 27.321661 rad/day
                    polar_axis: DVec3::Y, 
                },
            ],
        }
    }
}

