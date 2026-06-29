use bevy::prelude::*;
pub use lunco_core::{CelestialClock, TimeWarpState};
use lunco_core::SimTick;
use lunco_time::{MissionClock, TimeTransport, TransportMode, WorldTime};

pub fn get_default_celestial_clock() -> CelestialClock {
    use chrono::Utc;

    // Initializing from current system time
    let now = Utc::now();
    let unix_timestamp = now.timestamp() as f64 + (now.timestamp_subsec_nanos() as f64 / 1e9);

    // JD = (Unix Timestamp / 86400.0) + 2440587.5
    let epoch = (unix_timestamp / 86400.0) + 2440587.5;

    CelestialClock {
        epoch,
        speed_multiplier: 1.0,
        paused: false,
    }
}

pub fn jd_to_utc_string(epoch: f64) -> String {
    use chrono::{TimeZone, Utc};

    // Convert Julian Date to Unix Timestamp (seconds since 1970-01-01)
    // JD 2440587.5 is Unix Epoch
    let unix_secs = (epoch - 2440587.5) * 86400.0;

    if let Some(dt) = Utc
        .timestamp_opt(unix_secs as i64, ((unix_secs.rem_euclid(1.0)) * 1e9) as u32)
        .single()
    {
        dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
    } else {
        format!("JD {:.2}", epoch)
    }
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
