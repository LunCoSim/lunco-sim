//! # lunco-celestial-ephemeris
//!
//! Analytic planetary position provider for `lunco-celestial`.
//!
//! This crate is the heavy half of the celestial split: it pulls in
//! `celestial-ephemeris` (VSOP2013 + ELP/MPP02), `celestial-time`, and
//! `celestial-core` — none of which build on Windows MSVC because
//! `celestial-eop-data`'s `build.rs` shells out to the Unix `date`
//! command.
//!
//! Apps that need analytic planetary positions add [`EphemerisPlugin`]. USD
//! scene animation remains authored as ordinary xform time samples.

use bevy::math::DVec3;
use bevy::prelude::*;
use celestial_core::Vector3;
use celestial_ephemeris::{Vsop2013Earth, Vsop2013Sun, moon::ElpMpp02Moon, planets::Vsop2013Emb};
use celestial_time::TDB;
use celestial_time::julian::JulianDate;
use lunco_celestial::ephemeris_id::{EARTH, EARTH_MOON_BARYCENTER, MOON, SUN};
use lunco_celestial::frames::{EclipticAu, IcrfAu};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use lunco_celestial::ephemeris::{EphemerisProvider, EphemerisResource};

/// Reuses the small analytic body set when frame consumers ask for the same
/// exact ephemeris sample during one render/physics transaction.
#[derive(Default)]
struct EpochPositionCache {
    epoch_bits: Option<u64>,
    positions: HashMap<i32, Option<EclipticAu>>,
}

impl EpochPositionCache {
    fn get_or_evaluate(
        &mut self,
        body_id: i32,
        epoch_jd: f64,
        evaluate: impl FnOnce() -> Option<EclipticAu>,
    ) -> Option<EclipticAu> {
        let epoch_bits = epoch_jd.to_bits();
        if self.epoch_bits != Some(epoch_bits) {
            self.epoch_bits = Some(epoch_bits);
            self.positions.clear();
        }
        if let Some(position) = self.positions.get(&body_id) {
            return *position;
        }
        let position = evaluate();
        self.positions.insert(body_id, position);
        position
    }
}

/// Concrete implementation of the analytic [`EphemerisProvider`].
pub struct CelestialEphemerisProvider {
    _sun: Vsop2013Sun,
    earth: Vsop2013Earth,
    emb: Vsop2013Emb,
    moon: ElpMpp02Moon,
    /// Parent relationships from the canonical body catalog.
    parents: HashMap<i32, i32>,
    /// Same-epoch consumers share ELP/MPP02 and VSOP evaluations.
    position_cache: Mutex<EpochPositionCache>,
}

/// The parent tree, straight out of the body registry — no second copy.
fn parents_from_registry() -> HashMap<i32, i32> {
    lunco_celestial::CelestialBodyRegistry::default_system()
        .bodies
        .iter()
        .filter_map(|b| b.parent_id.map(|p| (b.ephemeris_id, p)))
        .collect()
}

impl CelestialEphemerisProvider {
    /// Build the analytic provider with the canonical body's parent tree.
    pub fn new() -> Self {
        Self {
            _sun: Vsop2013Sun,
            earth: Vsop2013Earth::new(),
            emb: Vsop2013Emb,
            moon: ElpMpp02Moon::new(),
            parents: parents_from_registry(),
            position_cache: Mutex::default(),
        }
    }
}

impl Default for CelestialEphemerisProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod position_cache_tests {
    use super::*;

    #[test]
    fn position_cache_reuses_exact_epoch_results_and_replaces_stale_epoch() {
        let mut cache = EpochPositionCache::default();
        let mut evaluations = 0;
        let epoch = 2_451_545.0;
        let expected = Some(EclipticAu::new(DVec3::new(1.0, 2.0, 3.0)));

        let first = cache.get_or_evaluate(MOON, epoch, || {
            evaluations += 1;
            expected
        });
        let repeated = cache.get_or_evaluate(MOON, epoch, || {
            evaluations += 1;
            expected
        });

        assert_eq!(first.map(EclipticAu::raw), expected.map(EclipticAu::raw));
        assert_eq!(
            repeated.map(EclipticAu::raw),
            expected.map(EclipticAu::raw)
        );
        assert_eq!(evaluations, 1, "same-epoch Moon work must be reused");

        let next_epoch = epoch + 1.0;
        let next = cache.get_or_evaluate(MOON, next_epoch, || {
            evaluations += 1;
            None
        });
        let repeated_missing = cache.get_or_evaluate(MOON, next_epoch, || {
            evaluations += 1;
            expected
        });

        assert!(next.is_none());
        assert!(repeated_missing.is_none());
        assert_eq!(evaluations, 2, "a new epoch recomputes, including None");
    }
}

/// Rotate an **equatorial / ICRS** rectangular vector into **ecliptic J2000**
/// (rotation about +X by the J2000 mean obliquity).
///
/// The typed ICRF input and ecliptic output keep provider coordinates aligned
/// with downstream geodesy. The obliquity is shared with the IAU pole transform.
pub fn equatorial_to_ecliptic(p: IcrfAu) -> EclipticAu {
    let epsilon = lunco_celestial::iau::OBLIQUITY_J2000_DEG.to_radians();
    let (sin_e, cos_e) = epsilon.sin_cos();
    let p = p.raw();
    EclipticAu::new(DVec3::new(
        p.x,
        p.y * cos_e + p.z * sin_e,
        -p.y * sin_e + p.z * cos_e,
    ))
}

impl CelestialEphemerisProvider {
    /// Return no position when the analytical library cannot evaluate this epoch.
    fn emb_heliocentric(&self, tdb: &TDB) -> Option<Vector3> {
        self.emb.heliocentric_position(tdb).ok().or_else(|| {
            bevy::log::warn_once!(
                "[ephemeris] VSOP2013 EMB evaluation failed — Earth and Moon will not be placed."
            );
            None
        })
    }

    fn earth_heliocentric(&self, tdb: &TDB) -> Option<Vector3> {
        self.earth.heliocentric_position(tdb).ok().or_else(|| {
            bevy::log::warn_once!(
                "[ephemeris] VSOP2013 Earth evaluation failed — Earth and Moon will not be placed."
            );
            None
        })
    }

    fn moon_geocentric_icrs(&self, tdb: &TDB) -> Option<[f64; 3]> {
        let _span = bevy::log::info_span!("celestial_ephemeris_elp_mpp02").entered();
        self.moon.geocentric_position_icrs(tdb).ok().or_else(|| {
            bevy::log::warn_once!(
                "[ephemeris] ELP/MPP02 Moon evaluation failed — the Moon will not be placed."
            );
            None
        })
    }
}

impl EphemerisProvider for CelestialEphemerisProvider {
    fn parent_id(&self, body_id: i32) -> Option<i32> {
        self.parents.get(&body_id).copied()
    }

    fn position(&self, body_id: i32, epoch_jd: f64) -> Option<EclipticAu> {
        if !epoch_jd.is_finite() {
            return None;
        }
        if body_id == SUN {
            return Some(EclipticAu::ZERO);
        }
        if !matches!(body_id, EARTH_MOON_BARYCENTER | EARTH | MOON) {
            return self.evaluate_position(body_id, epoch_jd);
        }
        let mut cache = self
            .position_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.get_or_evaluate(body_id, epoch_jd, || {
            self.evaluate_position(body_id, epoch_jd)
        })
    }

    fn maximum_angular_rate_rad_per_day(&self) -> f64 {
        std::f64::consts::TAU / 27.321_661
    }
}

impl CelestialEphemerisProvider {
    fn evaluate_position(&self, body_id: i32, epoch_jd: f64) -> Option<EclipticAu> {
        let _span = bevy::log::info_span!("celestial_ephemeris_position", body_id).entered();
        let julian = JulianDate::new(epoch_jd, 0.0);
        let tdb = TDB::from_julian_date(julian);

        match body_id {
            EARTH_MOON_BARYCENTER => {
                let p = self.emb_heliocentric(&tdb)?;
                Some(equatorial_to_ecliptic(IcrfAu::new(DVec3::new(
                    p.x, p.y, p.z,
                ))))
            }
            EARTH => {
                let p_emb = self.emb_heliocentric(&tdb)?;
                let p_earth = self.earth_heliocentric(&tdb)?;
                Some(equatorial_to_ecliptic(IcrfAu::new(DVec3::new(
                    p_earth.x - p_emb.x,
                    p_earth.y - p_emb.y,
                    p_earth.z - p_emb.z,
                ))))
            }
            MOON => {
                let p_m_geo_arr = self.moon_geocentric_icrs(&tdb)?;
                const AU_KM: f64 = 149_597_870.7;
                let p_m_geo_au = equatorial_to_ecliptic(IcrfAu::new(DVec3::new(
                    p_m_geo_arr[0] / AU_KM,
                    p_m_geo_arr[1] / AU_KM,
                    p_m_geo_arr[2] / AU_KM,
                )));

                let p_emb = self.emb_heliocentric(&tdb)?;
                let p_earth = self.earth_heliocentric(&tdb)?;
                let p_earth_rel_emb = equatorial_to_ecliptic(IcrfAu::new(DVec3::new(
                    p_earth.x - p_emb.x,
                    p_earth.y - p_emb.y,
                    p_earth.z - p_emb.z,
                )));

                Some(p_m_geo_au + p_earth_rel_emb)
            }
            other_id => {
                bevy::log::warn_once!("[ephemeris] no analytic position for NAIF id {other_id}.");
                None
            }
        }
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use lunco_celestial::{CelestialBodyRegistry, Geodetic, solar_tangent_frame};

    /// Shared coordinate conversion used by the provider's consumers.
    use lunco_celestial::coords::ecliptic_to_bevy;

    /// End-to-end frame check: with the provider's equatorial→ecliptic
    /// conversion and the tilt-aware geodesy, the sun's elevation at the
    /// Shackleton site must stay GRAZING (bounded by the moon axis tilt +
    /// site colatitude, ~±2.5°) and must actually rise above +1° at some
    /// epoch within a year.
    #[test]
    fn shackleton_sun_stays_grazing_and_gets_lit_epochs() {
        let provider = CelestialEphemerisProvider::new();
        let registry = CelestialBodyRegistry::default_system();
        let moon = registry
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == 301)
            .unwrap();
        let site = Geodetic::new(-89.45, -136.7, 1200.0);

        let mut best = (0.0_f64, f64::MIN);
        for step in 0..=(366 * 4) {
            let jd = 2461228.5 + step as f64 * 0.25; // 6 h steps from 2026-07-07
            let p_moon = provider
                .global_position(MOON, jd)
                .expect("VSOP/ELP always have the Moon");
            let center_m = ecliptic_to_bevy(p_moon).raw();
            let frame = solar_tangent_frame(moon, &site, center_m, jd);
            let to_sun = ecliptic_to_bevy(-p_moon).normalize().raw();
            let elev_deg = to_sun.dot(frame.up).clamp(-1.0, 1.0).asin().to_degrees();
            assert!(
                elev_deg.abs() < 2.5,
                "polar sun must graze (|elev| < 2.5°), got {elev_deg:.2}° at JD {jd:.2}"
            );
            if elev_deg > best.1 {
                best = (jd, elev_deg);
            }
        }
        println!(
            "best lit epoch: JD {:.2} (elevation {:.3}°)",
            best.0, best.1
        );
        assert!(
            best.1 > 1.0,
            "Shackleton should reach >1° sun elevation within a year; best {:.3}°",
            best.1
        );
    }

    /// The tidally locked Moon keeps the sub-Earth point near lunar longitude
    /// zero, allowing for optical libration.
    #[test]
    fn moon_near_side_faces_earth_across_epochs() {
        use lunco_celestial::{body_fixed_to_geodetic, body_rotation};

        let provider = CelestialEphemerisProvider::new();
        let registry = CelestialBodyRegistry::default_system();
        let moon = registry
            .bodies
            .iter()
            .find(|b| b.ephemeris_id == 301)
            .unwrap();

        let mut worst = (0.0_f64, 0.0_f64);
        // ~14 months at 11-day steps: samples every phase of the libration cycle.
        for step in 0..40 {
            let jd = 2_451_545.0 + step as f64 * 11.0;

            // Earth as seen from the Moon, in the engine (ecliptic-Bevy) frame.
            let to_earth = ecliptic_to_bevy(
                provider.global_position(EARTH, jd).expect("Earth")
                    - provider.global_position(MOON, jd).expect("Moon"),
            )
            .normalize()
            .raw();

            // Into the Moon's body-fixed frame → the sub-Earth geodetic point.
            let body_fixed = body_rotation(moon, jd).inverse() * to_earth;
            let sub_earth = body_fixed_to_geodetic(body_fixed, 1.0);

            // Longitude is the one the missing W₀ destroyed. (Latitude librates
            // ±6.7° too, from the 1.54° pole tilt + the 5.1° orbit inclination.)
            let lon = sub_earth.lon_deg;
            assert!(
                lon.abs() < 10.0,
                "the sub-Earth point must stay near lunar lon 0 (tidal lock; \
                 optical libration is ±8°), got {lon:.2}° at JD {jd:.1} — \
                 ≈38° means the W₀ prime-meridian epoch went missing again"
            );
            assert!(
                sub_earth.lat_deg.abs() < 10.0,
                "sub-Earth latitude librates ±6.7°, got {:.2}° at JD {jd:.1}",
                sub_earth.lat_deg
            );
            if lon.abs() > worst.1.abs() {
                worst = (jd, lon);
            }
        }
        println!(
            "worst sub-Earth longitude: {:.2}° at JD {:.1}",
            worst.1, worst.0
        );

        // And it must genuinely LIBRATE, not be pinned at 0 by a degenerate
        // model that happens to satisfy the bound.
        assert!(
            worst.1.abs() > 1.0,
            "the sub-Earth longitude should librate by several degrees; \
             |max| was only {:.3}°",
            worst.1.abs()
        );
    }
}

/// Install the analytic planetary provider for the celestial contract.
///
/// ```ignore
/// app.add_plugins(lunco_celestial_spatial::CelestialPlugin)
///    .add_plugins(lunco_celestial_ephemeris::EphemerisPlugin);
/// ```
pub struct EphemerisPlugin;

impl Plugin for EphemerisPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(EphemerisResource {
            provider: Arc::new(CelestialEphemerisProvider::new()),
        });
    }
}
