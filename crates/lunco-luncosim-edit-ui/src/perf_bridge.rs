//! Bridge avian's `PhysicsTotalDiagnostics` into workbench's
//! `PerfStats.physics_ms`. Lives here (not in `lunco-workbench`)
//! because the workbench stays physics-agnostic — it just exposes
//! an `Option<f32>` field for any crate that knows about avian to
//! populate.

use avian3d::diagnostics::{
    PhysicsDiagnosticsPlugin, PhysicsTotalDiagnostics, PhysicsTotalDiagnosticsPlugin,
};
use bevy::prelude::*;
use lunco_core_runtime::PhysicsHealthSnapshot;

/// Adds avian's diagnostics plugins (the framework one + the
/// total-step one that actually inserts `PhysicsTotalDiagnostics`)
/// and a physics-cycle publisher that writes the shared
/// [`PhysicsHealthSnapshot`]. HUD and telemetry consume that same snapshot.
///
/// Cost note (don't be fooled by profiles): `PhysicsTotalDiagnosticsPlugin`
/// *appears* as a ~30 ms per-step spike, but it does not cause it. Its systems
/// are microseconds (`Instant::now`/`elapsed` + one resource write); they
/// bracket the physics step (`PhysicsStepSystems::First`/`Last`), so the span
/// merely *measures* the real step cost — the spike is the step itself.
/// Removing/gating the plugin removes the measurement, not the cost (no FPS
/// gain), and avian 0.6.1 exposes no runtime toggle: the step-timing system is
/// welded into the core `PhysicsStepSystems::Last` set, so it can't be
/// run-condition-gated from outside without gating real physics, and Bevy
/// plugins can't be removed post-startup. A prior build-time gate on the HUD
/// flag also broke runtime toggling ("phys reads zero", no data on flip).
/// Conclusion: keep the plugin always-on. Publishing the timing fact is cheap
/// and must not depend on whether a particular UI is currently visible.
pub struct PerfBridgePlugin;

impl Plugin for PerfBridgePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<PhysicsDiagnosticsPlugin>() {
            app.add_plugins(PhysicsDiagnosticsPlugin);
        }
        if !app.is_plugin_added::<PhysicsTotalDiagnosticsPlugin>() {
            app.add_plugins(PhysicsTotalDiagnosticsPlugin);
        }
        app.add_systems(
            FixedPostUpdate,
            publish_physics_step
                .after(avian3d::schedule::PhysicsSystems::StepSimulation)
                .in_set(lunco_core::RuntimeCycleSet::Simulation),
        );
    }
}

fn publish_physics_step(
    diags: Option<Res<PhysicsTotalDiagnostics>>,
    snapshot: Option<ResMut<PhysicsHealthSnapshot>>,
) {
    let Some(mut snapshot) = snapshot else {
        return;
    };
    let Some(d) = diags else {
        snapshot.step_time_ms = None;
        return;
    };
    let step_time_ms = d.step_time.as_secs_f64() * 1000.0;
    if snapshot.step_time_ms != Some(step_time_ms)
        || snapshot.step_number != u64::from(d.step_number)
    {
        snapshot.step_time_ms = Some(step_time_ms);
        snapshot.step_number = u64::from(d.step_number);
        snapshot.revision = snapshot.revision.wrapping_add(1);
    }
}
