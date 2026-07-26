//! How often the celestial tree is re-solved — expressed as an **angular error
//! budget**, not a rate.
//!
//! ## Why not Hz
//!
//! Sim time is warpable, so any fixed rate is wrong at every other warp factor.
//! On the Moon the sun sweeps `360° / 29.53 d = 12.2°/day` of *sim* time and the
//! Moon sweeps `13.2°/day` around the Earth. At 1× realtime that is 1.5e-4 °/s,
//! so re-solving at 60 Hz moves the sun by 2.5e-6° per frame — five orders of
//! magnitude past anything observable. At 10 000× warp the same 60 Hz is barely
//! adequate. A rate cannot be right in both regimes; a tolerance is right in all
//! of them, because the epoch step it permits scales with the warp itself.
//!
//! ## What sets the default
//!
//! Not the sun's visible position: 0.05° is a tenth of its 0.53° angular
//! diameter, invisible. **Shadow edges** set it. Lunar shadows are long and
//! hard, so a sun step of `t` degrees shifts a terrain shadow by `d·tan(t)` —
//! at a 30 km caster distance, 0.01° is ~5 m and 0.05° is ~26 m. Hence 0.01°.
//!
//! At that budget and 1× realtime the celestial systems solve about once every
//! **71 s of sim time** instead of 60×/s. The step is below the perceptual
//! threshold, so nothing needs interpolating — the poses simply advance in
//! increments too small to see.
//!
//! Measured cost of solving every frame on `sandbox_scene.usda`: ~10 ms/frame
//! across `ephemeris_update_system`, `update_solar_poses`,
//! `trajectory_alignment_system`, `update_sun_light_system` and
//! `anchor_solar_frame_to_site`. See
//! `docs/reviews/2026-07-26-fps-regression-analysis.md`.

use bevy::prelude::*;
use lunco_settings::SettingsSection;
use lunco_time::WorldTime;
use serde::{Deserialize, Serialize};

/// Fastest angular rate any tracked body sweeps, in degrees per day of sim time.
///
/// The Moon around the Earth (13.2°/day) rather than the sun over a lunar site
/// (12.2°/day): the budget must hold for the fastest thing on screen, so the
/// tolerance is converted through the tighter of the two.
const MAX_BODY_DEG_PER_DAY: f64 = 13.2;

/// How much celestial angular error is acceptable before the tree is re-solved.
///
/// Persisted, so it shows up in the workbench **Settings** menu next to every
/// other view preference.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct CelestialCadenceSettings {
    /// Allowed angular error in **degrees**. `0.0` means "solve every frame"
    /// (exact) — what a deterministic scene test wants; see
    /// [`CelestialCadenceSettings::EXACT`].
    pub tolerance_deg: f64,
}

impl CelestialCadenceSettings {
    /// Solve every frame. Deterministic runs (`scene_test`) use this so a
    /// developer's tolerance can never change a test's verdict.
    pub const EXACT: Self = Self { tolerance_deg: 0.0 };

    /// The epoch step (in Julian days) this tolerance permits. `0.0` for the
    /// exact setting, which makes the comparison in
    /// [`celestial_needs_solve`] always true.
    pub fn max_epoch_step_jd(&self) -> f64 {
        self.tolerance_deg.max(0.0) / MAX_BODY_DEG_PER_DAY
    }
}

impl Default for CelestialCadenceSettings {
    fn default() -> Self {
        // ~5 m of shadow travel at a 30 km caster distance. See module docs.
        Self { tolerance_deg: 0.01 }
    }
}

impl SettingsSection for CelestialCadenceSettings {
    const KEY: &'static str = "celestial_cadence";
}

/// The epoch the celestial tree was last solved at.
///
/// One resource, one writer ([`commit_celestial_epoch`]), read by the run
/// condition every gated system shares — so the whole cluster solves for the
/// *same* epoch or not at all. Systems must never keep their own `Local` copy of
/// this: two gates drift, and a half-advanced celestial tree puts the sun and
/// the bodies at different instants.
#[derive(Resource, Debug, Clone, Copy)]
pub struct CelestialSolvedEpoch {
    /// Julian date of the last solve. `f64::NEG_INFINITY` until the first one,
    /// so the first frame always solves whatever the tolerance is.
    pub jd: f64,
    /// [`CelestialInputsRevision`] at that solve — a structural change moves this
    /// and forces one re-solve regardless of the epoch.
    pub revision: u64,
}

impl Default for CelestialSolvedEpoch {
    fn default() -> Self {
        Self {
            jd: f64::NEG_INFINITY,
            revision: 0,
        }
    }
}

/// Structural changes the celestial cluster must re-solve for even when the epoch
/// has not moved: a site anchor appearing (scene load), the site being edited, or
/// the body hierarchy being (re)built.
///
/// The epoch is not the only input. Gating the cluster on epoch movement ALONE
/// starves it whenever the inputs change at a standing epoch — a scene loaded
/// while the sky is paused never gets its site frame, so `SiteAligned` is never
/// inserted, the sun is never aimed, and the scene renders black with a bright
/// light pointing under the ground.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CelestialInputsRevision(pub u64);

/// Bump [`CelestialInputsRevision`] on the edges that invalidate a solved tree.
///
/// Runs in `First`, before every gated consumer, so a change is visible to the
/// whole cluster in the SAME frame it happens.
pub fn bump_celestial_inputs_revision(
    mut rev: ResMut<CelestialInputsRevision>,
    site_added: Query<(), Added<crate::geo::SiteAnchor>>,
    site_moved: Query<(), Changed<crate::geo::GeodeticAnchor>>,
    decl_added: Query<(), Added<crate::CelestialBodyDecl>>,
    grid_added: Query<(), Added<crate::big_space_setup::SolarSystemRoot>>,
    mut decl_removed: RemovedComponents<crate::CelestialBodyDecl>,
    // [frames, bumps, site_added, site_moved, decl_added, grid_added, removed]
    mut stats: Local<[u32; 7]>,
) {
    // `removed.read()` must be DRAINED — an unread reader keeps redelivering, so
    // the revision would go on bumping for frames after the fact. `.count()`
    // consumes the iterator; the previous `.next().is_some()` took exactly one
    // event and left any others to come back next frame, which is the same bug
    // the comment was written to prevent.
    let removed = decl_removed.read().count();
    let (site_a, site_m) = (!site_added.is_empty(), !site_moved.is_empty());
    let (decl_a, grid_a) = (!decl_added.is_empty(), !grid_added.is_empty());

    let bumped = removed > 0 || site_a || site_m || decl_a || grid_a;
    if bumped {
        rev.0 = rev.0.wrapping_add(1);
    }

    // A structural edge is by definition occasional. If the inputs are dirty on
    // most frames the cadence gate silently becomes a no-op: the cluster
    // re-solves at frame rate and the cost reads as "celestial is just
    // expensive". Measured as a RATE over a window, not a consecutive streak —
    // an input dirty 9 frames in 10 resets a streak counter and would never
    // report, which is exactly how this went unnoticed.
    const WINDOW: u32 = 300;
    stats[0] += 1;
    stats[1] += u32::from(bumped);
    stats[2] += u32::from(site_a);
    stats[3] += u32::from(site_m);
    stats[4] += u32::from(decl_a);
    stats[5] += u32::from(grid_a);
    stats[6] += u32::from(removed > 0);
    if stats[0] >= WINDOW {
        if stats[1] * 2 > WINDOW {
            warn!(
                "[celestial] cadence gate ineffective: inputs revision bumped on \
                 {}/{} frames, so the celestial cluster is re-solving at frame \
                 rate. Dirty-frame counts — site_added={} site_moved={} \
                 decl_added={} grid_added={} decl_removed={}",
                stats[1], stats[0], stats[2], stats[3], stats[4], stats[5], stats[6],
            );
        }
        *stats = [0; 7];
    }
}

/// Run condition for the WHOLE celestial cluster: has the epoch moved far enough
/// to be worth re-solving, **or** have the structural inputs changed?
///
/// Also true whenever the clock has *rewound* (`abs`), which a scrub or a
/// scenario reset does.
///
/// One condition, applied to every member, because the cluster must advance
/// ATOMICALLY. Gating the expensive value-producers (ephemeris, poses,
/// trajectories) while leaving the site pin ungated puts the pin at the current
/// epoch and the bodies at the last solved one — the sun disc and the light then
/// disagree, and the world visibly snaps each time the gate finally fires ("I
/// move and it jumps back").
pub fn celestial_needs_solve(
    world: Option<Res<WorldTime>>,
    solved: Res<CelestialSolvedEpoch>,
    settings: Option<Res<CelestialCadenceSettings>>,
    revision: Res<CelestialInputsRevision>,
) -> bool {
    if revision.0 != solved.revision {
        return true;
    }
    // No clock yet (bare test app) — never gate; the old behaviour was to run.
    let Some(world) = world else {
        return true;
    };
    let step = settings
        .map(|s| s.max_epoch_step_jd())
        .unwrap_or_else(|| CelestialCadenceSettings::default().max_epoch_step_jd());
    // `>=` with a 0.0 step: any epoch, including an unchanged one, re-solves.
    // That is what `EXACT` promises, and it is why the comparison is not `>`.
    (world.epoch_jd - solved.jd).abs() >= step
}

/// Record the epoch AND the input revision the cluster just solved for.
///
/// Runs in `Last`, after every gated consumer in both `PreUpdate` and `Update`,
/// under the same condition — so within one frame every gated system sees the
/// same `solved` state and either all of them run or none do. Committing the
/// revision here is what makes one structural change cost exactly one extra
/// solve instead of re-solving forever.
pub fn commit_celestial_epoch(
    world: Option<Res<WorldTime>>,
    revision: Res<CelestialInputsRevision>,
    mut solved: ResMut<CelestialSolvedEpoch>,
) {
    if let Some(world) = world {
        solved.jd = world.epoch_jd;
    }
    solved.revision = revision.0;
}
