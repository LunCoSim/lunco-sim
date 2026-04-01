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
        // Initialize from system time or J2000
        Self {
            epoch: 2_451_545.0, // J2000.0 epoch default
            speed_multiplier: 1.0,
            paused: false,
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
