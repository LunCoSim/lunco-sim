//! How often the celestial tree is re-solved — expressed as an **angular error
//! budget**, not a rate.
//!
//! ## Why not Hz
//!
//! Sim time is warpable, so any fixed rate is wrong at every other warp factor.
//! The required epoch step is derived from a certified maximum angular-motion
//! bound supplied by the active ephemeris provider and authored frame models.
//! At low warp this avoids solving an unchanged render frame; at high warp the
//! same geometric error budget automatically opens the gate as often as needed.
//!
//! ## What sets the default
//!
//! Not the sun's visible position: 0.05° is a tenth of its 0.53° angular
//! diameter, invisible. **Shadow edges** set it. Lunar shadows are long and
//! hard, so a sun step of `t` degrees shifts a terrain shadow by `d·tan(t)` —
//! at a 30 km caster distance, 0.01° is ~5 m and 0.05° is ~26 m. Hence 0.01°.
//!
//! At that budget the solve interval is a property of the live bound: body spin
//! or an authored fast orbit can make it short, while a static provider can
//! make it unbounded. An unbounded provider explicitly selects exact solving;
//! it is never assigned a guessed rate. The whole dependent frame cluster
//! advances together within the declared geometric error.
//!
//! Measured cost of solving every frame on `sandbox_scene.usda`: ~10 ms/frame
//! across `ephemeris_update_system`, `update_solar_poses`,
//! `trajectory_alignment_system` and `update_sun_light_system`. See
//! `docs/architecture/42-ui-frame-discipline.md` §6.

use bevy::prelude::*;
use lunco_settings::SettingsSection;
use lunco_time::WorldTime;
use serde::{Deserialize, Serialize};

/// How much celestial angular error is acceptable before the tree is re-solved.
///
/// Persisted, so it shows up in the workbench **Settings** menu next to every
/// other view preference.
#[derive(Resource, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct CelestialCadenceSettings {
    /// Allowed angular error in **degrees**. A positive value is required for
    /// persisted preferences; deterministic scene tests use the in-memory
    /// [`CelestialCadenceSettings::EXACT`] override when they need every-frame
    /// solving.
    pub tolerance_deg: f64,
}

impl CelestialCadenceSettings {
    /// Solve every frame. Deterministic runs (`scene_test`) use this so a
    /// developer's tolerance can never change a test's verdict.
    pub const EXACT: Self = Self { tolerance_deg: 0.0 };

    /// The epoch step (in Julian days) this tolerance permits for a certified
    /// angular-rate bound. An unbounded rate deliberately returns `0.0`, which
    /// makes the shared gate solve every frame instead of under-sampling a
    /// provider whose motion it cannot prove bounded.
    pub fn max_epoch_step_jd(&self, maximum_rate_rad_per_day: f64) -> f64 {
        let tolerance_rad = self.tolerance_deg.max(0.0).to_radians();
        if tolerance_rad <= 0.0 || !maximum_rate_rad_per_day.is_finite() {
            0.0
        } else if maximum_rate_rad_per_day <= 0.0 {
            f64::INFINITY
        } else {
            tolerance_rad / maximum_rate_rad_per_day
        }
    }
}

impl Default for CelestialCadenceSettings {
    fn default() -> Self {
        // ~5 m of shadow travel at a 30 km caster distance. See module docs.
        Self {
            tolerance_deg: 0.01,
        }
    }
}

impl SettingsSection for CelestialCadenceSettings {
    const KEY: &'static str = "celestial_cadence";

    fn validate_section(&self) -> Result<(), String> {
        if self.tolerance_deg.is_finite() && self.tolerance_deg > 0.0 {
            Ok(())
        } else {
            Err("tolerance_deg must be finite and greater than zero".to_string())
        }
    }
}

/// Motion bound used by the shared celestial solve gate.
///
/// The value is assembled from three authoritative owners: the active
/// ephemeris provider's certified orbital bound, IAU body rotation elements in
/// [`CelestialBodyRegistry`], and authored Kepler orbits. It is cached here so
/// the run condition never walks a BigSpace hierarchy or performs a coordinate
/// conversion. `INFINITY` is an explicit provider contract meaning "unknown";
/// it selects exact solving rather than a hidden guessed rate.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct CelestialMotionBound {
    pub maximum_rate_rad_per_day: f64,
    pub provider_revision: u64,
}

impl Default for CelestialMotionBound {
    fn default() -> Self {
        Self {
            maximum_rate_rad_per_day: 0.0,
            provider_revision: 0,
        }
    }
}

fn kepler_max_rate_rad_per_day(orbit: &crate::KeplerOrbit, gm: f64) -> f64 {
    let a = orbit.elements.semi_major_axis_m;
    let e = orbit.elements.eccentricity;
    if !a.is_finite() || a <= 0.0 || !e.is_finite() || !(0.0..1.0).contains(&e) {
        return f64::INFINITY;
    }
    if !gm.is_finite() || gm <= 0.0 {
        return f64::INFINITY;
    }
    let mean_motion = (gm / a.powi(3)).sqrt();
    let periapsis_factor = (1.0 + e).sqrt() / (1.0 - e).powf(1.5);
    mean_motion * periapsis_factor * 86_400.0
}

/// Rebuild the bound after the motion model or celestial inputs change.
pub fn refresh_motion_bound(
    registry: Res<crate::CelestialBodyRegistry>,
    ephemeris: Option<Res<crate::ephemeris::EphemerisResource>>,
    q_orbits: Query<&crate::KeplerOrbit>,
    mut bound: ResMut<CelestialMotionBound>,
) {
    let provider_revision = ephemeris
        .as_ref()
        .map_or(0, |e| e.provider.motion_revision());
    let provider_rate = ephemeris
        .as_ref()
        .map_or(0.0, |e| e.provider.maximum_angular_rate_rad_per_day());
    let mut maximum_rate = provider_rate;

    for body in &registry.bodies {
        maximum_rate = maximum_rate.max(body.rotation_rate_rad_per_day().abs());
    }
    for orbit in &q_orbits {
        let Some(body) = registry.get(orbit.body) else {
            maximum_rate = f64::INFINITY;
            break;
        };
        maximum_rate = maximum_rate.max(kepler_max_rate_rad_per_day(orbit, body.gm));
    }

    if !maximum_rate.is_finite() {
        maximum_rate = f64::INFINITY;
    }
    *bound = CelestialMotionBound {
        maximum_rate_rad_per_day: maximum_rate,
        provider_revision,
    };
}

pub(crate) fn provider_motion_changed(
    ephemeris: Option<Res<crate::ephemeris::EphemerisResource>>,
    bound: Res<CelestialMotionBound>,
) -> bool {
    ephemeris
        .as_ref()
        .is_some_and(|e| e.provider.motion_revision() != bound.provider_revision)
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
/// while the sky is paused never gets its site frame, the sun is never aimed,
/// and the scene renders black with a bright light pointing under the ground.
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
    orbit_changed: Query<(), Or<(Added<crate::KeplerOrbit>, Changed<crate::KeplerOrbit>)>>,
    directional_light_added: Query<(), Added<bevy::light::DirectionalLight>>,
    mut decl_removed: RemovedComponents<crate::CelestialBodyDecl>,
    mut orbit_removed: RemovedComponents<crate::KeplerOrbit>,
    // [frames, bumps, site_added, site_moved, decl_added, grid_added,
    //  orbit_changed, directional_light_added, removed]
    mut stats: Local<[u32; 9]>,
) {
    // `removed.read()` must be DRAINED — an unread reader keeps redelivering, so
    // the revision would go on bumping for frames after the fact. `.count()`
    // consumes the iterator; the previous `.next().is_some()` took exactly one
    // event and left any others to come back next frame, which is the same bug
    // the comment was written to prevent.
    let removed = decl_removed.read().count();
    let orbit_removed = orbit_removed.read().count();
    let any_removed = removed > 0 || orbit_removed > 0;
    let (site_a, site_m) = (!site_added.is_empty(), !site_moved.is_empty());
    let (decl_a, grid_a) = (!decl_added.is_empty(), !grid_added.is_empty());
    let orbit_c = !orbit_changed.is_empty();
    // A USD light can arrive after the input revision was already committed,
    // so its appearance reopens the solve gate for the same frame transaction.
    let light_a = !directional_light_added.is_empty();

    let bumped = any_removed || site_a || site_m || decl_a || grid_a || orbit_c || light_a;
    if bumped {
        rev.0 = rev.0.wrapping_add(1);
    }

    // Which INPUT is dirty, for when the gate reports itself ineffective.
    // `lunco_core::gate` says *that* a gate stopped gating; only the gate's own
    // inputs can say *why*, and that is domain knowledge the substrate cannot
    // have. Attribution is therefore kept here — but the rate measurement and
    // the reporting threshold are not duplicated, they belong to the substrate.
    const WINDOW: u32 = 300;
    stats[0] += 1;
    stats[1] += u32::from(bumped);
    stats[2] += u32::from(site_a);
    stats[3] += u32::from(site_m);
    stats[4] += u32::from(decl_a);
    stats[5] += u32::from(grid_a);
    stats[6] += u32::from(orbit_c);
    stats[7] += u32::from(light_a);
    stats[8] += u32::from(any_removed);
    if stats[0] >= WINDOW {
        if stats[1] * 2 > WINDOW {
            info!(
                "[celestial] inputs revision bumped on {}/{} frames — \
                 site_added={} site_moved={} decl_added={} grid_added={} \
                 orbit_changed={} directional_light_added={} \
                 celestial_removed={}",
                stats[1],
                stats[0],
                stats[2],
                stats[3],
                stats[4],
                stats[5],
                stats[6],
                stats[7],
                stats[8],
            );
        }
        *stats = [0; 9];
    }
}

/// Run condition for the WHOLE celestial cluster: has the epoch moved far enough
/// to be worth re-solving, **or** have the structural inputs changed?
///
/// Also true whenever the clock has *rewound* (`abs`), which a scrub or a
/// scenario reset does.
///
/// One condition, applied to every member, because the cluster must advance
/// ATOMICALLY. Gating only part of the frame projection would put dependent
/// frames at different epochs — the sun and bodies would disagree and the
/// world would visibly snap each time the gate finally fired.
/// [`celestial_needs_solve`], wrapped so its firing rate is reported.
///
/// **This is the only way to obtain this gate.** The raw condition is
/// `pub(crate)` on purpose: a run condition that silently stops gating costs
/// exactly what it was added to save, and the failure is invisible without a
/// profiler. Effectiveness is a runtime property — no type can prove a gate
/// still gates — but *tracking* can be made unavoidable, and visibility is the
/// cheapest enforcement there is. A future registration site cannot reach the
/// untracked form to forget it.
///
/// The cluster shares ONE condition across five registration sites; naming it
/// here keeps that name from being repeated as a literal at each. Every site
/// evaluates it separately, so the tally counts evaluations, not frames — the
/// rate is what matters, not the absolute count.
pub fn tracked_needs_solve() -> impl bevy::ecs::schedule::SystemCondition<()> {
    lunco_core::gate::tracked("celestial_needs_solve", celestial_needs_solve)
}

/// Decide whether the current celestial inputs are outside the committed
/// solve. This is deliberately a pure epoch/revision decision: wall-clock
/// time is not a valid proxy for geometric error when simulation time is
/// warped.
fn epoch_requires_solve(
    current_jd: f64,
    solved_jd: f64,
    current_revision: u64,
    solved_revision: u64,
    max_epoch_step_jd: f64,
) -> bool {
    current_revision != solved_revision || (current_jd - solved_jd).abs() >= max_epoch_step_jd
}

pub(crate) fn celestial_needs_solve(
    world: Option<Res<WorldTime>>,
    solved: Res<CelestialSolvedEpoch>,
    settings: Option<Res<CelestialCadenceSettings>>,
    motion: Res<CelestialMotionBound>,
    revision: Res<CelestialInputsRevision>,
    activity: Option<Res<lunco_core::gate::GateActivity>>,
) -> bool {
    let step = settings.map_or_else(
        || CelestialCadenceSettings::default().max_epoch_step_jd(motion.maximum_rate_rad_per_day),
        |s| s.max_epoch_step_jd(motion.maximum_rate_rad_per_day),
    );
    if let Some(activity) = activity {
        activity.expect_open("celestial_needs_solve", step <= 0.0);
    }
    // No clock yet (bare test app) — never gate; the old behaviour was to run.
    let Some(world) = world else {
        return true;
    };
    // `>=` with a 0.0 step: any epoch, including an unchanged one, re-solves.
    // That is what `EXACT` promises, and it is why the comparison is not `>`.
    epoch_requires_solve(world.epoch_jd, solved.jd, revision.0, solved.revision, step)
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
    settings: Option<Res<CelestialCadenceSettings>>,
    motion: Res<CelestialMotionBound>,
    revision: Res<CelestialInputsRevision>,
    mut solved: ResMut<CelestialSolvedEpoch>,
    mut solves: Local<u64>,
) {
    if let Some(world) = world {
        // Why the EPOCH branch of `celestial_needs_solve` fires, for when
        // `lunco_core::gate` reports the cluster ungated and the revision
        // attribution stays quiet (i.e. structure is NOT the cause). The delta
        // is what the gate compares against `max_epoch_step_jd`; if it exceeds
        // the step every frame the clock is advancing faster than the tolerance
        // allows, and the cadence cannot help.
        let delta = (world.epoch_jd - solved.jd).abs();
        // The LIVE setting, resolved exactly as `celestial_needs_solve` resolves
        // it. Reading `default()` here instead was a real defect: with the
        // resource set to `EXACT` (tolerance 0 => step 0) the gate fires on
        // `delta >= 0.0` — always, even for an unchanged epoch — while this log,
        // comparing against the 0.01-degree default's step, stayed silent. A
        // diagnostic that does not read the same input as the thing it explains
        // can only mislead, and it did: it made an EXACT-mode configuration look
        // like a broken gate.
        let step = settings.map_or_else(
            || {
                CelestialCadenceSettings::default()
                    .max_epoch_step_jd(motion.maximum_rate_rad_per_day)
            },
            |s| s.max_epoch_step_jd(motion.maximum_rate_rad_per_day),
        );
        if step <= 0.0 {
            bevy::log::warn_once!(
                "[celestial] cadence tolerance is 0 (EXACT): the gate solves EVERY \
                 frame by definition, costing ~10 ms/frame. That is intended for \
                 `scene_test` determinism — outside one it is a misconfiguration."
            );
        }
        // Periodic, not `_once`: the first frame always exceeds the step because
        // `solved.jd` starts at the "never solved" sentinel (-inf), so a one-shot
        // log reports the bootstrap and tells you nothing about steady state —
        // which is the only interesting case.
        *solves += 1;
        if delta >= step && (*solves).is_multiple_of(300) {
            bevy::log::info!(
                "[celestial] epoch branch opens the gate: |{:.6} - {:.6}| = {:.3e} d \
                 >= step {:.3e} d ({:.1} s of epoch per solve)",
                world.epoch_jd,
                solved.jd,
                delta,
                step,
                delta * 86_400.0,
            );
        }
        solved.jd = world.epoch_jd;
    } else {
        // The gate returns TRUE unconditionally without a clock — worth saying
        // out loud, because it looks identical to "the epoch moved".
        bevy::log::warn_once!(
            "[celestial] no `WorldTime`: the cadence gate cannot gate and the \
             whole cluster solves every frame."
        );
    }
    solved.revision = revision.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_warp_epoch_advance_opens_the_gate_each_render_frame() {
        let motion = std::f64::consts::TAU / 27.321_661;
        let step = CelestialCadenceSettings::default().max_epoch_step_jd(motion);
        // At 100000x, one 60 Hz render interval represents this much simulated
        // time. It exceeds the angular-error budget, so the geometric pose must
        // be solved on that frame instead of being held for a wall-time quota.
        let one_frame_at_100kx = 100_000.0 / 60.0 / 86_400.0;
        assert!(one_frame_at_100kx > step);
        assert!(epoch_requires_solve(one_frame_at_100kx, 0.0, 0, 0, step,));
    }

    #[test]
    fn unchanged_epoch_stays_gated_until_a_structural_revision() {
        let step = CelestialCadenceSettings::default().max_epoch_step_jd(1.0);
        assert!(!epoch_requires_solve(10.0, 10.0, 4, 4, step));
        assert!(epoch_requires_solve(10.0, 10.0, 5, 4, step));
    }

    #[test]
    fn earth_rotation_is_included_in_the_motion_bound() {
        let registry = crate::CelestialBodyRegistry::default_system();
        let earth = registry
            .get(crate::ephemeris_id::EARTH)
            .expect("built-in Earth");
        assert!(earth.rotation_rate_rad_per_day() > 6.0);
        let step = CelestialCadenceSettings::default()
            .max_epoch_step_jd(earth.rotation_rate_rad_per_day());
        assert!(step < 0.0001, "Earth spin must constrain the epoch step");
    }

    #[test]
    fn an_unbounded_provider_forces_exact_solves() {
        assert_eq!(
            CelestialCadenceSettings::default().max_epoch_step_jd(f64::INFINITY),
            0.0
        );
    }

    #[test]
    fn exact_override_is_not_accepted_as_a_persisted_preference() {
        assert!(CelestialCadenceSettings::EXACT.validate_section().is_err());
        assert!(CelestialCadenceSettings {
            tolerance_deg: f64::NAN
        }
        .validate_section()
        .is_err());
        assert!(CelestialCadenceSettings::default()
            .validate_section()
            .is_ok());
    }

    #[test]
    fn kepler_rate_bound_uses_periapsis_not_mean_motion() {
        let orbit = crate::KeplerOrbit {
            body: crate::ephemeris_id::EARTH,
            elements: crate::KeplerianElements {
                semi_major_axis_m: 7_000_000.0,
                eccentricity: 0.5,
                ..default()
            },
        };
        let rate = kepler_max_rate_rad_per_day(&orbit, 3.986004418e14);
        let mean = (3.986004418e14 / orbit.elements.semi_major_axis_m.powi(3)).sqrt() * 86_400.0;
        assert!(rate > mean, "periapsis rate must bound mean motion");
    }
}
