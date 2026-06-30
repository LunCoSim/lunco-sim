use bevy::prelude::*;
pub use lunco_core::TimeWarpState;
use lunco_core::SimTick;
use lunco_time::MissionClock;

/// Format a **TDB** epoch (Julian Date) as a `YYYY-MM-DD HH:MM:SS UTC` string.
/// Delegates to the spine's single canonical formatter (doc 19 — T3), which
/// derives the correct UTC instant from TDB instead of mislabelling the master
/// epoch as UTC. Kept here as a thin alias so existing call sites are untouched.
pub fn jd_to_utc_string(epoch: f64) -> String {
    lunco_time::scales::tdb_jd_to_utc_string(epoch)
}

/// Startup: seed the [`MissionClock`] mission origin **and** calendar anchor from
/// the current wall clock (via the proper UTC→TAI→TT→TDB chain, doc 19 — T3) and
/// the current tick, so absolute time is anchored at the real launch instant
/// rather than the J2000 default. This is the single seed of the time spine —
/// the old `CelestialClock` middleman is gone; `WorldTime`/`TimeTransport` are
/// the only time authorities now.
pub fn seed_mission_clock_from_wall(tick: Res<SimTick>, mut mission: ResMut<MissionClock>) {
    *mission = MissionClock::anchored(lunco_time::scales::utc_now_tdb_jd(), tick.0);
}
