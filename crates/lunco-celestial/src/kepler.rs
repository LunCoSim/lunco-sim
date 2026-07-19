//! Two-body Keplerian propagation (doc 43 §2.2).
//!
//! Elements are referenced to the **body's equator**: inclination is measured
//! from the body's equatorial plane and RAAN about the body's pole — the same
//! pole latitudes use in [`crate::geo`], so "i = 90°" really does fly over the
//! geographic poles of the rendered globe (`geo::tests::
//! polar_orbit_passes_over_the_geographic_poles` locks exactly that).
//!
//! [`KeplerianElements::position_bevy_m`] returns the orbit in a **pole-up
//! ORBIT frame** (pole = +Y): body-centered meters, Bevy axes. To place it,
//! lift it into the engine frame with [`crate::geo::equatorial_frame`] — which
//! tilts +Y onto the body's real pole — and only then compose the body's spin.
//! `placement::place_celestial_bound_entities` is the reference consumer.
//!
//! **Do not skip that lift.** This doc used to make the claim above while the
//! code did not: the orbit was built about +Y, `placement` cancelled the FULL
//! body rotation on top, the two collapsed, and inclination ended up measured
//! about the **ecliptic** pole. For Earth (23.44° tilt) an ISS-like i = 51.6°
//! orbit had a ground-track latitude wrong by up to ±23.4°, and RAAN was not
//! comparable to any TLE.
//!
//! Classic chain: mean anomaly → Kepler's equation (Newton) → perifocal →
//! Rz(Ω)·Rx(i)·Rz(ω) in z-up math axes → remap to Bevy (x, z, −y), identical
//! to the axis remap in `coords::ecliptic_to_bevy`.

use bevy::math::DVec3;
use bevy::prelude::*;
use std::f64::consts::TAU;

/// Classical orbital elements (meters, degrees) at a reference epoch.
#[derive(Debug, Clone, Copy, PartialEq, Reflect)]
pub struct KeplerianElements {
    pub semi_major_axis_m: f64,
    pub eccentricity: f64,
    pub inclination_deg: f64,
    /// Right ascension of the ascending node (about the body pole).
    pub raan_deg: f64,
    pub arg_periapsis_deg: f64,
    /// Mean anomaly at [`Self::epoch_jd`].
    pub mean_anomaly_deg: f64,
    pub epoch_jd: f64,
}

impl Default for KeplerianElements {
    fn default() -> Self {
        Self {
            semi_major_axis_m: 6540.0e3,
            eccentricity: 0.0,
            inclination_deg: 0.0,
            raan_deg: 0.0,
            arg_periapsis_deg: 0.0,
            mean_anomaly_deg: 0.0,
            epoch_jd: lunco_time::J2000_JD,
        }
    }
}

/// Puts an entity on a Keplerian orbit around a registry body.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct KeplerOrbit {
    /// NAIF id of the central body (301 Moon, 399 Earth).
    pub body: i32,
    pub elements: KeplerianElements,
}

/// Solve Kepler's equation M = E − e·sinE for the eccentric anomaly (Newton).
///
/// Elliptic only: `e` is clamped below 1 (parabolic/hyperbolic orbits are not
/// modeled, and `e ≥ 1` would drive the Newton derivative to zero). A solve
/// that exits the iteration budget with a residual above tolerance logs the
/// degraded result rather than returning it silently.
pub fn solve_kepler(mean_anomaly_rad: f64, e: f64) -> f64 {
    let e = e.clamp(0.0, 0.999_999);
    let m = mean_anomaly_rad.rem_euclid(TAU);
    // High-eccentricity orbits converge better seeded at π.
    let mut ecc_anom = if e > 0.8 { std::f64::consts::PI } else { m };
    for _ in 0..30 {
        let f = ecc_anom - e * ecc_anom.sin() - m;
        let fp = 1.0 - e * ecc_anom.cos();
        let step = f / fp;
        ecc_anom -= step;
        if step.abs() < 1e-13 {
            break;
        }
    }
    let residual = ecc_anom - e * ecc_anom.sin() - m;
    if residual.abs() > 1e-9 {
        warn!("[kepler] Newton solve did not converge (M={m}, e={e}, residual={residual:e})");
    }
    ecc_anom
}

impl KeplerianElements {
    /// Orbital period in seconds for central-body `gm` (m³/s²).
    pub fn period_s(&self, gm: f64) -> f64 {
        let a = self.semi_major_axis_m;
        TAU * (a * a * a / gm).sqrt()
    }

    /// Body-centered position at `epoch_jd` in **Bevy axes, meters** (pole = +Y).
    pub fn position_bevy_m(&self, gm: f64, epoch_jd: f64) -> DVec3 {
        let a = self.semi_major_axis_m;
        let e = self.eccentricity.clamp(0.0, 0.999_999);
        let n = (gm / (a * a * a)).sqrt(); // mean motion, rad/s
        let dt_s = (epoch_jd - self.epoch_jd) * 86_400.0;
        let m = self.mean_anomaly_deg.to_radians() + n * dt_s;
        let ecc_anom = solve_kepler(m, e);
        let (sin_e, cos_e) = ecc_anom.sin_cos();
        // Perifocal (z-up math axes): x toward periapsis, z = orbit normal.
        let x_pf = a * (cos_e - e);
        let y_pf = a * (1.0 - e * e).sqrt() * sin_e;

        let (sin_o, cos_o) = self.raan_deg.to_radians().sin_cos();
        let (sin_i, cos_i) = self.inclination_deg.to_radians().sin_cos();
        let (sin_w, cos_w) = self.arg_periapsis_deg.to_radians().sin_cos();

        // Rz(Ω)·Rx(i)·Rz(ω) applied to (x_pf, y_pf, 0).
        let x1 = cos_w * x_pf - sin_w * y_pf;
        let y1 = sin_w * x_pf + cos_w * y_pf;
        let y2 = cos_i * y1;
        let z2 = sin_i * y1;
        let x = cos_o * x1 - sin_o * y2;
        let y = sin_o * x1 + cos_o * y2;
        let z = z2;

        // z-up math axes → Bevy Y-up (same remap as coords::ecliptic_to_bevy).
        DVec3::new(x, z, -y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GM_MOON: f64 = 4.9048695e12;

    #[test]
    fn kepler_solver_converges() {
        for e in [0.0, 0.3, 0.6, 0.9, 0.97] {
            for m_deg in [0.0, 45.0, 123.0, 180.0, 271.0, 359.0] {
                let m = (m_deg as f64).to_radians();
                let ecc = solve_kepler(m, e);
                let back = ecc - e * ecc.sin();
                assert!(
                    (back - m.rem_euclid(TAU)).abs() < 1e-10,
                    "e={e} M={m_deg}: E={ecc} → M'={back}"
                );
            }
        }
    }

    #[test]
    fn circular_orbit_keeps_radius_and_period() {
        let el = KeplerianElements {
            semi_major_axis_m: 2000.0e3,
            eccentricity: 0.0,
            inclination_deg: 90.0,
            ..Default::default()
        };
        let period_days = el.period_s(GM_MOON) / 86_400.0;
        let p0 = el.position_bevy_m(GM_MOON, el.epoch_jd);
        assert!((p0.length() - 2000.0e3).abs() < 1.0);
        for frac in [0.25, 0.5, 0.75] {
            let p = el.position_bevy_m(GM_MOON, el.epoch_jd + frac * period_days);
            assert!((p.length() - 2000.0e3).abs() < 1.0, "radius drifted at {frac}");
        }
        // Full period returns to the start.
        let p1 = el.position_bevy_m(GM_MOON, el.epoch_jd + period_days);
        assert!((p1 - p0).length() < 10.0, "period return error {}", (p1 - p0).length());
    }

    #[test]
    fn elliptic_orbit_hits_periapsis_and_apoapsis() {
        let a = 6540.0e3;
        let e = 0.6;
        let el = KeplerianElements {
            semi_major_axis_m: a,
            eccentricity: e,
            inclination_deg: 57.7,
            arg_periapsis_deg: 90.0,
            mean_anomaly_deg: 0.0,
            ..Default::default()
        };
        let p_peri = el.position_bevy_m(GM_MOON, el.epoch_jd);
        assert!((p_peri.length() - a * (1.0 - e)).abs() < 1.0);
        let half_period_days = 0.5 * el.period_s(GM_MOON) / 86_400.0;
        let p_apo = el.position_bevy_m(GM_MOON, el.epoch_jd + half_period_days);
        assert!((p_apo.length() - a * (1.0 + e)).abs() < 1.0);
        // ω=90° with i>0 puts periapsis at +Y (north) and apoapsis south — the
        // ELFO "apolune dwells over the south pole" shape.
        assert!(p_peri.y > 0.0, "periapsis north, got {:?}", p_peri);
        assert!(p_apo.y < 0.0, "apoapsis south, got {:?}", p_apo);
    }

    #[test]
    fn inclination_bounds_out_of_plane_motion() {
        let el = KeplerianElements {
            semi_major_axis_m: 3000.0e3,
            eccentricity: 0.0,
            inclination_deg: 30.0,
            ..Default::default()
        };
        let period_days = el.period_s(GM_MOON) / 86_400.0;
        let mut max_lat: f64 = 0.0;
        for k in 0..200 {
            let p = el.position_bevy_m(GM_MOON, el.epoch_jd + period_days * (k as f64) / 200.0);
            let lat = (p.y / p.length()).asin().to_degrees();
            max_lat = max_lat.max(lat.abs());
        }
        assert!((max_lat - 30.0).abs() < 0.5, "max |lat| {max_lat} for i=30°");
    }
}
