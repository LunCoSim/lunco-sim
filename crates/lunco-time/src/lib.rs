//! Unified mission-time spine (architecture doc 19 — T1).
//!
//! One stored master — the [`SimTick`](lunco_core::SimTick) in `lunco-core` (the
//! netcode/integrator substrate) — and **everything calendar/celestial is
//! *derived*, never accumulated**. This crate owns the layer *above* the tick:
//! the conversion anchor (tick ↔ epoch), the transport (play/pause/rate), the
//! live-world regime, and the derived [`WorldTime`] view that consumers read.
//!
//! The load-bearing rule is invariant 1 — **derive, never accumulate**. The old
//! `epoch += Δt` (`lunco-celestial/src/clock.rs`) drifted, was frame-rate
//! dependent and could not seek; here `epoch = epoch0 + (tick − tick0)/86400` is
//! an exact pure function of the integer tick.
//!
//! Two clocks deliberately diverge:
//! * **`sim_secs` / MET base** — pinned at mission start (`mission_tick0`), the
//!   integrator clock. Frozen while warping (the tick is frozen).
//! * **calendar `anchor`** — the epoch↔tick mapping, which *re-anchors* on warp
//!   exit / fast-forward so the calendar stays continuous across the seam. Its
//!   `tick0` moves; the mission base does **not**, so warp can never corrupt
//!   `sim_secs`.
//!
//! All real logic is the pure [`advance_clock`] function (unit-tested headless,
//! no Bevy `Time`). [`advance_world_clock`] is the thin Bevy adapter that feeds
//! it the tick + wall clock and writes the derived views.

use bevy::prelude::*;

use lunco_core::{SimTick, TimeWarpState, SECS_PER_TICK};

pub mod domain;
pub use domain::*;

/// Seconds in one day — the JD/epoch unit conversion.
pub const SECS_PER_DAY: f64 = 86_400.0;

/// J2000.0 epoch as a Julian Date (TDB). Default mission epoch.
pub const J2000_JD: f64 = 2_451_545.0;

/// Above this rate the realtime integrators (avian, Modelica) cannot keep up, so
/// the world clock switches to [`TimeRegime::KinematicWarp`]: the tick freezes
/// (physics pauses) and only **pure** consumers (ephemeris, lighting, sidereal)
/// advance, as a pure function of `epoch`. Makes the implicit
/// "`speed > 100 → physics_enabled = false`" cliff explicit.
pub const MAX_REALTIME_RATE: f64 = 100.0;

/// Transport play state. Replaces the scattered `paused` booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Reflect)]
pub enum TransportMode {
    /// Time advances at `rate`.
    #[default]
    Playing,
    /// Time is held; tick frozen, epoch frozen.
    Paused,
}

/// Which integration regime the *live world clock* is in (doc §5). Distinct from
/// offline run execution (which bakes to `timeSamples`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Reflect)]
pub enum TimeRegime {
    /// Tick advances; `epoch` slaved to tick; `rate` scales the fixed-step
    /// cadence uniformly (physics + Modelica + epoch move together). Bounded by
    /// solver stability ([`MAX_REALTIME_RATE`]).
    #[default]
    RealtimePhysics,
    /// Tick **frozen** (physics/Modelica paused); only pure consumers advance,
    /// `epoch` derived from a wall-clock preview at `rate`.
    KinematicWarp,
}

/// The transport authority: the single internal source of truth for play state
/// and rate. UI writes this; legacy UI still writes `CelestialClock` and a bridge
/// (in `lunco-celestial`) mirrors it here during migration.
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
        Self { mode: TransportMode::Playing, rate: 1.0 }
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
/// `tick0`. Piecewise-constant: it changes only on an explicit **re-anchor**
/// (warp exit / fast-forward), the host-authoritative, replicable event the
/// networking design calls for.
#[derive(Debug, Clone, Copy, Reflect)]
pub struct TimeAnchor {
    /// Julian Date (TDB) at `tick0`.
    pub epoch0_jd: f64,
    /// The tick that maps to `epoch0_jd`.
    pub tick0: u64,
}

impl Default for TimeAnchor {
    fn default() -> Self {
        Self { epoch0_jd: J2000_JD, tick0: 0 }
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

/// A non-deterministic wall-clock preview used **only** in
/// [`TimeRegime::KinematicWarp`] to advance the epoch while the tick is frozen.
/// `epoch = epoch0 + (real − real0)·rate/86400` — still derivation (recomputed
/// each frame), the only place wall time touches the epoch (display/environment
/// only, never sim logic — invariants 1/4).
#[derive(Debug, Clone, Copy, Reflect)]
pub struct WarpPreview {
    /// Epoch at warp entry / last re-seed.
    pub epoch0_jd: f64,
    /// `Time::<Real>` elapsed seconds at warp entry / last re-seed.
    pub real0_secs: f64,
    /// Warp rate captured at re-seed.
    pub rate: f64,
}

impl WarpPreview {
    #[inline]
    fn epoch_at(&self, real_secs: f64) -> f64 {
        self.epoch0_jd + (real_secs - self.real0_secs) * self.rate / SECS_PER_DAY
    }
}

/// The mission clock: the fixed mission origin (for the integrator `sim_secs` /
/// MET), the re-anchorable calendar mapping, the current regime, and the optional
/// warp preview. Thin state; all behavior is in [`advance_clock`].
#[derive(Resource, Debug, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct MissionClock {
    /// Fixed mission-start tick — defines the integrator clock (`sim_secs`/MET
    /// base). **Never** moved by warp; only an explicit mission reset.
    pub mission_tick0: u64,
    /// Epoch at `mission_tick0` — the MET calendar origin.
    pub mission_epoch0_jd: f64,
    /// Epoch↔tick calendar mapping (re-anchors on warp exit / fast-forward).
    pub anchor: TimeAnchor,
    /// Current live-world regime.
    pub regime: TimeRegime,
    /// Active only in [`TimeRegime::KinematicWarp`].
    pub warp: Option<WarpPreview>,
}

impl Default for MissionClock {
    fn default() -> Self {
        Self {
            mission_tick0: 0,
            mission_epoch0_jd: J2000_JD,
            anchor: TimeAnchor::default(),
            regime: TimeRegime::default(),
            warp: None,
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
            regime: TimeRegime::default(),
            warp: None,
        }
    }

    /// The integrator clock: continuous sim seconds since mission start.
    /// `(tick − mission_tick0)·SECS_PER_TICK`. Frozen while warping (tick frozen).
    /// This is the time the USD animation sampler keys on.
    #[inline]
    pub fn sim_secs(&self, tick: u64) -> f64 {
        (tick.wrapping_sub(self.mission_tick0) as i64) as f64 * SECS_PER_TICK
    }

    /// The current derived epoch given `tick` and (for warp) `real_secs`.
    #[inline]
    pub fn epoch_jd(&self, tick: u64, real_secs: f64) -> f64 {
        match self.regime {
            TimeRegime::RealtimePhysics => self.anchor.epoch_jd(tick),
            TimeRegime::KinematicWarp => self
                .warp
                .map_or_else(|| self.anchor.epoch_jd(tick), |w| w.epoch_at(real_secs)),
        }
    }

    /// Mission Elapsed Time, in seconds: calendar elapsed since mission start
    /// (`(epoch − mission_epoch0)·86400`). **Advances during warp** (the epoch
    /// advanced even though the integrator did not) — the honest answer.
    #[inline]
    pub fn met_secs(&self, tick: u64, real_secs: f64) -> f64 {
        (self.epoch_jd(tick, real_secs) - self.mission_epoch0_jd) * SECS_PER_DAY
    }
}

/// The derived views produced each frame — what the legacy resources and Bevy
/// `Time<Virtual>` get written from. Computed purely; never stored as truth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockSample {
    /// Derived epoch (Julian Date, TDB).
    pub epoch_jd: f64,
    /// Active regime after this step.
    pub regime: TimeRegime,
    /// `TimeWarpState.speed` view (rate while running, else 0).
    pub warp_speed: f64,
    /// `TimeWarpState.physics_enabled` view.
    pub physics_enabled: bool,
    /// What to set `Time::<Virtual>.relative_speed` to. This is the **knob
    /// unification**: one `rate` drives the fixed-step cadence (→ tick → epoch)
    /// AND the avian/`Res<Time>` consumers. Zero freezes the tick (paused or warp).
    pub relative_speed: f64,
}

/// The single pure step of the time spine. Resolves the regime (re-anchoring the
/// *calendar* mapping on transitions, leaving the mission origin fixed), derives
/// the epoch, and returns the views for the legacy resources + `Time<Virtual>`.
///
/// * `tick` — current [`SimTick`].
/// * `rate` — transport rate (clamped ≥0).
/// * `paused` — transport pause.
/// * `real_secs` — `Time::<Real>` elapsed seconds (only consulted in warp).
///
/// Mutates `clock.regime`/`clock.warp`/`clock.anchor` (calendar re-anchor on warp
/// exit). Never touches `mission_tick0`/`mission_epoch0_jd`.
pub fn advance_clock(
    clock: &mut MissionClock,
    tick: u64,
    rate: f64,
    paused: bool,
    real_secs: f64,
) -> ClockSample {
    let rate = rate.max(0.0);
    let running = !paused && rate > 0.0;
    let desired = if running && rate > MAX_REALTIME_RATE {
        TimeRegime::KinematicWarp
    } else {
        TimeRegime::RealtimePhysics
    };

    // Regime transitions re-anchor the calendar mapping so the epoch is
    // continuous across the seam (the mission origin is untouched).
    match (clock.regime, desired) {
        (TimeRegime::RealtimePhysics, TimeRegime::KinematicWarp) => {
            // Entering warp: seed the wall preview from the current realtime epoch.
            let cur = clock.anchor.epoch_jd(tick);
            clock.warp = Some(WarpPreview { epoch0_jd: cur, real0_secs: real_secs, rate });
        }
        (TimeRegime::KinematicWarp, TimeRegime::KinematicWarp) => {
            // Still warping but `rate` may have changed: re-seed at the current
            // epoch so the new rate takes effect without a jump.
            if let Some(w) = clock.warp {
                clock.warp = Some(WarpPreview {
                    epoch0_jd: w.epoch_at(real_secs),
                    real0_secs: real_secs,
                    rate,
                });
            }
        }
        (TimeRegime::KinematicWarp, TimeRegime::RealtimePhysics) => {
            // Leaving warp: re-anchor tick→epoch so realtime resumes from the
            // warped epoch (the piecewise-constant calendar re-anchor event).
            let cur = clock
                .warp
                .map_or_else(|| clock.anchor.epoch_jd(tick), |w| w.epoch_at(real_secs));
            clock.anchor = TimeAnchor { epoch0_jd: cur, tick0: tick };
            clock.warp = None;
        }
        (TimeRegime::RealtimePhysics, TimeRegime::RealtimePhysics) => {}
    }
    clock.regime = desired;

    let epoch_jd = clock.epoch_jd(tick, real_secs);

    let relative_speed = match (running, desired) {
        (false, _) => 0.0,                           // paused → freeze tick + physics
        (true, TimeRegime::RealtimePhysics) => rate, // physics keeps up; epoch ← tick
        (true, TimeRegime::KinematicWarp) => 0.0,    // tick frozen; epoch ← wall preview
    };
    let physics_enabled = running && matches!(desired, TimeRegime::RealtimePhysics);
    let warp_speed = if running { rate } else { 0.0 };

    ClockSample { epoch_jd, regime: desired, warp_speed, physics_enabled, relative_speed }
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
    /// Mission Elapsed Time, seconds (calendar elapsed; advances in warp).
    pub met_secs: f64,
    /// Current live-world regime.
    pub regime: TimeRegime,
}

/// System set for the spine step. `lunco-celestial`'s legacy bridge orders its
/// `CelestialClock` compat shims `.before`/`.after` this set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeSpineSet;

/// The Bevy adapter: feed [`advance_clock`] the tick + wall clock, write the
/// derived views (`WorldTime`, `TimeWarpState`, `Time<Virtual>.relative_speed`).
/// Runs in `PreUpdate` (before `FixedUpdate` physics/tick) so the regime gate and
/// the unified rate take effect this frame.
pub fn advance_world_clock(
    tick: Res<SimTick>,
    transport: Res<TimeTransport>,
    real: Res<Time<Real>>,
    mut clock: ResMut<MissionClock>,
    mut world: ResMut<WorldTime>,
    mut warp_state: ResMut<TimeWarpState>,
    mut virtual_time: ResMut<Time<Virtual>>,
) {
    let real_secs = real.elapsed_secs_f64();
    let paused = matches!(transport.mode, TransportMode::Paused);
    let sample = advance_clock(&mut clock, tick.0, transport.rate, paused, real_secs);

    world.epoch_jd = sample.epoch_jd;
    world.sim_secs = clock.sim_secs(tick.0);
    world.met_secs = clock.met_secs(tick.0, real_secs);
    world.regime = sample.regime;

    warp_state.speed = sample.warp_speed;
    warp_state.physics_enabled = sample.physics_enabled;

    virtual_time.set_relative_speed_f64(sample.relative_speed);
}

/// Installs the mission-time spine: resources + the `PreUpdate` step. Add once
/// (guarded callers use [`App::is_plugin_added`]). The legacy `CelestialClock`
/// bridge lives in `lunco-celestial` and orders around [`TimeSpineSet`].
pub struct TimePlugin;

impl Plugin for TimePlugin {
    fn build(&self, app: &mut App) {
        // `SimTick`/`TimeWarpState` live in `lunco-core`; `init_resource` is
        // idempotent, so this is harmless where `CelestialPlugin` also inserts
        // them and makes the spine self-sufficient where it doesn't.
        app.init_resource::<SimTick>()
            .init_resource::<TimeWarpState>()
            .init_resource::<MissionClock>()
            .init_resource::<TimeTransport>()
            .init_resource::<WorldTime>()
            .register_type::<MissionClock>()
            .register_type::<TimeTransport>()
            .register_type::<WorldTime>()
            .add_systems(PreUpdate, advance_world_clock.in_set(TimeSpineSet));

        // The clock tree (T5): TimeDomain/Playback/TimeBinding + the per-frame
        // resolve into `ResolvedDomains` (in `DomainResolveSet`, `Update`).
        domain::build_domain_tree(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::FIXED_HZ;

    const EPS: f64 = 1e-9;

    #[test]
    fn epoch_derives_from_tick_no_accumulation() {
        let a = TimeAnchor { epoch0_jd: J2000_JD, tick0: 0 };
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
        let mut c = MissionClock::default();
        let s = advance_clock(&mut c, 500, 1.0, true, 10.0);
        assert_eq!(s.relative_speed, 0.0);
        assert!(!s.physics_enabled);
        assert_eq!(s.warp_speed, 0.0);
    }

    #[test]
    fn realtime_rate_unifies_the_knob() {
        let mut c = MissionClock::default();
        let s = advance_clock(&mut c, 0, 50.0, false, 0.0);
        assert_eq!(s.regime, TimeRegime::RealtimePhysics);
        assert_eq!(s.relative_speed, 50.0); // one rate → relative_speed
        assert!(s.physics_enabled);
        assert_eq!(s.warp_speed, 50.0);
    }

    #[test]
    fn high_warp_switches_to_kinematic_and_freezes_tick() {
        let mut c = MissionClock::default();
        let s = advance_clock(&mut c, 0, 500.0, false, 0.0);
        assert_eq!(s.regime, TimeRegime::KinematicWarp);
        assert_eq!(s.relative_speed, 0.0); // tick frozen
        assert!(!s.physics_enabled);
        assert!(c.warp.is_some());
    }

    #[test]
    fn kinematic_warp_advances_epoch_from_wall_clock() {
        let mut c = MissionClock::anchored(J2000_JD, 0);
        // Enter warp at real_secs = 0.
        let s0 = advance_clock(&mut c, 0, 1000.0, false, 0.0);
        assert_eq!(s0.epoch_jd, J2000_JD);
        // 2 wall seconds later at 1000× = 2000 sim seconds advanced, tick unchanged.
        let s1 = advance_clock(&mut c, 0, 1000.0, false, 2.0);
        assert!((s1.epoch_jd - (J2000_JD + 2000.0 / SECS_PER_DAY)).abs() < EPS);
        // MET advances during warp even though sim_secs (tick-locked) does not.
        // Tolerance is loose because MET = (epoch − mission_epoch0)·86400 cancels a
        // ~2.45e6-magnitude single-`f64` JD; sub-ms MET precision needs the
        // two-part JulianDate (T3). ~4e-5 s error here, well inside this bound.
        assert!((c.met_secs(0, 2.0) - 2000.0).abs() < 1e-3);
        assert!(c.sim_secs(0).abs() < EPS);
    }

    #[test]
    fn leaving_warp_reanchors_epoch_continuously_but_not_sim_secs() {
        let mut c = MissionClock::anchored(J2000_JD, 0);
        advance_clock(&mut c, 0, 1000.0, false, 0.0); // enter warp
        let warped = advance_clock(&mut c, 0, 1000.0, false, 5.0).epoch_jd; // advance
        // Drop back to realtime at tick 0: epoch must continue from `warped`,
        // not snap back to the tick-derived J2000.
        let back = advance_clock(&mut c, 0, 1.0, false, 5.0);
        assert_eq!(back.regime, TimeRegime::RealtimePhysics);
        assert!((back.epoch_jd - warped).abs() < EPS);
        assert!(c.warp.is_none());
        // sim_secs is unaffected by the calendar re-anchor (mission origin fixed).
        assert!(c.sim_secs(0).abs() < EPS);
        // And the epoch now derives forward from the new calendar anchor.
        let later = advance_clock(&mut c, 60, 1.0, false, 6.0);
        assert!((later.epoch_jd - (warped + 1.0 / SECS_PER_DAY)).abs() < EPS);
        // While sim_secs advances from the *mission* origin, not the re-anchor.
        assert!((c.sim_secs(60) - 1.0).abs() < EPS);
    }

    #[test]
    fn paused_does_not_enter_warp_even_at_high_rate() {
        let mut c = MissionClock::default();
        let s = advance_clock(&mut c, 0, 5000.0, true, 0.0);
        assert_eq!(s.regime, TimeRegime::RealtimePhysics);
        assert_eq!(s.relative_speed, 0.0);
    }
}
