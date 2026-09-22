//! Application interaction clocks.
//!
//! Commands and one-shot Rhai/REPL evaluations are both application-scoped,
//! but they are not the same cadence. A command can be accepted without
//! evaluating a script, and a queued script can be evaluated after its command
//! has already completed. Keep one small clock for each path, driven from the
//! wall clock and advanced only when that path actually runs.

use bevy::prelude::{Real, Resource, Time};

/// One application interaction clock.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CadenceClock {
    /// Number of observations on this path.
    pub sequence: u64,
    /// Elapsed time accumulated between observations on this path.
    pub elapsed_secs: f64,
    /// Wall-clock interval since the previous observation, if one exists.
    pub interval_secs: Option<f64>,
    /// Instantaneous observations per second, if the last interval is non-zero.
    pub rate_hz: Option<f64>,
    last_wall_secs: Option<f64>,
}

impl CadenceClock {
    fn observe(&mut self, wall_secs: f64) {
        if !wall_secs.is_finite() {
            return;
        }
        let interval = self.last_wall_secs.map(|last| (wall_secs - last).max(0.0));
        if let Some(interval) = interval {
            self.elapsed_secs += interval;
            self.interval_secs = Some(interval);
            self.rate_hz = (interval > 0.0).then_some(1.0 / interval);
        }
        self.last_wall_secs = Some(wall_secs);
        self.sequence = self.sequence.wrapping_add(1);
    }
}

/// Independent application clocks for typed commands and one-shot REPL work.
///
/// Both clocks use `Time<Real>` only as their monotonic source. They do not
/// inherit `Time<Virtual>`, `Time<Fixed>`, simulation rate, or Twin generation.
/// `command` advances when the shared `CommandOccurred` fact is published;
/// `repl` advances when a queued script is actually evaluated.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct ApplicationCadence {
    /// Typed command occurrence cadence.
    pub command: CadenceClock,
    /// One-shot Rhai/REPL evaluation cadence.
    pub repl: CadenceClock,
}

impl ApplicationCadence {
    /// Record one accepted typed command at the current wall-clock time.
    pub fn observe_command(&mut self, time: &Time<Real>) {
        self.observe_command_at(time.elapsed_secs_f64());
    }

    /// Record one accepted typed command at an already-read wall-clock value.
    pub fn observe_command_at(&mut self, wall_secs: f64) {
        self.command.observe(wall_secs);
    }

    /// Record one actual one-shot script evaluation at the current wall-clock time.
    pub fn observe_repl(&mut self, time: &Time<Real>) {
        self.observe_repl_at(time.elapsed_secs_f64());
    }

    /// Record one actual one-shot script evaluation at an already-read wall-clock value.
    pub fn observe_repl_at(&mut self, wall_secs: f64) {
        self.repl.observe(wall_secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_clock_advances_only_when_its_path_is_observed() {
        let mut cadence = ApplicationCadence::default();
        let mut time = Time::<Real>::default();

        time.advance_by(std::time::Duration::from_secs(2));
        cadence.observe_command(&time);
        assert_eq!(cadence.command.sequence, 1);
        assert_eq!(cadence.repl.sequence, 0);
        assert_eq!(cadence.command.elapsed_secs, 0.0);

        time.advance_by(std::time::Duration::from_secs(3));
        cadence.observe_repl(&time);
        assert_eq!(cadence.command.sequence, 1);
        assert_eq!(cadence.repl.sequence, 1);

        time.advance_by(std::time::Duration::from_secs(1));
        cadence.observe_command(&time);
        assert_eq!(cadence.command.interval_secs, Some(4.0));
        assert_eq!(cadence.repl.interval_secs, None);
    }
}
