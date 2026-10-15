use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::time::{Real, TimeUpdateStrategy};
use std::time::Duration;

/// Change the native clock's update source without admitting a source-time gap.
///
/// Manual updates move Bevy's native wall-clock sample independently of the
/// computer clock. Reanchor that sample while preserving elapsed time, so the
/// next automatic update admits only time since this handoff. Fixed-cycle debt,
/// simulation state and transport settings are unaffected.
pub fn set_time_update_strategy(world: &mut World, strategy: TimeUpdateStrategy) {
    set_time_update_strategy_at(world, strategy, Instant::now());
}

fn set_time_update_strategy_at(world: &mut World, strategy: TimeUpdateStrategy, now: Instant) {
    if matches!(strategy, TimeUpdateStrategy::Automatic)
        && !matches!(
            world.get_resource::<TimeUpdateStrategy>(),
            None | Some(TimeUpdateStrategy::Automatic)
        )
    {
        let mut time = world.resource_mut::<Time<Real>>();
        // Sample using Bevy's native context, without adding the difference
        // between the synthetic and computer instants to logical elapsed time.
        let mut sample = Time::new_with(*time.context());
        sample.update_with_instant(now);
        *time.context_mut() = *sample.context();
        time.advance_by(Duration::ZERO);
    }
    world.insert_resource(strategy);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_handoff_preserves_elapsed_and_excludes_wall_clock_gap() {
        let start = Instant::now();
        let quantum = Duration::from_millis(16);
        for synthetic_offset in [Duration::ZERO, Duration::from_secs(100)] {
            let mut world = World::new();
            let mut time = Time::<Real>::new(start);
            time.update_with_instant(start);
            time.update_with_duration(synthetic_offset);
            time.set_wrap_period(Duration::from_secs(7));
            let elapsed = time.elapsed();
            let first_update = time.first_update();
            world.insert_resource(time);
            world.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));

            let handoff = start + Duration::from_secs(10);
            set_time_update_strategy_at(&mut world, TimeUpdateStrategy::Automatic, handoff);
            let mut time = world.resource_mut::<Time<Real>>();
            assert_eq!(time.elapsed(), elapsed);
            assert_eq!(time.delta(), Duration::ZERO);
            assert_eq!(time.first_update(), first_update);
            assert_eq!(time.startup(), start);
            assert_eq!(time.wrap_period(), Duration::from_secs(7));
            time.update_with_instant(handoff + quantum);
            assert_eq!(time.delta(), quantum);
            assert_eq!(time.elapsed(), elapsed + quantum);
            // Reapplying the automatic mode must retain an ordinary sample.
            set_time_update_strategy_at(
                &mut world,
                TimeUpdateStrategy::Automatic,
                handoff + quantum * 2,
            );
            assert_eq!(world.resource::<Time<Real>>().delta(), quantum);
            assert_eq!(
                world.resource::<Time<Real>>().last_update(),
                Some(handoff + quantum)
            );
        }
    }
}
