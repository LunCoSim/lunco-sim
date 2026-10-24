//! Checked civil-time projections around the canonical TDB mission epoch.
//!
//! Celestial owns leap seconds, scale conversion, parsing and UTC formatting.
//! The application supplies its portable wall clock and keeps canonical epochs
//! in f64. Failed conversions remain errors at the consuming UI/policy boundary.

use celestial_time::{
    TimeResult,
    julian::JulianDate,
    scales::{
        conversions::{
            ToTAI, ToTT, ToUTC,
            tt_tdb::{ToTDB, ToTTFromTDB},
        },
        tdb::TDB,
        utc::UTC,
    },
};

/// Convert one checked native UTC value to the mission's TDB coordinate.
fn tdb_from_utc(utc: UTC) -> TimeResult<f64> {
    Ok(utc
        .to_tai()?
        .to_tt()?
        .to_tdb_greenwich()?
        .to_julian_date()
        .to_f64())
}

/// Convert the portable application wall clock to a checked TDB epoch.
pub fn utc_now_tdb_jd() -> TimeResult<f64> {
    let now = chrono::Utc::now();
    tdb_from_utc(UTC::new(now.timestamp(), now.timestamp_subsec_nanos())?)
}

/// Parse a complete ISO 8601 UTC datetime with the native leap-second parser.
pub fn utc_string_to_tdb_jd(source: &str) -> TimeResult<f64> {
    tdb_from_utc(source.parse::<UTC>()?)
}

/// Format TDB as checked ISO 8601 UTC, including milliseconds and a Z suffix.
pub fn tdb_jd_to_utc_string(epoch_jd: f64) -> TimeResult<String> {
    let tdb = TDB::from_julian_date(JulianDate::from_f64(epoch_jd));
    let utc = tdb.to_tt_greenwich()?.to_tai()?.to_utc()?;
    Ok(format!("{}Z", utc.to_iso8601()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_time_applies_the_atomic_offset_and_round_trips() {
        let utc_jd = JulianDate::from_calendar(2000, 1, 1, 12, 0, 0.0)
            .unwrap()
            .to_f64();
        let tdb = utc_string_to_tdb_jd("2000-01-01T12:00:00Z").unwrap();
        assert!(((tdb - utc_jd) * crate::SECS_PER_DAY - 64.184).abs() < 0.001);
        let formatted = tdb_jd_to_utc_string(tdb).unwrap();
        assert_eq!(formatted, "2000-01-01T12:00:00.000Z");
        assert!(
            (utc_string_to_tdb_jd(&formatted).unwrap() - tdb).abs() * crate::SECS_PER_DAY < 0.001
        );
    }

    #[test]
    fn fractional_civil_seconds_and_real_leap_seconds_are_preserved() {
        for source in ["2026-07-22T06:00:06.125Z", "2016-12-31T23:59:60.500Z"] {
            let epoch = utc_string_to_tdb_jd(source).unwrap();
            let formatted = tdb_jd_to_utc_string(epoch).unwrap();
            assert_eq!(formatted, source);
        }
        let before = utc_string_to_tdb_jd("2016-12-31T23:59:59Z").unwrap();
        let leap = utc_string_to_tdb_jd("2016-12-31T23:59:60Z").unwrap();
        let after = utc_string_to_tdb_jd("2017-01-01T00:00:00Z").unwrap();
        assert!(((leap - before) * crate::SECS_PER_DAY - 1.0).abs() < 0.001);
        assert!(((after - leap) * crate::SECS_PER_DAY - 1.0).abs() < 0.001);
    }

    #[test]
    fn invalid_dates_and_nonfinite_epochs_remain_errors() {
        for source in [
            "",
            "tomorrow",
            "2026-13-45T99:99:99Z",
            "2026-07-22T06:00:60Z",
        ] {
            assert!(utc_string_to_tdb_jd(source).is_err(), "{source}");
        }
        for epoch in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(tdb_jd_to_utc_string(epoch).is_err());
        }
        assert!(utc_now_tdb_jd().unwrap().is_finite());
    }
}
