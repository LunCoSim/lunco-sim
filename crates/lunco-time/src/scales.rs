//! Time-scale projections (architecture doc 19 — T3).
//!
//! The master clock epoch is **TDB** (Barycentric Dynamical Time — the ephemeris
//! input). The civil / atomic / rotational scales (UTC, TAI, TT, UT1) and
//! Greenwich Mean Sidereal Time are *derived* from it via the zero-dependency
//! `celestial-time` crate, wrapped here so the rest of the workspace reads plain
//! `f64` Julian Dates / radians and never imports `celestial-time` directly — one
//! place to absorb its pre-1.0 API churn.
//!
//! Conversions: `UTC ↔ TAI` uses the leap-second table; `TAI ↔ TT` is the fixed
//! 32.184 s; `TT ↔ TDB` uses the periodic (~1.7 ms) barycentric terms at
//! Greenwich. `UT1` currently uses `DUT1 = 0` (no Earth-orientation data wired
//! yet) — UT1 ≈ UTC to < 0.9 s, so GMST is good to ~15″; wiring real EOP/DUT1 is
//! a documented follow-up.

use celestial_time::{
    // `UTC`/`TDB` are named directly; `TAI`/`TT`/`UT1` flow through by inference.
    // The `To*` traits are imported for their `to_*` methods on the scale newtypes.
    JulianDate, ToTAI, ToTDB, ToTT, ToTTFromTDB, ToUT1WithDUT1, ToUTC, GMST, TDB, UTC,
};

use crate::SECS_PER_DAY;

/// Julian Date of the Unix epoch (1970-01-01T00:00:00Z), for `chrono` interop.
const UNIX_EPOCH_JD: f64 = 2_440_587.5;

/// Convert a **UTC** Julian Date to a **TDB** Julian Date (UTC→TAI→TT→TDB).
/// Falls back to the input on the (rare) conversion error so callers never panic.
pub fn utc_jd_to_tdb_jd(utc_jd: f64) -> f64 {
    UTC::from_julian_date(JulianDate::from_f64(utc_jd))
        .to_tai()
        .and_then(|tai| tai.to_tt())
        .and_then(|tt| tt.to_tdb_greenwich())
        .map(|tdb| tdb.to_julian_date().to_f64())
        .unwrap_or(utc_jd)
}

/// The current wall-clock instant as a **TDB** Julian Date — the correct seed for
/// the mission clock. Replaces the old "treat `Utc::now()` as a JD" seed, which
/// was off by TT−UTC ≈ 69 s (32.184 s + leap seconds).
pub fn utc_now_tdb_jd() -> f64 {
    use chrono::{Datelike, Timelike, Utc};
    let now = Utc::now();
    let utc_jd = JulianDate::from_calendar(
        now.year(),
        now.month() as u8,
        now.day() as u8,
        now.hour() as u8,
        now.minute() as u8,
        now.second() as f64 + now.nanosecond() as f64 / 1.0e9,
    )
    .to_f64();
    utc_jd_to_tdb_jd(utc_jd)
}

/// Every derived scale at a given master **TDB** epoch. Julian Dates throughout;
/// GMST in radians `[0, 2π)`. Build with [`TimeScales::from_tdb_jd`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeScales {
    /// Barycentric Dynamical Time (the master) — Julian Date.
    pub tdb_jd: f64,
    /// Terrestrial Time — Julian Date.
    pub tt_jd: f64,
    /// International Atomic Time — Julian Date.
    pub tai_jd: f64,
    /// Coordinated Universal Time — Julian Date.
    pub utc_jd: f64,
    /// Universal Time (Earth rotation) — Julian Date (DUT1 = 0 approximation).
    pub ut1_jd: f64,
    /// Greenwich Mean Sidereal Time — radians `[0, 2π)`.
    pub gmst_rad: f64,
}

impl TimeScales {
    /// Derive every scale from the master TDB epoch. Each step degrades
    /// gracefully to the previous scale on a conversion error, so a bad epoch
    /// yields best-effort values rather than a panic.
    pub fn from_tdb_jd(tdb_jd: f64) -> Self {
        let tdb = TDB::from_julian_date(JulianDate::from_f64(tdb_jd));

        let tt = tdb.to_tt_greenwich().ok();
        let tt_jd = tt.as_ref().map_or(tdb_jd, |t| t.to_julian_date().to_f64());

        let tai = tt.as_ref().and_then(|t| t.to_tai().ok());
        let tai_jd = tai.as_ref().map_or(tt_jd, |t| t.to_julian_date().to_f64());

        let utc = tai.as_ref().and_then(|t| t.to_utc().ok());
        let utc_jd = utc.as_ref().map_or(tai_jd, |u| u.to_julian_date().to_f64());

        let ut1 = utc.as_ref().and_then(|u| u.to_ut1_with_dut1(0.0).ok());
        let ut1_jd = ut1.as_ref().map_or(utc_jd, |u| u.to_julian_date().to_f64());

        let gmst_rad = match (ut1.as_ref(), tt.as_ref()) {
            (Some(u), Some(t)) => GMST::from_ut1_and_tt(u, t).map(|g| g.radians()).unwrap_or(0.0),
            _ => 0.0,
        };

        Self { tdb_jd, tt_jd, tai_jd, utc_jd, ut1_jd, gmst_rad }
    }
}

/// Format a master **TDB** Julian Date as a UTC calendar string
/// (`YYYY-MM-DD HH:MM:SS UTC`). The single canonical formatter — it derives the
/// correct UTC instant from TDB, unlike the drifted `jd_to_utc_string` copies
/// that wrongly treated the master epoch as UTC.
pub fn tdb_jd_to_utc_string(tdb_jd: f64) -> String {
    use chrono::{TimeZone, Utc};
    let utc_jd = TimeScales::from_tdb_jd(tdb_jd).utc_jd;
    let unix_secs = (utc_jd - UNIX_EPOCH_JD) * SECS_PER_DAY;
    Utc.timestamp_opt(unix_secs.round() as i64, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| format!("JD {tdb_jd:.5} TDB"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At a J2000-era date TAI−UTC = 32 s and TT−TAI = 32.184 s, so TT−UTC =
    /// 64.184 s and TDB−UTC ≈ 64.184 s (the TDB−TT periodic term is ~ms). This
    /// is the conflation the old seed missed entirely (it had TDB = UTC).
    #[test]
    fn utc_to_tdb_applies_the_64_second_offset() {
        // 2000-01-01 12:00:00 UTC.
        let utc_jd = JulianDate::from_calendar(2000, 1, 1, 12, 0, 0.0).to_f64();
        let tdb_jd = utc_jd_to_tdb_jd(utc_jd);
        let offset_secs = (tdb_jd - utc_jd) * SECS_PER_DAY;
        assert!(
            (offset_secs - 64.184).abs() < 0.5,
            "TDB−UTC should be ≈64.184 s, got {offset_secs}"
        );
    }

    /// UTC → TDB → (derive UTC back) round-trips to sub-millisecond.
    #[test]
    fn tdb_round_trips_to_utc() {
        let utc_jd = JulianDate::from_calendar(2026, 6, 29, 18, 30, 0.0).to_f64();
        let tdb_jd = utc_jd_to_tdb_jd(utc_jd);
        let back = TimeScales::from_tdb_jd(tdb_jd).utc_jd;
        let err_secs = (back - utc_jd).abs() * SECS_PER_DAY;
        assert!(err_secs < 1.0e-3, "UTC round-trip error {err_secs} s");
    }

    /// The derived scales are ordered TDB ≳ TT > TAI > UTC (each later scale is
    /// "behind"), and the gaps are the canonical fixed offsets.
    #[test]
    fn scale_ladder_has_canonical_gaps() {
        let s = TimeScales::from_tdb_jd(JulianDate::from_calendar(2026, 1, 1, 0, 0, 0.0).to_f64());
        // TT − TAI = 32.184 s (exact).
        assert!(((s.tt_jd - s.tai_jd) * SECS_PER_DAY - 32.184).abs() < 1.0e-3);
        // TAI − UTC = 37 s (current leap-second count, since 2017).
        assert!(((s.tai_jd - s.utc_jd) * SECS_PER_DAY - 37.0).abs() < 1.0e-3);
        // TDB − TT is a small periodic term (< ~2 ms).
        assert!((s.tdb_jd - s.tt_jd).abs() * SECS_PER_DAY < 0.01);
    }

    /// GMST is a valid angle and advances at the sidereal rate: +1 hour of time
    /// → +1.0027379 h of sidereal angle (≈15.0411°).
    #[test]
    fn gmst_is_valid_and_advances_at_sidereal_rate() {
        use std::f64::consts::TAU;
        // A TDB epoch away from the 24 h wrap so +1 h doesn't roll over.
        let tdb0 = JulianDate::from_calendar(2026, 6, 29, 6, 0, 0.0).to_f64();
        let g0 = TimeScales::from_tdb_jd(tdb0).gmst_rad;
        assert!((0.0..TAU).contains(&g0), "GMST out of range: {g0}");

        let g1 = TimeScales::from_tdb_jd(tdb0 + 1.0 / 24.0).gmst_rad;
        let expected = TAU / 24.0 * 1.002_737_909_35; // sidereal gain over 1 solar hour
        let delta = (g1 - g0).rem_euclid(TAU);
        assert!(
            (delta - expected).abs() < 1.0e-3,
            "GMST should advance ~{expected} rad/h, got {delta}"
        );
    }
}
