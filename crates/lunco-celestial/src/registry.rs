use bevy::prelude::*;
use bevy::math::DVec3;

#[derive(Resource, Clone, Reflect)]
#[reflect(Resource)]
pub struct CelestialBodyRegistry {
    pub bodies: Vec<BodyDescriptor>,
}

pub use lunco_core::CelestialBody;

#[derive(Component, Debug, Clone)]
pub struct CelestialReferenceFrame {
    pub ephemeris_id: i32,
}

#[derive(Clone, Debug, Reflect)]
pub struct BodyDescriptor {
    pub name: String,
    pub ephemeris_id: i32,         // NAIF ID
    pub radius_m: f64,
    pub gm: f64,                   // Gravitational parameter (m³/s²)
    pub soi_radius_m: Option<f64>, // None for Sun
    pub parent_id: Option<i32>,    // NAIF ID of parent
    pub texture_path: Option<String>,
    pub rotation_rate_rad_per_day: f64, // Sidereal rotation rate
    pub polar_axis: DVec3,              // Body's rotation axis (unit vector)
}

impl CelestialBodyRegistry {
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
                    // Earth's North Pole (Eq Z) maps to Bevy Y per new AD-6 mapping
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
                    // Moon's North Pole (mostly Eq Z) maps to Bevy Y
                    polar_axis: DVec3::Y, 
                },
            ],
        }
    }
}
