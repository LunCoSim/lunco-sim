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

/// System set (inside [`InteractionSchedule`], runs FIRST) that restores each eased
/// entity's *authoritative* stepped pose before the step's writers run — undoing the
/// previous frame's render interpolation so the sim never reads its own smoothing.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InteractionRestoreSet;

/// System set (inside [`InteractionSchedule`], runs LAST) that snapshots each eased
/// entity's freshly-written pose. Pose writers order themselves
/// `.after(InteractionRestoreSet).before(InteractionRecordSet)`.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InteractionRecordSet;

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
            // 60 Hz constant. The staircase this would show on a faster display is
            // removed the correct way — a render-rate interpolation pass eases the
            // rendered pose between steps by `overstep_fraction` (see
            // [`InteractionEasing`]) — rather than by cranking the step rate and
            // hoping. It is NOT tied to the sim's 60 Hz tick (they only happen to
            // match); the interaction step never pauses or rate-scales.
            step_secs: 1.0 / 60.0,
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

/// Render-rate easing for an entity written on the interaction step.
///
/// The interaction cadence runs at a constant 60 Hz, so on a faster display a pose
/// written there is up to one step (~16 ms) stale between writes — a visible
/// staircase. This is the same fix avian uses for rigid bodies: keep the two most
/// recent *stepped* poses and, every rendered frame, write the `Transform` as their
/// interpolation by the step's [`overstep_fraction`](InteractionStep::overstep_fraction).
/// Constant-`dt` logic, display-rate smoothness.
///
/// Add it to any entity whose `Transform` is authored inside [`InteractionSchedule`]
/// (the avatar cameras do). The stepped writer keeps writing `Transform` as before;
/// [`record_interaction_poses`] snapshots it each step, and [`ease_interaction_poses`]
/// overwrites `Transform` with the eased value each frame.
#[derive(Component, Debug, Clone, Copy, Reflect, Default)]
#[reflect(Component)]
pub struct InteractionEased {
    /// Pose at the previous step.
    prev: Option<Transform>,
    /// Pose at the latest step (what the stepped writer produced).
    curr: Option<Transform>,
}


/// START of the interaction step: restore each eased entity's authoritative pose
/// (`curr`) into `Transform`, undoing the previous frame's render interpolation.
///
/// This is the piece that makes interpolation safe for *incremental* writers like
/// `apply_fly` (`pos += vel·dt`, reading `pos` back from `Transform`). Without it, the
/// writer would read the render-interpolated pose and integrate from there, folding
/// smoothing lag into the actual trajectory. With it, every step starts from the true
/// pose; only the rendered `Transform` is ever interpolated. (Same separation avian
/// keeps between `Position` and the eased `Transform`.)
fn restore_interaction_poses(mut q: Query<(&mut Transform, &InteractionEased)>) {
    for (mut tf, eased) in &mut q {
        if let Some(curr) = eased.curr {
            *tf = curr;
        }
    }
}

/// End of the interaction step: snapshot each eased entity's freshly-written pose into
/// its [`InteractionEased`] history (curr → prev, Transform → curr). Runs LAST in the
/// schedule so it sees the final pose the camera systems produced this step.
fn record_interaction_poses(mut q: Query<(&Transform, &mut InteractionEased)>) {
    for (tf, mut eased) in &mut q {
        eased.prev = eased.curr;
        eased.curr = Some(*tf);
    }
}

/// Render rate: write each eased entity's `Transform` as `lerp(prev, curr, overstep)`.
///
/// Runs in `PostUpdate` AFTER the step runner (so `curr`/`overstep` are this frame's)
/// and before transform propagation. On a step boundary `overstep ≈ 0` and the pose is
/// `prev`; just before the next step `overstep ≈ 1` and it reaches `curr` — the
/// standard one-step-behind interpolation (never extrapolates, so it cannot overshoot).
fn ease_interaction_poses(
    step: Res<InteractionStep>,
    mut q: Query<(&mut Transform, &InteractionEased)>,
) {
    let s = step.overstep_fraction.clamp(0.0, 1.0);
    for (mut tf, eased) in &mut q {
        if let (Some(prev), Some(curr)) = (eased.prev, eased.curr) {
            tf.translation = prev.translation.lerp(curr.translation, s);
            tf.rotation = prev.rotation.slerp(curr.rotation, s);
            tf.scale = prev.scale.lerp(curr.scale, s);
        }
    }
}

/// Wiring for the interaction cadence. Added by [`TimePlugin`](crate::TimePlugin).
pub(crate) fn build_interaction_cadence(app: &mut App) {
    app.init_resource::<InteractionStep>()
        .insert_resource(Time::new_with(Interaction))
        .register_type::<InteractionStep>()
        .register_type::<InteractionEased>()
        .init_schedule(InteractionSchedule)
        // Per step: restore the true pose FIRST (undo last frame's render ease), then
        // the pose writers run (in `AvatarCameraSet`, ordered between these two sets),
        // then record the true pose LAST for `ease_interaction_poses` to lerp.
        .add_systems(
            InteractionSchedule,
            (
                restore_interaction_poses.in_set(InteractionRestoreSet),
                record_interaction_poses.in_set(InteractionRecordSet),
            ),
        )
        .configure_sets(
            InteractionSchedule,
            InteractionRestoreSet.before(InteractionRecordSet),
        )
        .add_systems(
            PostUpdate,
            (
                // The step runner FIRST (drains wall time → N steps, updates
                // `overstep_fraction`), then the render-rate ease using it.
                run_interaction_schedule.in_set(InteractionStepSet),
                ease_interaction_poses,
            )
                .chain()
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

    /// The interpolation must NOT feed back into an incremental writer. A system that
    /// does `pos += 1` each step must advance by exactly one per step — even though
    /// `ease_interaction_poses` overwrites `Transform` with an interpolated value every
    /// render frame. This is the whole reason for the restore→step→record→ease order:
    /// the sim reads the true pose, only the render sees the eased one.
    #[test]
    fn interpolation_does_not_feed_back_into_an_incremental_writer() {
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<Time<Virtual>>()
            .init_resource::<Time>();
        super::build_interaction_cadence(&mut app);

        let e = app.world_mut().spawn((Transform::default(), InteractionEased::default())).id();
        // Incremental writer: pos.x += 1 each step, reading pos back from Transform.
        // Ordered BETWEEN restore and record, exactly as the avatar camera chain is —
        // that ordering is what makes the integration see the true pose, not the ease.
        app.add_systems(
            InteractionSchedule,
            (move |mut q: Query<&mut Transform>| {
                if let Ok(mut tf) = q.get_mut(e) {
                    tf.translation.x += 1.0;
                }
            })
            .after(InteractionRestoreSet)
            .before(InteractionRecordSet),
        );

        // Run 3 steps' worth of wall time, then eased render happens in PostUpdate.
        let step = app.world().resource::<InteractionStep>().step_secs;
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_secs_f64(step * 3.0));
        app.update();

        // The AUTHORITATIVE pose advanced exactly 3 — the eased Transform the renderer
        // sees is ≤ 3 (interpolated), but the writer's integration is uncontaminated.
        let curr = app.world().get::<InteractionEased>(e).unwrap().curr.unwrap();
        assert_eq!(curr.translation.x, 3.0, "the true pose must advance one per step");

        // A frame with ZERO steps (fast display, or wall time not yet a full step):
        // the writer does not run, the true pose holds, only the render ease moves.
        // Zero the wall delta first — a bare `App` does not refresh `Time<Real>`, so it
        // would otherwise re-apply the previous frame's delta.
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::ZERO);
        app.update();
        let curr2 = app.world().get::<InteractionEased>(e).unwrap().curr.unwrap();
        assert_eq!(curr2.translation.x, 3.0, "no step ⇒ the true pose must not move");
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
