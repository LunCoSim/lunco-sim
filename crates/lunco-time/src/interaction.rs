//! The **interaction cadence** (doc 19 §11e-bis): a constant-rate step that never
//! pauses and never rate-scales.
//!
//! # Why this exists
//!
//! Bevy ships exactly one fixed cadence, and it deliberately hangs it off the
//! pause/scale clock: `Time<Real>` → `Time<Virtual>` (pause + rate) → `Time<Fixed>`
//! (accumulator) → `FixedUpdate`. So `FixedUpdate` carries two properties that the
//! *simulation* wants and that *interaction* must not inherit:
//!
//! * it **stops on pause** — but a paused world must still let you fly the camera; and
//! * it **rate-scales** — an 8× sim would fly the avatar 8× faster.
//!
//! Which left the avatar with two bad options, and the code took both: ride
//! `FixedUpdate` for a constant `dt` and freeze on pause (the old `spring_arm_system`),
//! or ride the render frame and take a variable `dt` (`apply_fly` on `Time<Real>`) —
//! plus a duplicate system to cover the paused case. Cadence was standing in for clock.
//!
//! This is the third option: a fixed step rooted on the **wall clock**. Constant `dt`,
//! immune to pause and to sim rate, by construction — there is no path from
//! [`TimeTransport`](crate::TimeTransport) to it.
//!
//! # What is NOT here — and why the sim keeps Bevy's cadence
//!
//! The simulation still runs in `FixedUpdate`, rate-scaled via `Time<Virtual>`, and it
//! must:
//!
//! * **Rate is sub-stepping.** `sim_secs = tick × SECS_PER_TICK` — a tick is a *fixed
//!   size*. So 8× means "eight ticks per wall-frame", never "one tick of 8× the `dt`"
//!   (avian's `Time<Physics>::relative_speed` would do the latter: it multiplies the
//!   step size — a 133 ms solver step at 8× — and breaks the tick↔seconds invariant).
//!   More fixed runs per frame IS the mechanism, and that is what `Time<Virtual>`'s
//!   `relative_speed` buys.
//! * **Netcode is keyed to it.** `SimTick` is defined as one tick per fixed step
//!   (`lunco_core`), and prediction/rollback/input-recording are built on that 1:1 —
//!   as is lightyear's own tick manager.
//! * **avian's smoothing is keyed to it.** `bevy_transform_interpolation` captures
//!   start/end in `FixedFirst`/`FixedLast` and eases on `Time<Fixed>::overstep_fraction`.
//!
//! So the two cadences coexist, split by what they are FOR: `FixedUpdate` = the sim's
//! deterministic tick; [`InteractionSchedule`] = the constant, unpausable presentation
//! step. avian's *pause* is still driven from the time system — via
//! `lunco_physics::PhysicsHolds` → `Time<Physics>::pause()` — which is the part of
//! "control avian from time, not from the schedule" that genuinely belongs there.

use std::time::Duration;

use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

/// The clock context for [`InteractionSchedule`]. Inside that schedule the generic
/// `Time` resource IS this clock — exactly as `Time` is `Time<Fixed>` inside
/// `FixedUpdate` — so systems just read `Res<Time>` and get the constant step.
#[derive(Debug, Clone, Copy, Default, Reflect)]
pub struct Interaction;

/// The constant-rate, never-paused step. Avatar movement and every camera run here.
#[derive(ScheduleLabel, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InteractionSchedule;

/// System set holding the [`InteractionSchedule`] runner in `PostUpdate`.
///
/// It runs there — after avian's writeback and after `bevy_transform_interpolation`
/// has eased every body into its render pose (`RunFixedMainLoop`, before `Update`) —
/// so a camera following a rover reads the body's **smoothed** pose, and before
/// `TransformSystems::Propagate`, so the camera's own pose reaches `GlobalTransform`
/// this frame.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InteractionStepSet;

/// Accumulator + step size for the interaction cadence.
#[derive(Resource, Debug, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct InteractionStep {
    /// Seconds per step. Constant — this is the whole point.
    pub step_secs: f64,
    /// Unconsumed wall time.
    pub accumulator: f64,
    /// How far into the next step we are, `0..1`. For any consumer that wants to ease
    /// the camera between steps.
    pub overstep_fraction: f32,
    /// Hard cap on steps per frame, so a hitch (or a debugger breakpoint) cannot make
    /// the runner spiral: the accumulator is drained rather than chased.
    pub max_steps_per_frame: u32,
}

impl Default for InteractionStep {
    fn default() -> Self {
        Self {
            // 120 Hz: high enough that the camera is never more than ~8 ms stale on a
            // 60–144 Hz display (no visible staircase), cheap enough that running the
            // camera systems 2× per frame is free. It is NOT tied to the sim's 60 Hz
            // tick, and it must not be — that is the coupling this module removes.
            step_secs: 1.0 / 120.0,
            accumulator: 0.0,
            overstep_fraction: 0.0,
            max_steps_per_frame: 8,
        }
    }
}

/// Drain the wall clock into fixed interaction steps and run [`InteractionSchedule`]
/// once per step, with the generic `Time` set to the constant-`dt` [`Interaction`]
/// clock.
///
/// Reads `Time<Real>` — **not** `Time<Virtual>`. That single choice is what makes this
/// cadence immune to pause and to sim rate: there is no path from the transport to it.
pub fn run_interaction_schedule(world: &mut World) {
    let real_delta = world.resource::<Time<Real>>().delta_secs_f64();

    let (steps, step_secs) = {
        let mut step = world.resource_mut::<InteractionStep>();
        step.accumulator += real_delta;
        let dt = step.step_secs.max(1e-6);
        let mut n = 0;
        while step.accumulator >= dt && n < step.max_steps_per_frame {
            step.accumulator -= dt;
            n += 1;
        }
        // Hitch guard: if we hit the cap, drop the backlog instead of chasing it —
        // chasing a backlog on a *presentation* clock buys nothing and can spiral.
        if n == step.max_steps_per_frame {
            step.accumulator = 0.0;
        }
        step.overstep_fraction = (step.accumulator / dt) as f32;
        (n, dt)
    };

    if steps == 0 {
        return;
    }

    let delta = Duration::from_secs_f64(step_secs);
    for _ in 0..steps {
        world.resource_mut::<Time<Interaction>>().advance_by(delta);
        let generic = world.resource::<Time<Interaction>>().as_generic();
        *world.resource_mut::<Time>() = generic;
        world.run_schedule(InteractionSchedule);
    }

    // Restore the generic clock, so systems after us in `PostUpdate` see the virtual
    // clock they expect (same contract Bevy's fixed loop honours).
    let virtual_clock = world.resource::<Time<Virtual>>().as_generic();
    *world.resource_mut::<Time>() = virtual_clock;
}

/// Wiring for the interaction cadence. Added by [`TimePlugin`](crate::TimePlugin).
pub(crate) fn build_interaction_cadence(app: &mut App) {
    app.init_resource::<InteractionStep>()
        .insert_resource(Time::new_with(Interaction))
        .register_type::<InteractionStep>()
        .init_schedule(InteractionSchedule)
        .add_systems(
            PostUpdate,
            run_interaction_schedule
                .in_set(InteractionStepSet)
                .before(bevy::transform::TransformSystems::Propagate),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The load-bearing property: the interaction step is driven by the WALL clock, so
    /// pausing the sim (or running it at 8×) cannot touch it. There is no path from
    /// `TimeTransport` into this cadence — that is what makes it unpausable *by
    /// construction* rather than by a guard someone has to remember.
    #[test]
    fn steps_are_constant_and_survive_a_paused_sim() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<Time<Virtual>>()
            .init_resource::<Time>();
        super::build_interaction_cadence(&mut app);

        // The sim is PAUSED and would be 8× if it ran: neither may reach us.
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_relative_speed(8.0);

        // Count steps and the dt each one saw.
        #[derive(Resource, Default)]
        struct Seen {
            steps: u32,
            dts: Vec<f32>,
        }
        app.init_resource::<Seen>();
        app.add_systems(InteractionSchedule, |time: Res<Time>, mut seen: ResMut<Seen>| {
            seen.steps += 1;
            seen.dts.push(time.delta_secs());
        });

        // Advance the WALL clock by exactly 3 steps' worth (120 Hz ⇒ 25 ms).
        let step = app.world().resource::<InteractionStep>().step_secs;
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_secs_f64(step * 3.0));
        app.update();

        let seen = app.world().resource::<Seen>();
        assert_eq!(seen.steps, 3, "wall time must drain into fixed steps");
        for dt in &seen.dts {
            assert!(
                (*dt as f64 - step).abs() < 1e-6,
                "every interaction step must see the SAME dt (got {dt})"
            );
        }
    }

    /// A hitch must not spiral: the accumulator is dropped at the cap, not chased.
    #[test]
    fn a_long_hitch_is_dropped_not_chased() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<Time<Virtual>>()
            .init_resource::<Time>();
        super::build_interaction_cadence(&mut app);

        #[derive(Resource, Default)]
        struct Count(u32);
        app.init_resource::<Count>();
        app.add_systems(InteractionSchedule, |mut c: ResMut<Count>| c.0 += 1);

        // A 5-second stall at 120 Hz would be 600 steps.
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_secs(5));
        app.update();

        let cap = app.world().resource::<InteractionStep>().max_steps_per_frame;
        assert_eq!(app.world().resource::<Count>().0, cap);
        // …and the backlog is gone, so the next frame is not still catching up.
        assert_eq!(app.world().resource::<InteractionStep>().accumulator, 0.0);
    }
}
