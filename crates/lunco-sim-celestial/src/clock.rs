use bevy::prelude::*;

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
) {
    if clock.paused {
        return;
    }
    
    // speed_multiplier is based on real-time seconds.
    // 1 day = 86400 seconds.
    let dt_days = (time.delta_secs_f64() * clock.speed_multiplier) / 86400.0;
    clock.epoch += dt_days;
}
