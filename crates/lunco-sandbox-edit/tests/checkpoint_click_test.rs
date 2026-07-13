//! TDD for the interactive checkpoint commands [`AppendCheckpoint`] /
//! [`DeleteCheckpoint`] (ui-gated, but the command handlers themselves are
//! headless-testable — they route through the existing
//! `EngageAutopilot`/`SetAutopilotBehavior` and the `AutopilotBehaviorSpec`
//! mirror).

use bevy::prelude::*;
use lunco_autopilot::{AutopilotBehaviorSpec, BehaviorSpec, PatrolWaypoint};
use lunco_core::Command;
use lunco_sandbox_edit::ui::checkpoint_click::{
    register_all_commands, AppendCheckpoint, DeleteCheckpoint,
};

fn wire(app: &mut App) {
    app.add_plugins(lunco_core::LunCoCorePlugin);
    app.add_plugins(lunco_autopilot::AutopilotPlugin);
    // The checkpoint observers read these tunable resources (§3) — init them so
    // the test's `AppendCheckpoint` doesn't panic on a missing `Res`.
    app.init_resource::<lunco_sandbox_edit::checkpoint_gizmo::PatrolDefaults>();
    app.init_resource::<lunco_sandbox_edit::checkpoint_gizmo::CheckpointGizmoSettings>();
    register_all_commands(app);
}

#[test]
fn append_creates_patrol_on_vessel_with_no_prior_spec() {
    let mut app = App::new();
    wire(&mut app);
    let vessel = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(AppendCheckpoint {
        vessel,
        position: [1.0, 2.0, 3.0],
    });
    app.world_mut().flush();
    let spec = app
        .world()
        .entity(vessel)
        .get::<AutopilotBehaviorSpec>()
        .expect("spec mirror installed on vessel");
    let wps = spec.patrol_waypoints().expect("patrol with one waypoint");
    assert_eq!(wps.len(), 1);
    assert_eq!(wps[0].pos, [1.0, 2.0, 3.0]);
}

#[test]
fn append_extends_existing_patrol() {
    let mut app = App::new();
    wire(&mut app);
    let vessel = app
        .world_mut()
        .spawn(AutopilotBehaviorSpec::new(BehaviorSpec::Patrol {
            waypoints: vec![PatrolWaypoint::at([1.0, 0.0, 0.0])],
            speed: 0.5,
            radius: 2.0,
            dwell: 1.0,
        }))
        // An Autopilot actor must exist so `SetAutopilotBehavior` finds it
        // (its observer keys by `ap.vessel`).
        .id();
    app.world_mut().spawn(lunco_autopilot::Autopilot::forward(vessel, 0, 0.5));
    app.world_mut().flush();
    app.world_mut().trigger(AppendCheckpoint {
        vessel,
        position: [2.0, 0.0, 0.0],
    });
    app.world_mut().flush();
    let spec = app.world().entity(vessel).get::<AutopilotBehaviorSpec>().unwrap();
    let wps = spec.patrol_waypoints().unwrap();
    assert_eq!(wps.len(), 2);
    assert_eq!(wps[1].pos, [2.0, 0.0, 0.0]);
}

#[test]
fn delete_removes_index_and_spec_stays_patrol() {
    let mut app = App::new();
    wire(&mut app);
    let vessel = app
        .world_mut()
        .spawn(AutopilotBehaviorSpec::new(BehaviorSpec::Patrol {
            waypoints: vec![
                PatrolWaypoint::at([1.0, 0.0, 0.0]),
                PatrolWaypoint::at([2.0, 0.0, 0.0]),
                PatrolWaypoint::at([3.0, 0.0, 0.0]),
            ],
            speed: 0.5,
            radius: 2.0,
            dwell: 0.0,
        }))
        .id();
    app.world_mut().spawn(lunco_autopilot::Autopilot::forward(vessel, 0, 0.5));
    app.world_mut().flush();
    app.world_mut().trigger(DeleteCheckpoint { vessel, index: 1 });
    app.world_mut().flush();
    let spec = app.world().entity(vessel).get::<AutopilotBehaviorSpec>().unwrap();
    let wps = spec.patrol_waypoints().unwrap();
    assert_eq!(wps.len(), 2);
    assert_eq!(wps[0].pos, [1.0, 0.0, 0.0]);
    assert_eq!(wps[1].pos, [3.0, 0.0, 0.0]);
}

#[test]
fn delete_last_waypoint_replaces_with_brake() {
    let mut app = App::new();
    wire(&mut app);
    let vessel = app
        .world_mut()
        .spawn(AutopilotBehaviorSpec::new(BehaviorSpec::Patrol {
            waypoints: vec![PatrolWaypoint::at([1.0, 0.0, 0.0])],
            speed: 0.5,
            radius: 2.0,
            dwell: 0.0,
        }))
        .id();
    app.world_mut().spawn(lunco_autopilot::Autopilot::forward(vessel, 0, 0.5));
    app.world_mut().flush();
    app.world_mut().trigger(DeleteCheckpoint { vessel, index: 0 });
    app.world_mut().flush();
    // Deleting the last waypoint fires `ClearPatrol`, which REMOVES the spec
    // mirror entirely (brake + drop) — the canonical "stop & clear" verb.
    assert!(
        app.world().entity(vessel).get::<AutopilotBehaviorSpec>().is_none(),
        "deleting last waypoint → ClearPatrol removes the spec mirror"
    );
}