//! Integer child clocks admitted by the authoritative simulation tick.

use crate::SimTick;
use bevy::prelude::{Fixed, Reflect, Time};

/// Validate the native scheduling representation of the canonical physics tick.
/// Its nanosecond rounding is bookkeeping, not a second seconds coordinate.
pub fn simulation_timestep_seconds(fixed: &Time<Fixed>) -> Result<f64, String> {
    let expected = std::time::Duration::from_secs_f64(crate::SECS_PER_TICK);
    if fixed.timestep() != expected {
        return Err(format!(
            "fixed timestep {:?} does not represent the master physics interval {expected:?}",
            fixed.timestep()
        ));
    }
    Ok(crate::SECS_PER_TICK)
}

/// A resettable participant timeline whose only advancing input is `SimTick`.
///
/// Call once per master tick, including inactive ticks. Inactive observations
/// reanchor the child without accruing demand. A fresh child starts with the
/// interval ending at its first admitted tick; it never owes earlier history.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
pub struct TickClock {
    elapsed_ticks: u64,
    last_master_tick: Option<SimTick>,
}

impl TickClock {
    /// Number of admitted master intervals since this child was reset.
    pub fn elapsed_ticks(&self) -> u64 {
        self.elapsed_ticks
    }

    /// Project the integer timeline onto the shared seconds coordinate.
    pub fn elapsed_secs(&self) -> f64 {
        SimTick(self.elapsed_ticks).seconds()
    }

    /// Observe a master boundary, admitting its interval only when active.
    /// Duplicate or missing active boundaries and overflow leave state intact.
    pub fn observe(&mut self, master: SimTick, active: bool) -> Result<(), &'static str> {
        let distance = self
            .last_master_tick
            .map_or(1, |last| master.wrapping_diff(last));
        if distance < 0 {
            return Err("participant master tick moved backwards");
        }
        let elapsed_ticks = if active {
            if distance != 1 {
                return Err("participant must admit each master tick exactly once");
            }
            self.elapsed_ticks
                .checked_add(1)
                .ok_or("participant tick range exhausted")?
        } else {
            self.elapsed_ticks
        };
        self.elapsed_ticks = elapsed_ticks;
        self.last_master_tick = Some(master);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_child_inherits_master_admission_without_duration_accumulation() {
        assert_eq!(
            simulation_timestep_seconds(&Time::<Fixed>::from_hz(crate::FIXED_HZ)),
            Ok(crate::SECS_PER_TICK)
        );
        assert!(
            simulation_timestep_seconds(&Time::<Fixed>::from_hz(crate::FIXED_HZ * 2.0)).is_err()
        );
        let mut clock = TickClock::default();
        for tick in 1..=60_000 {
            clock.observe(SimTick(tick), true).unwrap();
        }
        assert_eq!(clock.elapsed_secs(), SimTick(60_000).seconds());
        assert_eq!(clock.elapsed_secs(), 1000.0);
        clock.observe(SimTick(60_100), false).unwrap();
        assert_eq!(clock.elapsed_ticks(), 60_000);
        clock.observe(SimTick(60_101), true).unwrap();
        assert_eq!(clock.elapsed_ticks(), 60_001);
        for invalid in [60_101, 60_100, 60_103] {
            let before = clock;
            assert!(clock.observe(SimTick(invalid), true).is_err());
            assert_eq!(clock, before);
        }
        clock = TickClock {
            elapsed_ticks: u64::MAX,
            last_master_tick: Some(SimTick(10)),
        };
        let before = clock;
        assert!(clock.observe(SimTick(11), true).is_err());
        assert_eq!(clock, before);
    }
}
