//! Run-condition effectiveness — catching a gate that has stopped gating.
//!
//! A change-driven system is written as `system.run_if(some_condition)`, and the
//! whole point is that on a quiescent scene the condition is `false` and the work
//! is skipped. The failure mode is that the condition silently degrades to
//! *always true*: the system runs every frame, produces an identical result, and
//! costs exactly what the gate was introduced to avoid.
//!
//! Nothing catches that. It compiles, the tests pass (they assert the condition
//! fires when it *should*, never that it stays quiet when it shouldn't), and the
//! only symptom is a profiler line. Two such gates were found in one afternoon on
//! this codebase — `celestial_needs_solve` and `scene_topology_changed` — both
//! correctly written, both no-ops in practice, neither visible without Tracy.
//!
//! [`tracked`] wraps any condition so it reports its own firing rate:
//!
//! ```ignore
//! app.add_systems(Update, expensive.run_if(tracked("celestial", needs_solve)));
//! ```
//!
//! It lives in `lunco-core` for the same reason [`crate::pacing`] does: the
//! subsystems that own these gates (`lunco-celestial`, `lunco-luncosim-edit`, …)
//! all depend on core and none depends on another.
//!
//! This measures *effectiveness*, not correctness. A gate that legitimately fires
//! every frame (something genuinely changes every frame) is reported too — the
//! report is a prompt to look, not a verdict. Silence is the useful signal.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

/// Evaluations counted before a gate's firing rate is judged.
///
/// Counted in *evaluations*, not frames: a condition on a `FixedUpdate` system is
/// evaluated per fixed tick, which is not the frame rate.
const WINDOW: u32 = 300;

/// Firing rate above which a gate is considered to have stopped gating.
/// Deliberately high — the claim being made is "this gate is not gating at all",
/// not "this gate could be tighter".
const INEFFECTIVE_RATE: f32 = 0.9;

/// Per-gate firing tally. One entry per [`tracked`] name.
#[derive(Default, Debug, Clone, Copy)]
pub struct GateStat {
    /// Evaluations in the current window.
    pub evaluations: u32,
    /// How many of those returned `true`.
    pub fired: u32,
    /// Whether this gate already reported — reported once per process, not once
    /// per window, so a permanently-open gate does not become log spam.
    pub reported: bool,
    /// Whether the owner explicitly declared an always-open mode for the
    /// current evaluation window. This is not inferred from the firing rate:
    /// deterministic scene tests, for example, deliberately solve every
    /// frame.
    pub expected_open: bool,
}

/// Firing rates for every [`tracked`] run condition.
///
/// The tally is behind a `Mutex` because a run condition must be a
/// **`ReadOnlySystem`** — Bevy will not accept one that takes `ResMut`. That
/// constraint is a large part of why gates go uninstrumented: there is no
/// obvious way to record anything from inside a condition. Interior mutability
/// is the way through, and the lock is uncontended in practice (one very short
/// critical section per gate evaluation).
#[derive(Resource, Default, Debug)]
pub struct GateActivity {
    stats: std::sync::Mutex<HashMap<&'static str, GateStat>>,
}

impl GateActivity {
    /// Record one evaluation of `name`. Takes `&self` so it is callable from a
    /// read-only run condition.
    pub fn record(&self, name: &'static str, fired: bool) {
        // A poisoned lock must not take the app down over a diagnostic.
        let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        let stat = stats.entry(name).or_default();
        stat.evaluations += 1;
        stat.fired += u32::from(fired);
    }

    /// Mark whether an always-open result is an explicit owner policy for the
    /// current evaluation window. The owner must update this on every
    /// evaluation so a runtime settings change immediately re-enables the
    /// effectiveness diagnostic.
    pub fn expect_open(&self, name: &'static str, expected: bool) {
        let mut stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        stats.entry(name).or_default().expected_open = expected;
    }

    /// Current tally for a gate, if it has ever been evaluated.
    pub fn get(&self, name: &str) -> Option<GateStat> {
        let stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        stats.get(name).copied()
    }

    /// Every gate and its tally — for a diagnostics panel or a test.
    pub fn snapshot(&self) -> Vec<(&'static str, GateStat)> {
        let stats = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        stats.iter().map(|(k, v)| (*k, *v)).collect()
    }
}

/// Wrap a run condition so its firing rate is measured and reported.
///
/// The returned condition is the original one — same value, same system params,
/// evaluated the same number of times. The only addition is a tally, so wrapping
/// a gate can never change whether the gated system runs.
pub fn tracked<M>(
    name: &'static str,
    condition: impl SystemCondition<M>,
) -> impl SystemCondition<()> {
    // `Res`, never `ResMut` — see [`GateActivity`]. `Option<Res<..>>` so a gate
    // used in an app that never added [`GatePlugin`] (a unit test spinning up a
    // bare `App`) still evaluates instead of panicking on a missing resource.
    // Built with the outer `into_system`, not left as the `IntoPipeSystem` that
    // `.pipe()` returns: `ReadOnlySystem` is implemented for the BUILT
    // `PipeSystem<A, B>`, and a condition must be read-only. Returning the
    // unbuilt form fails the bound even though both halves are read-only.
    IntoSystem::into_system(IntoSystem::into_system(condition).pipe(
        move |In(fired): In<bool>, activity: Option<Res<GateActivity>>| {
            if let Some(activity) = activity {
                activity.record(name, fired);
            }
            fired
        },
    ))
}

/// Report gates that fire on essentially every evaluation.
///
/// Runs in `Last`. Reports each gate at most once: the point is to surface a
/// design failure, and a gate that is stuck open would otherwise log forever.
pub fn report_ineffective_gates(activity: Res<GateActivity>) {
    let mut stats = activity.stats.lock().unwrap_or_else(|e| e.into_inner());
    for (name, stat) in stats.iter_mut() {
        if stat.evaluations < WINDOW {
            continue;
        }
        if stat.expected_open {
            stat.evaluations = 0;
            stat.fired = 0;
            continue;
        }
        let rate = stat.fired as f32 / stat.evaluations as f32;
        if rate >= INEFFECTIVE_RATE && !stat.reported {
            stat.reported = true;
            warn!(
                "[gate] `{name}` fired on {}/{} evaluations ({:.0}%) — this run \
                 condition is not gating. The system it guards is running as if \
                 unconditional, and is paying the cost the gate was added to \
                 avoid. Check what marks its inputs dirty every frame.",
                stat.fired,
                stat.evaluations,
                rate * 100.0,
            );
        }
        // Reset the window either way, so a gate that recovers is judged on
        // fresh evidence rather than on a tally from a long-past scene.
        stat.evaluations = 0;
        stat.fired = 0;
    }
}

/// Installs gate-effectiveness tracking. Idempotent.
pub struct GatePlugin;

impl Plugin for GatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GateActivity>()
            .add_systems(Last, report_ineffective_gates);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn always_true() -> bool {
        true
    }
    fn always_false() -> bool {
        false
    }

    #[derive(Resource, Default)]
    struct Ran(u32);

    fn bump(mut r: ResMut<Ran>) {
        r.0 += 1;
    }

    /// Wrapping a condition must not change whether the system runs.
    #[test]
    fn tracked_is_transparent() {
        let mut app = App::new();
        app.add_plugins(GatePlugin).init_resource::<Ran>();
        app.add_systems(Update, bump.run_if(tracked("open", always_true)));
        app.update();
        app.update();
        assert_eq!(app.world().resource::<Ran>().0, 2);

        let mut app = App::new();
        app.add_plugins(GatePlugin).init_resource::<Ran>();
        app.add_systems(Update, bump.run_if(tracked("shut", always_false)));
        app.update();
        assert_eq!(app.world().resource::<Ran>().0, 0);
    }

    /// The tally is what a report is drawn from, so it must track both outcomes.
    #[test]
    fn records_firing_rate() {
        let mut app = App::new();
        app.add_plugins(GatePlugin).init_resource::<Ran>();
        app.add_systems(Update, bump.run_if(tracked("open", always_true)));
        app.update();
        app.update();
        app.update();

        let stat = app
            .world()
            .resource::<GateActivity>()
            .get("open")
            .expect("gate was evaluated");
        assert_eq!(stat.evaluations, 3);
        assert_eq!(stat.fired, 3);

        let mut app = App::new();
        app.add_plugins(GatePlugin).init_resource::<Ran>();
        app.add_systems(Update, bump.run_if(tracked("shut", always_false)));
        app.update();
        let stat = app.world().resource::<GateActivity>().get("shut").unwrap();
        assert_eq!(stat.evaluations, 1);
        assert_eq!(stat.fired, 0);
    }

    #[test]
    fn explicit_always_open_mode_is_recorded_as_policy() {
        let activity = GateActivity::default();
        activity.record("deterministic", true);
        activity.expect_open("deterministic", true);

        let stat = activity.get("deterministic").unwrap();
        assert!(stat.expected_open);
    }
}
