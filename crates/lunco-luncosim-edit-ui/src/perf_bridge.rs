//! Bridge avian's `PhysicsTotalDiagnostics` into workbench's
//! `PerfStats.physics_ms`. Lives here (not in `lunco-workbench`)
//! because the workbench stays physics-agnostic — it just exposes
//! an `Option<f32>` field for any crate that knows about avian to
//! populate.

use avian3d::diagnostics::PhysicsTotalDiagnostics;
use bevy::prelude::*;
use lunco_core_runtime::PhysicsHealthSnapshot;

/// Publishes the shared physics-step sample into [`PhysicsHealthSnapshot`].
/// The shared `UsdAvianPlugin` owns installation of Avian's diagnostics in all
/// hosts; the HUD and telemetry consume this UI-facing snapshot.
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
        app.add_systems(
            FixedPostUpdate,
            publish_physics_step
                .after(avian3d::schedule::PhysicsSystems::StepSimulation)
                .in_set(lunco_core::RuntimeCycleSet::Simulation),
        );
        app.add_systems(Update, publish_physics_rate);
    }
}

fn publish_physics_rate(
    mut snapshot: ResMut<PhysicsHealthSnapshot>,
    mut window: Local<Option<(bevy::platform::time::Instant, u64)>>,
) {
    if snapshot.step_time_ms.is_none() {
        if snapshot.fps.is_some() {
            snapshot.fps = None;
            snapshot.revision = snapshot.revision.wrapping_add(1);
        }
        *window = None;
        return;
    }
    let now = bevy::platform::time::Instant::now();
    let Some((start, previous)) = *window else {
        *window = Some((now, snapshot.step_number));
        return;
    };
    let elapsed = now.duration_since(start).as_secs_f64();
    if elapsed < 1.0 {
        return;
    }
    let fps = snapshot
        .step_number
        .checked_sub(previous)
        .map(|steps| steps as f64 / elapsed);
    if snapshot.fps != fps {
        snapshot.fps = fps;
        snapshot.revision = snapshot.revision.wrapping_add(1);
    }
    *window = Some((now, snapshot.step_number));
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
