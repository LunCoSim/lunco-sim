//! End-to-end (headless ECS) proof of the two waypoint lifecycle contracts:
//!
//! 1. **Appending a waypoint while the autopilot is running** rebuilds the
//!    compiled tree RESUMING at the leg the rover was on — the new waypoints get
//!    driven and the rover does not U-turn to the first waypoint, with no
//!    program restart. (Regression: the old no-rebuild logic left the new legs
//!    permanently unreachable, and a plain rebuild reset the cursor to leg 0.)
//! 2. **Scene reset** (`SceneTeardown`) despawns the autopilot actors and
//!    releases their session claims, so the waypoints (route + reached state)
//!    reset cleanly with the scene and the respawned vessel can be re-engaged.

use bevy::ecs::system::RunSystemOnce;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_autopilot::usd_tree::{compile_behavior_xml, BehaviorXml};
use lunco_autopilot::{
    autopilot_session, setup_autopilot_session, teardown_autopilot_actors, Autopilot,
    AutopilotBehavior, DriveCtx,
};
use lunco_core::coords::GridPos;
use lunco_core::session::SessionRbac;
use lunco_core::{GlobalEntityId, NetworkRole, SessionRegistry};

/// The editor's route shape: a plain `sequence[drive_to…]` (one-way — the rover
/// stops at the last waypoint). Coordinates, so `compile_behavior_xml` bakes
/// them verbatim without any prim bindings.
fn route(targets: &[&str]) -> String {
    let legs: String = targets
        .iter()
        .map(|t| format!("        <Action ID=\"drive_to\" target=\"{t}\"/>\n"))
        .collect();
    format!(
        "<root BTCPP_format=\"4\" main_tree_to_execute=\"MainTree\">\n  \
         <BehaviorTree ID=\"MainTree\">\n    <Sequence>\n{legs}    </Sequence>\n  </BehaviorTree>\n</root>"
    )
}

/// A `DriveCtx` for manually ticking the compiled tree, positioned at `pos`.
fn ctx_at(pos: [f64; 3]) -> DriveCtx {
    DriveCtx {
        self_gid: 0,
        pos: GridPos(DVec3::from_array(pos)),
        fwd: Vec3::X,
        now: 0.0,
        out: (0.0, 0.0, 0.0),
        targets: Default::default(),
        clearance: Default::default(),
        fired: Vec::new(),
    }
}

/// The leg index the tree is executing — the route progress a rebuild must keep.
fn route_cursor(tree: &AutopilotBehavior) -> usize {
    tree.route_cursor().expect("route tree reports a cursor")
}

#[test]
fn appending_waypoints_while_running_resumes_route_and_drives_the_new_legs() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let frame = app
        .world_mut()
        .spawn((
            lunco_core::WorldGridConfig::default().grid(),
            CellCoord::ZERO,
            Transform::default(),
        ))
        .id();
    app.insert_resource(lunco_core::ActivePhysicsFrame(frame));
    app.add_systems(Update, compile_behavior_xml);

    // A vessel with a 3-waypoint route, parked just before waypoint 0.
    let vessel = app
        .world_mut()
        .spawn((
            BehaviorXml(route(&["10;0;0", "20;0;0", "30;0;0"])),
            Transform::from_xyz(0.0, 0.0, 0.0),
        ))
        .id();
    // Engaged autopilot actor for the vessel.
    let actor = app.world_mut().spawn(Autopilot::holding(vessel, 0)).id();

    // First compile (Update 1 sees the fresh BehaviorXml/Transform; Update 2
    // flushes the deferred insertion of the compiled tree).
    app.update();
    app.update();

    {
        let mut tree = app
            .world_mut()
            .get_mut::<AutopilotBehavior>(actor)
            .expect("first compile inserts a tree onto the actor");
        assert_eq!(route_cursor(&tree), 0, "route starts at leg 0");

        // Drive: arrive at waypoint 0 (10,0,0) → advance to leg 1 (20,0,0) and be
        // mid-drive there when the waypoint is appended — the classic "add a point
        // while running" moment.
        let mut c = ctx_at([10.0, 0.0, 0.0]);
        tree.0.tick(&mut c); // leg 0 succeeds → leg 1 now running
        assert_eq!(route_cursor(&tree), 1);
        let mut c = ctx_at([15.0, 0.0, 0.0]);
        tree.0.tick(&mut c); // en route to leg 1
        assert_eq!(route_cursor(&tree), 1, "mid-drive to waypoint 1");
    }

    // The user adds two more waypoints → the mission XML changes → recompile.
    app.world_mut().get_mut::<BehaviorXml>(vessel).unwrap().0 =
        route(&["10;0;0", "20;0;0", "30;0;0", "40;0;0", "50;0;0"]);
    app.update();

    // The tree must be REBUILT (so the new legs exist at all) and RESUMED at
    // leg 1 — not reset to leg 0 (the U-turn).
    let mut tree = app
        .world_mut()
        .get_mut::<AutopilotBehavior>(actor)
        .expect("rebuild keeps a tree on the actor");
    assert_eq!(
        route_cursor(&tree),
        1,
        "append while running must resume at the current leg, not U-turn to leg 0"
    );

    // The rebuilt route now has the two appended legs — the rover drives
    // 20 → 30 → 40 (new) → 50 (new), then the one-way route completes.
    let mut c = ctx_at([20.0, 0.0, 0.0]);
    tree.0.tick(&mut c); // leg 1 succeeds → leg 2 running
    assert_eq!(route_cursor(&tree), 2);
    let mut c = ctx_at([30.0, 0.0, 0.0]);
    tree.0.tick(&mut c); // leg 2 succeeds → APPENDED leg 3 running
    assert_eq!(route_cursor(&tree), 3, "the appended legs must be driven");
    let mut c = ctx_at([40.0, 0.0, 0.0]);
    tree.0.tick(&mut c); // appended leg 3 succeeds → appended leg 4 running
    assert_eq!(route_cursor(&tree), 4);
    let mut c = ctx_at([50.0, 0.0, 0.0]);
    assert_eq!(
        tree.0.tick(&mut c),
        lunco_behavior::Status::Success,
        "one-way route completes after the appended legs"
    );
}

#[test]
fn resume_restores_cursor_and_skips_completed_legs() {
    // A mission re-compiled after an append: the spec has 4 legs, the old tree
    // was mid-leg-2. `resume(2)` must rebuild the FULL route (so legs 3+ exist)
    // and continue at leg 2 — not restart at leg 0.
    let spec = lunco_autopilot::BehaviorSpec::Sequence {
        children: vec![
            lunco_autopilot::BehaviorSpec::DriveTo {
                target: [10.0, 0.0, 0.0],
                speed: 0.5,
                radius: 3.0,
            },
            lunco_autopilot::BehaviorSpec::DriveTo {
                target: [20.0, 0.0, 0.0],
                speed: 0.5,
                radius: 3.0,
            },
            lunco_autopilot::BehaviorSpec::DriveTo {
                target: [30.0, 0.0, 0.0],
                speed: 0.5,
                radius: 3.0,
            },
            lunco_autopilot::BehaviorSpec::DriveTo {
                target: [40.0, 0.0, 0.0],
                speed: 0.5,
                radius: 3.0,
            },
        ],
    };
    let mut tree = AutopilotBehavior::resume(&spec, Some(2));
    assert_eq!(route_cursor(&tree), 2);

    // The rover is nowhere near leg 2 yet: leg 2 must be DRIVEN (not skipped —
    // resuming the cursor must never mark incomplete legs as done).
    let mut c = ctx_at([0.0, 0.0, 0.0]);
    assert_eq!(tree.0.tick(&mut c), lunco_behavior::Status::Running);
    assert_eq!(route_cursor(&tree), 2);

    // Arrive at leg 2 (30,0,0) → the appended leg 3 (40,0,0) still runs.
    let mut c = ctx_at([30.0, 0.0, 0.0]);
    tree.0.tick(&mut c);
    assert_eq!(
        route_cursor(&tree),
        3,
        "the appended leg must still be driven"
    );

    // A cursor past the last leg clamps: the sequence completes immediately.
    let mut done = AutopilotBehavior::resume(&spec, Some(99));
    assert_eq!(route_cursor(&done), spec_leg_count(&spec));
    let mut c = ctx_at([0.0, 0.0, 0.0]);
    assert_eq!(done.0.tick(&mut c), lunco_behavior::Status::Success);
}

fn spec_leg_count(spec: &lunco_autopilot::BehaviorSpec) -> usize {
    match spec {
        lunco_autopilot::BehaviorSpec::Sequence { children } => children.len(),
        other => panic!("unexpected spec shape {other:?}"),
    }
}

#[test]
fn scene_teardown_despawns_actors_and_releases_claims() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .insert_resource(NetworkRole::Standalone)
        .init_resource::<SessionRegistry>()
        .init_resource::<SessionRbac>();
    app.add_systems(Update, setup_autopilot_session);

    // Vessel carries the SAME api_id across a reload (GlobalEntityId derives
    // from the prim path, which the reset restores).
    let vessel = app.world_mut().spawn(GlobalEntityId::from_raw(0x77)).id();
    let actor = app
        .world_mut()
        .spawn(Autopilot::forward(vessel, 0, 0.5))
        .id();
    app.update(); // engage + claim
    assert!(
        app.world()
            .resource::<SessionRegistry>()
            .owns(autopilot_session(0), 0x77),
        "the actor must hold the vessel before the reset"
    );

    // The scene reset boundary (what `lunco-usd-sim` registers this system on).
    app.world_mut()
        .run_system_once(teardown_autopilot_actors)
        .expect("teardown system runs once");
    app.world_mut().flush();

    assert!(
        app.world().get_entity(actor).is_err(),
        "the actor is scene-derived state and must not survive the scene"
    );
    assert!(
        !app.world()
            .resource::<SessionRegistry>()
            .owns(autopilot_session(0), 0x77),
        "the claim must be released, or the respawned vessel (same gid) could never be re-engaged"
    );
}
