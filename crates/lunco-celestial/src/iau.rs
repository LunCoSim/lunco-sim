//! IAU/WGCCRE body rotation models, expressed in THIS engine's frame.
//!
//! # The frame problem (read this before touching the numbers)
//!
//! WGCCRE publishes three angles per body, **all referenced to the ICRF**:
//!
//! - `α₀`, `δ₀` — right ascension / declination of the body's north pole.
//! - `W = W₀ + Ẇ·d` — the angle **from the node `Q`** to the body's prime
//!   meridian, measured eastward along the body's equator. `Q` is the ascending
//!   node of the body equator on the **ICRF equator**; it lies at
//!   `RA = α₀ + 90°, Dec = 0`.
//!
//! This codebase's world frame is NOT the ICRF. It is **ecliptic J2000 in Bevy
//! Y-up axes** (`coords::ecliptic_to_bevy`), and `geo`'s longitude zero is
//! "+X of that frame". So `W₀` **cannot be pasted in as a rotation angle about
//! +Y** — its reference direction is `Q`, not the engine's +X.
//!
//! For the **Moon** the two happen to nearly coincide (`α₀ ≈ 270°` puts `Q` at
//! `RA ≈ 0°` — the equinox, which IS the engine's +X), which is why "just use
//! 38.3°" looks right there. For the **Earth** they do not: `α₀ = 0°` puts `Q`
//! at `RA = 90°`, so the prime meridian sits at `90° + 190.147° ≈ 280.15°` from
//! the equinox. Dropping the raw `190.147` in as a spin about +Y would leave
//! Earth **~90° wrong** — a different wrong answer, not a fix.
//!
//! **The resolution implemented here is the correct one (option (a)): we never
//! treat `W₀` as an engine angle at all.** We build the body's frame from the
//! published ICRF elements — pole `p̂` and node `Q̂` — transform BOTH into the
//! engine frame with an explicit, tested ICRF→ecliptic→Bevy map
//! ([`icrf_to_bevy`]), and compose
//!
//! ```text
//! R(t) = rot(p̂, W(t)) · basis(x = Q̂, y = p̂, z = Q̂ × p̂)
//! ```
//!
//! `basis(…)` maps the body-fixed axes `geo` uses (pole = +Y, lon 0 = +X) onto
//! the body's actual pole and node; `rot(p̂, W)` then sweeps the prime meridian
//! east by exactly the published `W`. The reference-direction offset is carried
//! by `Q̂` itself, so **no per-body fudge factor exists and none is needed** —
//! the same code is correct for Earth and Moon alike.
//!
//! # What is and is not modelled
//!
//! - **Modelled:** pole precession (the linear `T` rates) and, for the Moon, the
//!   full WGCCRE `E1…E13` periodic series on `α₀`, `δ₀` **and `W`** — which is
//!   what produces the Moon's 1.54° Cassini tilt (the *mean* `α₀/δ₀` alone put
//!   its pole within 0.02° of the ECLIPTIC pole, since the pole precesses about
//!   it on an 18.6 yr cone) and its physical libration in longitude.
//! - **Not modelled:** Earth precession/nutation beyond the linear pole rates,
//!   polar motion, and UT1−TT. Earth's spin phase is good to ~arcminutes over
//!   the mission epochs this simulator runs; it is not an astrometric product.
//! - **Not modelled:** light-time and stellar aberration — see `link.rs`.

use bevy::math::{DMat3, DQuat, DVec3};
use bevy::prelude::*;

/// Mean obliquity of the ecliptic at J2000 (IAU 1976/2006), degrees. The same
/// value `lunco-celestial-ephemeris::equatorial_to_ecliptic` rotates by — the
/// two MUST agree or poles and positions land in different frames.
pub const OBLIQUITY_J2000_DEG: f64 = 23.439_281;

/// Julian days per Julian century — the unit `T` in the WGCCRE polynomials.
const DAYS_PER_CENTURY: f64 = 36_525.0;

/// Which body-specific periodic (nutation/libration) series to apply on top of
/// the linear elements.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub enum PeriodicTerms {
    /// None — the linear model is the whole model.
    #[default]
    None,
    /// The lunar `E1…E13` series (WGCCRE Table 2). Load-bearing: without it the
    /// Moon's pole collapses onto the ecliptic pole and the 1.54° Cassini tilt —
    /// which is what gives polar sites their ±2° solar-elevation season —
    /// disappears.
    Moon,
}

/// A body's WGCCRE rotation elements, **as published: ICRF-referenced**.
///
/// Nothing here is engine-frame. [`IauRotation::rotation_bevy`] is the only
/// thing that crosses into the engine frame, and it does so explicitly.
#[derive(Clone, Debug, Reflect)]
pub struct IauRotation {
    /// `α₀` at J2000, degrees (ICRF).
    pub pole_ra_deg: f64,
    /// `dα₀/dT`, degrees per Julian century.
    pub pole_ra_rate_deg_per_century: f64,
    /// `δ₀` at J2000, degrees (ICRF).
    pub pole_dec_deg: f64,
    /// `dδ₀/dT`, degrees per Julian century.
    pub pole_dec_rate_deg_per_century: f64,
    /// `W₀` — prime meridian at J2000, degrees **east of the node `Q`**, NOT of
    /// the engine's +X. See the module docs; this is the number that is
    /// dangerous to misuse.
    pub w0_deg: f64,
    /// `Ẇ`, degrees per day. Positive = prograde.
    pub w_rate_deg_per_day: f64,
    /// Body-specific periodic series.
    pub periodic: PeriodicTerms,
}

impl IauRotation {
    /// Earth (WGCCRE 2015 Report, Table 1).
    ///
    /// `α₀ = 0.00 − 0.641 T`, `δ₀ = 90.00 − 0.557 T`,
    /// `W = 190.147 + 360.9856235 d`.
    pub fn earth() -> Self {
        Self {
            pole_ra_deg: 0.00,
            pole_ra_rate_deg_per_century: -0.641,
            pole_dec_deg: 90.00,
            pole_dec_rate_deg_per_century: -0.557,
            w0_deg: 190.147,
            w_rate_deg_per_day: 360.985_623_5,
            periodic: PeriodicTerms::None,
        }
    }

    /// Moon (WGCCRE 2015 Report, Table 2 — the full `E1…E13` model).
    ///
    /// `α₀ = 269.9949 + 0.0031 T − 3.8787 sin E1 − …`,
    /// `δ₀ = 66.5392 + 0.0130 T + 1.5419 cos E1 + …`,
    /// `W  = 38.3213 + 13.17635815 d − 1.4e-12 d² + 3.5610 sin E1 + …`
    pub fn moon() -> Self {
        Self {
            pole_ra_deg: 269.9949,
            pole_ra_rate_deg_per_century: 0.0031,
            pole_dec_deg: 66.5392,
            pole_dec_rate_deg_per_century: 0.0130,
            w0_deg: 38.3213,
            w_rate_deg_per_day: 13.176_358_15,
            periodic: PeriodicTerms::Moon,
        }
    }

    /// The body's spin rate in radians per day (the engine's cached form).
    pub fn rotation_rate_rad_per_day(&self) -> f64 {
        self.w_rate_deg_per_day.to_radians()
    }

    /// `(α₀, δ₀, W)` in **radians**, evaluated at `epoch_jd` (ICRF).
    pub fn elements_rad(&self, epoch_jd: f64) -> (f64, f64, f64) {
        let d = epoch_jd - lunco_time::J2000_JD;
        let t = d / DAYS_PER_CENTURY;

        let mut ra = self.pole_ra_deg + self.pole_ra_rate_deg_per_century * t;
        let mut dec = self.pole_dec_deg + self.pole_dec_rate_deg_per_century * t;
        let mut w = self.w0_deg + self.w_rate_deg_per_day * d;

        if self.periodic == PeriodicTerms::Moon {
            // WGCCRE lunar arguments E1..E13 (degrees), functions of d.
            let e = |a: f64, b: f64| (a + b * d).to_radians();
            let e1 = e(125.045, -0.052_992_1);
            let e2 = e(250.089, -0.105_984_2);
            let e3 = e(260.008, 13.012_000_9);
            let e4 = e(176.625, 13.340_715_4);
            let e5 = e(357.529, 0.985_600_3);
            let e6 = e(311.589, 26.405_708_4);
            let e7 = e(134.963, 13.064_993_0);
            let e8 = e(276.617, 0.328_714_6);
            let e9 = e(34.226, 1.748_487_7);
            let e10 = e(15.134, -0.158_976_3);
            let e11 = e(119.743, 0.003_609_6);
            let e12 = e(239.961, 0.164_357_3);
            let e13 = e(25.053, 12.959_008_8);

            ra += -3.8787 * e1.sin() - 0.1204 * e2.sin() + 0.0700 * e3.sin() - 0.0172 * e4.sin()
                + 0.0072 * e6.sin()
                - 0.0052 * e10.sin()
                + 0.0043 * e13.sin();

            dec += 1.5419 * e1.cos() + 0.0239 * e2.cos() - 0.0278 * e3.cos() + 0.0068 * e4.cos()
                - 0.0029 * e6.cos()
                + 0.0009 * e7.cos()
                + 0.0008 * e10.cos()
                - 0.0009 * e13.cos();

            // The d² term and the E-series on W: the Moon's physical libration
            // in longitude. Small (≤ 0.13°) but it is exactly the term that
            // makes "the near side faces Earth" true to arcminutes.
            w += -1.4e-12 * d * d + 3.5610 * e1.sin() + 0.1208 * e2.sin() - 0.0642 * e3.sin()
                + 0.0158 * e4.sin()
                + 0.0252 * e5.sin()
                - 0.0066 * e6.sin()
                - 0.0047 * e7.sin()
                - 0.0046 * e8.sin()
                + 0.0028 * e9.sin()
                + 0.0052 * e10.sin()
                + 0.0040 * e11.sin()
                + 0.0019 * e12.sin()
                - 0.0044 * e13.sin();
        }

        (ra.to_radians(), dec.to_radians(), w.to_radians())
    }

    /// The body's north pole as a unit vector in the **engine (ecliptic-Bevy)**
    /// frame at `epoch_jd`. This is the `polar_axis` every latitude is measured
    /// about, now DERIVED rather than hand-typed.
    pub fn pole_bevy(&self, epoch_jd: f64) -> DVec3 {
        let (ra, dec, _) = self.elements_rad(epoch_jd);
        icrf_to_bevy(unit_from_ra_dec(ra, dec))
    }

    /// Rotation taking a body-fixed vector (pole = +Y, lon 0 = +X, east toward
    /// −Z — the [`crate::geo`] convention) into the engine frame at `epoch_jd`.
    ///
    /// `R = rot(pole, W) · basis(Q, pole)`. See the module docs.
    pub fn rotation_bevy(&self, epoch_jd: f64) -> DQuat {
        let (ra, dec, w) = self.elements_rad(epoch_jd);
        let pole = icrf_to_bevy(unit_from_ra_dec(ra, dec)).normalize();
        // Q — the node of the body equator on the ICRF equator, at
        // (RA = α₀ + 90°, Dec = 0). W is measured east from HERE.
        let node = icrf_to_bevy(unit_from_ra_dec(ra + std::f64::consts::FRAC_PI_2, 0.0));

        // Body-fixed basis at W = 0: +X on the node, +Y on the pole.
        // Re-orthogonalize X against the pole (the two are perpendicular in
        // exact arithmetic; this only removes rounding).
        let x = (node - pole * node.dot(pole)).normalize();
        let z = x.cross(pole);
        let basis = DQuat::from_mat3(&DMat3::from_cols(x, pole, z));

        // Sweep the prime meridian east by W. `geo`'s east-positive convention
        // is exactly the direction a positive rotation about the pole carries
        // +X, so a positive (prograde) Ẇ advances east longitude — the property
        // `geo::tests::east_longitude_advances_with_body_rotation` locks.
        DQuat::from_axis_angle(pole, w) * basis
    }
}

/// Unit vector from right ascension / declination (radians), in the frame those
/// angles are measured in.
fn unit_from_ra_dec(ra: f64, dec: f64) -> DVec3 {
    let (sin_ra, cos_ra) = ra.sin_cos();
    let (sin_dec, cos_dec) = dec.sin_cos();
    DVec3::new(cos_dec * cos_ra, cos_dec * sin_ra, sin_dec)
}

/// **ICRF/equatorial → engine (ecliptic J2000, Bevy Y-up).** The explicit frame
/// transform the whole IAU story hangs on.
///
/// Two steps, both already load-bearing elsewhere in this codebase:
/// 1. equatorial → ecliptic: rotate about +X by the obliquity ε (identical to
///    `lunco-celestial-ephemeris::equatorial_to_ecliptic`, which is what puts
///    the *positions* in this frame).
/// 2. ecliptic → Bevy: the pure axis swap `(x, z, −y)` from
///    `coords::ecliptic_to_bevy` (Bevy Y = ecliptic north).
pub fn icrf_to_bevy(p: DVec3) -> DVec3 {
    let (sin_e, cos_e) = OBLIQUITY_J2000_DEG.to_radians().sin_cos();
    // 1. equatorial → ecliptic.
    let ecl = DVec3::new(p.x, p.y * cos_e + p.z * sin_e, -p.y * sin_e + p.z * cos_e);
    // 2. ecliptic → Bevy Y-up.
    DVec3::new(ecl.x, ecl.z, -ecl.y)
}

/// Exact inverse of [`icrf_to_bevy`] — engine frame back to ICRF/equatorial.
///
/// Needed whenever a result has to be compared against a published,
/// equatorially-referenced quantity (a right ascension, a TLE's RAAN). Angles
/// measured by projecting an equatorial vector into the ECLIPTIC plane are NOT
/// right ascensions: the 23.44° tilt skews them by up to ~1° (that skew is a
/// real trap — it produced a spurious 281.04° for a prime meridian whose RA is
/// 280.15°).
pub fn bevy_to_icrf(p: DVec3) -> DVec3 {
    let (sin_e, cos_e) = OBLIQUITY_J2000_DEG.to_radians().sin_cos();
    // 1. Bevy Y-up → ecliptic.
    let ecl = DVec3::new(p.x, -p.z, p.y);
    // 2. ecliptic → equatorial.
    DVec3::new(
        ecl.x,
        ecl.y * cos_e - ecl.z * sin_e,
        ecl.y * sin_e + ecl.z * cos_e,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ICRF→engine map must reproduce, from the published `α₀ = 0, δ₀ = 90`,
    /// the Earth polar axis this codebase previously HAND-TYPED as an ecliptic
    /// snapshot: `(0, 0.9174821, −0.3977769)` (obliquity tilt toward −Z).
    ///
    /// This is the pin that proves the frame transform is the right one — the
    /// derived value and the independently hand-derived value agree.
    #[test]
    fn icrf_to_bevy_reproduces_the_hand_typed_earth_pole() {
        let pole = IauRotation::earth().pole_bevy(lunco_time::J2000_JD);
        let hand_typed = DVec3::new(0.0, 0.917_482_1, -0.397_776_9);
        assert!(
            (pole - hand_typed).length() < 1e-6,
            "derived Earth pole {pole:?} must match the hand-typed ecliptic snapshot {hand_typed:?}"
        );
    }

    /// The Moon's pole must sit ~1.54° off the ecliptic pole (+Y) — Cassini's
    /// second law. This is produced ENTIRELY by the `E1` periodic terms: the
    /// mean α₀/δ₀ alone give a pole within 0.02° of +Y, and dropping the series
    /// would silently flatten the lunar obliquity (and with it the ±2° solar
    /// elevation season at Shackleton).
    #[test]
    fn moon_pole_has_the_cassini_tilt() {
        let moon = IauRotation::moon();
        for jd in [2_451_545.0, 2_461_228.5, 2_465_000.0] {
            let pole = moon.pole_bevy(jd);
            let tilt_deg = pole.dot(DVec3::Y).clamp(-1.0, 1.0).acos().to_degrees();
            assert!(
                (tilt_deg - 1.543).abs() < 0.10,
                "lunar pole must sit 1.543° from the ecliptic pole (Cassini), got {tilt_deg:.3}° at JD {jd}"
            );
        }

        // …and the mean elements ALONE do not: proof the series is load-bearing.
        let mean_only = IauRotation {
            periodic: PeriodicTerms::None,
            ..IauRotation::moon()
        };
        let flat = mean_only.pole_bevy(lunco_time::J2000_JD);
        let flat_tilt = flat.dot(DVec3::Y).clamp(-1.0, 1.0).acos().to_degrees();
        assert!(
            flat_tilt < 0.1,
            "without E1..E13 the lunar pole collapses onto the ecliptic pole \
             (got {flat_tilt:.3}°) — that is why PeriodicTerms::Moon exists"
        );
    }

    /// `rotation_bevy` must be a proper rotation that puts the body-fixed pole
    /// (+Y) exactly on the derived pole, at any epoch.
    #[test]
    fn rotation_carries_the_body_pole_onto_the_iau_pole() {
        for iau in [IauRotation::earth(), IauRotation::moon()] {
            for jd in [2_451_545.0, 2_461_228.5, 2_461_228.5 + 137.4] {
                let r = iau.rotation_bevy(jd);
                let mapped = r * DVec3::Y;
                let pole = iau.pole_bevy(jd);
                assert!(
                    (mapped - pole).length() < 1e-9,
                    "R·(+Y) must be the pole: {mapped:?} vs {pole:?}"
                );
                // Proper rotation: length-preserving, right-handed.
                assert!(((r * DVec3::X).cross(r * DVec3::Y) - r * DVec3::Z).length() < 1e-9);
            }
        }
    }

    /// **The frame-problem regression test.** Earth's prime meridian must sit at
    /// **right ascension `α₀ + 90° + W₀` = 280.147°** at J2000 — NOT at the raw
    /// `W₀ = 190.147°`.
    ///
    /// This is the exact trap "paste W₀ in as the spin angle" walks into: `W₀` is
    /// published east of the NODE `Q` (which for Earth sits at RA = α₀+90° = 90°,
    /// nowhere near the engine's +X), so using it directly leaves Earth **90°
    /// wrong** — one wrong phase traded for another. 280.147° is independently
    /// recognisable: it is Earth's rotation angle / GMST at J2000 (≈280.46°).
    ///
    /// RA is an angle **in the equatorial plane**, so the check goes back through
    /// [`bevy_to_icrf`]. Measuring the same vector in the ECLIPTIC plane instead
    /// reads 281.04° — the 23.44° tilt skews that projection by ~0.9°. (That skew
    /// is not academic: it is what this test measured on its first run.)
    #[test]
    fn earth_prime_meridian_is_at_ra_280_deg_not_190() {
        let earth = IauRotation::earth();
        // Where the lon-0 meridian points at J2000 — engine axes, back to ICRF.
        let pm = bevy_to_icrf(earth.rotation_bevy(lunco_time::J2000_JD) * DVec3::X);
        let ra = pm.y.atan2(pm.x).to_degrees().rem_euclid(360.0);
        let expected = earth.pole_ra_deg + 90.0 + earth.w0_deg; // 280.147
        assert!(
            (ra - expected).abs() < 1e-6,
            "Earth's prime meridian must be at RA α₀+90°+W₀ = {expected:.3}° \
             (≈ GMST at J2000), got {ra:.3}° — ~190° means someone pasted the \
             ICRF W₀ in as an engine spin angle"
        );
        assert!((expected - 280.147).abs() < 1e-9);
    }

    /// `bevy_to_icrf` really is the inverse of `icrf_to_bevy`.
    #[test]
    fn icrf_round_trips() {
        for v in [
            DVec3::X,
            DVec3::Y,
            DVec3::Z,
            DVec3::new(0.3, -0.5, 0.81).normalize(),
        ] {
            let back = bevy_to_icrf(icrf_to_bevy(v));
            assert!((back - v).length() < 1e-12, "{v:?} → {back:?}");
        }
    }

    /// Sanity on the published rates: they must reproduce the sidereal periods.
    #[test]
    fn spin_rates_match_the_sidereal_periods() {
        let earth_days = 360.0 / IauRotation::earth().w_rate_deg_per_day;
        assert!((earth_days - 0.997_269_68).abs() < 1e-6, "{earth_days}");
        let moon_days = 360.0 / IauRotation::moon().w_rate_deg_per_day;
        assert!((moon_days - 27.321_661).abs() < 1e-4, "{moon_days}");
    }
}
