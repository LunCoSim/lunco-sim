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

use crate::iau::IauRotation;

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
/// **Theory**: the gravitational constants come from **NAIF** kernel headers.
/// The ROTATION model is the **IAU WGCCRE** one, carried verbatim in [`iau`] as
/// the published ICRF elements (`α₀`, `δ₀`, `W₀`, `Ẇ` + the lunar periodic
/// series) — the spin axis and the body-fixed rotation are both DERIVED from
/// them ([`BodyDescriptor::polar_axis`], [`crate::geo::body_rotation`]).
///
/// It used to say "extracted from the IAU WGCCRE recommendations" while
/// actually carrying a hand-typed mean-of-2026 pole and **no prime-meridian
/// epoch at all** (`W₀` absent ⇒ the Moon rotated 38.3° from its true
/// orientation and its near side did not face Earth). The claim is now true;
/// see `iau.rs` for the frame transform that makes it true.
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
    /// The IAU/WGCCRE rotation elements, ICRF-referenced. `None` for
    /// non-rotating frames (the Sun's spin is irrelevant here; the EMB is a
    /// barycenter, not a body).
    pub iau: Option<IauRotation>,
}

impl BodyDescriptor {
    /// The body's north pole as a unit vector in the **engine (ecliptic-Bevy)**
    /// frame at `epoch_jd` — the axis latitudes are measured about.
    ///
    /// Time-varying, because the real thing is: the lunar pole precesses on a
    /// 18.6 yr cone (that motion IS its 1.54° Cassini tilt), and Earth's pole
    /// carries the linear WGCCRE rate. It used to be a hand-typed constant
    /// documented as a "mean-of-2026 snapshot — good to ~0.1°/yr"; it is now
    /// derived from the published elements at the epoch asked for.
    ///
    /// Bodies with no [`IauRotation`] return +Y (the ecliptic pole).
    pub fn polar_axis(&self, epoch_jd: f64) -> DVec3 {
        match &self.iau {
            Some(iau) => iau.pole_bevy(epoch_jd),
            None => DVec3::Y,
        }
    }

    /// Does this body spin?
    ///
    /// This used to be a cached `rotation_rate_rad_per_day: f64` field compared
    /// against `0.0`, kept "because hot paths test it every frame" — but an
    /// `Option::is_some()` is free, and the field was a **second source of truth
    /// for the rotation model**, guarded by a consistency test. A test that exists
    /// to prove two copies of a value agree is a sign the second copy should not
    /// exist. The IAU elements are now the only place rotation lives.
    pub fn spins(&self) -> bool {
        self.iau.is_some()
    }

    /// Sidereal rotation rate (rad/day), from the IAU elements. `0` if it does not spin.
    pub fn rotation_rate_rad_per_day(&self) -> f64 {
        self.iau.as_ref().map_or(0.0, |i| i.rotation_rate_rad_per_day())
    }
}

impl CelestialBodyRegistry {
    /// Generates a manifest of the primary inner solar system bodies.
    ///
    /// **Note**: rotation is authored ONCE, as the published IAU/WGCCRE
    /// elements ([`IauRotation`]). Everything the engine consumes — the polar
    /// axis in Bevy axes, the body-fixed rotation, the spin rate — is derived
    /// from them, so there is no second, hand-maintained copy to drift.
    pub fn default_system() -> Self {
        let earth_iau = IauRotation::earth();
        let moon_iau = IauRotation::moon();
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
                    iau: None,
                },
                BodyDescriptor {
                    name: "Earth-Moon Barycenter".to_string(),
                    ephemeris_id: 3,
                    radius_m: 0.0,
                    gm: 0.0,
                    soi_radius_m: None,
                    parent_id: Some(10), // Sun
                    texture_path: None,
                    iau: None,
                },
                BodyDescriptor {
                    name: "Earth".to_string(),
                    ephemeris_id: 399,
                    radius_m: 6371.0e3,
                    gm: 3.986004418e14,
                    soi_radius_m: Some(924.0e6),
                    parent_id: Some(3), // EMB
                    texture_path: Some("textures/earth.png".to_string()),
                    // = 360.9856235 °/day. The rate was always right; the PHASE
                    // (W₀ = 190.147°, east of the equator's node on the ICRF
                    // equator) was the missing half — without it every ground
                    // station sat ~90-190° of longitude off and DSN visibility
                    // windows were wrong by ~12.7 h.
                    iau: Some(earth_iau),
                },
                BodyDescriptor {
                    name: "Moon".to_string(),
                    ephemeris_id: 301,
                    radius_m: 1737.0e3,
                    gm: 4.9048695e12,
                    soi_radius_m: Some(66.0e6),
                    parent_id: Some(3), // EMB
                    texture_path: Some("textures/moon.png".to_string()),
                    // = 13.17635815 °/day, with W₀ = 38.3213°. The 1.543°
                    // Cassini tilt of the pole is no longer a hand-typed
                    // "mean-of-2026 snapshot": it falls out of the WGCCRE E1
                    // terms at whatever epoch is asked for.
                    iau: Some(moon_iau),
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `spins()` must agree with the presence of IAU elements — the invariant
    /// that replaced the cached `rotation_rate_rad_per_day` field.
    ///
    /// (There used to be a `cached_rate_matches_the_iau_elements` test here, whose
    /// entire job was to prove two copies of the spin rate agreed. That test is
    /// gone with the copy it guarded: rotation is authored once, as the IAU
    /// elements, and everything else is derived.)
    #[test]
    fn spins_iff_the_body_has_iau_elements() {
        for b in CelestialBodyRegistry::default_system().bodies {
            assert_eq!(
                b.spins(),
                b.iau.is_some(),
                "{}: spins() must mean 'has IAU elements'",
                b.name
            );
            assert_eq!(
                b.spins(),
                b.rotation_rate_rad_per_day() != 0.0,
                "{}: a spinning body must have a non-zero rate, and vice versa",
                b.name
            );
        }
    }

    /// The derived Earth/Moon poles must still land where the hand-typed
    /// constants did (that is the regression guard on the frame transform).
    #[test]
    fn derived_poles_match_the_retired_hand_typed_values() {
        let reg = CelestialBodyRegistry::default_system();
        let earth = reg.bodies.iter().find(|b| b.ephemeris_id == 399).unwrap();
        let moon = reg.bodies.iter().find(|b| b.ephemeris_id == 301).unwrap();

        let e = earth.polar_axis(lunco_time::J2000_JD);
        assert!((e - DVec3::new(0.0, 0.917_482_1, -0.397_776_9)).length() < 1e-6, "{e:?}");

        // The retired lunar snapshot was authored for mid-2026; compare there.
        let m = moon.polar_axis(2_461_228.5);
        let retired = DVec3::new(0.012_54, 0.999_64, -0.023_83).normalize();
        let off_deg = m.dot(retired).clamp(-1.0, 1.0).acos().to_degrees();
        assert!(
            off_deg < 0.5,
            "derived lunar pole {m:?} must agree with the retired 2026 snapshot to <0.5°, off {off_deg:.3}°"
        );
    }
}

