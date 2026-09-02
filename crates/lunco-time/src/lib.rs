//! Unified mission-time spine (architecture doc 19 — T1).
//!
//! One stored master — the [`SimTick`](lunco_core::SimTick) in `lunco-core` (the
//! netcode/integrator substrate) — and **everything calendar/celestial is
//! *derived*, never accumulated**. This crate owns the layer *above* the tick:
//! the conversion anchor (tick ↔ epoch), the transport (play/pause/rate), and
//! the derived [`WorldTime`] view that consumers read.
//!
//! The load-bearing rule is invariant 1 — **derive, never accumulate**. The old
//! `epoch += Δt` (`lunco-celestial/src/clock.rs`) drifted, was frame-rate
//! dependent and could not seek; here `epoch = epoch0 + (tick − tick0)/86400` is
//! an exact pure function of the integer tick.
//!
//! `sim_secs` / MET and the calendar epoch are both derived from the same fixed
//! tick and mission origin. All real logic is the pure [`advance_clock`] function
//! (unit-tested headless, no Bevy `Time`). [`advance_world_clock`] is the thin
//! Bevy adapter that projects its result onto Bevy's virtual clock.

use bevy::prelude::*;
use std::time::Duration;

use lunco_core::{SimTick, SECS_PER_TICK};

pub mod domain;
pub use domain::*;

pub mod interaction;
pub use interaction::{
    Interaction, InteractionEased, InteractionRecordSet, InteractionRenderSet,
    InteractionRestoreSet, InteractionSchedule, InteractionStep, InteractionStepSet,
};

pub mod scales;
pub use scales::{
    tdb_jd_to_utc_string, utc_jd_to_tdb_jd, utc_now_tdb_jd, utc_string_to_tdb_jd, TimeScales,
};

/// Seconds in one day — the JD/epoch unit conversion.
pub const SECS_PER_DAY: f64 = 86_400.0;

/// The highest live transport rate that runs the fixed-step integrators.
///
/// This is a measured safety boundary for the fixed-step budget. Requests above
/// it are rejected by [`SetTimeTransport`](crate::SetTimeTransport), so every
/// accepted live rate advances the causal simulation.
pub const MAX_REALTIME_RATE: f64 = 64.0;

/// The slowest selectable live transport rate. Pause is represented by
/// [`TransportMode::Paused`], so an accepted rate is always positive.
pub const MIN_REALTIME_RATE: f64 = 0.1;

/// User-selectable rates for the causal fixed-step transport.
///
/// Every UI that offers simulation rates must use this list. Keeping it beside
/// [`MIN_REALTIME_RATE`] and [`MAX_REALTIME_RATE`] prevents a control surface
/// from advertising a rate that the clock would interpret differently.
pub const REALTIME_RATE_OPTIONS: &[f64] = &[
    MIN_REALTIME_RATE,
    0.25,
    0.5,
    1.0,
    2.0,
    4.0,
    8.0,
    16.0,
    32.0,
    MAX_REALTIME_RATE,
];

/// Format a shared live transport rate for UI labels and command-facing help.
/// Keeping the spelling beside the canonical ladder preserves the fractional
/// slow-rate labels across every control surface.
pub fn realtime_rate_label(rate: f64) -> String {
    format!("{rate}x")
}

/// Maximum number of fixed simulation steps a rendered frame may drain while
/// running a realtime transport rate. This is a catch-up guard, not a rate cap:
/// frames that arrive on time still receive their complete requested rate.
pub const MAX_FIXED_STEPS_PER_FRAME: u32 = 64;

/// Raw wall-clock delta cap used by the fixed-step budget.
pub const BASE_VIRTUAL_MAX_DELTA: Duration = Duration::from_millis(33);

/// Return the raw frame delta that keeps a realtime transport inside the fixed
/// step budget. Bevy applies `Time<Virtual>::max_delta` before its relative
/// speed, so the cap must be divided by the requested rate.
pub fn fixed_step_raw_delta_limit(rate: f64, fixed_timestep: Duration) -> Duration {
    let rate = if rate.is_finite() {
        rate.clamp(0.0, MAX_REALTIME_RATE)
    } else {
        0.0
    };
    if rate == 0.0 {
        return BASE_VIRTUAL_MAX_DELTA;
    }
    let budget = fixed_timestep.mul_f64(MAX_FIXED_STEPS_PER_FRAME as f64 / rate);
    budget.min(BASE_VIRTUAL_MAX_DELTA)
}

/// Close the remainder of the current Bevy fixed-loop burst after a simulation
/// barrier is raised from inside one fixed iteration.
///
/// `Time<Virtual>::pause()` prevents the next render frame from accumulating
/// fixed time, but Bevy's current `FixedMain` loop may already have additional
/// `Time<Fixed>::overstep()` queued for this frame. A barrier that lets that
/// remainder run would advance some fixed consumers after the coupling decision
/// and others before it. Discarding only the unconsumed overstep preserves the
/// completed fixed tick and makes the barrier take effect at this boundary.
pub fn discard_fixed_overstep(fixed: &mut Time<Fixed>) {
    let remaining = fixed.overstep();
    if !remaining.is_zero() {
        fixed.discard_overstep(remaining);
    }
}

/// Keep Bevy's fixed-loop catch-up bounded for the current transport rate.
///
/// This is the only fixed-step budget projection. The transport still controls
/// the simulation rate through `Time<Virtual>::relative_speed`; this system only
/// limits how much raw wall time a hitch may turn into one catch-up burst.
fn apply_fixed_step_budget(
    transport: Res<TimeTransport>,
    fixed: Option<Res<Time<Fixed>>>,
    virtual_time: Option<ResMut<Time<Virtual>>>,
) {
    let (Some(fixed), Some(mut virtual_time)) = (fixed, virtual_time) else {
        return;
    };
    let limit = fixed_step_raw_delta_limit(transport.rate, fixed.timestep());
    if virtual_time.max_delta() != limit {
        virtual_time.set_max_delta(limit);
    }
}

/// J2000.0 epoch as a Julian Date (TDB). Default mission epoch.
pub const J2000_JD: f64 = 2_451_545.0;

/// Transport play state. Replaces the scattered `paused` booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Reflect)]
pub enum TransportMode {
    /// Time advances at `rate`.
    #[default]
    Playing,
    /// Time is held; tick frozen, epoch frozen.
    Paused,
}

/// The transport authority: the single internal source of truth for play state
/// and rate. UI and input surfaces dispatch [`SetTimeTransport`], which updates
/// this resource — it is the sole play/rate authority.
#[derive(Resource, Debug, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct TimeTransport {
    /// Play / pause.
    pub mode: TransportMode,
    /// Speed multiplier relative to real time (1.0 = realtime).
    pub rate: f64,
}

impl Default for TimeTransport {
    fn default() -> Self {
        Self {
            mode: TransportMode::Playing,
            rate: 1.0,
        }
    }
}

impl TimeTransport {
    /// Is time actually flowing (playing AND rate > 0)?
    #[inline]
    pub fn is_running(&self) -> bool {
        matches!(self.mode, TransportMode::Playing) && self.rate > 0.0
    }
}

/// The calendar conversion mapping — the bridge between the discrete `tick` and
/// the continuous `epoch` (Julian Date). `epoch0_jd` is the calendar instant at
/// `tick0`. It changes only when an authoritative mission epoch is authored.
#[derive(Debug, Clone, Copy, Reflect)]
pub struct TimeAnchor {
    /// Julian Date (TDB) at `tick0`.
    pub epoch0_jd: f64,
    /// The tick that maps to `epoch0_jd`.
    pub tick0: u64,
}

impl Default for TimeAnchor {
    fn default() -> Self {
        Self {
            epoch0_jd: J2000_JD,
            tick0: 0,
        }
    }
}

impl TimeAnchor {
    /// Continuous seconds since this anchor: `(tick − tick0)·SECS_PER_TICK`.
    /// Wrapping-safe. (For the *integrator* clock / MET use
    /// [`MissionClock::sim_secs`]; this is the calendar mapping's own offset.)
    #[inline]
    pub fn secs_since(&self, tick: u64) -> f64 {
        (tick.wrapping_sub(self.tick0) as i64) as f64 * SECS_PER_TICK
    }

    /// Derived epoch (Julian Date, TDB): `epoch0 + secs_since/86400`. **Pure** —
    /// no accumulation, seekable, frame-rate independent.
    #[inline]
    pub fn epoch_jd(&self, tick: u64) -> f64 {
        self.epoch0_jd + self.secs_since(tick) / SECS_PER_DAY
    }
}

/// The mission clock: the fixed mission origin (for the integrator `sim_secs` /
/// MET) and the re-anchorable calendar mapping.
#[derive(Resource, Debug, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct MissionClock {
    /// Fixed mission-start tick — defines the integrator clock (`sim_secs`/MET
    /// base). It moves only on an explicit mission reset.
    pub mission_tick0: u64,
    /// Epoch at `mission_tick0` — the MET calendar origin.
    pub mission_epoch0_jd: f64,
    /// Epoch↔tick calendar mapping.
    pub anchor: TimeAnchor,
}

impl Default for MissionClock {
    fn default() -> Self {
        Self {
            mission_tick0: 0,
            mission_epoch0_jd: J2000_JD,
            anchor: TimeAnchor::default(),
        }
    }
}

impl MissionClock {
    /// Construct a clock anchored at `epoch0_jd` for the given starting tick
    /// (sets both the mission origin and the calendar anchor).
    pub fn anchored(epoch0_jd: f64, tick0: u64) -> Self {
        Self {
            mission_tick0: tick0,
            mission_epoch0_jd: epoch0_jd,
            anchor: TimeAnchor { epoch0_jd, tick0 },
        }
    }

    /// The integrator clock: continuous sim seconds since mission start.
    /// `(tick − mission_tick0)·SECS_PER_TICK`. This is the time the USD
    /// animation sampler keys on.
    #[inline]
    pub fn sim_secs(&self, tick: u64) -> f64 {
        (tick.wrapping_sub(self.mission_tick0) as i64) as f64 * SECS_PER_TICK
    }

    /// The current derived epoch for the authoritative simulation tick.
    #[inline]
    pub fn epoch_jd(&self, tick: u64) -> f64 {
        self.anchor.epoch_jd(tick)
    }

    /// Mission Elapsed Time, in seconds: calendar elapsed since mission start
    /// (`(epoch − mission_epoch0)·86400`).
    #[inline]
    pub fn met_secs(&self, tick: u64) -> f64 {
        (self.epoch_jd(tick) - self.mission_epoch0_jd) * SECS_PER_DAY
    }

    /// Restore the calendar mapping to the mission origin. The authored mission
    /// origin itself is preserved so a scene reload can apply a new
    /// `SetMissionEpoch` without stale calendar state.
    pub fn reset_calendar(&mut self) {
        self.anchor = TimeAnchor {
            epoch0_jd: self.mission_epoch0_jd,
            tick0: self.mission_tick0,
        };
    }
}

/// Return the virtual-clock rate for the transport.
///
/// The command owner rejects rates above the fixed-step safety boundary. The
/// pure helper also clamps direct resource construction so the Bevy projection
/// cannot create an unbounded fixed-step burst.
pub fn advance_clock(rate: f64, paused: bool) -> f64 {
    // The command boundary rejects out-of-range values. Keep this pure owner
    // defensive for callers that construct the transport resource directly.
    let rate = if rate.is_finite() {
        rate.clamp(0.0, MAX_REALTIME_RATE)
    } else {
        0.0
    };
    if paused || rate == 0.0 {
        0.0
    } else {
        rate
    }
}

/// The derived, read-only time view every consumer reads. Written each frame by
/// [`advance_world_clock`]. Nothing keys off the raw `MissionClock`/`SimTick`
/// directly except the spine itself.
#[derive(Resource, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Resource)]
pub struct WorldTime {
    /// Derived epoch (Julian Date, TDB) — the ephemeris/lighting input.
    pub epoch_jd: f64,
    /// Integrator clock seconds since mission start — the animation sampler key.
    pub sim_secs: f64,
    /// Mission Elapsed Time, seconds.
    pub met_secs: f64,
}

impl WorldTime {
    /// Derive all civil/atomic/rotational scales (UTC/TAI/TT/UT1 + GMST) from the
    /// master TDB epoch (doc 19 — T3). See [`TimeScales`].
    #[inline]
    pub fn scales(&self) -> TimeScales {
        TimeScales::from_tdb_jd(self.epoch_jd)
    }

    /// The current epoch as a `YYYY-MM-DD HH:MM:SS UTC` string.
    #[inline]
    pub fn utc_string(&self) -> String {
        tdb_jd_to_utc_string(self.epoch_jd)
    }
}

/// System set for the spine step. Celestial/USD consumers order their epoch
/// readers `.after` this set so they see the freshly-derived `WorldTime`.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeSpineSet;

/// The Bevy adapter: feed [`advance_clock`] the transport, write the derived
/// `WorldTime`, and project the rate onto `Time<Virtual>` (the single control
/// state). Runs in `PreUpdate` before `FixedUpdate` so the unified rate takes
/// effect on the same frame.
pub fn advance_world_clock(
    tick: Res<SimTick>,
    transport: Res<TimeTransport>,
    clock: Res<MissionClock>,
    mut world: ResMut<WorldTime>,
    mut virtual_time: ResMut<Time<Virtual>>,
    coupling: Option<Res<lunco_core::SimulationBarrier>>,
) {
    // A Modelica result that feeds an Avian force/torque port is a barrier for
    // the whole deterministic simulation, not just for Avian. If only the
    // physics clock stopped, SimTick, Rhai, controllers, and co-simulation
    // propagation would continue producing state for a body that did not move.
    // Treat the barrier as a transport pause here so every FixedUpdate consumer
    // shares one coherent boundary. The resource is optional so the time spine
    // remains usable in small/headless apps that do not install co-simulation.
    let coupling_held = coupling.is_some_and(|state| state.held);
    let paused = matches!(transport.mode, TransportMode::Paused) || coupling_held;
    let relative_speed = advance_clock(transport.rate, paused);

    world.epoch_jd = clock.epoch_jd(tick.0);
    world.sim_secs = clock.sim_secs(tick.0);
    world.met_secs = clock.met_secs(tick.0);

    // Frozen transport is projected onto Bevy's `paused` flag, never onto
    // `relative_speed = 0`. Consumers treat relative speed as a divisor, so a
    // positive configured rate is retained while Bevy's pause state supplies
    // the zero effective speed.
    let frozen = relative_speed <= 0.0;
    let configured = if frozen { 1.0 } else { relative_speed };

    // Control projection is **change-driven**: only touch `Time<Virtual>` when the
    // rate actually changed. Comparing the value (rather than gating the whole
    // system on `resource_changed`) keeps it self-healing — if anything clobbers
    // `relative_speed` out of band, the mismatch is corrected next frame — while
    // avoiding a redundant per-frame write and the spurious change-detection it
    // would trigger.
    if virtual_time.relative_speed_f64() != configured {
        virtual_time.set_relative_speed_f64(configured);
    }
    if frozen != virtual_time.is_paused() {
        if frozen {
            virtual_time.pause();
        } else {
            virtual_time.unpause();
        }
    }
}

/// Startup: anchor the [`MissionClock`] mission origin **and** calendar anchor
/// from the current wall clock (via the proper UTC→TAI→TT→TDB chain — doc 19 T3)
/// at the current tick, so absolute mission time is anchored at the real launch
/// instant in **every** spine context (celestial, USD, modelica, workbench) — not
/// just where the ephemeris runs. The integrator clock (`sim_secs`) is unaffected:
/// at `Startup` the tick is still 0, so `mission_tick0` stays 0 — only the
/// calendar epoch moves off the `J2000` default.
///
/// **Skipped if the clock was already customized** away from the default (an app
/// or scenario that inserted a specific epoch, or a deterministic replay), so an
/// explicit override is never clobbered.
///
/// **Multiplayer:** the per-peer wall seed is a transient. The `anchor` is the
/// host-authoritative, replicable unit — the networking layer overwrites the
/// client's seed on first sync (doc 19 §transport). Sub-second machine-clock skew
/// is cosmetic for celestial visuals until then, and the epoch projection is
/// explicitly *not* required to be cross-peer bit-deterministic.
pub fn seed_mission_clock_from_wall(tick: Res<SimTick>, mut mission: ResMut<MissionClock>) {
    let is_default = mission.mission_tick0 == 0
        && mission.mission_epoch0_jd == J2000_JD
        && mission.anchor.tick0 == 0
        && mission.anchor.epoch0_jd == J2000_JD;
    if is_default {
        *mission = MissionClock::anchored(scales::utc_now_tdb_jd(), tick.0);
    }
}

/// Installs the mission-time spine: resources, the `PreUpdate` derivation step,
/// and the wall-clock seed at `Startup`. Add once (guarded callers use
/// [`App::is_plugin_added`]). Every consumer reads `WorldTime`; nothing else
/// seeds the clock.
pub struct TimePlugin;

impl Plugin for TimePlugin {
    fn build(&self, app: &mut App) {
        // `SimTick` lives in `lunco-core`; `init_resource` is idempotent, so this
        // is harmless where another plugin also inserts it and makes the spine
        // self-sufficient where it doesn't.
        // Own the virtual-clock baseline here as well. Applications must not
        // install a second max-delta/rate policy beside the time spine.
        app.init_resource::<Time<Virtual>>();
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_max_delta(BASE_VIRTUAL_MAX_DELTA);

        app.init_resource::<SimTick>()
            .init_resource::<MissionClock>()
            .init_resource::<TimeTransport>()
            .init_resource::<WorldTime>()
            .register_type::<MissionClock>()
            .register_type::<TimeTransport>()
            .register_type::<WorldTime>()
            .add_systems(
                PreUpdate,
                (
                    advance_world_clock.in_set(TimeSpineSet),
                    apply_fixed_step_budget.after(TimeSpineSet),
                ),
            )
            .add_systems(Startup, seed_mission_clock_from_wall);

        // The clock tree (T5): TimeDomain/Playback/TimeBinding + the per-frame
        // resolve into `ResolvedDomains` (in `DomainResolveSet`, `Update`).
        domain::build_domain_tree(app);
        // The constant-rate, never-paused presentation step (doc 19 §11e-bis). Beside
        // `FixedUpdate` (the sim's tick), not instead of it — see `interaction`.
        interaction::build_interaction_cadence(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::FIXED_HZ;

    const EPS: f64 = 1e-9;

    #[test]
    fn epoch_derives_from_tick_no_accumulation() {
        let a = TimeAnchor {
            epoch0_jd: J2000_JD,
            tick0: 0,
        };
        // 60 ticks = 1 second = 1/86400 day.
        assert!((a.epoch_jd(60) - (J2000_JD + 1.0 / SECS_PER_DAY)).abs() < EPS);
        // One full day later.
        let day_ticks = (SECS_PER_DAY * FIXED_HZ) as u64;
        assert!((a.epoch_jd(day_ticks) - (J2000_JD + 1.0)).abs() < EPS);
        // Deriving twice gives the identical value (no drift).
        assert_eq!(a.epoch_jd(12_345), a.epoch_jd(12_345));
    }

    #[test]
    fn sim_secs_round_trips_from_mission_origin() {
        let c = MissionClock::anchored(J2000_JD, 1_000);
        assert!(c.sim_secs(1_000).abs() < EPS);
        assert!((c.sim_secs(1_060) - 1.0).abs() < EPS);
        // Before the origin → negative (wrapping-safe).
        assert!((c.sim_secs(940) + 1.0).abs() < EPS);
    }

    #[test]
    fn paused_freezes_tick_and_physics() {
        // `relative_speed == 0` is the whole "paused" story — frozen tick + physics.
        assert_eq!(advance_clock(1.0, true), 0.0);
    }

    #[test]
    fn realtime_rate_unifies_the_knob() {
        // Every accepted rate stays on the causal fixed-step path.
        let rs = advance_clock(MAX_REALTIME_RATE, false);
        assert_eq!(rs, MAX_REALTIME_RATE); // one rate → relative_speed (> 0 ⇒ running)
    }

    #[test]
    fn realtime_rate_options_stay_inside_the_causal_transport() {
        assert!(!REALTIME_RATE_OPTIONS.is_empty());
        assert_eq!(
            REALTIME_RATE_OPTIONS.first().copied(),
            Some(MIN_REALTIME_RATE)
        );
        assert!(REALTIME_RATE_OPTIONS
            .windows(2)
            .all(|rates| rates[0] < rates[1]));
        assert_eq!(
            REALTIME_RATE_OPTIONS.last().copied(),
            Some(MAX_REALTIME_RATE)
        );
        assert!(REALTIME_RATE_OPTIONS
            .iter()
            .all(|&rate| advance_clock(rate, false) > 0.0));
        assert_eq!(realtime_rate_label(0.1), "0.1x");
        assert_eq!(realtime_rate_label(1.0), "1x");
    }

    #[derive(Resource, Default)]
    struct FixedRunCount(u32);

    fn count_fixed_runs(mut count: ResMut<FixedRunCount>) {
        count.0 += 1;
    }

    fn fixed_runs_after_manual_frames(rate: f64) -> u32 {
        let mut app = App::new();
        app.add_plugins((bevy::time::TimePlugin, TimePlugin))
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::from_secs_f64(1.0 / 120.0),
            ))
            .insert_resource(TimeTransport {
                mode: TransportMode::Playing,
                rate,
            })
            .init_resource::<FixedRunCount>()
            .add_systems(FixedUpdate, count_fixed_runs);

        for _ in 0..120 {
            app.update();
        }

        app.world().resource::<FixedRunCount>().0
    }

    #[test]
    fn realtime_transport_rate_scales_the_real_fixed_schedule() {
        let one_x = fixed_runs_after_manual_frames(1.0);
        let slow = fixed_runs_after_manual_frames(MIN_REALTIME_RATE);
        let four_x = fixed_runs_after_manual_frames(4.0);
        let eight_x = fixed_runs_after_manual_frames(8.0);
        let sixteen_x = fixed_runs_after_manual_frames(16.0);
        let thirty_two_x = fixed_runs_after_manual_frames(32.0);
        let sixty_four_x = fixed_runs_after_manual_frames(MAX_REALTIME_RATE);

        assert!(one_x > 0, "1x never entered FixedUpdate");
        assert!(
            slow <= one_x / 5,
            "0.1x fixed schedule ran {slow} ticks versus {one_x} at 1x"
        );
        assert!(
            four_x >= one_x * 3,
            "4x fixed schedule ran {four_x} ticks versus {one_x} at 1x"
        );
        assert!(
            eight_x >= one_x * 7,
            "8x fixed schedule ran {eight_x} ticks versus {one_x} at 1x"
        );
        assert!(
            sixteen_x >= one_x * 14,
            "16x fixed schedule ran {sixteen_x} ticks versus {one_x} at 1x"
        );
        assert!(
            thirty_two_x >= one_x * 28,
            "32x fixed schedule ran {thirty_two_x} ticks versus {one_x} at 1x"
        );
        assert!(
            sixty_four_x >= one_x * 56,
            "64x fixed schedule ran {sixty_four_x} ticks versus {one_x} at 1x"
        );
    }

    /// The ceiling exists because `max_delta`-clamped frames × `relative_speed`
    /// is the fixed-step burst size (see [`MAX_REALTIME_RATE`]). Lock it low
    /// enough that one hitched 33 ms frame cannot demand a runaway step count.
    #[test]
    fn realtime_ceiling_bounds_the_fixed_step_burst() {
        let fixed = Duration::from_secs_f64(1.0 / 60.0);
        let raw_limit = fixed_step_raw_delta_limit(MAX_REALTIME_RATE, fixed);
        let steps_per_hitched_frame =
            raw_limit.as_secs_f64() * MAX_REALTIME_RATE / fixed.as_secs_f64();
        assert!(
            steps_per_hitched_frame <= MAX_FIXED_STEPS_PER_FRAME as f64 + 1e-9,
            "MAX_REALTIME_RATE={MAX_REALTIME_RATE} lets one capped frame demand \
             {steps_per_hitched_frame:.0} fixed steps — above the central fixed-step budget"
        );
        assert_eq!(advance_clock(MAX_REALTIME_RATE, false), MAX_REALTIME_RATE);
        assert_eq!(
            advance_clock(MAX_REALTIME_RATE + 1.0, false),
            MAX_REALTIME_RATE
        );
    }

    #[test]
    fn fixed_barrier_discards_only_unconsumed_overstep() {
        let mut fixed = Time::<Fixed>::from_seconds(1.0);
        fixed.accumulate_overstep(Duration::from_secs(3));
        assert_eq!(fixed.overstep(), Duration::from_secs(3));

        discard_fixed_overstep(&mut fixed);

        assert_eq!(fixed.overstep(), Duration::ZERO);
        assert_eq!(fixed.elapsed(), Duration::ZERO);
    }

    #[test]
    fn rates_above_the_safe_ceiling_are_clamped() {
        assert_eq!(advance_clock(128.0, false), MAX_REALTIME_RATE);
        assert_eq!(advance_clock(5000.0, true), 0.0);
    }

    /// A frozen spine must raise Bevy's `paused` flag and keep `relative_speed`
    /// a POSITIVE rate — never 0.
    ///
    /// `relative_speed` is a rate that consumers divide by: lightyear's
    /// interpolation timeline computes `delta.div_f32(time.relative_speed())`, so
    /// a zero there yields `inf` and panics `Duration::from_secs_f32` ("cannot
    /// convert float seconds to Duration"). This crashed every networked client
    /// the moment it loaded a DEM-terrain scene, because the terrain readiness wait
    /// froze the world while the heightfield built. `effective_speed` (0 when
    /// paused) is what the tick/physics gates read, so freezing still works.
    #[test]
    fn frozen_spine_pauses_and_never_zeroes_relative_speed() {
        use bevy::ecs::system::RunSystemOnce;

        for (mode, rate) in [
            (TransportMode::Paused, 1.0),  // explicit user pause
            (TransportMode::Playing, 0.0), // rate 0 is also "frozen"
        ] {
            let mut world = bevy::prelude::World::new();
            world.insert_resource(lunco_core::SimTick(0));
            world.insert_resource(TimeTransport { mode, rate });
            world.insert_resource(MissionClock::default());
            world.insert_resource(WorldTime::default());
            world.insert_resource(Time::<Virtual>::default());

            world.run_system_once(advance_world_clock).unwrap();

            let vt = world.resource::<Time<Virtual>>();
            assert!(
                vt.is_paused(),
                "{mode:?}/{rate} should freeze via the paused flag"
            );
            assert!(
                vt.relative_speed_f64() > 0.0,
                "{mode:?}/{rate} left relative_speed at {} — consumers divide by it",
                vt.relative_speed_f64()
            );
        }
    }

    /// The complement: a running spine is unpaused and carries the rate.
    #[test]
    fn running_spine_projects_rate_and_unpauses() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = bevy::prelude::World::new();
        world.insert_resource(lunco_core::SimTick(0));
        world.insert_resource(TimeTransport {
            mode: TransportMode::Playing,
            rate: 2.0,
        });
        world.insert_resource(MissionClock::default());
        world.insert_resource(WorldTime::default());
        let mut vt = Time::<Virtual>::default();
        vt.pause(); // start frozen, so we prove the transition back
        world.insert_resource(vt);

        world.run_system_once(advance_world_clock).unwrap();

        let vt = world.resource::<Time<Virtual>>();
        assert!(!vt.is_paused());
        assert_eq!(vt.relative_speed_f64(), 2.0);
    }

    #[test]
    fn realtime_coupling_barrier_pauses_the_fixed_simulation() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = bevy::prelude::World::new();
        world.insert_resource(lunco_core::SimTick(0));
        world.insert_resource(TimeTransport::default());
        world.insert_resource(MissionClock::default());
        world.insert_resource(WorldTime::default());
        world.insert_resource(Time::<Virtual>::default());
        world.insert_resource(lunco_core::SimulationBarrier {
            held: true,
            ..Default::default()
        });

        world.run_system_once(advance_world_clock).unwrap();

        let vt = world.resource::<Time<Virtual>>();
        assert!(vt.is_paused());
        assert_eq!(vt.relative_speed_f64(), 1.0);
    }
}
