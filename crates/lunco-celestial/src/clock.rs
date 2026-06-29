use bevy::prelude::*;
pub use lunco_core::{CelestialClock, TimeWarpState};
use lunco_core::SimTick;
use lunco_time::{MissionClock, TimeTransport, TransportMode, WorldTime};

pub fn get_default_celestial_clock() -> CelestialClock {
    CelestialClock {
        // The epoch is **TDB** (the ephemeris input). Seed it from the current
        // wall clock via the proper UTC→TAI→TT→TDB chain (doc 19 — T3). The old
        // seed treated `Utc::now()` directly as a JD, conflating UTC with TDB and
        // landing ~69 s (TT−UTC) early.
        epoch: lunco_time::scales::utc_now_tdb_jd(),
        speed_multiplier: 1.0,
        paused: false,
    }
}

/// Format a **TDB** epoch (Julian Date) as a `YYYY-MM-DD HH:MM:SS UTC` string.
/// Delegates to the spine's single canonical formatter (doc 19 — T3), which
/// derives the correct UTC instant from TDB instead of mislabelling the master
/// epoch as UTC. Kept here as a thin alias so existing call sites are untouched.
pub fn jd_to_utc_string(epoch: f64) -> String {
    lunco_time::scales::tdb_jd_to_utc_string(epoch)
}

/// Startup: seed the [`MissionClock`] mission origin **and** calendar anchor from
/// the (wall-seeded) [`CelestialClock`] epoch and the current tick, so absolute
/// time is anchored at the real launch instant rather than the J2000 default.
pub fn seed_mission_clock_from_celestial(
    clock: Res<CelestialClock>,
    tick: Res<SimTick>,
    mut mission: ResMut<MissionClock>,
) {
    *mission = MissionClock::anchored(clock.epoch, tick.0);
}

/// Compat-in (runs **before** [`lunco_time::TimeSpineSet`]): legacy UI writes
/// `CelestialClock.{speed_multiplier, paused}`; mirror them onto the
/// [`TimeTransport`] authority during migration. Once UI writes `TimeTransport`
/// directly this shim (and the `CelestialClock` knobs) retire.
pub fn sync_transport_from_celestial(
    clock: Res<CelestialClock>,
    mut transport: ResMut<TimeTransport>,
) {
    transport.rate = clock.speed_multiplier;
    transport.mode = if clock.paused {
        TransportMode::Paused
    } else {
        TransportMode::Playing
    };
}

/// Compat-out (runs **after** [`lunco_time::TimeSpineSet`]): the spine derived the
/// epoch; copy it back onto `CelestialClock.epoch` so existing ephemeris /
/// telemetry / UI readers are untouched. This is a pure copy of the derived
/// view — **no accumulation** (the `epoch += Δt` drift bug is gone).
pub fn sync_celestial_from_world(world: Res<WorldTime>, mut clock: ResMut<CelestialClock>) {
    clock.epoch = world.epoch_jd;
}
