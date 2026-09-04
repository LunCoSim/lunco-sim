//! Exact mechanism and lifecycle contracts for waypoint editing.
//!
//! The production `autopilot_hold` Rhai gate drives a real rover through the
//! public patrol hot-swap path and checks that a mid-route append continues
//! forward. This file retains the exact cursor arithmetic and scene teardown
//! contracts, which are not uniquely observable through the public query API.

use bevy::ecs::system::RunSystemOnce;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_autopilot::{
    autopilot_session, setup_autopilot_session, teardown_autopilot_actors, Autopilot,
    AutopilotBehavior, DriveCtx,
};
use lunco_core::coords::GridPos;
use lunco_core::session::SessionRbac;
use lunco_core::{GlobalEntityId, NetworkRole, SessionRegistry};

/// A `DriveCtx` for manually ticking the compiled tree, positioned at `pos`.
fn ctx_at(pos: [f64; 3]) -> DriveCtx {
    DriveCtx {
        self_gid: 0,
        pos: GridPos(DVec3::from_array(pos)),
        fwd: Vec3::X,
        steering_geometry: lunco_core::SteeringGeometry::Differential,
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
