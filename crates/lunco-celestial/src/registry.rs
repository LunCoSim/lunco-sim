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
//! ## Reference Frame Semantics
//! [`ReferenceFrame`] states the centre and orientation of a coordinate value.
//! A scene or spatial adapter may associate that semantic value with its own
//! storage representation, but the analytical catalog does not own that
//! representation.

use bevy::math::DVec3;
use bevy::prelude::*;

use crate::iau::IauRotation;

/// Canonical NAIF ephemeris identifiers used by the built-in solar-system
/// projection.
///
/// Runtime code must not repeat opaque integer literals for known bodies: a
/// frame tagged with the wrong integer is structurally valid but belongs to a
/// different body.
pub mod ephemeris_id {
    /// Solar-system barycentre.
    pub const SOLAR_SYSTEM_BARYCENTER: i32 = 0;
    /// Earth-Moon barycentre.
    pub const EARTH_MOON_BARYCENTER: i32 = 3;
    /// Sun.
    pub const SUN: i32 = 10;
    /// Moon.
    pub const MOON: i32 = 301;
    /// Earth.
    pub const EARTH: i32 = 399;
}

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

/// **The** lunar radius, in metres — declared once in [`lunco_core`] and
/// re-exported here, where the simulation side has always named it.
///
/// The number itself lives in `lunco-core` because the offline `lunco-assets`
/// build tool stamps the same datum into every baked GeoTIFF and must not take
/// a Bevy-heavy dependency on this crate to reach it. See
/// [`lunco_core::MOON_MEAN_RADIUS_M`] for the IAU/WGCCRE citation and the
/// history. Do not re-type the value anywhere.
pub use lunco_core::MOON_MEAN_RADIUS_M;

/// Semantic identity of a celestial coordinate frame.
///
/// Runtime adapters may associate this identity with their own storage
/// hierarchy. The centre and orientation are deliberately one value so a
/// frame cannot simultaneously acquire conflicting "fixed" and "inertial"
/// tags. User-facing placement declares one of these frames; engine code
/// resolves it and performs the f64 transform before handing the result to a
/// runtime representation.
#[derive(
    Component,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Reflect,
    serde::Serialize,
    serde::Deserialize,
)]
#[reflect(Component)]
pub enum ReferenceFrame {
    /// The persistent world frame used by scenes with no celestial frame.
    /// This is a real semantic frame, not an implicit "whatever grid happens
    /// to be current" fallback.
    World,
    /// Ecliptic J2000 axes, translated to the named centre. The axes do not
    /// rotate with the body.
    EclipticJ2000 { center: i32 },
    /// IAU/WGCCRE body-fixed axes rotating with the named body.
    BodyFixed { body: i32 },
}

impl ReferenceFrame {
    /// NAIF ephemeris id at this frame's origin, when it has one.
    pub const fn center(self) -> Option<i32> {
        match self {
            Self::World => None,
            Self::EclipticJ2000 { center } => Some(center),
            Self::BodyFixed { body } => Some(body),
        }
    }

    /// Whether this frame rotates with a physical body.
    pub const fn body_fixed(self) -> Option<i32> {
        match self {
            Self::BodyFixed { body } => Some(body),
            Self::World | Self::EclipticJ2000 { .. } => None,
        }
    }
}

/// Semantic frame inherited by `entity` from its nearest tagged ancestor.
///
/// Precision sub-grids deliberately do not duplicate [`ReferenceFrame`]: a
/// lunar surface grid is still Moon-fixed, and repeating the tag would create
/// two owners for one semantic frame. Every consumer uses this ancestry walk
/// instead of assuming that a grid-direct entity's immediate parent carries
/// the tag.
pub fn inherited_reference_frame(
    entity: Entity,
    q_parents: &Query<&ChildOf>,
    q_frames: &Query<&ReferenceFrame>,
) -> Option<ReferenceFrame> {
    let mut current = entity;
    for _ in 0..32 {
        if let Ok(frame) = q_frames.get(current) {
            return Some(*frame);
        }
        current = q_parents.get(current).ok()?.parent();
    }
    None
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
    /// The IAU/WGCCRE rotation elements, ICRF-referenced. `None` for
    /// non-rotating frames (the Sun's spin is irrelevant here; the EMB is a
    /// barycenter, not a body).
    pub iau: Option<IauRotation>,
}

impl BodyDescriptor {
    /// Runtime body component derived from this catalog entry.
    ///
    /// Spawners use this instead of repeating name/id/radius triples. Gravity,
    /// rotation and SOI remain on the descriptor because they are services of
    /// the reference-frame catalog, not render-entity identity.
    pub fn body_component(&self) -> CelestialBody {
        CelestialBody {
            name: self.name.clone(),
            ephemeris_id: self.ephemeris_id,
            radius_m: self.radius_m,
        }
    }

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
        self.iau
            .as_ref()
            .map_or(0.0, |i| i.rotation_rate_rad_per_day())
    }
}

impl CelestialBodyRegistry {
    /// The body with this NAIF id, or `None` if the registry does not carry it.
    ///
    /// The lookup every caller was open-coding as
    /// `bodies.iter().find(|b| b.ephemeris_id == id)` — `pose.rs`, `link.rs` and
    /// `transform.rs` each had their own copy. `None` means "not in the registry",
    /// which callers must treat as "skip", never as a body at the origin.
    pub fn get(&self, ephemeris_id: i32) -> Option<&BodyDescriptor> {
        self.bodies.iter().find(|b| b.ephemeris_id == ephemeris_id)
    }

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
                    ephemeris_id: ephemeris_id::SUN,
                    radius_m: 695_700_000.0,
                    gm: 1.327_124_400_18e20,
                    soi_radius_m: None,
                    parent_id: None,
                    iau: None,
                },
                BodyDescriptor {
                    name: "Earth-Moon Barycenter".to_string(),
                    ephemeris_id: ephemeris_id::EARTH_MOON_BARYCENTER,
                    radius_m: 0.0,
                    gm: 0.0,
                    soi_radius_m: None,
                    parent_id: Some(ephemeris_id::SUN),
                    iau: None,
                },
                BodyDescriptor {
                    name: "Earth".to_string(),
                    ephemeris_id: ephemeris_id::EARTH,
                    radius_m: 6371.0e3,
                    gm: 3.986004418e14,
                    soi_radius_m: Some(924.0e6),
                    parent_id: Some(ephemeris_id::EARTH_MOON_BARYCENTER),
                    // = 360.9856235 °/day. The rate was always right; the PHASE
                    // (W₀ = 190.147°, east of the equator's node on the ICRF
                    // equator) was the missing half — without it every ground
                    // station sat ~90-190° of longitude off and DSN visibility
                    // windows were wrong by ~12.7 h.
                    iau: Some(earth_iau),
                },
                BodyDescriptor {
                    name: "Moon".to_string(),
                    ephemeris_id: ephemeris_id::MOON,
                    radius_m: MOON_MEAN_RADIUS_M,
                    gm: 4.9048695e12,
                    soi_radius_m: Some(66.0e6),
                    parent_id: Some(ephemeris_id::EARTH_MOON_BARYCENTER),
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
        let earth = reg
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == ephemeris_id::EARTH)
            .unwrap();
        let moon = reg
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == ephemeris_id::MOON)
            .unwrap();

        let e = earth.polar_axis(lunco_time::J2000_JD);
        assert!(
            (e - DVec3::new(0.0, 0.917_482_1, -0.397_776_9)).length() < 1e-6,
            "{e:?}"
        );

        // The retired lunar snapshot was authored for mid-2026; compare there.
        let m = moon.polar_axis(2_461_228.5);
        let retired = DVec3::new(0.012_54, 0.999_64, -0.023_83).normalize();
        let off_deg = m.dot(retired).clamp(-1.0, 1.0).acos().to_degrees();
        assert!(
            off_deg < 0.5,
            "derived lunar pole {m:?} must agree with the retired 2026 snapshot to <0.5°, off {off_deg:.3}°"
        );
    }

    #[test]
    fn builtin_catalog_has_unique_complete_identity_and_parent_links() {
        let registry = CelestialBodyRegistry::default_system();
        let mut ids = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for body in &registry.bodies {
            assert!(
                ids.insert(body.ephemeris_id),
                "duplicate NAIF id {}",
                body.ephemeris_id
            );
            assert!(
                names.insert(body.name.as_str()),
                "duplicate body name {}",
                body.name
            );
            assert!(!body.name.trim().is_empty());
            assert!(body.radius_m >= 0.0 && body.radius_m.is_finite());
            assert!(body.gm >= 0.0 && body.gm.is_finite());
            if let Some(parent) = body.parent_id {
                assert!(
                    registry.get(parent).is_some(),
                    "{} names missing parent NAIF {}",
                    body.name,
                    parent
                );
            }
        }
        for id in [
            ephemeris_id::SUN,
            ephemeris_id::EARTH_MOON_BARYCENTER,
            ephemeris_id::EARTH,
            ephemeris_id::MOON,
        ] {
            assert!(registry.get(id).is_some(), "missing built-in NAIF {id}");
        }
    }
}
