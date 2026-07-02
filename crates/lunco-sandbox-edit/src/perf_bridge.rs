//! Bridge avian's `PhysicsTotalDiagnostics` into workbench's
//! `PerfStats.physics_ms`. Lives here (not in `lunco-workbench`)
//! because the workbench stays physics-agnostic — it just exposes
//! an `Option<f32>` field for any crate that knows about avian to
//! populate.

use avian3d::diagnostics::{
    PhysicsDiagnosticsPlugin, PhysicsTotalDiagnostics, PhysicsTotalDiagnosticsPlugin,
};
use bevy::prelude::*;
use lunco_workbench::perf_hud::{PerfHudSettings, PerfStats};

/// Adds avian's diagnostics plugins (the framework one + the
/// total-step one that actually inserts `PhysicsTotalDiagnostics`)
/// and a sampler that copies `step_time` into `PerfStats.physics_ms`
/// when the HUD is enabled.
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
/// Conclusion: keep the plugin always-on; gate only our own `sample_physics_step`
/// below (which reads the always-live resource, so it's correct-on-toggle).
pub struct PerfBridgePlugin;

impl Plugin for PerfBridgePlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<PhysicsDiagnosticsPlugin>() {
            app.add_plugins(PhysicsDiagnosticsPlugin);
        }
        if !app.is_plugin_added::<PhysicsTotalDiagnosticsPlugin>() {
            app.add_plugins(PhysicsTotalDiagnosticsPlugin);
        }
        app.add_systems(Update, sample_physics_step);
    }
}

fn sample_physics_step(
    diags: Option<Res<PhysicsTotalDiagnostics>>,
    // `Option<Res>` so binaries that don't include the workbench
    // (and therefore don't init `PerfHudSettings`) can still use
    // `SandboxEditPlugin` for selection/gizmo. When the resource is
    // missing, behave as if the perf HUD is off.
    settings: Option<Res<PerfHudSettings>>,
    // Same rationale: optional so the system tolerates a missing
    // workbench (joint_minimal etc.).
    stats: Option<ResMut<PerfStats>>,
) {
    let Some(mut stats) = stats else { return; };
    let enabled = settings.as_deref().map(|s| s.enabled).unwrap_or(false);
    if !enabled {
        if stats.physics_ms.is_some() {
            stats.physics_ms = None;
        }
        return;
    }
    let Some(d) = diags else {
        stats.physics_ms = None;
        return;
    };
    stats.physics_ms = Some(d.step_time.as_secs_f32() * 1000.0);
}
