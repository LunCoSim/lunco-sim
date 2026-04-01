use bevy::prelude::*;
use lunco_sim_core::TimeWarpState;

#[derive(Resource)]
pub struct CelestialClock {
    pub epoch: f64,            // Julian Date (TDB)
    pub speed_multiplier: f64, // 1.0 = real-time
    pub paused: bool,
}

impl Default for CelestialClock {
    fn default() -> Self {
        use chrono::Utc;
        
        // Initializing from current system time
        let now = Utc::now();
        let unix_timestamp = now.timestamp() as f64 + (now.timestamp_subsec_nanos() as f64 / 1e9);
        
        // JD = (Unix Timestamp / 86400.0) + 2440587.5
        let epoch = (unix_timestamp / 86400.0) + 2440587.5;
        
        Self {
            epoch,
            speed_multiplier: 1.0,
            paused: false,
        }
    }
}

impl CelestialClock {
    pub fn to_utc_string(&self) -> String {
        use chrono::{Utc, TimeZone};
        
        // Convert Julian Date to Unix Timestamp (seconds since 1970-01-01)
        // JD 2440587.5 is Unix Epoch
        let unix_secs = (self.epoch - 2440587.5) * 86400.0;
        
        // Handle negative JD? Not for this sim.
        if let Some(dt) = Utc.timestamp_opt(unix_secs as i64, ((unix_secs.rem_euclid(1.0)) * 1e9) as u32).single() {
             dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
        } else {
             format!("JD {:.2}", self.epoch)
        }
    }
}

pub fn celestial_clock_tick_system(
    time: Res<Time>,
    mut clock: ResMut<CelestialClock>,
    mut time_warp: ResMut<TimeWarpState>,
) {
    if clock.paused {
        time_warp.speed = 0.0;
        time_warp.physics_enabled = true; // Still allow physics? Or false? 
        // Spec says: speed > 100x sets physics_enabled = false.
        // If paused, speed is effectively 0.
        return;
    }
    
    time_warp.speed = clock.speed_multiplier;
    time_warp.physics_enabled = clock.speed_multiplier <= 100.0;

    // speed_multiplier is based on real-time seconds.
    // 1 day = 86400 seconds.
    let dt_days = (time.delta_secs_f64() * clock.speed_multiplier) / 86400.0;
    clock.epoch += dt_days;
}
